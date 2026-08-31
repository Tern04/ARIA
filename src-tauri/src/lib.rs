mod collectors;
mod wallpaper;
mod window;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // On the NVIDIA proprietary driver under a Wayland compositor, this
    // transparent window mispresents damaged regions (widget updates, hover) as
    // opaque black. These env vars work around the two known culprits — the
    // DMABUF renderer path and NVIDIA explicit sync (buffers scanned out before
    // the render completes). They must be set before the webview (GTK)
    // initializes. Each is only set if not already provided, so a launch can
    // override any of them for debugging.
    #[cfg(target_os = "linux")]
    {
        let set_default = |k: &str, v: &str| {
            if std::env::var_os(k).is_none() {
                std::env::set_var(k, v);
            }
        };
        set_default("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        set_default("__NV_DISABLE_EXPLICIT_SYNC", "1");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
            window::spawn_wallpaper_watch(main.clone());
            // The asset-protocol scope is runtime state, so the saved
            // wallpaper has to be re-granted on every launch.
            wallpaper::init(app.handle());
            collectors::spawn_all(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            window::set_overlay,
            window::desktop_background,
            window::display_server,
            wallpaper::wallpaper_config,
            wallpaper::wallpaper_set,
            wallpaper::wallpaper_media_url,
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
