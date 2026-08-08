# Capture Exclusion Design

**Date:** 2026-08-08  
**Status:** Approved for planning  
**Scope:** CallerInterview native desktop application on Windows and macOS

## Goal

CallerInterview must protect its native Live-mode windows from ordinary screen-capture and screen-sharing software. Live mode fails closed when the application cannot apply and confirm the operating-system protection. Prepare and Review remain available when protection is unavailable.

This is a privacy boundary, not an undetectability claim. The application remains visible to the local user, in normal operating-system process listings, and where required by platform conventions. It does not attempt to evade proctoring, device management, cameras, modified operating systems, or privileged capture software.

## Approaches Considered

### 1. Enforced native protection (selected)

Apply protection before showing the window, verify the native state, reapply it at lifecycle boundaries, and gate Live mode on a confirmed result. This adds platform-specific code but provides an explicit and testable contract.

### 2. Tauri configuration only

Set `contentProtected` in `tauri.conf.json`. This is simple and useful as defense in depth, but Tauri's Windows implementation discards the native API return value. It cannot support a fail-closed product claim by itself.

### 3. Windows-only Win32 implementation

Call `SetWindowDisplayAffinity` directly and ignore macOS. This provides strong Windows control but violates the product's Windows-and-macOS scope.

## Product Contract

The native application exposes one of four states:

- `protected`: protection was applied and confirmed for every Live-mode window.
- `applying`: a short transitional state while protection is being applied or rechecked.
- `unavailable`: the operating system rejected protection or confirmation failed.
- `unsupported`: the platform or browser preview cannot provide the required native guarantee.

Only `protected` permits Live mode. A failed or unknown state blocks all commands that start live audio, screen capture, transcription, or guidance. Preparation, document setup, settings, saved sessions, notes, and review continue to work.

The UI displays a persistent shield indicator. A blocked start explains that screen-sharing protection could not be confirmed and provides a retry action. It never silently falls back to an unprotected Live session.

## Native Architecture

Add a small `capture_protection` module with a platform-neutral policy and platform adapters.

### Windows

1. Set Tauri's `contentProtected` window configuration as defense in depth.
2. Obtain the native `HWND` after the main window exists.
3. Call `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`.
4. Call `GetWindowDisplayAffinity` and require `WDA_EXCLUDEFROMCAPTURE` before reporting `protected`.
5. Treat a failed call, mismatched affinity, invalid handle, or unsupported Windows version as `unavailable`.

### macOS

1. Set Tauri's `contentProtected` window configuration as defense in depth.
2. Apply `NSWindowSharingNone` through the native window adapter.
3. Query the window sharing type and require `NSWindowSharingNone` before reporting `protected`.
4. Treat a failed call or mismatched state as `unavailable`.

### Other platforms and browser preview

Return `unsupported` and block Live mode. The browser preview remains useful for UI development but is never presented as capture protected.

## Lifecycle and Data Flow

Protection is applied and checked:

1. during native window setup, before Live mode is available;
2. immediately before every transition into Live mode;
3. after the protected window is shown or restored; and
4. after any future operation that recreates or replaces a protected window.

The native host owns the authoritative state. React may display it but cannot set `protected` itself. The command that enters Live mode performs a fresh native check at the command boundary, preventing stale frontend state or direct command invocation from bypassing the gate.

If protection is lost while Live mode is active, the host stops new capture and generation work, asks the sidecar to stop the active stream, persists the session state, and surfaces the blocking warning. Recovery requires a successful reapply-and-confirm cycle.

## Error Handling

- Native errors are mapped to stable, non-sensitive error codes.
- Detailed OS errors are written only to local diagnostics.
- A protection failure never starts or resumes Live mode.
- Repeated retries are user initiated or occur at a bounded lifecycle event; there is no busy retry loop.
- Application shutdown and persisted-session recovery remain available while Live mode is blocked.

## Testing

### Automated

- Policy tests prove that only a confirmed native result enables Live mode.
- Configuration tests require `contentProtected: true` for each native Live window.
- Platform adapter tests cover success, API failure, and mismatched verification state.
- Command-boundary tests prove direct Live-start invocation fails when protection is unavailable.
- Frontend tests cover all four states, retry behavior, and browser-preview blocking.
- Existing Python, frontend, Rust debug/release, Clippy, and packaging checks remain required.

### Native acceptance

On supported Windows and macOS machines:

- verify the protection state before entering Live mode;
- share the entire display in Zoom, Microsoft Teams, and Google Meet;
- record the display with OBS and use the operating-system screenshot tool;
- confirm protected CallerInterview content is excluded or replaced by the platform's protected-content treatment;
- force protection application to fail and confirm Live mode remains blocked; and
- restore/show the window and confirm protection is reapplied.

Results are recorded per OS and capture application version. A release is blocked if the native API cannot be confirmed on its supported clean-machine test image.

## Limitations

Operating-system capture exclusion is the strongest practical application-level control, but it cannot guarantee invisibility from cameras, kernel-level or privileged software, remote administration tools, modified systems, or every future capture implementation. Product copy must say `capture protected` only after confirmation and must not claim universal invisibility or undetectability.

## Acceptance Criteria

1. Native window creation enables content protection by default.
2. Windows and macOS adapters apply and verify their native protection state.
3. Live mode cannot start unless the native host reports `protected` immediately before entry.
4. Losing protection stops active capture and blocks resumption.
5. Browser preview and unsupported platforms cannot enter Live mode.
6. The UI always exposes the current protection state and a bounded retry path.
7. Automated suites and manual capture checks pass before release.
