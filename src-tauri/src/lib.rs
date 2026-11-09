mod commands;
mod config;
mod launch;
mod memory;
mod metadata;
mod minecraft;
mod platform;
mod runtime;
mod settings;
mod skin;
mod sync;
mod updater;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_launcher_state,
            commands::get_game_status,
            commands::get_memory_status,
            commands::save_install_directory,
            commands::play,
            commands::synchronize_config_files,
            skin::get_current_texture,
            skin::load_texture_preview,
            skin::save_texture,
            skin::delete_texture,
            updater::check_and_download_update,
            updater::install_downloaded_update
        ])
        .run(tauri::generate_context!())
        .expect("failed to run 1stMay launcher");
}
