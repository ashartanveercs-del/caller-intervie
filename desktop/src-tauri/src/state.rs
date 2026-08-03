use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;

use crate::sidecar::{
    SidecarSupervisor, TauriEventSink, TauriSidecarLauncher, PRODUCTION_HANDSHAKE_TIMEOUT,
};

pub struct AppState {
    pub sidecar: SidecarSupervisor,
}

impl AppState {
    pub fn new(app: AppHandle) -> Self {
        let launcher = Arc::new(TauriSidecarLauncher::new(app.clone()));
        let sink = Arc::new(TauriEventSink::new(app));
        Self {
            sidecar: SidecarSupervisor::with_launcher_and_sink(
                launcher,
                sink,
                Duration::from_secs(1),
                Some(PRODUCTION_HANDSHAKE_TIMEOUT),
            ),
        }
    }
}
