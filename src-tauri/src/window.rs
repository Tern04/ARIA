use tauri::WebviewWindow;

// kCGDesktopWindowLevel + 1: above the wallpaper, below desktop icons.
#[cfg(target_os = "macos")]
const DESKTOP_LEVEL: isize = -2147483623 + 1;
// NSFloatingWindowLevel: above normal app windows while pinned.
#[cfg(target_os = "macos")]
const OVERLAY_LEVEL: isize = 3;

#[cfg(target_os = "macos")]
fn set_window_level(window: &WebviewWindow, level: isize) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    // ns_window() must be called on the main thread.
    let ns_window = window.ns_window().map_err(|e| e.to_string())? as *mut AnyObject;
    unsafe {
        let _: () = msg_send![ns_window, setLevel: level];
    }
    Ok(())
}

/// Pin the HUD to the desktop layer: above the wallpaper, below every app
/// window, visible on all Spaces/workspaces, excluded from the app switcher.
/// Called from Tauri's setup hook, which runs on the main thread.
#[cfg(target_os = "macos")]
pub fn set_desktop_layer(window: &WebviewWindow) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    // canJoinAllSpaces | stationary | ignoresCycle
    const COLLECTION_BEHAVIOR: usize = (1 << 0) | (1 << 4) | (1 << 6);

    let ns_window = window
        .ns_window()
        .expect("main window has no NSWindow handle") as *mut AnyObject;
    unsafe {
        let _: () = msg_send![ns_window, setLevel: DESKTOP_LEVEL];
        let _: () = msg_send![ns_window, setCollectionBehavior: COLLECTION_BEHAVIOR];
    }
}

/// Windows (WorkerW parenting) and Linux (_NET_WM_WINDOW_TYPE_DESKTOP hint)
/// are implemented once those machines are available for testing. Until
/// then the HUD behaves as a normal frameless window there.
#[cfg(not(target_os = "macos"))]
pub fn set_desktop_layer(_window: &WebviewWindow) {}

/// Toggle between the desktop layer and a floating overlay above app windows.
#[tauri::command]
pub fn set_overlay(window: WebviewWindow, above: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let w = window.clone();
        let level = if above { OVERLAY_LEVEL } else { DESKTOP_LEVEL };
        window
            .run_on_main_thread(move || {
                if let Err(e) = set_window_level(&w, level) {
                    eprintln!("set_overlay failed: {e}");
                }
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, above);
        Ok(())
    }
}
