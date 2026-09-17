use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const TICK: Duration = Duration::from_secs(5);
// Ticks are not counted when the user has been idle longer than this.
const IDLE_LIMIT: f64 = 120.0;
// Enough rows to fill the large widget size; smaller sizes clip the rest.
const TOP_APPS: usize = 8;

#[derive(Serialize, Clone)]
struct ScreenTime {
    total: u64,
    apps: Vec<AppTime>,
    /// Total from the previous day's saved file, for the trend line.
    yesterday_total: Option<u64>,
    /// Which backend is feeding the tracker — the widget uses it to explain
    /// itself when a per-app breakdown isn't obtainable on this session.
    /// "macos" | "windows" | "x11" | "cosmic" | "wlr" | "xwayland" | "idle-only"
    /// | "unsupported"
    source: &'static str,
}

/// What has focus right now — the auto-switch rules' live signal.
#[derive(Serialize, Clone)]
struct Activity {
    app: Option<String>,
    idle: bool,
}

#[derive(Serialize, Clone)]
struct AppTime {
    name: String,
    secs: u64,
}

/// One day's tally. `total` is counted independently of `apps` so a session
/// where the focused window can't be resolved (GNOME on Wayland exposes no
/// such API) still reports honest screen time, just with no breakdown.
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
struct DayUsage {
    total: u64,
    apps: HashMap<String, u64>,
}

/// Day files written before `total` was split out are a bare app→secs map.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredDay {
    Modern(DayUsage),
    Legacy(HashMap<String, u64>),
}

impl From<StoredDay> for DayUsage {
    fn from(v: StoredDay) -> Self {
        match v {
            StoredDay::Modern(d) => d,
            StoredDay::Legacy(apps) => DayUsage {
                total: apps.values().sum(),
                apps,
            },
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut day = today();
        let mut usage = load_day(&app, &day).unwrap_or_default();
        let mut yesterday_total = load_total(&app, &yesterday());
        let mut tick = 0u32;
        loop {
            tokio::time::sleep(TICK).await;
            tick += 1;

            let now = today();
            if now != day {
                save_day(&app, &day, &usage);
                day = now;
                usage = DayUsage::default();
                yesterday_total = load_total(&app, &yesterday());
            }

            let idle = idle_seconds() >= IDLE_LIMIT;
            let focused = frontmost_app(&app);
            if !idle {
                usage.total += TICK.as_secs();
                if let Some(name) = &focused {
                    *usage.apps.entry(name.clone()).or_insert(0) += TICK.as_secs();
                }
            }
            // Every tick, not only on change: the board's auto-switch rules
            // time their own enter/exit delays and need a steady clock, and a
            // listener registered after a change would otherwise wait forever.
            if let Err(e) = app.emit("activity", Activity { app: focused, idle }) {
                eprintln!("activity emit failed: {e}");
            }

            if tick % 3 == 0 {
                if let Err(e) = app.emit("screentime", summarize(&usage, yesterday_total)) {
                    eprintln!("screentime emit failed: {e}");
                }
            }
            if tick % 12 == 0 {
                save_day(&app, &day, &usage);
            }
        }
    });
}

/// Other Unix without the required window/idle facilities — no tracker.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn spawn(_app: AppHandle) {}

fn summarize(usage: &DayUsage, yesterday_total: Option<u64>) -> ScreenTime {
    let mut apps: Vec<AppTime> = usage
        .apps
        .iter()
        .map(|(name, secs)| AppTime {
            name: name.clone(),
            secs: *secs,
        })
        .collect();
    apps.sort_by(|a, b| b.secs.cmp(&a.secs));
    apps.truncate(TOP_APPS);
    ScreenTime {
        total: usage.total,
        apps,
        yesterday_total,
        source: source(),
    }
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn yesterday() -> String {
    (chrono::Local::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string()
}

/// Sum of a saved day file, if one exists (the tracker has been writing
/// them since day one; this is the first reader).
fn load_total(app: &AppHandle, day: &str) -> Option<u64> {
    Some(load_day(app, day)?.total)
}

/// One day's screen time as `(total seconds, apps by time)` — what the
/// history report needs to say what the machine was used *for*. Today's file
/// is written every minute, so it lags by at most that.
pub fn day_totals(app: &AppHandle, day: &str) -> (Option<u64>, Vec<(String, u64)>) {
    let Some(usage) = load_day(app, day) else {
        return (None, Vec::new());
    };
    let mut apps: Vec<(String, u64)> = usage.apps.into_iter().collect();
    apps.sort_by(|a, b| b.1.cmp(&a.1));
    apps.truncate(TOP_APPS);
    (Some(usage.total), apps)
}

fn day_file(app: &AppHandle, day: &str) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?.join("screentime");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("{day}.json")))
}

fn load_day(app: &AppHandle, day: &str) -> Option<DayUsage> {
    let raw = std::fs::read_to_string(day_file(app, day)?).ok()?;
    serde_json::from_str::<StoredDay>(&raw).ok().map(Into::into)
}

fn save_day(app: &AppHandle, day: &str, usage: &DayUsage) {
    let Some(path) = day_file(app, day) else {
        return;
    };
    match serde_json::to_string(usage) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("screentime save failed: {e}");
            }
        }
        Err(e) => eprintln!("screentime serialize failed: {e}"),
    }
}

/// Name of the frontmost application. NSWorkspace is queried on the main
/// thread; the tracker loop itself runs on the async runtime.
#[cfg(target_os = "macos")]
fn frontmost_app(app: &AppHandle) -> Option<String> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use std::ffi::CStr;

    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let name = unsafe {
            let ws: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let front: *mut AnyObject = msg_send![ws, frontmostApplication];
            if front.is_null() {
                None
            } else {
                let name: *mut AnyObject = msg_send![front, localizedName];
                if name.is_null() {
                    None
                } else {
                    let utf8: *const std::os::raw::c_char = msg_send![name, UTF8String];
                    if utf8.is_null() {
                        None
                    } else {
                        Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
                    }
                }
            }
        };
        let _ = tx.send(name);
    })
    .ok()?;
    rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
}

#[cfg(target_os = "macos")]
fn source() -> &'static str {
    "macos"
}

#[cfg(target_os = "macos")]
fn idle_seconds() -> f64 {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        // state 0 = combined session, event type u32::MAX = any input
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }
    unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) }
}

/// Executable stem of the foreground window's process (e.g. "chrome").
#[cfg(target_os = "windows")]
fn frontmost_app(_app: &AppHandle) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            proc,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(proc);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    }
}

#[cfg(target_os = "windows")]
fn source() -> &'static str {
    "windows"
}

#[cfg(target_os = "windows")]
fn idle_seconds() -> f64 {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return 0.0; // unknown — count the tick rather than drop it
    }
    let now = unsafe { GetTickCount() };
    now.wrapping_sub(info.dwTime) as f64 / 1000.0
}

/// Which session facilities are available. Resolved once from the environment:
/// a Wayland session exposes the focused window only through a compositor
/// protocol, an X11 session through `_NET_ACTIVE_WINDOW`, and neither is
/// reachable from the other. Guessing wrong used to mean the Wayland client
/// failed to connect and logged "Could not find wayland compositor" every five
/// seconds for the lifetime of the app on every X11 session.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq)]
enum Backend {
    X11,
    Wayland,
    Unsupported,
}

#[cfg(target_os = "linux")]
fn backend() -> Backend {
    static B: std::sync::OnceLock<Backend> = std::sync::OnceLock::new();
    *B.get_or_init(|| {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Backend::Wayland
        } else if std::env::var_os("DISPLAY").is_some() {
            Backend::X11
        } else {
            Backend::Unsupported
        }
    })
}

/// Log a diagnostic at most once per distinct message. The Linux backends run
/// in retry loops, and a permanent condition (missing protocol, no X11 auth)
/// would otherwise scroll the console forever.
#[cfg(target_os = "linux")]
fn warn_once(msg: &str) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    if let Ok(mut seen) = SEEN.lock() {
        if seen.iter().any(|m| m == msg) {
            return;
        }
        seen.push(msg.to_string());
    }
    eprintln!("{msg}");
}

/// Whether the XWayland fallback is available: a Wayland session whose
/// compositor names no windows, but which still runs an X server we can ask.
/// Resolved once — `DISPLAY` does not appear mid-session.
#[cfg(target_os = "linux")]
fn xwayland_fallback() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| {
        backend() == Backend::Wayland
            && std::env::var_os("DISPLAY").is_some()
            && wl::source() == "idle-only"
    })
}

#[cfg(target_os = "linux")]
fn frontmost_app(_app: &AppHandle) -> Option<String> {
    match backend() {
        Backend::X11 => x11::active_app(),
        Backend::Wayland => {
            wl::ensure_started();
            // GNOME/Mutter implements no toplevel protocol, so the Wayland
            // client can never name a window there. XWayland still can, and
            // every Proton game is an XWayland client — which is the whole
            // reason the game trigger exists. Native Wayland windows stay
            // invisible to this, so it is a fallback and never an override:
            // a compositor that does name windows is always believed first.
            wl::active_app().or_else(|| xwayland_fallback().then(x11::focused_app).flatten())
        }
        Backend::Unsupported => None,
    }
}

#[cfg(target_os = "linux")]
fn idle_seconds() -> f64 {
    match backend() {
        // MIT-SCREEN-SAVER gives a real running counter.
        Backend::X11 => x11::idle_seconds(),
        // ext-idle-notify reports idle as a threshold crossing (idled/resumed
        // at `IDLE_LIMIT`), not a counter, so map that boolean back onto the
        // numeric contract the shared loop expects. Until the client connects
        // this reports "active", so early ticks count rather than drop.
        Backend::Wayland => {
            wl::ensure_started();
            if wl::is_idle() {
                IDLE_LIMIT + 1.0
            } else {
                0.0
            }
        }
        Backend::Unsupported => 0.0,
    }
}

#[cfg(target_os = "linux")]
fn source() -> &'static str {
    match backend() {
        Backend::X11 => "x11",
        Backend::Wayland => {
            wl::ensure_started();
            // Naming *some* windows is a different state from naming none, and
            // the widget says so: an app breakdown that silently omits every
            // native Wayland window would read as a wrong total, not a partial
            // one.
            match wl::source() {
                "idle-only" if xwayland_fallback() => "xwayland",
                other => other,
            }
        }
        Backend::Unsupported => "unsupported",
    }
}

/// A reverse-DNS app id or WM_CLASS → a short display label:
/// "com.system76.CosmicTerm" → "CosmicTerm", "firefox" → "Firefox".
#[cfg(target_os = "linux")]
fn friendly(app_id: &str) -> String {
    let base = app_id.strip_suffix(".desktop").unwrap_or(app_id);
    let seg = base.rsplit('.').next().unwrap_or(base);
    let seg = if seg.is_empty() { base } else { seg };
    let mut chars = seg.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => app_id.to_string(),
    }
}

/// X11 backend: the focused window from `_NET_ACTIVE_WINDOW` (EWMH, honoured by
/// Mutter, KWin, i3, …) and idle time from the MIT-SCREEN-SAVER extension — the
/// same source `xprintidle` uses. Pure Rust over the X11 socket via x11rb, so
/// there is no libX11/libxcb link dependency.
///
/// Fails soft the same way the Wayland client does: any error reports "no
/// active app" / "not idle" and drops the connection so the next tick (5 s)
/// reconnects.
#[cfg(target_os = "linux")]
mod x11 {
    use std::error::Error;
    use std::sync::Mutex;
    use x11rb::connection::{Connection as _, RequestConnection as _};
    use x11rb::protocol::screensaver::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};
    use x11rb::rust_connection::RustConnection;

    type Res<T> = Result<T, Box<dyn Error>>;

    struct Conn {
        conn: RustConnection,
        root: Window,
        net_active_window: u32,
        net_wm_name: u32,
        utf8_string: u32,
        has_screensaver: bool,
    }

    impl Conn {
        fn open() -> Res<Conn> {
            let (conn, screen) = x11rb::connect(None)?;
            let root = conn.setup().roots[screen].root;
            let intern = |name: &str| -> Res<u32> {
                Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
            };
            let net_active_window = intern("_NET_ACTIVE_WINDOW")?;
            let net_wm_name = intern("_NET_WM_NAME")?;
            let utf8_string = intern("UTF8_STRING")?;
            let has_screensaver = conn
                .extension_information(x11rb::protocol::screensaver::X11_EXTENSION_NAME)?
                .is_some();
            // Only worth saying on a real X11 session, where this connection
            // is also the idle source. On the XWayland fallback the idle
            // signal comes from Wayland's ext-idle-notify and X is asked for
            // nothing but the focused window, so the warning would send the
            // next reader looking for a fault that isn't there.
            if !has_screensaver && super::backend() == super::Backend::X11 {
                super::warn_once(
                    "screentime x11: MIT-SCREEN-SAVER unavailable — idle time will not be detected",
                );
            }
            Ok(Conn {
                conn,
                root,
                net_active_window,
                net_wm_name,
                utf8_string,
                has_screensaver,
            })
        }
    }

    /// Run `f` against a live connection, opening one on demand. A failure is
    /// reported once and the connection dropped, so the next call reconnects.
    fn with_conn<T>(f: impl FnOnce(&Conn) -> Res<T>) -> Option<T> {
        static CONN: Mutex<Option<Conn>> = Mutex::new(None);
        let mut guard = CONN.lock().ok()?;
        if guard.is_none() {
            match Conn::open() {
                Ok(c) => *guard = Some(c),
                Err(e) => {
                    super::warn_once(&format!("screentime x11: {e}"));
                    return None;
                }
            }
        }
        match f(guard.as_ref()?) {
            Ok(v) => Some(v),
            Err(e) => {
                super::warn_once(&format!("screentime x11: {e}"));
                *guard = None;
                None
            }
        }
    }

    /// WM_CLASS is "instance\0class\0"; the class half is the stable, tidier
    /// one ("Navigator"/"firefox" → prefer "firefox"'s class "Firefox").
    pub(super) fn class_name(raw: &[u8]) -> Option<String> {
        let mut parts = raw.split(|b| *b == 0).filter(|p| !p.is_empty());
        let instance = parts.next();
        let class = parts.next().or(instance)?;
        let name = super::friendly(String::from_utf8_lossy(class).trim());
        (!name.is_empty()).then_some(name)
    }

    pub fn active_app() -> Option<String> {
        with_conn(|c| {
            let focused = c
                .conn
                .get_property(false, c.root, c.net_active_window, AtomEnum::WINDOW, 0, 1)?
                .reply()?;
            let Some(win) = focused.value32().and_then(|mut v| v.next()) else {
                return Ok(None); // no EWMH property — nothing focused
            };
            if win == 0 {
                return Ok(None);
            }
            // Per-window reads race with the window closing; a protocol error
            // there means "unknown", not "the connection is broken".
            let class = c
                .conn
                .get_property(false, win, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
                .reply();
            if let Ok(class) = class {
                if let Some(name) = class_name(&class.value) {
                    return Ok(Some(name));
                }
            }
            let title = c
                .conn
                .get_property(false, win, c.net_wm_name, c.utf8_string, 0, 256)?
                .reply();
            Ok(title.ok().and_then(|t| {
                let s = String::from_utf8_lossy(&t.value).trim().to_string();
                (!s.is_empty()).then(|| super::friendly(&s))
            }))
        })
        .flatten()
    }

    /// The focused toplevel, for the XWayland fallback only.
    ///
    /// Asks two independent questions and believes the answer only when they
    /// agree: `_NET_ACTIVE_WINDOW` must name a window, and the X input focus
    /// must sit inside that same window. Measured on GNOME 48/Mutter with a
    /// Proton game running (2026-09-17):
    ///
    /// ```text
    ///   in the game      focus 0x05400003 steam_app_2357570   active 0x05400003
    ///   alt-tabbed out   focus 0x00600003 <no WM_CLASS>       active 0x00000000
    /// ```
    ///
    /// Mutter clears `_NET_ACTIVE_WINDOW` when a native Wayland window takes
    /// focus, and XWayland parks the input focus on an internal window that
    /// carries no WM_CLASS — so either signal alone would do here. Requiring
    /// both is for the compositor that does neither: a stale "the game is in
    /// front" would pin the Gaming board to a game you left an hour ago, and
    /// this project has already shipped that bug once. Disagreement reads as
    /// "nothing of ours is focused", which is the safe direction — a rule that
    /// lets go too eagerly is a worse board for a moment, not a stuck one.
    ///
    /// The input focus is usually a child of the toplevel (a focus proxy), so
    /// it is walked up to the direct child of the root before comparing; only
    /// toplevels carry WM_CLASS.
    pub fn focused_app() -> Option<String> {
        with_conn(|c| {
            let active = c
                .conn
                .get_property(false, c.root, c.net_active_window, AtomEnum::WINDOW, 0, 1)?
                .reply()?
                .value32()
                .and_then(|mut v| v.next())
                .unwrap_or(0);
            // 0 = no X client is active. Under XWayland that is the honest
            // report that a Wayland window has the focus.
            if active == 0 {
                return Ok(None);
            }
            // 0 = None, 1 = PointerRoot: no client window holds the focus.
            let focus = c.conn.get_input_focus()?.reply()?.focus;
            if focus <= 1 || focus == c.root {
                return Ok(None);
            }
            let mut win = focus;
            for _ in 0..MAX_DEPTH {
                if win == active {
                    break;
                }
                let Ok(tree) = c.conn.query_tree(win)?.reply() else {
                    return Ok(None); // the window went away mid-walk
                };
                if tree.parent == c.root || tree.parent == 0 {
                    break;
                }
                win = tree.parent;
            }
            if win != active {
                return Ok(None); // the two disagree — believe neither
            }
            let class = c
                .conn
                .get_property(false, win, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
                .reply();
            // No WM_CLASS is not a client window; deliberately no _NET_WM_NAME
            // fallback here, unlike active_app — a title is not an app id, and
            // the game trigger matches on the id.
            Ok(class.ok().and_then(|cl| class_name(&cl.value)))
        })
        .flatten()
    }

    /// Depth cap on the walk to the toplevel; a reparenting WM adds one or two
    /// frames, never sixteen, and a cycle must not hang the tracker.
    const MAX_DEPTH: usize = 16;

    /// Seconds since the last keyboard/pointer input. 0 (i.e. "active") when
    /// the extension is missing, so ticks are counted rather than dropped.
    pub fn idle_seconds() -> f64 {
        with_conn(|c| {
            if !c.has_screensaver {
                return Ok(0.0);
            }
            Ok(c.conn
                .screensaver_query_info(c.root)?
                .reply()
                .map(|i| i.ms_since_user_input as f64 / 1000.0)
                .unwrap_or(0.0))
        })
        .unwrap_or(0.0)
    }
}

/// Background Wayland client feeding the screen-time tracker on Linux. It binds
/// COSMIC's `zcosmic_toplevel_info_v1` (which window is `activated`) and the
/// standard `ext_idle_notifier_v1` (idle threshold), publishing the focused
/// app id and an idle flag into shared state the tracker loop polls. Bindings
/// are generated from the vendored protocol XML under `protocols/`.
///
/// Everything is gated to Linux and fails soft: on a non-COSMIC compositor (no
/// toplevel-info global) or any protocol error the client simply reports no
/// active app, and the tracker records nothing — the same visible result as
/// the old no-op stub.
#[cfg(target_os = "linux")]
mod wl {
    use super::IDLE_LIMIT;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::{Mutex, Once};
    use std::time::Duration;
    use wayland_client::backend::ObjectId;
    use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

    pub mod cosmic {
        use wayland_client;
        use wayland_client::protocol::*;
        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/cosmic-toplevel-info-v1.xml");
        }
        use self::__interfaces::*;
        wayland_scanner::generate_client_code!("protocols/cosmic-toplevel-info-v1.xml");
    }
    pub mod wlr {
        use wayland_client;
        use wayland_client::protocol::*;
        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!(
                "protocols/wlr-foreign-toplevel-management-unstable-v1.xml"
            );
        }
        use self::__interfaces::*;
        wayland_scanner::generate_client_code!(
            "protocols/wlr-foreign-toplevel-management-unstable-v1.xml"
        );
    }

    pub mod idle {
        use wayland_client;
        use wayland_client::protocol::*;
        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/ext-idle-notify-v1.xml");
        }
        use self::__interfaces::*;
        wayland_scanner::generate_client_code!("protocols/ext-idle-notify-v1.xml");
    }

    use cosmic::zcosmic_toplevel_handle_v1::{Event as HEvent, ZcosmicToplevelHandleV1};
    use cosmic::zcosmic_toplevel_info_v1::ZcosmicToplevelInfoV1;
    use cosmic::zcosmic_workspace_handle_v1::ZcosmicWorkspaceHandleV1;
    use idle::ext_idle_notification_v1::{Event as NEvent, ExtIdleNotificationV1};
    use idle::ext_idle_notifier_v1::ExtIdleNotifierV1;
    use wlr::zwlr_foreign_toplevel_handle_v1::{Event as WEvent, ZwlrForeignToplevelHandleV1};
    use wlr::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;

    static ACTIVE: Mutex<Option<String>> = Mutex::new(None);
    static IDLE: AtomicBool = AtomicBool::new(false);
    static START: Once = Once::new();

    /// Which toplevel protocol the compositor turned out to offer. GNOME/Mutter
    /// offers neither, but does implement ext-idle-notify — so screen time is
    /// still countable there, just without a per-app breakdown.
    const SRC_IDLE_ONLY: u8 = 0;
    const SRC_COSMIC: u8 = 1;
    const SRC_WLR: u8 = 2;
    static SOURCE: AtomicU8 = AtomicU8::new(SRC_IDLE_ONLY);

    pub fn source() -> &'static str {
        match SOURCE.load(Ordering::Relaxed) {
            SRC_COSMIC => "cosmic",
            SRC_WLR => "wlr",
            _ => "idle-only",
        }
    }

    /// Friendly name of the focused app, or None when unknown/nothing focused.
    pub fn active_app() -> Option<String> {
        ACTIVE.lock().ok().and_then(|g| g.clone())
    }

    /// Whether the session has been idle past `IDLE_LIMIT`.
    pub fn is_idle() -> bool {
        IDLE.load(Ordering::Relaxed)
    }

    /// Start the Wayland client once, on the first tracker tick. Only ever
    /// reached when `WAYLAND_DISPLAY` is set (see `Backend`), so it can no
    /// longer spin on "Could not find wayland compositor" in an X11 session.
    pub fn ensure_started() {
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("aria-screentime-wl".into())
                .spawn(run);
        });
    }

    fn run() {
        // Reconnect on any protocol error or disconnect so a transient failure
        // (or a compositor restart) can't permanently stop tracking. A
        // *permanent* failure must not scroll the console, so the message is
        // logged once per distinct text and the retry backs off.
        let mut delay = Duration::from_secs(5);
        const MAX_DELAY: Duration = Duration::from_secs(120);
        loop {
            match run_once() {
                Ok(()) => delay = Duration::from_secs(5),
                Err(e) => {
                    super::warn_once(&format!("screentime wayland: {e}"));
                    if let Ok(mut g) = ACTIVE.lock() {
                        *g = None; // don't keep crediting a stale app after a drop
                    }
                    delay = (delay * 2).min(MAX_DELAY);
                }
            }
            std::thread::sleep(delay);
        }
    }

    /// One connection lifetime: bind the globals, arm idle, then dispatch until
    /// the connection errors or closes.
    fn run_once() -> Result<(), Box<dyn std::error::Error>> {
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());

        let mut w = Watcher::default();
        // First roundtrip resolves globals and binds them (registry handler).
        queue.roundtrip(&mut w)?;

        // Arm a single idle notification at the tracker's idle threshold.
        if let (Some(notifier), Some(seat)) = (w.notifier.clone(), w.seat.clone()) {
            let timeout_ms = (IDLE_LIMIT as u32).saturating_mul(1000);
            notifier.get_idle_notification(timeout_ms, &seat, &qh, ());
        }

        loop {
            queue.blocking_dispatch(&mut w)?;
        }
    }

    #[derive(Default)]
    struct Watcher {
        app_ids: HashMap<ObjectId, String>,
        active: Option<ObjectId>,
        seat: Option<wl_seat::WlSeat>,
        notifier: Option<ExtIdleNotifierV1>,
    }

    /// The `state` arg is a packed array of u32 enum values; `activated` (2)
    /// marks the focused window. Both protocols use the same numbering.
    fn is_activated(bytes: &[u8]) -> bool {
        bytes
            .chunks_exact(4)
            .any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == 2)
    }

    impl Watcher {
        /// Record focus gained/lost for one toplevel. Losing it must clear the
        /// active id: only ever *setting* it meant a window that was closed or
        /// unfocused without another taking over kept earning screen time.
        fn set_activated(&mut self, id: ObjectId, activated: bool) {
            if activated {
                self.active = Some(id);
            } else if self.active.as_ref() == Some(&id) {
                self.active = None;
            } else {
                return; // nothing changed for the focused window
            }
            self.publish();
        }

        /// Mirror the currently-activated toplevel's app id into shared state.
        fn publish(&self) {
            let name = self
                .active
                .as_ref()
                .and_then(|id| self.app_ids.get(id))
                .map(|id| super::friendly(id));
            if let Ok(mut g) = ACTIVE.lock() {
                *g = name;
            }
        }
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Watcher {
        fn event(
            state: &mut Self,
            reg: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                match interface.as_str() {
                    "zcosmic_toplevel_info_v1" => {
                        reg.bind::<ZcosmicToplevelInfoV1, _, _>(name, 1, qh, ());
                        SOURCE.store(SRC_COSMIC, Ordering::Relaxed);
                    }
                    // wlroots-family compositors (KDE Plasma, sway, Hyprland,
                    // wayfire). COSMIC wins if both are advertised — it is the
                    // native one there.
                    "zwlr_foreign_toplevel_manager_v1" => {
                        reg.bind::<ZwlrForeignToplevelManagerV1, _, _>(name, 1, qh, ());
                        let _ = SOURCE.compare_exchange(
                            SRC_IDLE_ONLY,
                            SRC_WLR,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        );
                    }
                    // Outputs must be bound so the toplevel handle's
                    // output_enter/leave events resolve their wl_output arg.
                    "wl_output" => {
                        reg.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                    }
                    "wl_seat" => {
                        state.seat =
                            Some(reg.bind::<wl_seat::WlSeat, _, _>(name, version.min(1), qh, ()));
                    }
                    "ext_idle_notifier_v1" => {
                        state.notifier =
                            Some(reg.bind::<ExtIdleNotifierV1, _, _>(name, 1, qh, ()));
                    }
                    _ => {}
                }
            }
        }
    }

    impl Dispatch<ZcosmicToplevelInfoV1, ()> for Watcher {
        fn event(
            _: &mut Self,
            _: &ZcosmicToplevelInfoV1,
            _: cosmic::zcosmic_toplevel_info_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
        // The `toplevel` event (opcode 0) creates a child handle.
        wayland_client::event_created_child!(Watcher, ZcosmicToplevelInfoV1, [
            0 => (ZcosmicToplevelHandleV1, ()),
        ]);
    }

    impl Dispatch<ZcosmicToplevelHandleV1, ()> for Watcher {
        fn event(
            state: &mut Self,
            handle: &ZcosmicToplevelHandleV1,
            event: HEvent,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let id = handle.id();
            match event {
                HEvent::AppId { app_id } => {
                    state.app_ids.insert(id, app_id);
                    state.publish();
                }
                HEvent::State { state: bytes } => {
                    state.set_activated(id, is_activated(&bytes));
                }
                HEvent::Closed => {
                    state.app_ids.remove(&id);
                    if state.active.as_ref() == Some(&id) {
                        state.active = None;
                    }
                    state.publish();
                }
                _ => {}
            }
        }
    }

    impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Watcher {
        fn event(
            _: &mut Self,
            _: &ZwlrForeignToplevelManagerV1,
            _: wlr::zwlr_foreign_toplevel_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
        // The `toplevel` event (opcode 0) creates a child handle.
        wayland_client::event_created_child!(Watcher, ZwlrForeignToplevelManagerV1, [
            0 => (ZwlrForeignToplevelHandleV1, ()),
        ]);
    }

    impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Watcher {
        fn event(
            state: &mut Self,
            handle: &ZwlrForeignToplevelHandleV1,
            event: WEvent,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let id = handle.id();
            match event {
                WEvent::AppId { app_id } => {
                    state.app_ids.insert(id, app_id);
                    state.publish();
                }
                WEvent::State { state: bytes } => {
                    state.set_activated(id, is_activated(&bytes));
                }
                WEvent::Closed => {
                    state.app_ids.remove(&id);
                    if state.active.as_ref() == Some(&id) {
                        state.active = None;
                    }
                    state.publish();
                }
                _ => {}
            }
        }
    }

    impl Dispatch<ExtIdleNotificationV1, ()> for Watcher {
        fn event(
            _: &mut Self,
            _: &ExtIdleNotificationV1,
            event: NEvent,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                NEvent::Idled => IDLE.store(true, Ordering::Relaxed),
                NEvent::Resumed => IDLE.store(false, Ordering::Relaxed),
            }
        }
    }

    // These objects carry no information ARIA reads; the impls just satisfy the
    // dispatch requirements for their proxies.
    impl Dispatch<wl_output::WlOutput, ()> for Watcher {
        fn event(
            _: &mut Self,
            _: &wl_output::WlOutput,
            _: wl_output::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<wl_seat::WlSeat, ()> for Watcher {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ExtIdleNotifierV1, ()> for Watcher {
        fn event(
            _: &mut Self,
            _: &ExtIdleNotifierV1,
            _: idle::ext_idle_notifier_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ZcosmicWorkspaceHandleV1, ()> for Watcher {
        fn event(
            _: &mut Self,
            _: &ZcosmicWorkspaceHandleV1,
            _: cosmic::zcosmic_workspace_handle_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn friendly_shortens_reverse_dns_ids() {
        assert_eq!(friendly("com.system76.CosmicTerm"), "CosmicTerm");
        assert_eq!(friendly("firefox"), "Firefox");
        assert_eq!(friendly("org.gnome.Nautilus.desktop"), "Nautilus");
        assert_eq!(friendly("Code"), "Code");
        // Degenerate input must not panic.
        assert_eq!(friendly(""), "");
    }

    #[test]
    fn day_file_reads_the_legacy_bare_map() {
        // Files written before `total` was split out are app -> secs.
        let legacy: DayUsage = serde_json::from_str::<StoredDay>(r#"{"Firefox":30,"Code":15}"#)
            .expect("legacy shape must still parse")
            .into();
        assert_eq!(legacy.total, 45, "total is derived from the app tallies");
        assert_eq!(legacy.apps.get("Firefox"), Some(&30));

        // The X11 sessions this release fixes left behind a pile of empty
        // files; they must load as a clean zero, not an error.
        let empty: DayUsage = serde_json::from_str::<StoredDay>("{}").unwrap().into();
        assert_eq!(empty.total, 0);
        assert!(empty.apps.is_empty());
    }

    #[test]
    fn day_file_round_trips_the_current_shape() {
        let mut before = DayUsage {
            total: 900, // counted even where no app could be resolved
            ..DayUsage::default()
        };
        before.apps.insert("Firefox".into(), 300);
        let json = serde_json::to_string(&before).unwrap();
        let after: DayUsage = serde_json::from_str::<StoredDay>(&json).unwrap().into();
        assert_eq!(after.total, 900);
        assert_eq!(after.apps, before.apps);
        // Crucially, total is NOT re-derived: idle-only sessions have no apps.
        let idle_only: DayUsage = serde_json::from_str::<StoredDay>(r#"{"total":60,"apps":{}}"#)
            .unwrap()
            .into();
        assert_eq!(idle_only.total, 60);
        assert!(idle_only.apps.is_empty());
    }

    #[test]
    fn summarize_ranks_and_truncates() {
        let mut usage = DayUsage {
            total: 100,
            ..DayUsage::default()
        };
        for i in 0..TOP_APPS + 3 {
            usage.apps.insert(format!("app{i}"), i as u64);
        }
        let s = summarize(&usage, Some(42));
        assert_eq!(s.total, 100);
        assert_eq!(s.yesterday_total, Some(42));
        assert_eq!(s.apps.len(), TOP_APPS);
        assert!(
            s.apps.windows(2).all(|w| w[0].secs >= w[1].secs),
            "apps are ranked by time desc"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wm_class_prefers_the_class_over_the_instance() {
        // WM_CLASS is "instance\0class\0".
        assert_eq!(
            x11::class_name(b"navigator\0Firefox\0").as_deref(),
            Some("Firefox")
        );
        // Only one string present — use it.
        assert_eq!(
            x11::class_name(b"cosmic-term\0").as_deref(),
            Some("Cosmic-term")
        );
        assert_eq!(x11::class_name(b"").as_deref(), None);
        assert_eq!(x11::class_name(b"\0\0").as_deref(), None);
    }

    /// Live check against the running X server. Skipped on Wayland-only or
    /// headless machines so the suite stays runnable anywhere.
    #[cfg(target_os = "linux")]
    #[test]
    fn x11_backend_reads_the_real_session() {
        if std::env::var_os("DISPLAY").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
            eprintln!("skipping: not an X11 session");
            return;
        }
        let app = x11::active_app();
        eprintln!("x11 active_app = {app:?}");
        assert!(
            app.is_some(),
            "an X11 session with a focused window must resolve an app name"
        );
        let idle = x11::idle_seconds();
        eprintln!("x11 idle_seconds = {idle}");
        assert!(
            idle >= 0.0 && idle.is_finite(),
            "idle seconds must be sane, got {idle}"
        );
    }
}
