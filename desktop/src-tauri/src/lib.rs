// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use std::time::Duration;

use tauri::Manager;

pub mod commands;
pub mod protocol;
pub mod sidecar;
pub mod state;
pub mod storage;

fn sidecar_smoke_requested() -> bool {
    std::env::args().any(|argument| argument == "--sidecar-smoke")
}

async fn wait_for_smoke_ready(sidecar: &sidecar::SidecarSupervisor) -> Result<(), String> {
    tokio::time::timeout(
        sidecar::PRODUCTION_HANDSHAKE_TIMEOUT + Duration::from_secs(5),
        async {
            loop {
                match sidecar.status().await.state {
                    sidecar::SidecarState::Ready => return Ok(()),
                    sidecar::SidecarState::Failed => {
                        return Err("sidecar entered failed state during handshake".into())
                    }
                    _ => tokio::time::sleep(Duration::from_millis(25)).await,
                }
            }
        },
    )
    .await
    .map_err(|_| "sidecar did not emit correlated sidecar.ready within 50 seconds".to_owned())?
}

async fn finish_smoke(
    sidecar: &sidecar::SidecarSupervisor,
    readiness: Result<(), String>,
) -> Result<(), String> {
    let cleanup = sidecar.shutdown().await.map_err(|error| error.to_string());
    match (readiness, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (_, Err(error)) => Err(format!("sidecar smoke cleanup failed: {error}")),
        (Err(error), Ok(())) => Err(error),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let smoke_mode = sidecar_smoke_requested();
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let state = state::AppState::new(app.handle().clone())?;
            let sidecar = state.sidecar.clone();
            let app_handle = app.handle().clone();
            app.manage(state);
            tauri::async_runtime::spawn(async move {
                let start = sidecar.start().await;
                if !smoke_mode {
                    return;
                }
                let readiness = match start {
                    Ok(_) => wait_for_smoke_ready(&sidecar).await,
                    Err(error) => Err(error.to_string()),
                };
                let result = finish_smoke(&sidecar, readiness).await;
                if let Err(error) = result {
                    eprintln!("sidecar smoke failed: {error}");
                    app_handle.exit(1);
                } else {
                    app_handle.exit(0);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::sidecar_status,
            commands::storage_health,
            commands::send_sidecar_command,
            commands::restart_sidecar,
            commands::create_session,
            commands::save_session_brief,
            commands::complete_session,
            commands::list_sessions,
            commands::get_session,
            commands::get_timeline,
            commands::restore_active_session,
            commands::delete_session,
            commands::associate_request_with_turn,
            commands::get_request_turn_associations
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::sidecar::{SidecarError, SidecarPort, SidecarState, SidecarSupervisor};

    #[test]
    fn persistent_session_commands_are_registered_with_tauri() {
        let source = include_str!("lib.rs");
        for command in [
            "commands::storage_health",
            "commands::create_session",
            "commands::save_session_brief",
            "commands::complete_session",
            "commands::list_sessions",
            "commands::get_session",
            "commands::get_timeline",
            "commands::restore_active_session",
            "commands::delete_session",
            "commands::associate_request_with_turn",
            "commands::get_request_turn_associations",
        ] {
            assert!(source.contains(command), "missing Tauri command: {command}");
        }
    }

    #[tokio::test]
    async fn smoke_cleanup_failure_is_reported_as_a_failure() {
        struct FailingCleanupPort;

        #[async_trait]
        impl SidecarPort for FailingCleanupPort {
            async fn write(&self, _bytes: Vec<u8>) -> Result<(), SidecarError> {
                Ok(())
            }

            async fn kill(&self) -> Result<(), SidecarError> {
                Err(SidecarError::new(
                    "sidecar_kill_failed",
                    "DEEPSEEK_API_KEY=cleanup-secret",
                ))
            }
        }

        let sidecar = SidecarSupervisor::with_port(Arc::new(FailingCleanupPort));
        let error = super::finish_smoke(&sidecar, Ok(())).await.unwrap_err();

        assert!(error.contains("cleanup"));
        assert!(!error.contains("cleanup-secret"));
        assert_eq!(sidecar.status().await.state, SidecarState::Failed);
    }
}
