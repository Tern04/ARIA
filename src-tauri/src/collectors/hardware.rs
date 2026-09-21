use serde::Serialize;
use std::time::Duration;
use sysinfo::{Components, System};
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone)]
struct HardwareStats {
    cpu: f32,
    /// Per-core usage in the kernel's core order. A game pinning two threads
    /// while fourteen idle reads as ~12% average, so the average alone hides
    /// exactly the case the gaming widgets exist to show.
    cores: Vec<f32>,
    cpu_freq_mhz: u64,
    /// Package/die temperature, when a sensor is exposed. `None` rather than
    /// a fake 0 on machines that publish nothing (common on Windows), so the
    /// widget can say "no sensor" instead of claiming the CPU is at freezing.
    cpu_temp: Option<f32>,
    drive_temp: Option<f32>,
    ram: f32,
    ram_used_gb: f32,
    ram_total_gb: f32,
    net_rx_bps: u64,
    net_tx_bps: u64,
    uptime_secs: u64,
}

/// GPU telemetry. Only `gpu` (utilization) is available on every platform.
/// Linux fills the rest from `nvidia-smi`; macOS adds name and unified-memory
/// use. Missing fields stay `None`, which the widget renders as "—".
#[derive(Serialize, Clone, Default)]
struct GpuStats {
    gpu: f32,
    name: Option<String>,
    vram_used_mb: Option<f32>,
    vram_total_mb: Option<f32>,
    temp_c: Option<f32>,
    power_w: Option<f32>,
    power_limit_w: Option<f32>,
    clock_mhz: Option<u32>,
    mem_clock_mhz: Option<u32>,
    fan_pct: Option<f32>,
}

/// Sensor labels are vendor- and platform-specific ("Tctl" on AMD,
/// "Package id 0" on Intel, "PMU tdie" on Apple silicon), so match the most
/// specific spelling first and fall back to the vaguest. A component that
/// exists but reports nothing usable (Linux yields NaN on a failed read) is
/// skipped rather than shown.
fn pick_temp(components: &Components, keys: &[&str]) -> Option<f32> {
    for key in keys {
        for c in components.list() {
            if !c.label().to_lowercase().contains(key) {
                continue;
            }
            match c.temperature() {
                Some(t) if t.is_finite() && t > 0.0 => return Some(t),
                _ => {}
            }
        }
    }
    None
}

const CPU_TEMP_KEYS: &[&str] = &["tctl", "tdie", "package id", "cpu", "core 0", "soc"];
const DRIVE_TEMP_KEYS: &[&str] = &["composite", "nvme", "ssd", "drive"];

pub fn spawn(app: AppHandle) {
    spawn_gpu(app.clone());
    tauri::async_runtime::spawn(async move {
        const GIB: f32 = 1_073_741_824.0;
        let mut sys = System::new();
        let mut networks = sysinfo::Networks::new_with_refreshed_list();
        let mut components = Components::new_with_refreshed_list();
        loop {
            // Frequency as well as usage: a boost clock sagging under load is
            // a thermal-throttle tell, which is the point of the gaming board.
            sys.refresh_cpu_all();
            sys.refresh_memory();
            components.refresh(false);
            // received()/transmitted() are deltas since the last refresh,
            // i.e. bytes per POLL interval.
            networks.refresh(true);
            let (rx, tx) = networks
                .iter()
                .fold((0u64, 0u64), |(r, t), (_, n)| (r + n.received(), t + n.transmitted()));
            let cpus = sys.cpus();
            let stats = HardwareStats {
                cpu: sys.global_cpu_usage(),
                cores: cpus.iter().map(|c| c.cpu_usage()).collect(),
                cpu_freq_mhz: cpus.first().map(|c| c.frequency()).unwrap_or(0),
                cpu_temp: pick_temp(&components, CPU_TEMP_KEYS),
                drive_temp: pick_temp(&components, DRIVE_TEMP_KEYS),
                ram: sys.used_memory() as f32 / sys.total_memory() as f32 * 100.0,
                ram_used_gb: sys.used_memory() as f32 / GIB,
                ram_total_gb: sys.total_memory() as f32 / GIB,
                net_rx_bps: rx / POLL.as_secs(),
                net_tx_bps: tx / POLL.as_secs(),
                uptime_secs: System::uptime(),
            };
            super::history::note_hardware(stats.cpu, stats.ram, stats.cpu_temp, stats.drive_temp);
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
                Ok(Ok(stats)) => {
                    if let Err(e) = app.emit("gpu", stats) {
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
                super::history::note_gpu(gpu, None, None, None);
                if let Err(e) = app.emit(
                    "gpu",
                    GpuStats {
                        gpu,
                        ..Default::default()
                    },
                ) {
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

/// Linux GPU telemetry via `nvidia-smi`. Desktop NVIDIA is the common case
/// here; other vendors would need a sysfs/`radeontop` path, so when the tool is
/// absent (non-NVIDIA machine, driver missing) we back off and leave the gauge
/// blank rather than spamming — same visible result as the old no-op stub.
///
/// Polls on the same 2 s beat as the CPU side so the history graph's GPU trace
/// lines up with its CPU trace instead of drawing a coarser stair-step.
#[cfg(target_os = "linux")]
fn spawn_gpu(app: AppHandle) {
    const GPU_POLL: Duration = POLL;
    const BACKOFF: Duration = Duration::from_secs(120);

    tauri::async_runtime::spawn(async move {
        loop {
            match tauri::async_runtime::spawn_blocking(sample_gpu_nvidia).await {
                Ok(Ok(stats)) => {
                    super::history::note_gpu(
                        stats.gpu,
                        stats.temp_c,
                        stats.vram_used_mb,
                        stats.vram_total_mb,
                    );
                    if let Err(e) = app.emit("gpu", stats) {
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

/// One `nvidia-smi` query for everything the GPU widget shows. Multi-GPU rigs
/// report a line per card; the first is the primary and the only one the HUD
/// has room for.
#[cfg(target_os = "linux")]
fn sample_gpu_nvidia() -> Result<GpuStats, String> {
    // `name` goes last: every other field is numeric, so whatever follows the
    // ninth comma is the model string, commas and all.
    const FIELDS: &str = "utilization.gpu,memory.used,memory.total,temperature.gpu,\
power.draw,power.limit,clocks.current.sm,clocks.current.memory,fan.speed,name";

    let out = std::process::Command::new("nvidia-smi")
        .args([
            &format!("--query-gpu={FIELDS}"),
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
    let line = text.lines().next().ok_or("nvidia-smi printed nothing")?;
    parse_nvidia(line).ok_or_else(|| format!("unparsable nvidia-smi row: {}", line.trim()))
}

/// A field is `None` when the card does not report it — an unsupported sensor
/// prints `[N/A]`, and a blower-less card prints nothing for fan speed. Only
/// utilization is required; the rest degrade to "—" in the widget.
#[cfg(target_os = "linux")]
fn parse_nvidia(line: &str) -> Option<GpuStats> {
    let mut f = line.splitn(10, ',').map(str::trim);
    let num = |v: Option<&str>| v.and_then(|s| s.parse::<f32>().ok());
    let gpu: f32 = f.next()?.parse().ok()?;
    let vram_used_mb = num(f.next());
    let vram_total_mb = num(f.next());
    let temp_c = num(f.next());
    let power_w = num(f.next());
    let power_limit_w = num(f.next());
    let clock_mhz = num(f.next()).map(|v| v as u32);
    let mem_clock_mhz = num(f.next()).map(|v| v as u32);
    let fan_pct = num(f.next());
    let name = f
        .next()
        .filter(|s| !s.is_empty() && *s != "[N/A]")
        .map(str::to_string);
    Some(GpuStats {
        gpu: gpu.clamp(0.0, 100.0),
        name,
        vram_used_mb,
        vram_total_mb,
        temp_c,
        power_w,
        power_limit_w,
        clock_mhz,
        mem_clock_mhz,
        fan_pct,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn spawn_gpu(_app: AppHandle) {}

#[cfg(target_os = "macos")]
fn sample_gpu() -> Result<GpuStats, String> {
    let gpu = sample_gpu_residency()?;
    let device = metal_device();
    Ok(GpuStats {
        gpu,
        name: device.name.clone(),
        vram_used_mb: sample_gpu_memory_mb(),
        vram_total_mb: device.working_set_mb,
        ..Default::default()
    })
}

#[cfg(target_os = "macos")]
struct MetalDevice {
    name: Option<String>,
    working_set_mb: Option<f32>,
}

/// Apple silicon has no dedicated VRAM: the GPU draws from unified memory up
/// to a cap Metal calls the recommended working set (~2/3 to 3/4 of RAM,
/// raised by `iogpu.wired_limit_mb`). That cap is the meaningful "total", not
/// hw.memsize. It is fixed for the boot, so ask Metal once.
#[cfg(target_os = "macos")]
fn metal_device() -> &'static MetalDevice {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use std::ffi::CStr;
    use std::sync::OnceLock;

    #[link(name = "Metal", kind = "framework")]
    extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
    }

    static DEVICE: OnceLock<MetalDevice> = OnceLock::new();
    DEVICE.get_or_init(|| unsafe {
        let dev = MTLCreateSystemDefaultDevice();
        if dev.is_null() {
            return MetalDevice {
                name: None,
                working_set_mb: None,
            };
        }
        let bytes: u64 = msg_send![dev, recommendedMaxWorkingSetSize];
        let ns_name: *mut AnyObject = msg_send![dev, name];
        let name = if ns_name.is_null() {
            None
        } else {
            let utf8: *const std::os::raw::c_char = msg_send![ns_name, UTF8String];
            (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
        };
        // MTLCreateSystemDefaultDevice returns +1.
        let _: () = msg_send![dev, release];
        MetalDevice {
            name,
            working_set_mb: (bytes > 0).then(|| bytes as f32 / 1_048_576.0),
        }
    })
}

/// GPU-resident memory from the AGX driver's PerformanceStatistics, the same
/// source Activity Monitor uses. Readable without root, unlike powermetrics.
#[cfg(target_os = "macos")]
fn sample_gpu_memory_mb() -> Option<f32> {
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_gpu_in_use_bytes(&String::from_utf8_lossy(&out.stdout)).map(|b| b as f32 / 1_048_576.0)
}

/// The closing quote before `=` keeps this off the neighbouring
/// "In use system memory (driver)" key.
#[cfg(target_os = "macos")]
fn parse_gpu_in_use_bytes(output: &str) -> Option<u64> {
    const KEY: &str = "\"In use system memory\"=";
    let rest = &output[output.find(KEY)? + KEY.len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(target_os = "macos")]
fn sample_gpu_residency() -> Result<f32, String> {
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
