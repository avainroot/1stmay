mod commands;
mod config;
mod launch;
mod memory;
mod metadata;
mod minecraft;
mod platform;
mod runtime;
mod settings;
mod sync;
mod updater;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_launcher_state,
            commands::get_game_status,
            commands::get_memory_status,
            commands::save_install_directory,
            commands::play,
            updater::check_and_download_update,
            updater::install_downloaded_update
        ])
        .run(tauri::generate_context!())
        .expect("failed to run 1stMay launcher");
}
