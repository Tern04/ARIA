//! What the machine has been doing, kept on disk.
//!
//! Every widget here shows *now*: the current load, the current temperature,
//! the current round trip. None of them can tell you that the GPU runs eight
//! degrees hotter than it used to at the same load, or that tonight's ping is
//! bad by the standards of this hour on this connection. That needs a record,
//! so this collector keeps one: a sample a minute, a file a day, thirty days
//! of them, and then they are deleted.
//!
//! The samples are not collected a second time — the widgets' own collectors
//! hand their latest reading to `note_*` as they emit, and the sampler here
//! writes down whatever the last one said. So a metric this machine does not
//! publish (no GPU, no CPU sensor) is simply absent from the record rather
//! than being invented as a zero.
//!
//! The comparisons themselves live in the frontend (`src/lib/insights.js`),
//! over the aggregates `history_report` returns: a day is summed here, where
//! the files are, and judged there, where it can be unit-tested.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// One sample a minute: fine enough to see a session, small enough that a
/// month of them is a few megabytes.
const TICK: Duration = Duration::from_secs(60);
/// Samples are held in memory and flushed in batches; a crash loses at most
/// this much.
const FLUSH_EVERY: u32 = 5;
const RETENTION_DAYS: i64 = 30;
/// A load bucket needs this many samples before its average means anything.
const MIN_BUCKET: usize = 5;

/// The most recent reading from each collector, as it emitted it.
#[derive(Default, Clone)]
struct Latest {
    cpu: Option<f32>,
    ram: Option<f32>,
    cpu_temp: Option<f32>,
    drive_temp: Option<f32>,
    gpu: Option<f32>,
    gpu_temp: Option<f32>,
    vram: Option<f32>,
    ping: Option<f32>,
    loss: Option<f32>,
}

static LATEST: Mutex<Option<Latest>> = Mutex::new(None);

fn update(f: impl FnOnce(&mut Latest)) {
    if let Ok(mut guard) = LATEST.lock() {
        f(guard.get_or_insert_with(Latest::default));
    }
}

pub fn note_hardware(cpu: f32, ram: f32, cpu_temp: Option<f32>, drive_temp: Option<f32>) {
    update(|l| {
        l.cpu = Some(cpu);
        l.ram = Some(ram);
        l.cpu_temp = cpu_temp;
        l.drive_temp = drive_temp;
    });
}

pub fn note_gpu(gpu: f32, temp: Option<f32>, vram_used: Option<f32>, vram_total: Option<f32>) {
    update(|l| {
        l.gpu = Some(gpu);
        l.gpu_temp = temp;
        l.vram = match (vram_used, vram_total) {
            (Some(used), Some(total)) if total > 0.0 => Some(used / total * 100.0),
            _ => None,
        };
    });
}

pub fn note_ping(ms: Option<f32>, loss_pct: Option<f32>) {
    update(|l| {
        l.ping = ms;
        l.loss = loss_pct;
    });
}

/// A minute of the machine's life. Every field is optional and omitted when
/// absent, so a day file on a machine with no GPU costs nothing for one.
#[derive(Serialize, Deserialize, Clone, Default)]
struct Sample {
    t: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ram: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ct: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vram: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ping: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loss: Option<f32>,
}

#[derive(Serialize, Deserialize, Default)]
struct DayFile {
    day: String,
    samples: Vec<Sample>,
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        prune(&app);
        let mut day = today();
        let mut file = load_day(&app, &day).unwrap_or(DayFile {
            day: day.clone(),
            samples: Vec::new(),
        });
        let mut tick = 0u32;
        loop {
            tokio::time::sleep(TICK).await;
            tick += 1;

            let now = today();
            if now != day {
                save_day(&app, &file);
                prune(&app);
                day = now.clone();
                file = DayFile {
                    day: day.clone(),
                    samples: Vec::new(),
                };
            }

            // Nothing has reported yet (every collector still starting up):
            // a row of empties would only dilute the averages.
            let Some(latest) = LATEST.lock().ok().and_then(|l| l.clone()) else {
                continue;
            };
            file.samples.push(Sample {
                t: chrono::Local::now().timestamp(),
                cpu: latest.cpu,
                ram: latest.ram,
                ct: latest.cpu_temp,
                dt: latest.drive_temp,
                gpu: latest.gpu,
                gt: latest.gpu_temp,
                vram: latest.vram,
                ping: latest.ping,
                loss: latest.loss,
            });
            if tick % FLUSH_EVERY == 0 {
                save_day(&app, &file);
            }
        }
    });
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?.join("history");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn day_file(app: &AppHandle, day: &str) -> Option<PathBuf> {
    Some(dir(app)?.join(format!("{day}.json")))
}

fn load_day(app: &AppHandle, day: &str) -> Option<DayFile> {
    let raw = std::fs::read_to_string(day_file(app, day)?).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_day(app: &AppHandle, file: &DayFile) {
    let Some(path) = day_file(app, &file.day) else {
        return;
    };
    match serde_json::to_string(file) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("history: could not write {}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("history: could not serialise {}: {e}", file.day),
    }
}

/// Delete day files past the retention window. The record is for spotting
/// what changed recently, not an archive, and it is the user's disk.
fn prune(app: &AppHandle) {
    let Some(dir) = dir(app) else { return };
    let cutoff = (chrono::Local::now() - chrono::Duration::days(RETENTION_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(day) = name.strip_suffix(".json") else {
            continue;
        };
        // ISO dates sort lexicographically, so this is a date comparison.
        if day < cutoff.as_str() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

// ── the report ──────────────────────────────────────────────

/// Average/peak of one metric over a day, and how many samples carried it.
#[derive(Serialize, Default, Clone)]
pub struct Stat {
    avg: f32,
    max: f32,
    n: usize,
}

#[derive(Default)]
struct Acc {
    sum: f64,
    max: f32,
    n: usize,
}

impl Acc {
    fn push(&mut self, v: Option<f32>) {
        let Some(v) = v.filter(|v| v.is_finite()) else {
            return;
        };
        self.sum += v as f64;
        self.max = self.max.max(v);
        self.n += 1;
    }
    fn stat(&self) -> Option<Stat> {
        (self.n > 0).then(|| Stat {
            avg: (self.sum / self.n as f64) as f32,
            max: self.max,
            n: self.n,
        })
    }
}

#[derive(Serialize)]
pub struct DayReport {
    date: String,
    samples: usize,
    cpu: Option<Stat>,
    ram: Option<Stat>,
    cpu_temp: Option<Stat>,
    gpu: Option<Stat>,
    gpu_temp: Option<Stat>,
    vram: Option<Stat>,
    ping: Option<Stat>,
    /// Share of samples that reported any packet loss, 0–100.
    loss_pct: Option<f32>,
    /// GPU temperature averaged per 10 % load bucket ("0", "10", … "90"),
    /// which is what makes "hotter than usual" a fair comparison: an idle day
    /// and a gaming day are not otherwise comparable.
    gpu_temp_by_load: BTreeMap<String, Stat>,
    /// Round trip averaged per hour of the day ("0".."23") — a connection that
    /// is bad every evening is not the same story as one that is bad now.
    ping_by_hour: BTreeMap<String, Stat>,
    /// Screen time from the tracker's own files, so the report can say what
    /// the machine was used *for*, not just how hard.
    screen_secs: Option<u64>,
    screen_apps: Vec<AppSecs>,
}

#[derive(Serialize)]
pub struct AppSecs {
    name: String,
    secs: u64,
}

/// The last `days` days, most recent last. Days with no file are skipped
/// rather than reported as zero — the machine was off, not idle.
#[tauri::command]
pub fn history_report(app: AppHandle, days: u32) -> Vec<DayReport> {
    let days = days.clamp(1, RETENTION_DAYS as u32);
    let mut out = Vec::new();
    for back in (0..days as i64).rev() {
        let date = (chrono::Local::now() - chrono::Duration::days(back))
            .format("%Y-%m-%d")
            .to_string();
        // A day with no sample file can still have screen time: the tracker
        // has been keeping its own files since long before this collector
        // existed, and skipping those days would hide history that is already
        // on disk. What is missing stays missing — absent, never zero.
        let samples = load_day(&app, &date).map(|f| f.samples).unwrap_or_default();
        let report = summarize(&app, &date, &samples);
        // Neither samples nor screen time: the machine was off that day.
        if report.samples == 0 && report.screen_secs.unwrap_or(0) == 0 {
            continue;
        }
        out.push(report);
    }
    out
}

fn summarize(app: &AppHandle, date: &str, samples: &[Sample]) -> DayReport {
    let (mut cpu, mut ram, mut ct, mut gpu, mut gt, mut vram, mut ping) = (
        Acc::default(),
        Acc::default(),
        Acc::default(),
        Acc::default(),
        Acc::default(),
        Acc::default(),
        Acc::default(),
    );
    let mut by_load: BTreeMap<String, Acc> = BTreeMap::new();
    let mut by_hour: BTreeMap<String, Acc> = BTreeMap::new();
    let mut lossy = 0usize;
    let mut loss_seen = 0usize;

    for s in samples {
        cpu.push(s.cpu);
        ram.push(s.ram);
        ct.push(s.ct);
        gpu.push(s.gpu);
        gt.push(s.gt);
        vram.push(s.vram);
        ping.push(s.ping);
        if let Some(loss) = s.loss {
            loss_seen += 1;
            if loss > 0.0 {
                lossy += 1;
            }
        }
        if let (Some(load), Some(temp)) = (s.gpu, s.gt) {
            let bucket = ((load / 10.0).floor() as i32 * 10).clamp(0, 90);
            by_load.entry(bucket.to_string()).or_default().push(Some(temp));
        }
        if let (Some(ms), Some(hour)) = (s.ping, hour_of(s.t)) {
            by_hour.entry(hour.to_string()).or_default().push(Some(ms));
        }
    }

    let stats = |map: BTreeMap<String, Acc>| -> BTreeMap<String, Stat> {
        map.into_iter()
            .filter_map(|(k, acc)| {
                acc.stat()
                    .filter(|s| s.n >= MIN_BUCKET)
                    .map(|s| (k, s))
            })
            .collect()
    };
    let (screen_secs, screen_apps) = super::screentime::day_totals(app, date);

    DayReport {
        date: date.to_string(),
        samples: samples.len(),
        cpu: cpu.stat(),
        ram: ram.stat(),
        cpu_temp: ct.stat(),
        gpu: gpu.stat(),
        gpu_temp: gt.stat(),
        vram: vram.stat(),
        ping: ping.stat(),
        loss_pct: (loss_seen > 0).then(|| lossy as f32 / loss_seen as f32 * 100.0),
        gpu_temp_by_load: stats(by_load),
        ping_by_hour: stats(by_hour),
        screen_secs,
        screen_apps: screen_apps
            .into_iter()
            .map(|(name, secs)| AppSecs { name, secs })
            .collect(),
    }
}

fn hour_of(ts: i64) -> Option<u32> {
    use chrono::Timelike;
    chrono::DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&chrono::Local).hour())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: i64, gpu: f32, gt: f32) -> Sample {
        Sample {
            t,
            gpu: Some(gpu),
            gt: Some(gt),
            ..Sample::default()
        }
    }

    #[test]
    fn an_accumulator_ignores_missing_and_nonsense_readings() {
        let mut acc = Acc::default();
        acc.push(Some(10.0));
        acc.push(None);
        acc.push(Some(f32::NAN));
        acc.push(Some(20.0));
        let stat = acc.stat().unwrap();
        assert_eq!(stat.n, 2);
        assert_eq!(stat.avg, 15.0);
        assert_eq!(stat.max, 20.0);
        assert!(Acc::default().stat().is_none());
    }

    /// A day file is written to the user's disk every day for a month, so its
    /// size is a real cost and worth pinning down rather than assuming. A full
    /// day is 1440 samples with every field present; anything much over this
    /// means a field was added without thinking about the retention window.
    #[test]
    fn a_full_day_file_stays_small() {
        let day = DayFile {
            day: "2026-09-17".into(),
            samples: (0..1440)
                .map(|i| Sample {
                    t: 1_789_600_000 + i * 60,
                    // Values with the ugly mantissas real sensors produce, so
                    // this measures serde's full f32 output, not round numbers.
                    cpu: Some(37.412_94),
                    ram: Some(61.883_57),
                    ct: Some(54.216_78),
                    dt: Some(31.749_21),
                    gpu: Some(88.331_46),
                    gt: Some(71.604_83),
                    vram: Some(46.927_15),
                    ping: Some(18.462_09),
                    loss: Some(0.0),
                })
                .collect(),
        };
        let bytes = serde_json::to_string(&day).unwrap().len();
        let per_day_kb = bytes / 1024;
        let window_mb = per_day_kb * RETENTION_DAYS as usize / 1024;
        println!("day file: {per_day_kb} KB, {window_mb} MB over the retention window");
        assert!(
            per_day_kb < 300,
            "a day file grew to {per_day_kb} KB ({window_mb} MB over the {RETENTION_DAYS}-day window)"
        );
    }

    #[test]
    fn gpu_temperature_is_bucketed_by_load() {
        // 0-9% and 90-100% land in the first and last bucket; a bucket under
        // MIN_BUCKET samples is dropped as too thin to mean anything.
        let mut samples: Vec<Sample> = Vec::new();
        for i in 0..MIN_BUCKET {
            samples.push(sample(0, 5.0, 40.0));
            samples.push(sample(0, 95.0, 80.0 + i as f32));
        }
        samples.push(sample(0, 55.0, 60.0)); // lonely bucket
        let mut by_load: BTreeMap<String, Acc> = BTreeMap::new();
        for s in &samples {
            if let (Some(load), Some(temp)) = (s.gpu, s.gt) {
                let bucket = ((load / 10.0).floor() as i32 * 10).clamp(0, 90);
                by_load.entry(bucket.to_string()).or_default().push(Some(temp));
            }
        }
        let kept: Vec<String> = by_load
            .iter()
            .filter(|(_, a)| a.n >= MIN_BUCKET)
            .map(|(k, _)| k.clone())
            .collect();
        assert_eq!(kept, vec!["0".to_string(), "90".to_string()]);
    }
}
