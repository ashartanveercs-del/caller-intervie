// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use std::time::Duration;

use tauri::Manager;

pub mod commands;
pub mod protocol;
pub mod sidecar;
pub mod state;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let smoke_mode = sidecar_smoke_requested();
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let state = state::AppState::new(app.handle().clone());
            let sidecar = state.sidecar.clone();
            let app_handle = app.handle().clone();
            app.manage(state);
            tauri::async_runtime::spawn(async move {
                let start = sidecar.start().await;
                if !smoke_mode {
                    return;
                }
                let result = match start {
                    Ok(_) => wait_for_smoke_ready(&sidecar).await,
                    Err(error) => Err(error.to_string()),
                };
                let _ = sidecar.shutdown().await;
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
            commands::send_sidecar_command,
            commands::restart_sidecar
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
