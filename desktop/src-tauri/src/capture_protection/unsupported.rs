use std::sync::Arc;

use tauri::WebviewWindow;

use super::{CaptureProtectionFailure, CaptureProtectionTarget};

pub(crate) fn platform_target(_window: WebviewWindow) -> Arc<dyn CaptureProtectionTarget> {
    Arc::new(UnsupportedTarget)
}

struct UnsupportedTarget;

impl CaptureProtectionTarget for UnsupportedTarget {
    fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
        Err(CaptureProtectionFailure::Unsupported)
    }
}
