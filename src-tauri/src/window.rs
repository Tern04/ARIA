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

/// Windows: glue the HUD to the wallpaper layer by re-parenting it into the
/// WorkerW window Explorer draws the wallpaper on (the Rainmeter/Wallpaper
/// Engine technique) — behind every app window and the desktop icons.
#[cfg(target_os = "windows")]
pub fn set_desktop_layer(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        eprintln!("desktop layer: window has no HWND");
        return;
    };
    match win_desktop::desktop_parent() {
        Some(parent) => {
            win_desktop::DESKTOP_PARENT.store(parent, std::sync::atomic::Ordering::Relaxed);
            win_desktop::reparent(hwnd.0 as isize, Some(parent));
        }
        None => eprintln!("desktop layer: no Progman/WorkerW found; HUD stays a normal window"),
    }
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
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows::core::{w, BOOL};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, SendMessageTimeoutW, SetParent, SetWindowPos,
        HWND_NOTOPMOST, HWND_TOPMOST, SMTO_NORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    /// Wallpaper parent (WorkerW or Progman) found at startup, kept so the
    /// pin toggle can re-attach after floating the HUD above app windows.
    pub static DESKTOP_PARENT: AtomicIsize = AtomicIsize::new(0);

    fn hwnd(v: isize) -> HWND {
        HWND(v as *mut core::ffi::c_void)
    }

    /// Ask Explorer to spawn the wallpaper WorkerW and return it. On shells
    /// where none appears (Win11 24H2 hosts the icons under Progman itself),
    /// Progman is the right parent instead.
    pub fn desktop_parent() -> Option<isize> {
        unsafe {
            let progman = FindWindowW(w!("Progman"), None).ok()?;
            // 0x052C is the undocumented "split off a wallpaper WorkerW"
            // message Explorer has honored since Windows 8.
            let _ = SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0xD),
                LPARAM(0x1),
                SMTO_NORMAL,
                1000,
                None,
            );
            let mut found: isize = 0;
            let _ = EnumWindows(
                Some(find_worker),
                LPARAM(&mut found as *mut isize as isize),
            );
            if found != 0 {
                Some(found)
            } else {
                Some(progman.0 as isize)
            }
        }
    }

    /// The wallpaper WorkerW is the sibling right after the window hosting
    /// the desktop icons (SHELLDLL_DefView).
    extern "system" fn find_worker(h: HWND, out: LPARAM) -> BOOL {
        unsafe {
            if FindWindowExW(Some(h), None, w!("SHELLDLL_DefView"), None).is_ok() {
                if let Ok(worker) = FindWindowExW(None, Some(h), w!("WorkerW"), None) {
                    *(out.0 as *mut isize) = worker.0 as isize;
                    return BOOL(0); // stop enumerating
                }
            }
            BOOL(1)
        }
    }

    pub fn reparent(hud: isize, parent: Option<isize>) {
        unsafe {
            if let Err(e) = SetParent(hwnd(hud), parent.map(hwnd)) {
                eprintln!("desktop layer: SetParent failed: {e}");
            }
        }
    }

    /// Pin above everything, or drop back into the wallpaper layer.
    pub fn set_overlay(hud: isize, above: bool) {
        unsafe {
            if above {
                reparent(hud, None);
                let _ = SetWindowPos(
                    hwnd(hud),
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            } else {
                let _ = SetWindowPos(
                    hwnd(hud),
                    Some(HWND_NOTOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                let parent = DESKTOP_PARENT.load(Ordering::Relaxed);
                if parent != 0 {
                    reparent(hud, Some(parent));
                }
            }
        }
    }
}
