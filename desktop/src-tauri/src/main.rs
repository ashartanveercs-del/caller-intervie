// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|argument| argument == "--sidecar-smoke") {
        if let Err(error) = desktop_lib::sidecar::smoke_packaged_sidecar() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    desktop_lib::run()
}
