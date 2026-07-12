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
}

#[derive(Serialize, Clone)]
struct AppTime {
    name: String,
    secs: u64,
}

#[cfg(target_os = "macos")]
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut day = today();
        let mut usage = load_day(&app, &day).unwrap_or_default();
        let mut tick = 0u32;
        loop {
            tokio::time::sleep(TICK).await;
            tick += 1;

            let now = today();
            if now != day {
                save_day(&app, &day, &usage);
                day = now;
                usage = HashMap::new();
            }

            if idle_seconds() < IDLE_LIMIT {
                if let Some(name) = frontmost_app(&app) {
                    *usage.entry(name).or_insert(0) += TICK.as_secs();
                }
            }

            if tick % 3 == 0 {
                if let Err(e) = app.emit("screentime", summarize(&usage)) {
                    eprintln!("screentime emit failed: {e}");
                }
            }
            if tick % 12 == 0 {
                save_day(&app, &day, &usage);
            }
        }
    });
}

/// Per-OS trackers for Windows/Linux land with those machines.
#[cfg(not(target_os = "macos"))]
pub fn spawn(_app: AppHandle) {}

fn summarize(usage: &HashMap<String, u64>) -> ScreenTime {
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
    ScreenTime { total, apps }
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
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
