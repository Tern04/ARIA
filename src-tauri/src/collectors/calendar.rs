use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime,
    TimeZone, Timelike, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const POLL: Duration = Duration::from_secs(300);
// Agenda horizon: recurring events are expanded across this window and no
// further, so a weekly event contributes at most a handful of occurrences.
const WINDOW_DAYS: i64 = 28;
// Rows the large agenda can show; smaller sizes clip the rest.
const EVENTS_SHOWN: usize = 40;
// Guard against a pathological RRULE (e.g. FREQ=SECONDLY) generating forever.
const MAX_OCCURRENCES: usize = 500;

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CalendarState {
    Disconnected {
        reason: String,
    },
    Connected {
        feeds: Vec<FeedSummary>,
        events: Vec<Event>,
        today_count: usize,
        /// One entry per day in the window that has events — the month grid
        /// paints a dot on each. Not truncated with the agenda.
        day_counts: Vec<DayCount>,
    },
}

#[derive(Serialize, Clone)]
struct FeedSummary {
    label: String,
    color: usize,
    ok: bool, // false when this feed failed to fetch/parse this poll
}

#[derive(Serialize, Clone)]
struct DayCount {
    date: String, // local YYYY-MM-DD
    count: usize,
}

#[derive(Serialize, Clone)]
struct Event {
    feed: String,
    color: usize,
    title: String,
    location: String,
    all_day: bool,
    start: i64,          // unix seconds, for sorting
    day_key: String,     // local YYYY-MM-DD, for grouping and month dots
    day_label: String,   // "Today" / "Tomorrow" / "Fri 25.7."
    time_label: String,  // "14:00", or "" for all-day
}

#[derive(Deserialize, Serialize, Clone)]
struct Feed {
    label: String,
}

#[derive(Deserialize, Serialize, Default)]
struct CalendarConfig {
    feeds: Vec<Feed>,
}

/// Wakes the poll loop early (widget refresh button).
static REFRESH: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[tauri::command]
pub fn cal_refresh() {
    REFRESH.notify_one();
}

pub fn spawn(app: AppHandle, mut ready: tokio::sync::watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.wait_for(|r| *r).await;
        let client = match reqwest::Client::builder().user_agent("aria-hud").build() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("calendar: failed to build http client: {e}");
                return;
            }
        };
        loop {
            if let Err(e) = refresh(&app, &client).await {
                eprintln!("calendar emit failed: {e}");
            }
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = REFRESH.notified() => {}
            }
        }
    });
}

fn config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("calendar.json"))
}

/// Config for the read path: no file (or no feeds) is a clean "not configured".
fn load_config(app: &AppHandle) -> Result<CalendarConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Err("no calendars".into());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut cfg: CalendarConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    cfg.feeds.retain(|f| !f.label.is_empty());
    if cfg.feeds.is_empty() {
        return Err("no calendars".into());
    }
    Ok(cfg)
}

fn save_config(app: &AppHandle, cfg: &CalendarConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(app)?, json).map_err(|e| e.to_string())
}

/// Config for the mutation path: a missing file is empty, but a corrupt one is
/// a hard error — rewriting it would wipe the user's existing feeds.
fn read_config_for_update(app: &AppHandle) -> Result<CalendarConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(CalendarConfig::default());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("calendar.json is invalid, fix it first: {e}"))
}

/// Fetch every feed and emit — shared by the poll loop and the commands.
async fn refresh(app: &AppHandle, client: &reqwest::Client) -> Result<(), String> {
    let state = match load_config(app) {
        Ok(cfg) => fetch_all(client, &cfg).await,
        Err(reason) => CalendarState::Disconnected { reason },
    };
    app.emit("calendar", state).map_err(|e| e.to_string())
}

async fn fetch_all(client: &reqwest::Client, cfg: &CalendarConfig) -> CalendarState {
    let now = Local::now();
    let window_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| Local.from_local_datetime(&d).single())
        .unwrap_or(now);
    let window_end = window_start + ChronoDuration::days(WINDOW_DAYS);

    let mut feeds = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    for (i, feed) in cfg.feeds.iter().enumerate() {
        let ok = match fetch_feed(client, &feed.label).await {
            Ok(text) => {
                let mut evs = expand_feed(&text, &feed.label, i, window_start, window_end);
                events.append(&mut evs);
                true
            }
            Err(e) => {
                eprintln!("calendar {}: {e}", feed.label);
                false
            }
        };
        feeds.push(FeedSummary {
            label: feed.label.clone(),
            color: i,
            ok,
        });
    }

    events.sort_by_key(|e| e.start);

    // Month dots span the whole window; count before the agenda is truncated.
    let mut per_day: BTreeMap<String, usize> = BTreeMap::new();
    for e in &events {
        *per_day.entry(e.day_key.clone()).or_default() += 1;
    }
    let day_counts = per_day
        .into_iter()
        .map(|(date, count)| DayCount { date, count })
        .collect();

    let today_key = now.format("%Y-%m-%d").to_string();
    let today_count = events.iter().filter(|e| e.day_key == today_key).count();

    events.truncate(EVENTS_SHOWN);

    CalendarState::Connected {
        feeds,
        events,
        today_count,
        day_counts,
    }
}

/// Fetch one feed's ICS text. The secret iCal URL lives in the keychain under
/// account "ical:<label>", never in the plaintext config.
async fn fetch_feed(client: &reqwest::Client, label: &str) -> Result<String, String> {
    let url = super::keychain_secret(&format!("ical:{label}"))?;
    let resp = client
        .get(url.trim())
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    resp.text().await.map_err(|e| e.to_string())
}

/// Add (or replace) a feed from the widget form: the secret iCal URL goes to
/// the keychain, the label to calendar.json, then refresh immediately.
#[tauri::command]
pub async fn cal_add_feed(app: AppHandle, label: String, url: String) -> Result<(), String> {
    let label = label.trim().to_string();
    let url = url.trim().to_string();
    if label.is_empty() || url.is_empty() {
        return Err("label and iCal URL are required".into());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") && !url.starts_with("webcal://") {
        return Err("that doesn't look like an iCal URL".into());
    }
    // webcal:// is just http(s) with a custom scheme; normalize so reqwest fetches it.
    let url = url.replacen("webcal://", "https://", 1);
    let mut cfg = read_config_for_update(&app)?;
    super::store_secret(&format!("ical:{label}"), &url)?;
    cfg.feeds.retain(|f| f.label != label);
    cfg.feeds.push(Feed { label });
    save_config(&app, &cfg)?;
    let client = reqwest::Client::builder()
        .user_agent("aria-hud")
        .build()
        .map_err(|e| e.to_string())?;
    refresh(&app, &client).await
}

#[tauri::command]
pub async fn cal_remove_feed(app: AppHandle, label: String) -> Result<(), String> {
    let mut cfg = read_config_for_update(&app)?;
    cfg.feeds.retain(|f| f.label != label);
    save_config(&app, &cfg)?;
    // Best-effort: a missing secret must not block removing the feed.
    if let Err(e) = super::delete_secret(&format!("ical:{label}")) {
        eprintln!("calendar: secret delete for {label}: {e}");
    }
    let client = reqwest::Client::builder()
        .user_agent("aria-hud")
        .build()
        .map_err(|e| e.to_string())?;
    refresh(&app, &client).await
}

// ── ICS parsing ─────────────────────────────────────────────

/// A start instant plus whether the source property was an all-day DATE.
#[derive(Clone, Copy)]
struct IcsTime {
    at: DateTime<Local>,
    all_day: bool,
}

/// Parse every VEVENT in the ICS text and expand recurrences into the window.
fn expand_feed(
    text: &str,
    feed: &str,
    color: usize,
    window_start: DateTime<Local>,
    window_end: DateTime<Local>,
) -> Vec<Event> {
    let lines = unfold(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "BEGIN:VEVENT" {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim() != "END:VEVENT" {
                j += 1;
            }
            emit_event(&lines[i + 1..j], feed, color, window_start, window_end, &mut out);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// RFC 5545 line folding: a CRLF followed by a space or tab continues the
/// previous logical line. Join those back before parsing properties.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        lines.push(line.to_string());
    }
    lines
}

fn emit_event(
    body: &[String],
    feed: &str,
    color: usize,
    window_start: DateTime<Local>,
    window_end: DateTime<Local>,
    out: &mut Vec<Event>,
) {
    let mut title = String::new();
    let mut location = String::new();
    let mut dtstart: Option<IcsTime> = None;
    let mut rrule: Option<String> = None;
    let mut exdates: BTreeSet<i64> = BTreeSet::new();

    for line in body {
        let Some((name, params, value)) = split_property(line) else {
            continue;
        };
        match name.as_str() {
            "SUMMARY" => title = unescape_text(&value),
            "LOCATION" => location = unescape_text(&value),
            "DTSTART" => dtstart = parse_time(&params, &value),
            "RRULE" => rrule = Some(value),
            "EXDATE" => {
                for part in value.split(',') {
                    if let Some(t) = parse_time(&params, part) {
                        exdates.insert(t.at.timestamp());
                    }
                }
            }
            _ => {}
        }
    }

    let Some(start) = dtstart else { return };
    if title.is_empty() {
        title = "(busy)".into();
    }

    let mut push = |at: DateTime<Local>| {
        if at < window_start || at >= window_end || exdates.contains(&at.timestamp()) {
            return;
        }
        out.push(build_event(feed, color, &title, &location, at, start.all_day, window_start));
    };

    match &rrule {
        Some(rule) => expand_rrule(rule, start.at, window_start, window_end, &mut push),
        None => push(start.at),
    }
}

/// Turn one recurrence rule into concrete start instants, invoking `push` for
/// each. Covers the common desktop-calendar shapes (FREQ with INTERVAL, COUNT,
/// UNTIL and — for weekly — BYDAY); anything exotic falls back to the base day.
fn expand_rrule(
    rule: &str,
    base: DateTime<Local>,
    window_start: DateTime<Local>,
    window_end: DateTime<Local>,
    push: &mut impl FnMut(DateTime<Local>),
) {
    let mut freq = "";
    let mut interval: i64 = 1;
    let mut count: Option<usize> = None;
    let mut until: Option<DateTime<Local>> = None;
    let mut byday: Vec<Weekday> = Vec::new();
    for part in rule.split(';') {
        let Some((k, v)) = part.split_once('=') else { continue };
        match k {
            "FREQ" => freq = v,
            "INTERVAL" => interval = v.parse().unwrap_or(1).max(1),
            "COUNT" => count = v.parse().ok(),
            "UNTIL" => until = parse_time(&BTreeMap::new(), v).map(|t| t.at),
            "BYDAY" => byday = v.split(',').filter_map(parse_weekday).collect(),
            _ => {}
        }
    }

    let end = until.map(|u| u.min(window_end)).unwrap_or(window_end);
    let mut emitted = 0usize;
    let mut guard = 0usize;
    let mut emit = |at: DateTime<Local>, emitted: &mut usize| -> bool {
        if let Some(c) = count {
            if *emitted >= c {
                return false;
            }
        }
        if at > end {
            return false;
        }
        push(at);
        *emitted += 1;
        true
    };

    // Recurrences keep their wall-clock time-of-day, so step by calendar dates
    // (DST-free) and re-localize with the base time — never by a fixed 24 h
    // duration, which would shift 08:00 to 09:00 across a DST boundary.
    let tod = base.time();
    let base_date = base.date_naive();
    let end_date = end.date_naive();

    match freq {
        "WEEKLY" if !byday.is_empty() => {
            // Walk week by week from the base week's Monday; within each active
            // week emit every requested weekday (COUNT/UNTIL still bound it).
            let base_monday =
                base_date - ChronoDuration::days(base.weekday().num_days_from_monday() as i64);
            let mut week = 0i64;
            // Without a COUNT, occurrences before the window are irrelevant, so
            // skip whole interval-weeks up to it rather than iterating years.
            if count.is_none() && base_monday < window_start.date_naive() {
                let weeks = (window_start.date_naive() - base_monday).num_days() / 7;
                week = (weeks / interval).max(0);
            }
            'weeks: loop {
                let monday = base_monday + ChronoDuration::days(week * interval * 7);
                if monday > end_date + ChronoDuration::days(7) {
                    break;
                }
                for wd in &byday {
                    let date = monday + ChronoDuration::days(wd.num_days_from_monday() as i64);
                    if date < base_date {
                        continue; // occurrences before DTSTART don't exist
                    }
                    let Some(at) = local_at(date, tod) else { continue };
                    if !emit(at, &mut emitted) && (count.is_some() || at > end) {
                        break 'weeks;
                    }
                }
                week += 1;
                guard += 1;
                if guard > MAX_OCCURRENCES {
                    break;
                }
            }
        }
        "DAILY" | "WEEKLY" => {
            let step = interval * if freq == "DAILY" { 1 } else { 7 };
            // A COUNT-less rule may have started years ago; jump ahead to the
            // window instead of stepping day-by-day into the guard.
            let mut date = if count.is_none() {
                fast_forward_date(base_date, step, window_start.date_naive())
            } else {
                base_date
            };
            loop {
                let Some(at) = local_at(date, tod) else { break };
                if !emit(at, &mut emitted) {
                    break;
                }
                date += ChronoDuration::days(step);
                guard += 1;
                if guard > MAX_OCCURRENCES {
                    break;
                }
            }
        }
        "MONTHLY" | "YEARLY" => {
            // Few enough steps per year that reaching the window needs no
            // fast-forward; add_months rebuilds from Y/M/D so it stays DST-safe.
            let mut at = base;
            loop {
                if !emit(at, &mut emitted) {
                    break;
                }
                at = add_months(at, if freq == "MONTHLY" { interval } else { 12 * interval });
                guard += 1;
                if guard > MAX_OCCURRENCES {
                    break;
                }
            }
        }
        _ => push(base), // unknown FREQ: at least show the first instance
    }
}

/// Build a local instant from a date and a wall-clock time, resolving DST gaps
/// to the earliest valid instant.
fn local_at(date: NaiveDate, tod: chrono::NaiveTime) -> Option<DateTime<Local>> {
    Local.from_local_datetime(&date.and_time(tod)).earliest()
}

/// First occurrence date at or after the window start, stepping in whole
/// `step`-day increments from the base. Date math only, so it is DST-free.
fn fast_forward_date(base: NaiveDate, step: i64, target: NaiveDate) -> NaiveDate {
    if base >= target || step <= 0 {
        return base;
    }
    let gap = (target - base).num_days();
    let mut date = base + ChronoDuration::days((gap / step) * step);
    while date < target {
        date += ChronoDuration::days(step);
    }
    date
}

/// Shift a datetime by whole months, clamping to the last valid day (e.g. a
/// Jan 31 monthly rule lands on Feb 28). Keeps the wall-clock time.
fn add_months(dt: DateTime<Local>, months: i64) -> DateTime<Local> {
    let total = (dt.year() as i64) * 12 + (dt.month0() as i64) + months;
    let year = total.div_euclid(12) as i32;
    let month0 = total.rem_euclid(12) as u32;
    let last = days_in_month(year, month0 + 1);
    let day = dt.day().min(last);
    NaiveDate::from_ymd_opt(year, month0 + 1, day)
        .and_then(|d| d.and_hms_opt(dt.hour(), dt.minute(), dt.second()))
        .and_then(|d| Local.from_local_datetime(&d).earliest())
        .unwrap_or(dt)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();
    (first_next - ChronoDuration::days(1)).day()
}

fn build_event(
    feed: &str,
    color: usize,
    title: &str,
    location: &str,
    at: DateTime<Local>,
    all_day: bool,
    window_start: DateTime<Local>,
) -> Event {
    let day = at.date_naive();
    let today = window_start.date_naive();
    let day_label = if day == today {
        "Today".to_string()
    } else if day == today + ChronoDuration::days(1) {
        "Tomorrow".to_string()
    } else {
        format!("{} {}", at.format("%a"), at.format("%-d.%-m."))
    };
    Event {
        feed: feed.to_string(),
        color,
        title: title.to_string(),
        location: location.to_string(),
        all_day,
        start: at.timestamp(),
        day_key: at.format("%Y-%m-%d").to_string(),
        day_label,
        time_label: if all_day { String::new() } else { at.format("%H:%M").to_string() },
    }
}

/// Split "NAME;PARAM=x;PARAM2=y:VALUE" into (name, params, value). The colon
/// that ends the parameters is the first one not inside a quoted param value.
fn split_property(line: &str) -> Option<(String, BTreeMap<String, String>, String)> {
    let mut in_quote = false;
    let mut colon = None;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(idx);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], line[colon + 1..].to_string());
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let mut params = BTreeMap::new();
    for p in parts {
        if let Some((k, v)) = p.split_once('=') {
            params.insert(k.to_ascii_uppercase(), v.trim_matches('"').to_string());
        }
    }
    Some((name, params, value))
}

/// Parse an ICS date or date-time value into a local instant.
///   * VALUE=DATE (YYYYMMDD)              -> all-day, local midnight
///   * ...T...Z                           -> UTC, converted to local
///   * ...T... with TZID or floating      -> interpreted as local wall-clock
///
/// Full TZID resolution needs a tz database; treating a TZID/floating time as
/// local wall-clock is correct for the common case (your own calendar shown in
/// your own timezone) and keeps the collector dependency-light.
fn parse_time(params: &BTreeMap<String, String>, value: &str) -> Option<IcsTime> {
    let value = value.trim();
    let is_date = params.get("VALUE").map(|v| v == "DATE").unwrap_or(false)
        || (value.len() == 8 && !value.contains('T'));
    if is_date {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        let at = Local.from_local_datetime(&date.and_hms_opt(0, 0, 0)?).single()?;
        return Some(IcsTime { at, all_day: true });
    }
    let naive = NaiveDateTime::parse_from_str(value.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok()?;
    let at = if value.ends_with('Z') {
        chrono::Utc.from_utc_datetime(&naive).with_timezone(&Local)
    } else {
        Local.from_local_datetime(&naive).single()?
    };
    Some(IcsTime { at, all_day: false })
}

fn parse_weekday(token: &str) -> Option<Weekday> {
    // BYDAY entries may carry an ordinal prefix (e.g. "2MO"); we only need the
    // two-letter day code for weekly rules.
    let code = token.trim().get(token.len().saturating_sub(2)..)?;
    match code {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> (DateTime<Local>, DateTime<Local>) {
        let start = Local.with_ymd_and_hms(2026, 7, 20, 0, 0, 0).unwrap(); // a Monday
        (start, start + ChronoDuration::days(WINDOW_DAYS))
    }

    #[test]
    fn unfolds_continuation_lines() {
        let lines = unfold("SUMMARY:Long\r\n  title\r\nLOCATION:Home\r\n");
        assert_eq!(lines[0], "SUMMARY:Long title");
        assert_eq!(lines[1], "LOCATION:Home");
    }

    #[test]
    fn all_day_and_timed_parse() {
        let mut p = BTreeMap::new();
        p.insert("VALUE".to_string(), "DATE".to_string());
        assert!(parse_time(&p, "20260724").unwrap().all_day);
        let t = parse_time(&BTreeMap::new(), "20260724T120000Z").unwrap();
        assert!(!t.all_day);
    }

    #[test]
    fn weekly_byday_expands_each_listed_day() {
        let (ws, we) = window();
        let ics = "BEGIN:VEVENT\nSUMMARY:Practice\nDTSTART:20260720T170000\n\
                   RRULE:FREQ=WEEKLY;BYDAY=MO,WE\nEND:VEVENT\n";
        let evs = expand_feed(ics, "Family", 0, ws, we);
        // 4 weeks × {Mon, Wed} = 8 occurrences in the window.
        assert_eq!(evs.len(), 8);
        assert!(evs.iter().all(|e| e.title == "Practice"));
        assert_eq!(evs[0].time_label, "17:00");
    }

    #[test]
    fn count_limits_recurrence() {
        let (ws, we) = window();
        let ics = "BEGIN:VEVENT\nSUMMARY:Standup\nDTSTART:20260720T090000\n\
                   RRULE:FREQ=DAILY;COUNT=3\nEND:VEVENT\n";
        let evs = expand_feed(ics, "Work", 0, ws, we);
        assert_eq!(evs.len(), 3);
    }

    #[test]
    fn exdate_removes_one_occurrence() {
        let (ws, we) = window();
        let ics = "BEGIN:VEVENT\nSUMMARY:Daily\nDTSTART:20260720T090000\n\
                   RRULE:FREQ=DAILY;COUNT=3\nEXDATE:20260721T090000\nEND:VEVENT\n";
        let evs = expand_feed(ics, "Work", 0, ws, we);
        assert_eq!(evs.len(), 2);
    }

    #[test]
    fn long_running_daily_reaches_window() {
        let (ws, we) = window();
        // Daily since 2019, no end — must still show inside the 2026 window.
        let ics = "BEGIN:VEVENT\nSUMMARY:Meds\nDTSTART:20190101T080000\n\
                   RRULE:FREQ=DAILY\nEND:VEVENT\n";
        let evs = expand_feed(ics, "Home", 0, ws, we);
        assert_eq!(evs.len(), WINDOW_DAYS as usize); // one per day in the window
        assert_eq!(evs[0].time_label, "08:00");
    }

    #[test]
    fn single_event_outside_window_is_dropped() {
        let (ws, we) = window();
        let ics = "BEGIN:VEVENT\nSUMMARY:Past\nDTSTART:20260101T090000\nEND:VEVENT\n";
        assert!(expand_feed(ics, "F", 0, ws, we).is_empty());
    }
}

/// Reverse the RFC 5545 TEXT escaping (\n \, \; \\).
fn unescape_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}
