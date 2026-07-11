// Suppresses the console window that would otherwise appear alongside the
// GUI on Windows release builds; debug builds keep the console so
// `eprintln!`/panics are visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod dto;
mod elevate;
mod ipc_client;
mod state;

use state::AppState;

fn main() {
    // Loaded once, synchronously, before the window opens. `ConfigStore::load`
    // already returns a fresh default config (with a newly generated
    // per-install MAC) rather than an error when no config file exists yet --
    // see `village_core::config`'s documented first-run behavior -- so this
    // only actually fails for a corrupted/unreadable config file, which is
    // treated as fatal for v1 rather than adding a speculative "reset my
    // config" recovery flow for a case that shouldn't arise in normal use.
    let app_state = AppState::load_or_default().expect("failed to load or initialize Village config");

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::list_servers,
            commands::add_server_from_code,
            commands::update_server,
            commands::delete_server,
            commands::export_invite_code,
            commands::generate_invite_code_from_fields,
            commands::save_raw_as_server,
            commands::connect,
            commands::disconnect,
            commands::get_status,
            commands::ensure_service_installed,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Village application");
}
