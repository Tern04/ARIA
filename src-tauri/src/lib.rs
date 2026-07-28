mod collectors;
mod window;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let main = app
                .get_webview_window("main")
                .expect("main window missing from config");
            window::set_desktop_layer(&main);
            window::spawn_interactivity_watch(main.clone());
            collectors::spawn_all(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            window::set_overlay,
            collectors::frontend_ready,
            collectors::music::music_control,
            collectors::music::music_playlists,
            collectors::music::music_play_playlist,
            collectors::stag::stag_login,
            collectors::stag::stag_logout,
            collectors::stag::stag_refresh,
            collectors::email::email_refresh,
            collectors::github::github_refresh,
            collectors::crypto::crypto_refresh,
            collectors::discord::discord_setup,
            collectors::discord::discord_logout,
            collectors::discord::discord_list_friends,
            collectors::discord::discord_add_friend,
            collectors::discord::discord_remove_friend,
            collectors::github::github_setup,
            collectors::github::github_logout,
            collectors::email::mail_add_account,
            collectors::email::mail_remove_account,
            collectors::calendar::cal_add_feed,
            collectors::calendar::cal_remove_feed,
            collectors::calendar::cal_refresh
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
