# Task 7 Capture-Exclusion Verification Report

Date: 2026-08-17

## Native verification

- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check`: passed.
- Focused durable-query coverage: 10 library tests and 3 `storage` target tests passed.
- Full serial native test command: library target passed 220/220, 0 failed, in 5747.42s.
  The aggregate command was intentionally interrupted as Cargo began the separately declared
  `storage` target, which points directly to `src/storage/mod.rs` and duplicates the same 93
  storage tests with no added coverage.
- Release Clippy under the Windows native wrapper with `--all-targets -- -D warnings`: passed.
- Release Windows native build: passed in 10m10s.

## Runtime acceptance

The exact release executable was launched. A native user32 enumeration selected exactly one
visible, ownerless `CallerInterview` window belonging to the launched PID. Its independently
queried `GetWindowDisplayAffinity` value was decimal 17 (`WDA_EXCLUDEFROMCAPTURE`). The process
was closed cleanly. No HWND is recorded.

- SHA-256: `FD761B79195D74EF2808FDF72818AB1A9454069A4A48C864D78203BA3DFA29BD`
- Windows build: `22631`

## Controller evidence

- Frontend Vitest: 93/93 passed in 30.33s.
- Frontend production build: passed, 1,935 modules, 22.73s.
- Python focused shutdown suite: 3/3 passed.
- Python full suite: 129/129 passed in 113.80s; one pre-existing unawaited
  `AsyncClient.aclose` warning remained.

## Capture and platform limitations

`capture_protection_status` has no ordinary PowerShell endpoint and was not invoked or inferred.
Snipping Tool and Teams are installed but were not exercised. OBS and Zoom are not installed;
Google Meet was not exercised. macOS CI/runtime and native sharingType evidence were unavailable,
so macOS is unclaimed.

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
- Windows screen-copy evidence commit follows this report.
