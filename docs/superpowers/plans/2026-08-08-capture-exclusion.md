# Capture Exclusion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Make native CallerInterview Live mode fail closed unless the operating system has applied and confirmed capture exclusion for the main window.

**Architecture:** A Rust CaptureProtectionController owns the authoritative state and delegates native work to a testable target. Windows and macOS targets apply and query their OS window-sharing state. Every capture-sensitive sidecar command performs a fresh native check before dispatch, while React only displays status and blocks navigation as a usability layer.

**Tech Stack:** Tauri 2.11, Rust 2021, windows 0.61, objc2 0.6, objc2-app-kit 0.3, React 19, TypeScript, Vitest, Testing Library, i18next.

## Global Constraints

- Live mode fails closed unless native state is protected.
- Prepare and Review remain usable when protection is unavailable.
- Browser preview and unsupported operating systems cannot enter Live mode.
- Product copy says "capture protected"; it never claims universal invisibility or undetectability.
- Capture protection is not proctoring evasion and does not hide the process from the local operating system.
- Native protection is applied at startup, before protected dispatch, and after restore/focus lifecycle events.
- Stable client errors contain no native handles, OS diagnostics, session identifiers, or secrets.
- Existing storage, sidecar, frontend, release, Clippy, and packaging checks remain release gates.

## File Map

- Create desktop/src-tauri/src/capture_protection/mod.rs: state model, controller, target trait, stable failures, platform selection, and unit tests.
- Create desktop/src-tauri/src/capture_protection/windows.rs: Win32 apply-and-query adapter.
- Create desktop/src-tauri/src/capture_protection/macos.rs: AppKit apply-and-query adapter.
- Create desktop/src-tauri/src/capture_protection/unsupported.rs: explicit unsupported adapter.
- Modify desktop/src-tauri/src/lib.rs: register commands, initialize protection, and reapply on focus.
- Modify desktop/src-tauri/src/state.rs: store the shared controller in AppState.
- Modify desktop/src-tauri/src/commands.rs: expose status/retry and fail-closed dispatch authorization.
- Modify desktop/src-tauri/src/sidecar.rs: stop active capture when protection is lost.
- Modify desktop/src-tauri/tauri.conf.json and Cargo.toml: enable content protection and native dependencies.
- Modify desktop/src/platform/{types,desktop,browser}.ts and tests: expose the protection contract.
- Modify desktop/src/app/RuntimeProvider.tsx and tests: retain and refresh protection state.
- Create desktop/src/components/CaptureProtectionIndicator.tsx and its test.
- Modify desktop/src/app/{AppShell,router}.tsx and tests: persistent shield and blocked Live route.
- Modify both locale JSON files and global.css: localized compact states and blocker styling.

---

### Task 1: Authoritative Capture-Protection Controller

**Files:**
- Create: desktop/src-tauri/src/capture_protection/mod.rs
- Modify: desktop/src-tauri/src/lib.rs

**Interfaces:**
- Produces CaptureProtectionState::{Applying, Protected, Unavailable, Unsupported}.
- Produces CaptureProtectionStatus { state, code, message }, camel-case fields and snake-case enum values.
- Produces CaptureProtectionTarget::apply_and_verify(&self) -> Result<(), CaptureProtectionFailure>.
- Produces CaptureProtectionController::{with_target, status, reapply_and_verify, require_protected}.
- require_protected returns SidecarError code capture_protection_required unless state is Protected.

- [ ] **Step 1: Write controller tests before production code**

Add tests in capture_protection/mod.rs using a fake target with queued outcomes:

~~~rust
#[test]
fn only_a_confirmed_target_result_marks_the_window_protected() {
    let target = Arc::new(FakeTarget::new([
        Err(CaptureProtectionFailure::ApplyFailed),
        Ok(()),
    ]));
    let controller = CaptureProtectionController::with_target(target);

    assert_eq!(
        controller.reapply_and_verify().state,
        CaptureProtectionState::Unavailable
    );
    assert_eq!(
        controller.reapply_and_verify().state,
        CaptureProtectionState::Protected
    );
}

#[test]
fn every_non_protected_state_fails_closed() {
    for state in [
        CaptureProtectionState::Applying,
        CaptureProtectionState::Unavailable,
        CaptureProtectionState::Unsupported,
    ] {
        let controller = CaptureProtectionController::with_status_for_test(state);
        assert_eq!(
            controller.require_protected().unwrap_err().code(),
            "capture_protection_required"
        );
    }
}
~~~

- [ ] **Step 2: Run focused tests and verify RED**

From desktop/src-tauri:

~~~powershell
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','capture_protection::tests','--','--nocapture')
~~~

Expected: compilation fails because the capture_protection module and types do not exist.

- [ ] **Step 3: Implement the minimal common controller**

Use std::sync::RwLock for status. reapply_and_verify sets Applying, invokes the target exactly once, and stores Protected or a stable failure. Serialized status does not include native handles or raw OS error text.

~~~rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureProtectionState {
    Applying,
    Protected,
    Unavailable,
    Unsupported,
}

pub trait CaptureProtectionTarget: Send + Sync {
    fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure>;
}

pub struct CaptureProtectionController {
    target: Arc<dyn CaptureProtectionTarget>,
    status: RwLock<CaptureProtectionStatus>,
}
~~~

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run the Step 2 command. Expected: all capture_protection::tests pass.

- [ ] **Step 5: Commit**

~~~powershell
git add desktop/src-tauri/src/capture_protection/mod.rs desktop/src-tauri/src/lib.rs
git commit -m "feat: add capture protection controller"
~~~

---

### Task 2: Native Windows and macOS Verification

**Files:**
- Create: desktop/src-tauri/src/capture_protection/windows.rs
- Create: desktop/src-tauri/src/capture_protection/macos.rs
- Create: desktop/src-tauri/src/capture_protection/unsupported.rs
- Modify: desktop/src-tauri/src/capture_protection/mod.rs
- Modify: desktop/src-tauri/src/lib.rs
- Modify: desktop/src-tauri/src/state.rs
- Modify: desktop/src-tauri/tauri.conf.json
- Modify: desktop/src-tauri/Cargo.toml

**Interfaces:**
- Consumes CaptureProtectionTarget and CaptureProtectionController from Task 1.
- Produces platform_target(window: tauri::WebviewWindow) -> Arc<dyn CaptureProtectionTarget>.
- Produces AppState.capture_protection: Arc<CaptureProtectionController>.

- [ ] **Step 1: Write failing configuration and adapter tests**

Add a lib.rs test that parses the checked-in Tauri config and pure adapter tests that reject every affinity/sharing value except the exact protected constant.

~~~rust
#[test]
fn native_window_configuration_enables_capture_protection() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(config["app"]["windows"][0]["contentProtected"], true);
}

#[cfg(windows)]
#[test]
fn windows_requires_exclude_from_capture_affinity() {
    assert!(is_excluded_affinity(WDA_EXCLUDEFROMCAPTURE.0));
    assert!(!is_excluded_affinity(WDA_NONE.0));
    assert!(!is_excluded_affinity(WDA_MONITOR.0));
}
~~~

- [ ] **Step 2: Run focused tests and verify RED**

~~~powershell
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','capture_protection','--','--nocapture')
~~~

Expected: config assertion fails and platform target functions are missing.

- [ ] **Step 3: Add exact target dependencies**

~~~toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.61.3", features = ["Win32_Foundation", "Win32_UI_WindowsAndMessaging"] }

[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6.4"
objc2-app-kit = { version = "0.3.2", features = ["NSWindow"] }
~~~

The Windows target calls Tauri set_content_protected(true), SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE), then GetWindowDisplayAffinity, and requires decimal 17.

The macOS target calls Tauri set_content_protected(true), converts window.ns_window() to NSWindow, calls setSharingType(NSWindowSharingType::None), then requires sharingType() == NSWindowSharingType::None.

The unsupported target always returns CaptureProtectionFailure::Unsupported.

- [ ] **Step 4: Enable startup and focus enforcement**

Set "contentProtected": true on the main window. During setup, obtain window label main, create the target/controller, call reapply_and_verify, and pass the controller into AppState. On a main-window Focused(true) event, spawn one reapply without blocking the event loop.

- [ ] **Step 5: Run Windows tests and macOS compilation in CI**

~~~powershell
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','capture_protection','--','--nocapture')
cargo check --manifest-path desktop/src-tauri/Cargo.toml --target x86_64-apple-darwin
~~~

Expected: Windows tests pass. If Apple frameworks prevent a Windows-host cross-check, the same cargo check and tests run on the macOS CI runner; macOS support stays unclaimed until that job passes.

- [ ] **Step 6: Commit**

~~~powershell
git add desktop/src-tauri/src/capture_protection desktop/src-tauri/src/lib.rs desktop/src-tauri/src/state.rs desktop/src-tauri/tauri.conf.json desktop/src-tauri/Cargo.toml desktop/src-tauri/Cargo.lock
git commit -m "feat: enforce native capture exclusion"
~~~

---

### Task 3: Fail-Closed Sidecar Command Boundary

**Files:**
- Modify: desktop/src-tauri/src/commands.rs
- Modify: desktop/src-tauri/src/lib.rs

**Interfaces:**
- Consumes AppState.capture_protection.
- Produces Tauri commands capture_protection_status and retry_capture_protection.
- Produces command_requires_capture_protection(&Envelope) -> bool.
- Protected set: session.start, query.trigger, listening.set with enabled true, and audio.system.set with enabled true.
- Stop and false-valued disable commands remain available.

- [ ] **Step 1: Add failing table-driven policy tests**

~~~rust
#[test]
fn capture_sensitive_commands_require_fresh_protection() {
    assert!(command_requires_capture_protection(&session_start_command()));
    assert!(command_requires_capture_protection(&listening_command(true)));
    assert!(command_requires_capture_protection(&audio_system_command(true)));
    assert!(command_requires_capture_protection(&query_command()));
    assert!(!command_requires_capture_protection(&listening_command(false)));
    assert!(!command_requires_capture_protection(&session_stop_command()));
}
~~~

Add an async test with a failing fake target and a dispatch closure that panics if called.

- [ ] **Step 2: Run focused tests and verify RED**

~~~powershell
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','commands::tests::capture_','--','--nocapture')
~~~

Expected: compilation fails because policy helper and gate are absent.

- [ ] **Step 3: Implement status/retry commands and the dispatch gate**

Before durable authorization or sidecar dispatch, reapply and require Protected for a protected command. Return stable code capture_protection_required and message "Live mode is unavailable because screen capture protection could not be confirmed." Register both status commands in generate_handler!.

- [ ] **Step 4: Run focused and existing command tests**

~~~powershell
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','commands::tests','--','--nocapture')
~~~

Expected: capture policy and all existing command tests pass.

- [ ] **Step 5: Commit**

~~~powershell
git add desktop/src-tauri/src/commands.rs desktop/src-tauri/src/lib.rs
git commit -m "feat: gate live dispatch on capture protection"
~~~

---

### Task 4: Stop Active Capture When Protection Is Lost

**Files:**
- Modify: desktop/src-tauri/src/lib.rs
- Modify: desktop/src-tauri/src/sidecar.rs

**Interfaces:**
- Consumes SidecarSupervisor::current_runtime_session().
- Produces SidecarSupervisor::stop_for_capture_protection_loss() -> Result<(), SidecarError>.
- Sends session.stop when a runtime session exists; if bounded dispatch fails, shuts down the sidecar.

- [ ] **Step 1: Add failing sidecar safety tests**

Using the existing fake port/launcher harness, start a runtime session, call stop_for_capture_protection_loss, and assert exactly one session.stop frame. Add a poisoned-port case and assert no active child/session remains after bounded fallback shutdown.

- [ ] **Step 2: Run and verify RED**

~~~powershell
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','capture_protection_loss','--','--nocapture','--test-threads=1')
~~~

Expected: compilation fails because the safety method is absent.

- [ ] **Step 3: Implement bounded stop-or-shutdown**

Build a valid empty-payload session.stop envelope with a fresh UUID, current runtime session id, PROTOCOL_VERSION, and epoch milliseconds. Attempt send under existing serialization. If it fails, call shutdown so capture cannot continue.

In the main-window focus handler, compare the previous and new states. If Protected becomes unavailable, spawn stop_for_capture_protection_loss. Never auto-restart while unprotected.

- [ ] **Step 4: Run focused and full native debug tests**

~~~powershell
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','capture_protection_loss','--','--nocapture','--test-threads=1')
.\scripts\windows-native-build.ps1 -CargoArguments @('test','--lib','--','--test-threads=1')
~~~

Expected: focused and complete native suites pass without hangs.

- [ ] **Step 5: Commit**

~~~powershell
git add desktop/src-tauri/src/lib.rs desktop/src-tauri/src/sidecar.rs
git commit -m "fix: stop live capture when protection is lost"
~~~

---

### Task 5: Platform Contract and Browser Fail-Closed Behavior

**Files:**
- Modify: desktop/src/platform/types.ts
- Modify: desktop/src/platform/desktop.ts
- Modify: desktop/src/platform/desktop.test.ts
- Modify: desktop/src/platform/browser.ts
- Modify: desktop/src/platform/browser.test.ts

**Interfaces:**
- Produces CaptureProtectionState = "applying" | "protected" | "unavailable" | "unsupported".
- Produces CaptureProtectionStatus { state, code?, message? }.
- Adds captureProtectionStatus() and retryCaptureProtection() to PlatformApi.

- [ ] **Step 1: Write failing adapter tests**

Assert desktop invokes capture_protection_status and retry_capture_protection. Assert browser reports unsupported with code browser_preview and rejects session.start and truthy enable commands while allowing stop/disable.

- [ ] **Step 2: Run and verify RED**

~~~powershell
pnpm --dir desktop test --run src/platform/desktop.test.ts src/platform/browser.test.ts
~~~

Expected: type/tests fail because methods and behavior are absent.

- [ ] **Step 3: Implement DTO mapping and browser behavior**

Keep DTO mapping inside desktop.ts. Mirror the native protected command predicate in browser.ts. Throw an Error augmented with code capture_protection_required for protected commands.

- [ ] **Step 4: Run adapter tests and build**

~~~powershell
pnpm --dir desktop test --run src/platform/desktop.test.ts src/platform/browser.test.ts
pnpm --dir desktop build
~~~

Expected: tests and TypeScript/Vite build pass.

- [ ] **Step 5: Commit**

~~~powershell
git add desktop/src/platform
git commit -m "feat: expose capture protection platform state"
~~~

---

### Task 6: Persistent Shield and Blocked Live Route

**Files:**
- Modify: desktop/src/app/RuntimeProvider.tsx
- Modify: desktop/src/app/RuntimeProvider.test.tsx
- Create: desktop/src/components/CaptureProtectionIndicator.tsx
- Create: desktop/src/components/CaptureProtectionIndicator.test.tsx
- Modify: desktop/src/app/AppShell.tsx
- Modify: desktop/src/app/AppShell.test.tsx
- Modify: desktop/src/app/router.tsx
- Create: desktop/src/app/router.test.tsx
- Modify: desktop/src/i18n/locales/en.json
- Modify: desktop/src/i18n/locales/ar-XB.json
- Modify: desktop/src/styles/global.css

**Interfaces:**
- Consumes PlatformApi capture methods.
- Adds captureProtection and retryCaptureProtection() to RuntimeContextValue.
- Produces CaptureProtectionIndicator with shield icon, status text, and retry control only when actionable.

- [ ] **Step 1: Write failing runtime and UI tests**

Cover initial Applying, resolved Protected, unavailable retry, unsupported browser, and an unavailable /live/:sessionId route. Assert role=status for the shield, role=alert for the blocker, absence of Live content, continued Prepare rendering, and one platform retry call.

~~~tsx
expect(screen.getByRole("status", { name: /capture protected/i })).toBeVisible();
expect(screen.getByRole("alert")).toHaveTextContent(/Live mode is blocked/i);
expect(
  screen.getByRole("button", { name: /retry capture protection/i }),
).toBeEnabled();
~~~

- [ ] **Step 2: Run and verify RED**

~~~powershell
pnpm --dir desktop test --run src/app/RuntimeProvider.test.tsx src/components/CaptureProtectionIndicator.test.tsx src/app/AppShell.test.tsx src/app/router.test.tsx
~~~

Expected: failures because runtime state, component, copy, and route guard are absent.

- [ ] **Step 3: Implement compact protected-state UI**

Load status after provider mount and ignore late completion after unmount. Expose one deduplicated retry callback. Use ShieldCheck for Protected and ShieldAlert otherwise. Keep footer unframed. LiveRoute renders normal content only for Protected.

English copy:
- Capture protected
- Checking capture protection
- Capture protection unavailable
- Live mode requires the desktop app
- Live mode is blocked because screen capture protection could not be confirmed.
- Retry capture protection

Add pseudo-localized ar-XB equivalents. Ensure wrapping at 960x640 and 200% text.

- [ ] **Step 4: Run focused, full frontend, and build checks**

~~~powershell
pnpm --dir desktop test --run src/app/RuntimeProvider.test.tsx src/components/CaptureProtectionIndicator.test.tsx src/app/AppShell.test.tsx src/app/router.test.tsx
pnpm --dir desktop test --run
pnpm --dir desktop build
~~~

Expected: all frontend tests and production build pass.

- [ ] **Step 5: Commit**

~~~powershell
git add desktop/src/app desktop/src/components desktop/src/i18n desktop/src/styles/global.css
git commit -m "feat: show fail-closed capture protection status"
~~~

---

### Task 7: Native Acceptance, Release Verification, and Ledger

**Files:**
- Modify: .superpowers/sdd/2026-08-03-foundation-interview-shell/task-9-10-integration-map.md

**Interfaces:**
- Consumes all prior tasks.
- Produces recorded Windows affinity, capture-tool, regression, release, and macOS CI evidence.

- [ ] **Step 1: Run complete automated verification**

~~~powershell
cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check
pnpm --dir desktop test --run
pnpm --dir desktop build
python -m pytest ai_assistant/tests -q
.\desktop\src-tauri\scripts\windows-native-build.ps1 -CargoArguments @('test','--manifest-path','desktop/src-tauri/Cargo.toml','--','--test-threads=1')
.\desktop\src-tauri\scripts\windows-native-build.ps1 -CargoArguments @('clippy','--release','--manifest-path','desktop/src-tauri/Cargo.toml','--all-targets','--','-D','warnings')
.\desktop\src-tauri\scripts\windows-native-build.ps1 -CargoArguments @('build','--release','--manifest-path','desktop/src-tauri/Cargo.toml')
~~~

Expected: every command exits zero and Clippy emits no warnings.

- [ ] **Step 2: Verify the running Windows affinity**

Launch the newly built app, invoke capture_protection_status, and require state protected. Independently query the main HWND with GetWindowDisplayAffinity and require decimal 17. Record executable hash, Windows build, and status.

- [ ] **Step 3: Perform capture-tool acceptance**

Share the entire display in Zoom, Teams, and Google Meet; record with OBS; capture with Snipping Tool. Confirm CallerInterview is omitted or replaced by protected-content treatment. Use a test target returning Unavailable and confirm no protected command reaches the sidecar.

- [ ] **Step 4: Verify macOS on supported CI or a clean machine**

Run tests, release build, and native NSWindow sharingType == None assertion. Record macOS and capture-app versions. Do not mark macOS supported without this evidence.

- [ ] **Step 5: Update and commit the ledger**

Record commit ids, exact pass counts, affinity value, manual matrix, and limitations. Never include credentials, session content, or raw native handles.

~~~powershell
git add .superpowers/sdd/2026-08-03-foundation-interview-shell/task-9-10-integration-map.md
git commit -m "docs: record capture exclusion verification"
~~~

- [ ] **Step 6: Review, integrate, and push**

Use superpowers:requesting-code-review, resolve findings, rerun affected tests, integrate verified commits into agent/tauri-interview-foundation, and push the branch to GitHub. Stop the test app afterward unless the user asks to keep it open.
