// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use tauri::Manager;

pub mod commands;
pub mod protocol;
pub mod sidecar;
pub mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = state::AppState::new(app.handle().clone());
            let sidecar = state.sidecar.clone();
            app.manage(state);
            tauri::async_runtime::spawn(async move {
                let _ = sidecar.start().await;
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
