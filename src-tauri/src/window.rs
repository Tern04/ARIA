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

/// Linux transparency workaround: WebKitGTK on NVIDIA renders a transparent
/// window's see-through regions as flickering black garbage (reproduces on both
/// COSMIC/Wayland and GNOME/Xorg — see CLAUDE.md). To avoid any transparent
/// region, the Linux frontend paints an *opaque* background behind the glass.
/// This returns the current desktop wallpaper as a `data:` URI so that
/// background is the real wallpaper; the frontend sizes and offsets it to the
/// monitor so it lines up with the desktop showing around the window. Returns
/// `None` when no wallpaper can be resolved (solid-colour fallback frontend).
#[cfg(target_os = "linux")]
#[tauri::command]
pub fn desktop_background() -> Option<String> {
    encode_wallpaper(&current_wallpaper_path()?)
}

/// Read a wallpaper file and return it as a `data:` URI.
#[cfg(target_os = "linux")]
fn encode_wallpaper(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/png",
    };
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

/// Resolve the current desktop wallpaper file. Tries COSMIC's runtime state
/// (it rotates a folder, so gsettings holds no usable path) first, then the
/// GNOME/Pop!_OS gsettings key.
#[cfg(target_os = "linux")]
fn current_wallpaper_path() -> Option<std::path::PathBuf> {
    cosmic_wallpaper_path().or_else(gnome_wallpaper_path)
}

/// COSMIC writes the wallpaper currently shown on each output to
/// `~/.local/state/cosmic/com.system76.CosmicBackground/v1/wallpapers` as a RON
/// list of `("OUTPUT", Path("…"))` — refreshed on every rotation. Return the
/// first entry whose file exists (all outputs share one image in the common
/// "same on all" setup).
#[cfg(target_os = "linux")]
fn cosmic_wallpaper_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let state = std::path::Path::new(&home)
        .join(".local/state/cosmic/com.system76.CosmicBackground/v1/wallpapers");
    let text = std::fs::read_to_string(&state).ok()?;
    for chunk in text.split("Path(\"").skip(1) {
        if let Some(end) = chunk.find("\")") {
            let p = std::path::PathBuf::from(&chunk[..end]);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Read the GNOME/Pop!_OS wallpaper path from gsettings (prefer the dark
/// variant). Returns the file path if it exists.
#[cfg(target_os = "linux")]
fn gnome_wallpaper_path() -> Option<std::path::PathBuf> {
    for key in ["picture-uri-dark", "picture-uri"] {
        let out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.background", key])
            .output()
            .ok()?;
        let raw = String::from_utf8_lossy(&out.stdout);
        let uri = raw.trim().trim_matches('\'');
        if let Some(rest) = uri.strip_prefix("file://") {
            // Minimal percent-decode for the common cases (spaces etc.).
            let decoded = rest.replace("%20", " ");
            let p = std::path::PathBuf::from(decoded);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Watch for wallpaper changes — COSMIC rotates every few minutes, and the user
/// can change it manually — and push the new wallpaper to the frontend so the
/// opaque background never drifts out of sync with the real desktop. Cheaply
/// polls the resolved path and only re-encodes when it actually changes.
#[cfg(target_os = "linux")]
pub fn spawn_wallpaper_watch(window: WebviewWindow) {
    use tauri::Emitter;
    tauri::async_runtime::spawn(async move {
        let mut last = current_wallpaper_path();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let now = current_wallpaper_path();
            if now != last {
                last = now.clone();
                if let Some(uri) = now.as_deref().and_then(encode_wallpaper) {
                    let _ = window.emit("desktop-background", uri);
                }
            }
        }
    });
}

/// Report the display server the session is running under: "wayland", "x11",
/// or "unknown". The frontend only attempts wallpaper alignment on X11 — on
/// Wayland the compositor refuses to reveal a window's absolute position
/// (reports {0,0}), so alignment is impossible and the wallpaper is cover-fit
/// to the window instead.
#[cfg(target_os = "linux")]
#[tauri::command]
pub fn display_server() -> String {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "wayland".into()
    } else if std::env::var_os("DISPLAY").is_some() {
        "x11".into()
    } else {
        "unknown".into()
    }
}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub fn desktop_background() -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_wallpaper_watch(_window: WebviewWindow) {}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub fn display_server() -> String {
    "unknown".into()
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
