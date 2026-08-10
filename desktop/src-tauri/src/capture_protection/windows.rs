use std::sync::Arc;

use tauri::WebviewWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
};

use super::{CaptureProtectionFailure, CaptureProtectionTarget};

pub(crate) fn platform_target(window: WebviewWindow) -> Arc<dyn CaptureProtectionTarget> {
    Arc::new(WindowsTarget { window })
}

struct WindowsTarget {
    window: WebviewWindow,
}

impl CaptureProtectionTarget for WindowsTarget {
    fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
        self.window
            .set_content_protected(true)
            .map_err(|_| CaptureProtectionFailure::ApplyFailed)?;

        let hwnd = self
            .window
            .hwnd()
            .map_err(|_| CaptureProtectionFailure::ApplyFailed)?;
        unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }
            .map_err(|_| CaptureProtectionFailure::ApplyFailed)?;

        let mut affinity = 0_u32;
        unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }
            .map_err(|_| CaptureProtectionFailure::VerificationFailed)?;
        if is_excluded_affinity(affinity) {
            Ok(())
        } else {
            Err(CaptureProtectionFailure::VerificationFailed)
        }
    }
}

pub(super) fn is_excluded_affinity(affinity: u32) -> bool {
    affinity == WDA_EXCLUDEFROMCAPTURE.0
}
