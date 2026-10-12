// Prevents the console window from showing up on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            lustre_lib::commands::get_devices,
            lustre_lib::commands::get_saved_cards,
            lustre_lib::commands::rename_card,
            lustre_lib::commands::delete_card,
            lustre_lib::commands::start_scan,
            lustre_lib::commands::stop_scan,
            lustre_lib::commands::flash_skin,
            lustre_lib::commands::flash_skin_batch,
            lustre_lib::commands::restore_original,
            lustre_lib::commands::check_backup_exists,
            lustre_lib::commands::check_apple_support,
            lustre_lib::commands::get_gallery_skins,
            lustre_lib::commands::detect_bank_name,
        ])
        .run(tauri::generate_context!())
        .expect("failed to launch lustre");
}
