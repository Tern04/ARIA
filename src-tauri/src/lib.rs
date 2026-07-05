mod collectors;
mod window;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let main = app
                .get_webview_window("main")
                .expect("main window missing from config");
            window::set_desktop_layer(&main);
            collectors::spawn_all(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![window::set_overlay])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
