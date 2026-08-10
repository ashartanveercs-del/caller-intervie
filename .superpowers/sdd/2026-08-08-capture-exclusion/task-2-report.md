# Task 2 Report: Native Capture Exclusion

## Changed files

- `desktop/src-tauri/src/capture_protection/windows.rs`: Windows apply-and-query target requiring `WDA_EXCLUDEFROMCAPTURE` (`17`).
- `desktop/src-tauri/src/capture_protection/macos.rs`: AppKit target that keeps only a `WebviewWindow` and executes the raw `NSWindow` work on Tauri's main thread.
- `desktop/src-tauri/src/capture_protection/unsupported.rs`: explicit unsupported target returning `Unsupported`.
- `desktop/src-tauri/src/capture_protection/mod.rs`: platform target selection and exact-value adapter tests.
- `desktop/src-tauri/src/lib.rs`: config test, startup apply-and-verify, and focused-window reapply.
- `desktop/src-tauri/src/state.rs`: shared `Arc<CaptureProtectionController>` in `AppState`.
- `desktop/src-tauri/tauri.conf.json`: main-window `contentProtected: true`.
- `desktop/src-tauri/Cargo.toml` and `desktop/src-tauri/Cargo.lock`: Windows and macOS target dependencies.
- `.superpowers/sdd/2026-08-08-capture-exclusion/task-2-report.md`: Task 2 implementation and verification evidence.

`Cargo.toml` was already stat-dirty before this task. Its Git-normalized worktree hash was
`9dafb9ce2136f916fdf08749b846a05db52fbf45`, the same as `HEAD`, and `git diff -- Cargo.toml`
was empty. This task adds only the three required target dependency declarations.

## TDD evidence

All wrapper invocations use the project-native cache and the cached portable Strawberry Perl:

```powershell
$strawberryRoot = 'C:\Users\Admin\.cache\strawberry-perl-5.42.2.1'
$env:PERL = Join-Path $strawberryRoot 'perl\bin\perl.exe'
$env:PATH = (Join-Path $strawberryRoot 'perl\bin') + ';' + (Join-Path $strawberryRoot 'c\bin') + ';' + $env:PATH
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','capture_protection','--','--nocapture')
```

### RED: configuration contract

Before adding `contentProtected`, the exact command above exited `1` and produced:

```text
running 4 tests
test capture_protection::tests::only_a_confirmed_target_result_marks_the_window_protected ... ok
test capture_protection::tests::every_non_protected_state_fails_closed ... ok
test tests::native_window_configuration_enables_capture_protection ... FAILED
test capture_protection::tests::newer_reapply_cannot_be_bypassed_by_stale_protected_result ... ok

test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 203 filtered out

thread 'tests::native_window_configuration_enables_capture_protection' panicked at src\lib.rs:235:9:
assertion `left == right` failed
  left: Null
 right: true
```

### RED: Windows adapter contract

After adding the exact-affinity test but before creating the adapter, the same command exited `1`:

```text
error[E0433]: cannot find `windows` in `capture_protection`
   --> src\capture_protection\mod.rs:170:44
    |
170 |         assert!(crate::capture_protection::windows::is_excluded_affinity(17));
    |                                            ^^^^^^^ could not find `windows` in `capture_protection`

error: could not compile `desktop` (lib test) due to 2 previous errors
```

### GREEN: focused Windows suite

After implementation, the exact wrapper command exited `0`:

```text
running 5 tests
test capture_protection::tests::windows_requires_exact_exclude_from_capture_affinity ... ok
test capture_protection::tests::every_non_protected_state_fails_closed ... ok
test capture_protection::tests::only_a_confirmed_target_result_marks_the_window_protected ... ok
test tests::native_window_configuration_enables_capture_protection ... ok
test capture_protection::tests::newer_reapply_cannot_be_bypassed_by_stale_protected_result ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 203 filtered out
```

The wrapper also emitted the pre-existing `src/commands.rs:369` unreachable-pattern warning. Task 2 did not modify that file.

## Platform compilation

Command attempted:

```powershell
rustup target add x86_64-apple-darwin
cargo check --manifest-path desktop/src-tauri/Cargo.toml --target x86_64-apple-darwin
```

The Rust target installed, but the cross-check exited `1` in `objc2-exception-helper` because this Windows host has no Darwin C/Objective-C compiler:

```text
error: failed to run custom build command for `objc2-exception-helper v0.1.1`
error occurred in cc-rs: failed to find tool "cc": program not found
```

The macOS adapter has not been compiled or runtime-tested on a macOS runner. macOS runtime support is therefore not claimed; the cargo check and native behavior must run in macOS CI.

## Formatting and self-review

- Ran `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml` after implementation.
- Ran `git diff --check` with no whitespace errors.
- Verified Windows calls Tauri `set_content_protected(true)`, then `SetWindowDisplayAffinity`, then `GetWindowDisplayAffinity`, accepting only `17`.
- Verified macOS stores only `WebviewWindow`; the raw-pointer cast, `setSharingType`, and `sharingType` query are all contained in the on-main-thread helper. Worker calls marshal that entire helper through `run_on_main_thread`.
- Verified startup performs apply-and-verify before `AppState` is managed, and focused main-window events trigger one asynchronous reapply whose macOS native work returns to the main thread.
- Unsupported targets return `CaptureProtectionFailure::Unsupported`, preserving the controller's fail-closed state.

## Concerns

- Windows exact-value unit tests and configuration tests pass, but native runtime affinity still needs manual/CI acceptance on an actual supported Windows 10 2004+ environment.
- The host cannot provide macOS compile or runtime evidence; macOS CI remains required before shipping a macOS support claim.
