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

/// Linux: pin the HUD to the desktop layer with EWMH window states on X11
/// (see `x11_desktop` for why the `_NET_WM_WINDOW_TYPE_DESKTOP` hint is not
/// used). Under Wayland the HUD stays a normal window — COSMIC's layer-shell
/// is the mechanism there, and GNOME/Wayland offers none at all.
#[cfg(target_os = "linux")]
pub fn set_desktop_layer(_window: &WebviewWindow) {
    if x11_desktop::available() {
        x11_desktop::init();
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
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
    #[cfg(target_os = "linux")]
    {
        let _ = window;
        if x11_desktop::available() {
            x11_desktop::set_overlay(above);
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
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

/// Resolve the current desktop wallpaper file, asking the desktop that is
/// actually running first.
///
/// Both sources persist across sessions, so a fixed order lets a dormant one
/// win: a Pop!_OS box that has booted COSMIC even once keeps its
/// `com.system76.CosmicBackground` state file forever, and the image it names
/// (a stock `/usr/share/backgrounds/cosmic/…` one) still exists — so trying
/// COSMIC unconditionally shows that wallpaper on a GNOME session while the
/// real one sits unread in gsettings. Picking by session makes the running
/// desktop authoritative and leaves the other as a fallback.
#[cfg(target_os = "linux")]
fn current_wallpaper_path() -> Option<std::path::PathBuf> {
    if running_cosmic() {
        cosmic_wallpaper_path().or_else(gnome_wallpaper_path)
    } else {
        gnome_wallpaper_path().or_else(cosmic_wallpaper_path)
    }
}

/// Whether cosmic-comp is the session's desktop. `XDG_CURRENT_DESKTOP` is a
/// colon-separated list, so match a component rather than the whole value.
#[cfg(target_os = "linux")]
fn running_cosmic() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|v| {
        v.split(':')
            .any(|part| part.eq_ignore_ascii_case("cosmic"))
    })
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

/// X11 desktop layer: EWMH window *states* on an ordinary managed window.
///
/// `_NET_WM_WINDOW_TYPE_DESKTOP` — the hint Conky-style widgets use — is not a
/// workable path under Mutter. GNOME Shell draws the wallpaper itself and was
/// never designed for a client to claim that layer, so the placement code for a
/// desktop-type window is an untested path: it pinned the window to the far
/// edge of the combined virtual screen and applied a fresh "new window" offset
/// on top of the previous position on every relaunch (y drifted -74, then
/// -148). Re-asserting the position from a move/resize watcher only fed the
/// loop, since the correction is itself a move.
///
/// So the type hint is left alone and the layer is expressed as state instead:
/// BELOW keeps the HUD under every normal window but above the wallpaper,
/// STICKY shows it on all workspaces, and SKIP_TASKBAR/SKIP_PAGER take it out
/// of the switcher — the equivalents of the macOS collection behaviour and the
/// Windows tool-window + bottom clamp. All four are ordinary, supported paths
/// (they appear in Mutter's `_NET_SUPPORTED`), the window stays managed, and
/// BELOW is a persistent layer rather than a per-raise decision — so unlike the
/// Windows path no watcher is needed to stop a click from raising the HUD.
///
/// X11 only. Under Wayland the webview has no X window; COSMIC's layer-shell is
/// the mechanism there, and GNOME/Wayland offers none at all.
#[cfg(target_os = "linux")]
mod x11_desktop {
    use std::error::Error;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, EventMask,
        Window,
    };
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    type Res<T> = Result<T, Box<dyn Error>>;

    const ADD: u32 = 1;
    const REMOVE: u32 = 0;
    /// _NET_WM_STATE source indication: 1 = normal application.
    const SOURCE_APP: u32 = 1;

    struct Conn {
        conn: RustConnection,
        root: Window,
        net_wm_state: u32,
        net_wm_pid: u32,
        net_client_list: u32,
        below: u32,
        above: u32,
        sticky: u32,
        skip_taskbar: u32,
        skip_pager: u32,
    }

    impl Conn {
        fn open() -> Res<Conn> {
            let (conn, screen) = x11rb::connect(None)?;
            let root = conn.setup().roots[screen].root;
            let intern = |name: &str| -> Res<u32> {
                Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
            };
            Ok(Conn {
                net_wm_state: intern("_NET_WM_STATE")?,
                net_wm_pid: intern("_NET_WM_PID")?,
                net_client_list: intern("_NET_CLIENT_LIST")?,
                below: intern("_NET_WM_STATE_BELOW")?,
                above: intern("_NET_WM_STATE_ABOVE")?,
                sticky: intern("_NET_WM_STATE_STICKY")?,
                skip_taskbar: intern("_NET_WM_STATE_SKIP_TASKBAR")?,
                skip_pager: intern("_NET_WM_STATE_SKIP_PAGER")?,
                conn,
                root,
            })
        }

        /// Our own toplevel, found by matching `_NET_WM_PID` against this
        /// process across `_NET_CLIENT_LIST`. Going through the X server rather
        /// than `WebviewWindow::gtk_window()` keeps the `gtk` crate (and the
        /// job of matching the exact version tauri links) out of the build.
        /// Only managed, mapped windows are listed, which is also exactly when
        /// a `_NET_WM_STATE` client message is the correct way to set state.
        fn find_toplevel(&self) -> Res<Option<Window>> {
            let pid = std::process::id();
            let list = self
                .conn
                .get_property(
                    false,
                    self.root,
                    self.net_client_list,
                    AtomEnum::WINDOW,
                    0,
                    // In 32-bit units: far more toplevels than a session has,
                    // and well clear of any server-side length overflow.
                    1024,
                )?
                .reply()?;
            let Some(windows) = list.value32() else {
                return Ok(None);
            };
            for w in windows {
                let prop = self
                    .conn
                    .get_property(false, w, self.net_wm_pid, AtomEnum::CARDINAL, 0, 1)?
                    .reply()?;
                if prop.value32().and_then(|mut v| v.next()) == Some(pid) {
                    return Ok(Some(w));
                }
            }
            Ok(None)
        }

        /// EWMH `_NET_WM_STATE` request. A mapped, managed window's state is
        /// changed by asking the WM with a client message to the root, not by
        /// writing the property directly.
        fn set_state(&self, window: Window, action: u32, first: u32, second: u32) -> Res<()> {
            let event = ClientMessageEvent::new(
                32,
                window,
                self.net_wm_state,
                [action, first, second, SOURCE_APP, 0],
            );
            self.conn.send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                event,
            )?;
            self.conn.flush()?;
            Ok(())
        }
    }

    /// The resolved toplevel, cached so the pin toggle doesn't re-scan the
    /// client list on every call.
    static HUD: Mutex<Option<Window>> = Mutex::new(None);

    /// Which layer the HUD is supposed to be on. The watcher restores states
    /// toward this, so re-asserting can never undo the pin toggle.
    static WANT_ABOVE: AtomicBool = AtomicBool::new(false);

    fn warn(msg: &str) {
        eprintln!("desktop layer x11: {msg}");
    }

    /// True when the session is a real X11 one. Under Wayland the webview is a
    /// Wayland surface with no X window to address, and XWayland would only
    /// ever hand back some other client's.
    pub fn available() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some()
    }

    /// Apply the desktop-layer states. The window is not in `_NET_CLIENT_LIST`
    /// until Mutter has managed it, which has not necessarily happened by the
    /// time tauri's setup hook runs, so this retries briefly before giving up.
    pub fn init() {
        tauri::async_runtime::spawn(async move {
            let conn = match Conn::open() {
                Ok(c) => c,
                Err(e) => return warn(&format!("connect failed: {e} — HUD stays a normal window")),
            };
            let mut window = None;
            for _ in 0..50 {
                match conn.find_toplevel() {
                    Ok(Some(w)) => {
                        window = Some(w);
                        break;
                    }
                    Ok(None) => {}
                    Err(e) => return warn(&format!("client list unreadable: {e}")),
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let Some(w) = window else {
                return warn("own window never appeared in _NET_CLIENT_LIST — HUD stays a normal window");
            };
            *HUD.lock().unwrap() = Some(w);
            apply(&conn, w);
            watch(w);
        });
    }

    /// Put the HUD on its desired layer. Two atoms travel per request, so this
    /// is two calls, not four.
    fn apply(conn: &Conn, w: Window) {
        let layer = if WANT_ABOVE.load(Ordering::Relaxed) {
            conn.above
        } else {
            conn.below
        };
        for (a, b) in [(layer, conn.sticky), (conn.skip_taskbar, conn.skip_pager)] {
            if let Err(e) = conn.set_state(w, ADD, a, b) {
                warn(&format!("setting state failed: {e}"));
            }
        }
    }

    /// Restore the desktop-layer states whenever the WM drops them.
    ///
    /// Mutter converts a request for a monitor-sized undecorated window into
    /// fullscreen, and that conversion resets `_NET_WM_STATE` — so restoring a
    /// filled window at startup, or turning Fill screen on, silently undid the
    /// layer. (Toggling fullscreen on a settled window does *not* clear it,
    /// which is why this only showed up on the fill path.)
    ///
    /// Unlike the position watcher that made the `_NET_WM_WINDOW_TYPE_DESKTOP`
    /// attempt unusable, this cannot feed itself: re-adding a state moves
    /// nothing, and the re-assert is skipped unless something is actually
    /// missing, so the PropertyNotify our own change produces ends the cycle.
    fn watch(w: Window) {
        std::thread::spawn(move || {
            let conn = match Conn::open() {
                Ok(c) => c,
                Err(e) => return warn(&format!("state watch: connect failed: {e}")),
            };
            let aux = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
            let observed = conn
                .conn
                .change_window_attributes(w, &aux)
                .map_err(|e| e.to_string())
                .and_then(|c| c.check().map_err(|e| e.to_string()));
            if let Err(e) = observed {
                return warn(&format!("state watch: cannot observe the window: {e}"));
            }
            loop {
                match conn.conn.wait_for_event() {
                    Ok(Event::PropertyNotify(e)) if e.atom == conn.net_wm_state => {
                        if missing(&conn, w).unwrap_or(false) {
                            apply(&conn, w);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => return warn(&format!("state watch ended: {e}")),
                }
            }
        });
    }

    /// Whether any state the desktop layer depends on has been dropped.
    fn missing(conn: &Conn, w: Window) -> Result<bool, Box<dyn Error>> {
        let prop = conn
            .conn
            .get_property(false, w, conn.net_wm_state, AtomEnum::ATOM, 0, 32)?
            .reply()?;
        let held: Vec<u32> = prop.value32().map(Iterator::collect).unwrap_or_default();
        let layer = if WANT_ABOVE.load(Ordering::Relaxed) {
            conn.above
        } else {
            conn.below
        };
        Ok([layer, conn.sticky, conn.skip_taskbar, conn.skip_pager]
            .iter()
            .any(|a| !held.contains(a)))
    }

    /// Pin above every app window, or drop back to the desktop layer. BELOW and
    /// ABOVE are mutually exclusive in the WM, but it will not infer that from
    /// one being added — the other has to be removed explicitly.
    pub fn set_overlay(above: bool) {
        let Some(w) = *HUD.lock().unwrap() else {
            return warn("pin toggled before the window was resolved — ignored");
        };
        let conn = match Conn::open() {
            Ok(c) => c,
            Err(e) => return warn(&format!("connect failed: {e}")),
        };
        WANT_ABOVE.store(above, Ordering::Relaxed);
        let (add, remove) = if above {
            (conn.above, conn.below)
        } else {
            (conn.below, conn.above)
        };
        if let Err(e) = conn
            .set_state(w, REMOVE, remove, 0)
            .and_then(|()| conn.set_state(w, ADD, add, 0))
        {
            warn(&format!("pin toggle failed: {e}"));
        }
    }
}
