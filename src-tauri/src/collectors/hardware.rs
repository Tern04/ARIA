use serde::Serialize;
use std::time::Duration;
use sysinfo::System;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone)]
struct HardwareStats {
    cpu: f32,
    ram: f32,
    ram_used_gb: f32,
    ram_total_gb: f32,
    net_rx_bps: u64,
    net_tx_bps: u64,
    uptime_secs: u64,
}

#[derive(Serialize, Clone)]
struct GpuStats {
    gpu: f32,
}

pub fn spawn(app: AppHandle) {
    spawn_gpu(app.clone());
    tauri::async_runtime::spawn(async move {
        const GIB: f32 = 1_073_741_824.0;
        let mut sys = System::new();
        let mut networks = sysinfo::Networks::new_with_refreshed_list();
        loop {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            // received()/transmitted() are deltas since the last refresh,
            // i.e. bytes per POLL interval.
            networks.refresh(true);
            let (rx, tx) = networks
                .iter()
                .fold((0u64, 0u64), |(r, t), (_, n)| (r + n.received(), t + n.transmitted()));
            let stats = HardwareStats {
                cpu: sys.global_cpu_usage(),
                ram: sys.used_memory() as f32 / sys.total_memory() as f32 * 100.0,
                ram_used_gb: sys.used_memory() as f32 / GIB,
                ram_total_gb: sys.total_memory() as f32 / GIB,
                net_rx_bps: rx / POLL.as_secs(),
                net_tx_bps: tx / POLL.as_secs(),
                uptime_secs: System::uptime(),
            };
            if let Err(e) = app.emit("hardware", stats) {
                eprintln!("hardware emit failed: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

/// Apple Silicon GPU utilization via powermetrics, which needs root.
/// Requires a passwordless sudo rule for /usr/bin/powermetrics; when that is
/// missing, `sudo -n` fails fast and we back off instead of spamming.
#[cfg(target_os = "macos")]
fn spawn_gpu(app: AppHandle) {
    use tauri::Emitter;

    const GPU_POLL: Duration = Duration::from_secs(5);
    const BACKOFF: Duration = Duration::from_secs(120);

    tauri::async_runtime::spawn(async move {
        loop {
            let sampled = tauri::async_runtime::spawn_blocking(sample_gpu).await;
            match sampled {
                Ok(Ok(gpu)) => {
                    if let Err(e) = app.emit("gpu", GpuStats { gpu }) {
                        eprintln!("gpu emit failed: {e}");
                    }
                    tokio::time::sleep(GPU_POLL).await;
                }
                Ok(Err(e)) => {
                    eprintln!("gpu sample failed: {e}");
                    tokio::time::sleep(BACKOFF).await;
                }
                Err(e) => {
                    eprintln!("gpu task failed: {e}");
                    tokio::time::sleep(BACKOFF).await;
                }
            }
        }
    });
}

/// GPU utilization from the PDH "GPU Engine" counters — the same numbers
/// Task Manager shows. The 3D engine type tracks its headline GPU figure
/// closely enough for a gauge. Utilization is a delta counter, so the query
/// handle must live across samples: the whole lifecycle runs on one plain
/// thread (AppHandle is Send, emitting from it is fine).
#[cfg(target_os = "windows")]
fn spawn_gpu(app: AppHandle) {
    use tauri::Emitter;
    use windows::core::w;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCollectQueryData, PdhOpenQueryW, PDH_HCOUNTER, PDH_HQUERY,
    };

    const GPU_POLL: Duration = Duration::from_secs(5);

    std::thread::spawn(move || {
        let mut query = PDH_HQUERY::default();
        let mut counter = PDH_HCOUNTER::default();
        unsafe {
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                eprintln!("gpu: PdhOpenQuery failed; gauge disabled");
                return;
            }
            if PdhAddEnglishCounterW(
                query,
                w!(r"\GPU Engine(*engtype_3D)\Utilization Percentage"),
                0,
                &mut counter,
            ) != 0
            {
                eprintln!("gpu: GPU Engine counter unavailable; gauge disabled");
                return;
            }
            // Prime the baseline; deltas start with the second collect.
            let _ = PdhCollectQueryData(query);
        }
        loop {
            std::thread::sleep(GPU_POLL);
            if let Some(gpu) = sample_gpu_pdh(query, counter) {
                if let Err(e) = app.emit("gpu", GpuStats { gpu }) {
                    eprintln!("gpu emit failed: {e}");
                }
            }
        }
    });
}

/// Sum the per-process 3D-engine instances into one utilization figure.
#[cfg(target_os = "windows")]
fn sample_gpu_pdh(
    query: windows::Win32::System::Performance::PDH_HQUERY,
    counter: windows::Win32::System::Performance::PDH_HCOUNTER,
) -> Option<f32> {
    use windows::Win32::System::Performance::{
        PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_FMT_COUNTERVALUE_ITEM_W,
        PDH_FMT_DOUBLE, PDH_MORE_DATA,
    };

    unsafe {
        if PdhCollectQueryData(query) != 0 {
            return None;
        }
        // Two-call pattern: first call sizes the buffer, second fills it.
        let mut buf_size = 0u32;
        let mut count = 0u32;
        let status =
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut buf_size, &mut count, None);
        if status != PDH_MORE_DATA || buf_size == 0 {
            return None;
        }
        let mut buf = vec![0u8; buf_size as usize];
        if PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut buf_size,
            &mut count,
            Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
        ) != 0
        {
            return None;
        }
        let items = std::slice::from_raw_parts(
            buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
            count as usize,
        );
        let total: f64 = items
            .iter()
            .map(|item| item.FmtValue.Anonymous.doubleValue)
            .sum();
        Some((total as f32).clamp(0.0, 100.0))
    }
}

/// Linux GPU utilization via `nvidia-smi`. Desktop NVIDIA is the common case
/// here; other vendors would need a sysfs/`radeontop` path, so when the tool is
/// absent (non-NVIDIA machine, driver missing) we back off and leave the gauge
/// blank rather than spamming — same visible result as the old no-op stub.
#[cfg(target_os = "linux")]
fn spawn_gpu(app: AppHandle) {
    const GPU_POLL: Duration = Duration::from_secs(5);
    const BACKOFF: Duration = Duration::from_secs(120);

    tauri::async_runtime::spawn(async move {
        loop {
            match tauri::async_runtime::spawn_blocking(sample_gpu_nvidia).await {
                Ok(Ok(gpu)) => {
                    if let Err(e) = app.emit("gpu", GpuStats { gpu }) {
                        eprintln!("gpu emit failed: {e}");
                    }
                    tokio::time::sleep(GPU_POLL).await;
                }
                Ok(Err(e)) => {
                    eprintln!("gpu sample failed: {e}");
                    tokio::time::sleep(BACKOFF).await;
                }
                Err(e) => {
                    eprintln!("gpu task failed: {e}");
                    tokio::time::sleep(BACKOFF).await;
                }
            }
        }
    });
}

/// First GPU's utilization percentage from `nvidia-smi`. Multi-GPU rigs report
/// one line per card; the first is the primary and enough for a single gauge.
#[cfg(target_os = "linux")]
fn sample_gpu_nvidia() -> Result<f32, String> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "nvidia-smi exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .next()
        .and_then(|l| l.trim().parse::<f32>().ok())
        .map(|v| v.clamp(0.0, 100.0))
        .ok_or_else(|| "no utilization value in nvidia-smi output".into())
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn spawn_gpu(_app: AppHandle) {}

#[cfg(target_os = "macos")]
fn sample_gpu() -> Result<f32, String> {
    let out = std::process::Command::new("sudo")
        .args([
            "-n",
            "/usr/bin/powermetrics",
            "--samplers",
            "gpu_power",
            "-i",
            "1000",
            "-n",
            "1",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "powermetrics exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_gpu_residency(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "no GPU residency line in powermetrics output".into())
}

/// Output format shifts between macOS versions; look for any line like
/// "GPU HW active residency:  12.34% ..." and take the first percentage.
#[cfg(target_os = "macos")]
fn parse_gpu_residency(output: &str) -> Option<f32> {
    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("gpu") && lower.contains("active residency") {
            let after_colon = line.split(':').nth(1)?;
            let pct = after_colon.trim().split('%').next()?.trim();
            if let Ok(v) = pct.parse::<f32>() {
                return Some(v.clamp(0.0, 100.0));
            }
        }
    }
    None
}
