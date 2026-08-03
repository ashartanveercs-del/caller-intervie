use tauri::State;

use crate::{
    protocol::Envelope,
    sidecar::{SidecarError, SidecarStatus},
    state::AppState,
};

pub type CommandError = SidecarError;

#[tauri::command]
pub async fn sidecar_status(state: State<'_, AppState>) -> Result<SidecarStatus, CommandError> {
    Ok(state.sidecar.status().await)
}

#[tauri::command]
pub async fn send_sidecar_command(
    state: State<'_, AppState>,
    command: Envelope,
) -> Result<(), CommandError> {
    state.sidecar.send(command).await
}

#[tauri::command]
pub async fn restart_sidecar(state: State<'_, AppState>) -> Result<SidecarStatus, CommandError> {
    state.sidecar.restart().await
}

#[cfg(test)]
mod tests {
    #[test]
    fn webview_capability_exposes_no_shell_plugin_permissions() {
        let capability = include_str!("../capabilities/default.json");

        assert!(!capability.contains("shell:"));
        assert!(capability.contains("core:default"));
    }
}
