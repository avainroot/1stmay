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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_launcher_state,
            commands::get_game_status,
            commands::get_memory_status,
            commands::save_install_directory,
            commands::play
        ])
        .run(tauri::generate_context!())
        .expect("failed to run 1stMay launcher");
}
