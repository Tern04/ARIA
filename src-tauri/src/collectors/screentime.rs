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
}

#[derive(Serialize, Clone)]
struct AppTime {
    name: String,
    secs: u64,
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
                usage = HashMap::new();
                yesterday_total = load_total(&app, &yesterday());
            }

            if idle_seconds() < IDLE_LIMIT {
                if let Some(name) = frontmost_app(&app) {
                    *usage.entry(name).or_insert(0) += TICK.as_secs();
                }
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

fn summarize(usage: &HashMap<String, u64>, yesterday_total: Option<u64>) -> ScreenTime {
    let total = usage.values().sum();
    let mut apps: Vec<AppTime> = usage
        .iter()
        .map(|(name, secs)| AppTime {
            name: name.clone(),
            secs: *secs,
        })
        .collect();
    apps.sort_by(|a, b| b.secs.cmp(&a.secs));
    apps.truncate(TOP_APPS);
    ScreenTime {
        total,
        apps,
        yesterday_total,
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
    Some(load_day(app, day)?.values().sum())
}

fn day_file(app: &AppHandle, day: &str) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?.join("screentime");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("{day}.json")))
}

fn load_day(app: &AppHandle, day: &str) -> Option<HashMap<String, u64>> {
    let raw = std::fs::read_to_string(day_file(app, day)?).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_day(app: &AppHandle, day: &str, usage: &HashMap<String, u64>) {
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

/// Linux/COSMIC: the focused window's app comes from a tiny background Wayland
/// client (see `wl`), since Wayland exposes neither the focused window nor idle
/// time over any CLI or D-Bus here.
#[cfg(target_os = "linux")]
fn frontmost_app(_app: &AppHandle) -> Option<String> {
    wl::ensure_started();
    wl::active_app()
}

/// ext-idle-notify reports idle as a threshold crossing (idled/resumed at
/// `IDLE_LIMIT`), not a running counter, so map that boolean back onto the
/// numeric contract the shared loop expects: past the limit when idle, zero
/// otherwise. Until the Wayland client connects this reports "active", so
/// early ticks count rather than drop.
#[cfg(target_os = "linux")]
fn idle_seconds() -> f64 {
    if wl::is_idle() {
        IDLE_LIMIT + 1.0
    } else {
        0.0
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
    use std::sync::atomic::{AtomicBool, Ordering};
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

    static ACTIVE: Mutex<Option<String>> = Mutex::new(None);
    static IDLE: AtomicBool = AtomicBool::new(false);
    static START: Once = Once::new();

    /// Friendly name of the focused app, or None when unknown/nothing focused.
    pub fn active_app() -> Option<String> {
        ACTIVE.lock().ok().and_then(|g| g.clone())
    }

    /// Whether the session has been idle past `IDLE_LIMIT`.
    pub fn is_idle() -> bool {
        IDLE.load(Ordering::Relaxed)
    }

    /// Start the Wayland client once, on the first tracker tick.
    pub fn ensure_started() {
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("aria-screentime-wl".into())
                .spawn(run);
        });
    }

    fn run() {
        // Reconnect on any protocol error or disconnect so a transient failure
        // (or a compositor restart) can't permanently stop tracking.
        loop {
            if let Err(e) = run_once() {
                eprintln!("screentime wayland: {e}");
                if let Ok(mut g) = ACTIVE.lock() {
                    *g = None; // don't keep crediting a stale app after a drop
                }
            }
            std::thread::sleep(Duration::from_secs(5));
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

    impl Watcher {
        /// Mirror the currently-activated toplevel's app id into shared state.
        fn publish(&self) {
            let name = self
                .active
                .as_ref()
                .and_then(|id| self.app_ids.get(id))
                .map(|id| friendly(id));
            if let Ok(mut g) = ACTIVE.lock() {
                *g = name;
            }
        }
    }

    /// A reverse-DNS app id → a short display label:
    /// "com.system76.CosmicTerm" → "CosmicTerm", "firefox" → "Firefox".
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
                    // The state arg is a packed array of u32 enum values;
                    // `activated` (2) marks the focused window.
                    let activated = bytes
                        .chunks_exact(4)
                        .any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == 2);
                    if activated {
                        state.active = Some(id);
                        state.publish();
                    }
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
