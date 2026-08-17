# Task 7 Capture-Exclusion Verification Report

Date: 2026-08-17

## Native verification

- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check`: passed.
- Focused durable-query coverage: 10 library tests and 3 `storage` target tests passed.
- Full serial native library run: 220/220 passed, 0 failed, in 5747.42s. The original
  all-target invocation was interrupted only after this pass when Cargo began the separately
  declared `storage` target, which points directly to `src/storage/mod.rs` and duplicates the
  same 93 storage tests.
- Fresh release-profile library gate through the native wrapper: 220/220 passed, 0 failed, in
  73.73s of test execution with four bounded workers; the complete wrapper command exited zero.
- Focused fail-closed acceptance:
  `commands::tests::protected_dispatch_loss_stops_the_active_runtime_before_returning` passed
  1/1. Its Unavailable target proves the protected dispatch closure is never reached, returns
  `capture_protection_required`, stops the active runtime, and clears runtime authorization.
- Release Clippy under the Windows native wrapper with `--all-targets -- -D warnings`: passed.
- Release Windows native build: passed in 10m10s.

## Runtime acceptance

The exact release executable was launched. A native user32 enumeration selected exactly one
visible, ownerless `CallerInterview` window belonging to the launched PID. Its independently
queried `GetWindowDisplayAffinity` value was decimal 17 (`WDA_EXCLUDEFROMCAPTURE`). The process
was closed cleanly. No HWND is recorded.

The exact release executable was launched again. Its React provider invoked
`capture_protection_status` on mount, and Windows UI Automation found the resulting status
element with the exact accessible label `Capture protected`. The process exited zero after the
check. This verifies the end-to-end app-reported controller state in addition to the independent
user32 affinity query.

- SHA-256: `FD761B79195D74EF2808FDF72818AB1A9454069A4A48C864D78203BA3DFA29BD`
- Windows build: `22631`

## Controller evidence

- Frontend Vitest: 93/93 passed in 30.33s.
- Frontend production build: passed, 1,935 modules, 22.73s.
- Python focused shutdown suite: 4/4 passed in 9.26s.
- Python full suite: 130/130 passed in 117.00s with no warning summary.

## Capture and platform limitations

Snipping Tool and Teams are installed but were not exercised. OBS and Zoom are not installed;
Google Meet was not exercised. macOS CI/runtime and native sharingType evidence were unavailable,
so macOS is unclaimed. These checks remain explicit pre-GA platform gates. They do not block
integration of the Windows implementation and cannot be used to claim support for those tools or
macOS.

## Windows screen-copy acceptance

Controller verification launched the exact release executable, selected exactly one visible
ownerless `CallerInterview` window for its launched process, and reconfirmed
`GetWindowDisplayAffinity == 17`. It captured only that window's screen rectangle with
`System.Drawing.Graphics.CopyFromScreen` using `SourceCopy`. Visual inspection found the
underlying desktop across the entire crop and no CallerInterview content, confirming exclusion
for this Windows screen-copy capture API. The app then closed cleanly. This result does not
cover Snipping Tool, Teams, Google Meet, OBS, Zoom, or macOS; all remain unverified.

## Commits

- `3c4a4ac` `fix: resolve Rust warning gate`
- `5b58f72` `docs: record capture exclusion verification`
- `9c71f8a` `docs: add Windows capture acceptance evidence`
- `26b0b61` `test: cover lazy runtime imports`
