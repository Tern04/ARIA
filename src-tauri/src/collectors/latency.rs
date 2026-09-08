//! Network latency to a chosen host — the one number a game reacts to that
//! none of the other collectors report. Shells out to the system `ping`
//! rather than opening a raw ICMP socket, which would need CAP_NET_RAW (or
//! admin on Windows) and would make the whole app require elevation.

use serde::Serialize;
use std::collections::VecDeque;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;

const POLL: Duration = Duration::from_secs(3);
/// 30 samples ≈ 90 s of history — enough for the sparkline to show a spike
/// settling, short enough that a fixed jitter figure still means "now".
const WINDOW: usize = 30;
pub const DEFAULT_HOST: &str = "1.1.1.1";

/// The host currently being pinged. A watch channel so the command can retarget
/// the running loop without restarting it.
pub struct LatencyHost(watch::Sender<String>);

#[derive(Serialize, Clone)]
struct Latency {
    host: String,
    /// The most recent round trip, or `None` when that packet was lost.
    last_ms: Option<f32>,
    avg_ms: Option<f32>,
    min_ms: Option<f32>,
    max_ms: Option<f32>,
    /// Mean absolute difference between consecutive replies. Jitter, not
    /// spread: a steady 90 ms plays better than a 40 ms average that swings.
    jitter_ms: Option<f32>,
    loss_pct: f32,
    /// Oldest → newest, holes included, so the sparkline can draw the gaps.
    samples: Vec<Option<f32>>,
}

/// Point the ping at another host. Rejected rather than passed through when it
/// is not a plain hostname or IP: `ping` takes flags positionally, so a value
/// starting with `-` would become one, and a space would split into arguments.
#[tauri::command]
pub fn latency_set_host(host: String, state: tauri::State<LatencyHost>) -> Result<(), String> {
    let host = host.trim().to_string();
    if !valid_host(&host) {
        return Err(format!("'{host}' is not a hostname or IP address"));
    }
    state
        .0
        .send(host)
        .map_err(|_| "latency collector is not running".to_string())
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
}

pub fn spawn(app: AppHandle) {
    let (tx, mut rx) = watch::channel(DEFAULT_HOST.to_string());
    app.manage(LatencyHost(tx));

    tauri::async_runtime::spawn(async move {
        let mut history: VecDeque<Option<f32>> = VecDeque::with_capacity(WINDOW);
        let mut host = rx.borrow_and_update().clone();
        loop {
            // A retarget invalidates the window: mixing two hosts' timings
            // into one average would report a latency neither of them has.
            if rx.has_changed().unwrap_or(false) {
                host = rx.borrow_and_update().clone();
                history.clear();
            }
            let target = host.clone();
            let sample = tauri::async_runtime::spawn_blocking(move || ping_once(&target))
                .await
                .unwrap_or(None);
            if history.len() == WINDOW {
                history.pop_front();
            }
            history.push_back(sample);
            if let Err(e) = app.emit("latency", summarize(&host, &history)) {
                eprintln!("latency emit failed: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

fn summarize(host: &str, history: &VecDeque<Option<f32>>) -> Latency {
    let got: Vec<f32> = history.iter().flatten().copied().collect();
    let jitter = if got.len() > 1 {
        let steps: f32 = got.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
        Some(steps / (got.len() - 1) as f32)
    } else {
        None
    };
    Latency {
        host: host.to_string(),
        last_ms: history.back().copied().flatten(),
        avg_ms: (!got.is_empty()).then(|| got.iter().sum::<f32>() / got.len() as f32),
        min_ms: got.iter().copied().reduce(f32::min),
        max_ms: got.iter().copied().reduce(f32::max),
        jitter_ms: jitter,
        loss_pct: if history.is_empty() {
            0.0
        } else {
            (history.len() - got.len()) as f32 / history.len() as f32 * 100.0
        },
        samples: history.iter().copied().collect(),
    }
}

/// One echo request, with a one-second deadline. `None` covers every kind of
/// failure alike — timeout, unknown host, no route — because to the widget
/// they are all "no reply", and a lost packet is normal enough not to log.
fn ping_once(host: &str) -> Option<f32> {
    // The wait flag disagrees across platforms: seconds on Linux, milliseconds
    // on macOS (-W) and on Windows (-w).
    #[cfg(target_os = "linux")]
    let args = ["-c", "1", "-W", "1", host];
    #[cfg(target_os = "macos")]
    let args = ["-c", "1", "-W", "1000", host];
    #[cfg(target_os = "windows")]
    let args = ["-n", "1", "-w", "1000", host];

    let out = std::process::Command::new("ping").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ping(&String::from_utf8_lossy(&out.stdout))
}

/// Pull the round trip out of the reply line. Every `ping` prints it as
/// `time=1.23 ms`, `time=1ms` or — for a sub-millisecond LAN hop on Windows —
/// `time<1ms`, so match the prefix and take the number that follows.
fn parse_ping(output: &str) -> Option<f32> {
    for line in output.lines() {
        let Some(idx) = line.find("time").filter(|_| line.contains("ms")) else {
            continue;
        };
        let rest = &line[idx + 4..];
        let mut chars = rest.chars();
        let sep = chars.next()?;
        if sep != '=' && sep != '<' && sep != '>' {
            continue;
        }
        let digits: String = chars
            .as_str()
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(ms) = digits.parse::<f32>() {
            return Some(ms);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_common_reply_formats() {
        assert_eq!(
            parse_ping("64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=6.61 ms"),
            Some(6.61)
        );
        assert_eq!(
            parse_ping("Reply from 1.1.1.1: bytes=32 time=7ms TTL=57"),
            Some(7.0)
        );
        assert_eq!(
            parse_ping("Reply from 192.168.1.1: bytes=32 time<1ms TTL=64"),
            Some(1.0)
        );
        assert_eq!(parse_ping("Request timeout for icmp_seq 0"), None);
    }

    #[test]
    fn rejects_hosts_that_would_become_flags_or_arguments() {
        assert!(valid_host("1.1.1.1"));
        assert!(valid_host("eu.pool.ntp.org"));
        assert!(!valid_host("-c"));
        assert!(!valid_host("1.1.1.1 -f"));
        assert!(!valid_host(""));
    }

    #[test]
    fn jitter_is_the_step_between_replies_not_the_spread() {
        let history: VecDeque<Option<f32>> = vec![Some(10.0), None, Some(12.0), Some(11.0)].into();
        let s = summarize("h", &history);
        assert_eq!(s.loss_pct, 25.0);
        assert_eq!(s.avg_ms, Some(11.0));
        assert_eq!(s.jitter_ms, Some(1.5));
        assert_eq!(s.last_ms, Some(11.0));
    }
}
