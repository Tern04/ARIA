use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const WS_BASE: &str = "https://stag-ws.zcu.cz/ws";
const POLL: Duration = Duration::from_secs(900);
const POLL_DISCONNECTED: Duration = Duration::from_secs(300);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_EXAMS: usize = 3;

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum StagState {
    Disconnected {
        reason: String,
    },
    Connected {
        program: String,
        semester: String,
        days: Vec<DaySchedule>,
        exams: Vec<ExamEntry>,
    },
}

#[derive(Serialize, Clone)]
struct DaySchedule {
    label: String,
    today: bool,
    classes: Vec<ClassEntry>,
}

#[derive(Serialize, Clone)]
struct ClassEntry {
    time: String,
    subject: String,
    kind: String,
    room: String,
}

#[derive(Serialize, Clone)]
struct ExamEntry {
    date: String,
    subject: String,
}

pub fn spawn(app: AppHandle, mut ready: tokio::sync::watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.wait_for(|r| *r).await;
        let client = match reqwest::Client::builder().user_agent("aria-hud").build() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("stag: failed to build http client: {e}");
                return;
            }
        };
        loop {
            let state = poll(&client)
                .await
                .unwrap_or_else(|reason| StagState::Disconnected { reason });
            let connected = matches!(state, StagState::Connected { .. });
            if let Err(e) = app.emit("stag", state) {
                eprintln!("stag emit failed: {e}");
            }
            tokio::time::sleep(if connected { POLL } else { POLL_DISCONNECTED }).await;
        }
    });
}

async fn poll(client: &reqwest::Client) -> Result<StagState, String> {
    let ticket = super::keychain_secret("stag").map_err(|_| "not connected".to_string())?;
    let os_cislo = student_number(client, &ticket).await?;

    let today = chrono::Local::now().date_naive();
    let monday = today - ChronoDuration::days(today.weekday().num_days_from_monday() as i64);
    let sunday = monday + ChronoDuration::days(6);

    let schedule_url = format!(
        "{WS_BASE}/services/rest2/rozvrhy/getRozvrhByStudent?osCislo={os_cislo}&datumOd={}&datumDo={}&outputFormat=JSON",
        monday.format("%-d.%-m.%Y"),
        sunday.format("%-d.%-m.%Y"),
    );
    let exams_url = format!(
        "{WS_BASE}/services/rest2/terminy/getTerminyProStudenta?osCislo={os_cislo}&outputFormat=JSON"
    );
    let info_url = format!(
        "{WS_BASE}/services/rest2/student/getStudentInfo?osCislo={os_cislo}&outputFormat=JSON"
    );
    let (year, term) = academic_term(today);
    let subjects_url = format!(
        "{WS_BASE}/services/rest2/predmety/getPredmetyByStudent?osCislo={os_cislo}&rok={year}&semestr={term}&outputFormat=JSON"
    );

    let (schedule, exams, info, subjects) = tokio::join!(
        get(client, &ticket, &schedule_url),
        get(client, &ticket, &exams_url),
        get(client, &ticket, &info_url),
        get(client, &ticket, &subjects_url),
    );

    let days = parse_week(&schedule?, today);
    let exams = parse_exams(&exams.unwrap_or_default(), today);

    let program = info
        .map(|i| {
            let name = i["nazevSp"].as_str().unwrap_or("?").to_string();
            match i["rocnik"].as_str() {
                Some(r) => format!("{name} · {r}. ročník"),
                None => name,
            }
        })
        .unwrap_or_else(|_| "?".into());

    let semester = subjects
        .ok()
        .and_then(|s| {
            let list = s["predmetStudenta"].as_array()?.clone();
            let credits: u64 = list.iter().filter_map(|p| p["kredity"].as_u64()).sum();
            Some(format!(
                "{term} {year}/{} · {} subjects · {credits} cr",
                (year + 1) % 100,
                list.len()
            ))
        })
        .unwrap_or_default();

    Ok(StagState::Connected {
        program,
        semester,
        days,
        exams,
    })
}

fn parse_week(body: &serde_json::Value, today: NaiveDate) -> Vec<DaySchedule> {
    let Some(akce) = body["rozvrhovaAkce"].as_array() else {
        return Vec::new();
    };
    // BTreeMap keyed by date keeps the week ordered.
    let mut by_day: BTreeMap<NaiveDate, Vec<(String, ClassEntry)>> = BTreeMap::new();
    for a in akce {
        let Some(date) = a["datum"]["value"]
            .as_str()
            .and_then(|d| NaiveDate::parse_from_str(d, "%d.%m.%Y").ok())
        else {
            continue;
        };
        let Some(entry) = parse_class(a) else {
            continue;
        };
        let day_zkr = a["denZkr"].as_str().unwrap_or("?").to_string();
        by_day.entry(date).or_default().push((day_zkr, entry));
    }

    by_day
        .into_iter()
        .map(|(date, mut items)| {
            items.sort_by(|a, b| a.1.time.cmp(&b.1.time));
            let day_zkr = items[0].0.clone();
            DaySchedule {
                label: format!("{} {}", day_zkr, date.format("%-d.%-m.")),
                today: date == today,
                classes: items.into_iter().map(|(_, c)| c).collect(),
            }
        })
        .collect()
}

fn parse_class(akce: &serde_json::Value) -> Option<ClassEntry> {
    let from = text(akce, &["hodinaSkutOd", "casOd", "hodinaOd"])?;
    let to = text(akce, &["hodinaSkutDo", "casDo", "hodinaDo"]).unwrap_or("?");
    let subject = match (text(akce, &["katedra"]), text(akce, &["predmet", "zkratka"])) {
        (Some(dept), Some(code)) => format!("{dept}/{code}"),
        (None, Some(code)) => code.to_string(),
        _ => text(akce, &["nazev"]).unwrap_or("?").to_string(),
    };
    let room = match (text(akce, &["budova"]), text(akce, &["mistnost"])) {
        (Some(b), Some(m)) => format!("{b}-{m}"),
        (None, Some(m)) => m.to_string(),
        _ => String::new(),
    };
    Some(ClassEntry {
        time: format!("{from}–{to}"),
        subject,
        kind: text(akce, &["typAkceZkr"]).unwrap_or("").to_string(),
        room,
    })
}

fn parse_exams(body: &serde_json::Value, today: NaiveDate) -> Vec<ExamEntry> {
    let Some(terminy) = body["termin"].as_array() else {
        return Vec::new();
    };
    let mut exams: Vec<(NaiveDate, ExamEntry)> = terminy
        .iter()
        .filter_map(|t| {
            let date = text(t, &["datum"])
                .and_then(|d| NaiveDate::parse_from_str(d, "%d.%m.%Y").ok())?;
            if date < today {
                return None;
            }
            let subject = match (text(t, &["katedra"]), text(t, &["predmet", "zkratka"])) {
                (Some(dept), Some(code)) => format!("{dept}/{code}"),
                (None, Some(code)) => code.to_string(),
                _ => text(t, &["nazev"]).unwrap_or("?").to_string(),
            };
            let when = match text(t, &["casOd"]) {
                Some(time) => format!("{} {time}", date.format("%-d.%-m.")),
                None => date.format("%-d.%-m.").to_string(),
            };
            Some((date, ExamEntry { date: when, subject }))
        })
        .collect();
    exams.sort_by_key(|(d, _)| *d);
    exams.truncate(MAX_EXAMS);
    exams.into_iter().map(|(_, e)| e).collect()
}

/// STAG serves several shapes depending on version; probe alternate keys and
/// accept both plain strings and {value: "..."} wrappers.
fn text<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| {
        let field = &v[*k];
        field.as_str().or_else(|| field["value"].as_str())
    })
}

/// Academic year runs Sep–Aug; winter term (ZS) Sep–Jan, summer (LS) Feb–Aug.
fn academic_term(today: NaiveDate) -> (i32, &'static str) {
    match today.month() {
        9..=12 => (today.year(), "ZS"),
        1 => (today.year() - 1, "ZS"),
        _ => (today.year() - 1, "LS"),
    }
}

async fn student_number(client: &reqwest::Client, ticket: &str) -> Result<String, String> {
    let url = format!(
        "{WS_BASE}/services/rest2/help/getStagUserListForLoginTicket?ticket={ticket}&outputFormat=JSON"
    );
    let body: serde_json::Value = get(client, ticket, &url).await?;
    body["stagUserInfo"]
        .as_array()
        .and_then(|users| {
            users
                .iter()
                .find_map(|u| u["osCislo"].as_str().filter(|s| !s.is_empty()))
        })
        .map(String::from)
        .ok_or_else(|| "ticket valid but no student number — reconnect".into())
}

async fn get(client: &reqwest::Client, ticket: &str, url: &str) -> Result<serde_json::Value, String> {
    let resp = client
        .get(url)
        .basic_auth(ticket, Some(""))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("ticket expired — reconnect".into());
    }
    resp.error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

/// Open the CAS login page in the default browser and catch the redirect on a
/// localhost listener. STAG appends ?stagUserTicket=... to originalURL.
#[tauri::command]
pub async fn stag_login(app: AppHandle) -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let login_url = format!(
        "{WS_BASE}/login?originalURL=http%3A%2F%2Flocalhost%3A{port}%2F&longTicket=1"
    );
    tauri_plugin_opener::open_url(&login_url, None::<&str>).map_err(|e| e.to_string())?;

    let ticket = tauri::async_runtime::spawn_blocking(move || wait_for_ticket(listener))
        .await
        .map_err(|e| e.to_string())??;

    let entry = keyring::Entry::new("ARIA", "stag").map_err(|e| e.to_string())?;
    entry.set_password(&ticket).map_err(|e| e.to_string())?;

    // Refresh immediately instead of waiting out the collector interval.
    let client = reqwest::Client::builder()
        .user_agent("aria-hud")
        .build()
        .map_err(|e| e.to_string())?;
    let state = poll(&client)
        .await
        .unwrap_or_else(|reason| StagState::Disconnected { reason });
    app.emit("stag", state).map_err(|e| e.to_string())
}

fn wait_for_ticket(listener: TcpListener) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;

    while Instant::now() < deadline {
        let (stream, _) = match listener.accept() {
            Ok(conn) => conn,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        stream.set_nonblocking(false).ok();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .ok();

        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            continue;
        }
        let ticket = request_line
            .split("stagUserTicket=")
            .nth(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect::<String>()
            })
            .filter(|t| !t.is_empty());

        let mut stream = reader.into_inner();
        let page = if ticket.is_some() {
            "ARIA connected. You can close this tab."
        } else {
            "No ticket in request." // favicon probe etc.; keep listening
        };
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            )
            .as_bytes(),
        );

        if let Some(t) = ticket {
            return Ok(t);
        }
    }
    Err("login timed out after 5 minutes".into())
}
