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
        // Drop the macOS window shadow: on a translucent, mostly-transparent
        // HUD its rounded-rect corners bleed through the panels as uneven
        // curves. Without it, only the panels' own clean corners show.
        let _: () = msg_send![ns_window, setHasShadow: false];
    }
}

/// Windows: pin the HUD to the bottom of the z-order — above the wallpaper
/// and desktop icons, below every app window. A normal top-level window is
/// used on purpose: re-parenting into Explorer's wallpaper WorkerW (the
/// Wallpaper Engine trick) puts the HUD behind SHELLDLL_DefView, which eats
/// all mouse input, and leaves a stale frame on the wallpaper if the process
/// dies. Called from Tauri's setup hook, which runs on the main thread (the
/// window subclass must be installed from the thread that owns the HWND).
#[cfg(target_os = "windows")]
pub fn set_desktop_layer(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        eprintln!("desktop layer: window has no HWND");
        return;
    };
    win_desktop::init_desktop_layer(hwnd.0 as isize);
}

/// Linux (_NET_WM_WINDOW_TYPE_DESKTOP hint) is implemented once that machine
/// is available for testing. Until then the HUD behaves as a normal window.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn set_desktop_layer(_window: &WebviewWindow) {}

#[cfg(target_os = "macos")]
fn alt_pressed() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state: i32) -> u64;
    }
    const ALT_FLAG: u64 = 0x0008_0000; // kCGEventFlagMaskAlternate
    unsafe { CGEventSourceFlagsState(0) & ALT_FLAG != 0 }
}

#[cfg(target_os = "windows")]
fn alt_pressed() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};
    (unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } as u16) & 0x8000 != 0
}

/// The HUD covers a large desktop area, so by default it must not eat clicks
/// meant for icons and windows beneath it. Mouse events are ignored unless
/// the user holds Option/Alt; polling the global modifier state is the only
/// way to notice the key while the window is ignoring input.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn spawn_interactivity_watch(window: WebviewWindow) {
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        let mut interactive: Option<bool> = None;
        loop {
            let alt = alt_pressed();
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

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
    #[cfg(target_os = "windows")]
    {
        let w = window.clone();
        window
            .run_on_main_thread(move || {
                if let Ok(hwnd) = w.hwnd() {
                    win_desktop::set_overlay(hwnd.0 as isize, above);
                }
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (window, above);
        Ok(())
    }
}

/// Raw Win32 plumbing for the desktop layer. HWNDs travel as isize so the
/// version of the `windows` crate tauri links internally never has to match
/// the one used here.
#[cfg(target_os = "windows")]
mod win_desktop {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_BOTTOM,
        HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        WINDOWPOS, WM_NCDESTROY, WM_WINDOWPOSCHANGING, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };

    /// While true, the subclass proc clamps every z-order change to
    /// HWND_BOTTOM, so click-activation can never raise the HUD above app
    /// windows. Cleared while the pin toggle floats the HUD as an overlay.
    static FORCE_BOTTOM: AtomicBool = AtomicBool::new(true);

    const SUBCLASS_ID: usize = 1;

    fn hwnd(v: isize) -> HWND {
        HWND(v as *mut core::ffi::c_void)
    }

    /// Clamp z-order changes to the bottom while in desktop mode. Only
    /// hwndInsertAfter is rewritten — position and size pass through, so
    /// moving/resizing the HUD works in both modes.
    unsafe extern "system" fn bottom_clamp(
        h: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match msg {
            WM_WINDOWPOSCHANGING if FORCE_BOTTOM.load(Ordering::Relaxed) => {
                let wp = lparam.0 as *mut WINDOWPOS;
                if !wp.is_null() && !(*wp).flags.contains(SWP_NOZORDER) {
                    (*wp).hwndInsertAfter = HWND_BOTTOM;
                }
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(h, Some(bottom_clamp), SUBCLASS_ID);
            }
            _ => {}
        }
        DefSubclassProc(h, msg, wparam, lparam)
    }

    pub fn init_desktop_layer(hud: isize) {
        let h = hwnd(hud);
        unsafe {
            // Tool window: no taskbar button, no Alt-Tab entry — the
            // equivalent of macOS ignoresCycle. WS_EX_NOACTIVATE is left
            // off deliberately: the HUD has text inputs, and blocking
            // activation would keep the webview from taking keyboard focus.
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            SetWindowLongPtrW(
                h,
                GWL_EXSTYLE,
                (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize),
            );
            if !SetWindowSubclass(h, Some(bottom_clamp), SUBCLASS_ID, 0).as_bool() {
                eprintln!("desktop layer: SetWindowSubclass failed; HUD may raise on click");
            }
            FORCE_BOTTOM.store(true, Ordering::Relaxed);
            let _ = SetWindowPos(
                h,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    /// Pin above everything, or drop back to the bottom of the z-order.
    /// HWND_BOTTOM on a topmost window also clears its topmost status, so
    /// the return trip is a single call.
    pub fn set_overlay(hud: isize, above: bool) {
        let insert_after = if above {
            FORCE_BOTTOM.store(false, Ordering::Relaxed);
            HWND_TOPMOST
        } else {
            FORCE_BOTTOM.store(true, Ordering::Relaxed);
            HWND_BOTTOM
        };
        unsafe {
            let _ = SetWindowPos(
                hwnd(hud),
                Some(insert_after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}
