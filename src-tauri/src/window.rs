use tauri::WebviewWindow;

// kCGDesktopIconWindowLevel + 1: above Finder's full-screen desktop-icon
// window, below all app windows. One level lower (below the icon layer)
// looks fine but Finder's transparent icon window swallows every click.
#[cfg(target_os = "macos")]
const DESKTOP_LEVEL: isize = -2147483603 + 1;
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

/// The HUD covers a large desktop area, so by default it must not eat clicks
/// meant for icons and windows beneath it. Mouse events are ignored unless
/// the user holds Option; polling the global modifier state is the only way
/// to notice the key while the window is ignoring input.
#[cfg(target_os = "macos")]
pub fn spawn_interactivity_watch(window: WebviewWindow) {
    use tauri::Emitter;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state: i32) -> u64;
    }
    const ALT_FLAG: u64 = 0x0008_0000; // kCGEventFlagMaskAlternate

    tauri::async_runtime::spawn(async move {
        let mut interactive: Option<bool> = None;
        loop {
            let alt = unsafe { CGEventSourceFlagsState(0) } & ALT_FLAG != 0;
            if interactive != Some(alt) {
                interactive = Some(alt);
                if let Err(e) = window.set_ignore_cursor_events(!alt) {
                    eprintln!("set_ignore_cursor_events failed: {e}");
                }
                let _ = window.emit("interactive", alt);
            }
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn spawn_interactivity_watch(_window: WebviewWindow) {}

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
