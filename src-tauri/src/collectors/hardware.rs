use serde::Serialize;
use std::time::Duration;
use sysinfo::System;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone)]
struct HardwareStats {
    cpu: f32,
    ram: f32,
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut sys = System::new();
        loop {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let stats = HardwareStats {
                cpu: sys.global_cpu_usage(),
                ram: sys.used_memory() as f32 / sys.total_memory() as f32 * 100.0,
            };
            if let Err(e) = app.emit("hardware", stats) {
                eprintln!("hardware emit failed: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}
