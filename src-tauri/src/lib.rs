// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

pub mod ble;
pub mod commands;
pub mod events;
pub mod protocol;
pub mod secure_storage;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(commands::shared_manager());
    #[cfg(target_os = "linux")]
    let builder = builder.plugin(tauri_plugin_dialog::init());
    builder
        .invoke_handler(tauri::generate_handler![
            commands::scan_devices,
            commands::connect,
            commands::disconnect,
            commands::authenticate,
            commands::get_storage_info,
            commands::install_watchface,
            #[cfg(target_os = "linux")]
            commands::get_saved_authkey,
            #[cfg(target_os = "linux")]
            commands::save_authkey,
            #[cfg(target_os = "linux")]
            commands::clear_saved_authkey,
            #[cfg(target_os = "android")]
            commands::get_saved_authkey,
            #[cfg(target_os = "android")]
            commands::save_authkey,
            #[cfg(target_os = "android")]
            commands::clear_saved_authkey,
            #[cfg(target_os = "android")]
            commands::pick_watchface_file,
            #[cfg(target_os = "android")]
            commands::read_authkey,
            #[cfg(target_os = "android")]
            commands::open_storage_permission_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
