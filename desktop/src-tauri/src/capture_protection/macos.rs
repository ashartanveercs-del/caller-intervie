use std::sync::{mpsc, Arc};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSWindow, NSWindowSharingType};
use tauri::WebviewWindow;

use super::{CaptureProtectionFailure, CaptureProtectionTarget};

pub(crate) fn platform_target(window: WebviewWindow) -> Arc<dyn CaptureProtectionTarget> {
    Arc::new(MacosTarget { window })
}

struct MacosTarget {
    window: WebviewWindow,
}

impl CaptureProtectionTarget for MacosTarget {
    fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
        apply_and_verify_macos(&self.window)
    }
}

fn apply_and_verify_macos(window: &WebviewWindow) -> Result<(), CaptureProtectionFailure> {
    if MainThreadMarker::new().is_some() {
        return apply_and_verify_macos_on_main(window);
    }

    let (sender, receiver) = mpsc::sync_channel(1);
    let window = window.clone();
    window
        .run_on_main_thread(move || {
            let _ = sender.send(apply_and_verify_macos_on_main(&window));
        })
        .map_err(|_| CaptureProtectionFailure::ApplyFailed)?;
    receiver
        .recv()
        .map_err(|_| CaptureProtectionFailure::ApplyFailed)?
}

fn apply_and_verify_macos_on_main(window: &WebviewWindow) -> Result<(), CaptureProtectionFailure> {
    window
        .set_content_protected(true)
        .map_err(|_| CaptureProtectionFailure::ApplyFailed)?;

    let raw = window
        .ns_window()
        .map_err(|_| CaptureProtectionFailure::ApplyFailed)?;
    // SAFETY: Tauri returned this borrowed NSWindow for a live WebviewWindow,
    // and this helper runs only on AppKit's main thread.
    let ns_window: &NSWindow = unsafe { &*raw.cast::<NSWindow>() };
    ns_window.setSharingType(NSWindowSharingType::None);

    if is_excluded_sharing_type(ns_window.sharingType()) {
        Ok(())
    } else {
        Err(CaptureProtectionFailure::VerificationFailed)
    }
}

pub(super) fn is_excluded_sharing_type(sharing_type: NSWindowSharingType) -> bool {
    sharing_type == NSWindowSharingType::None
}
