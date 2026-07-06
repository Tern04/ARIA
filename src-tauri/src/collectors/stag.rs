use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const WS_BASE: &str = "https://stag-ws.zcu.cz/ws";
const POLL: Duration = Duration::from_secs(900);
const POLL_DISCONNECTED: Duration = Duration::from_secs(300);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

// Fixed ZČU teaching-period start times, indexed by period number (1-based).
const PERIOD_START: [&str; 15] = [
    "", "07:30", "08:25", "09:20", "10:15", "11:10", "12:05", "13:00", "13:55", "14:50", "15:45",
    "16:40", "17:35", "18:30", "19:25",
];

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum StagState {
    Disconnected {
        reason: String,
    },
    Connected {
        program: String,
        semester: String,
        total_credits: u64,
        timetable: Timetable,
        courses: Vec<Course>,
    },
}

#[derive(Serialize, Clone)]
struct Timetable {
    min_period: u8,
    max_period: u8,
    periods: Vec<PeriodHeader>,
    classes: Vec<ClassEntry>,
}

#[derive(Serialize, Clone)]
struct PeriodHeader {
    period: u8,
    start: String,
}

#[derive(Serialize, Clone)]
struct ClassEntry {
    day: u8, // 0 = Monday
    day_label: String,
    start_period: u8,
    end_period: u8,
    time: String,
    subject: String,
    kind: String,
    room: String,
}

#[derive(Serialize, Clone)]
struct Course {
    code: String,
    name: String,
    credits: u64,
    compulsory: bool,
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
    let (year, term) = academic_term(today);
    let (from, to) = teaching_range(year, term);

    let schedule_url = format!(
        "{WS_BASE}/services/rest2/rozvrhy/getRozvrhByStudent?osCislo={os_cislo}&datumOd={}&datumDo={}&outputFormat=JSON",
        from.format("%-d.%-m.%Y"),
        to.format("%-d.%-m.%Y"),
    );
    let info_url = format!(
        "{WS_BASE}/services/rest2/student/getStudentInfo?osCislo={os_cislo}&outputFormat=JSON"
    );
    let subjects_url = format!(
        "{WS_BASE}/services/rest2/predmety/getPredmetyByStudent?osCislo={os_cislo}&rok={year}&semestr={term}&outputFormat=JSON"
    );

    let (schedule, info, subjects) = tokio::join!(
        get(client, &ticket, &schedule_url),
        get(client, &ticket, &info_url),
        get(client, &ticket, &subjects_url),
    );

    let timetable = parse_timetable(&schedule?);
    let courses = parse_courses(&subjects.unwrap_or_default());
    let total_credits = courses.iter().map(|c| c.credits).sum();

    let program = info
        .map(|i| {
            let name = i["nazevSp"].as_str().unwrap_or("?").to_string();
            match i["rocnik"].as_str() {
                Some(r) => format!("{name} · {r}. ročník"),
                None => name,
            }
        })
        .unwrap_or_else(|_| "?".into());

    let semester = format!("{term} {year}/{}", (year + 1) % 100);

    Ok(StagState::Connected {
        program,
        semester,
        total_credits,
        timetable,
        courses,
    })
}

/// Build the recurring weekly grid from the whole teaching semester: regular
/// classes recur every week, so dedupe by day+period+subject+type. Exams
/// (typAkce "Zkouška") are dropped — they are not part of the weekly rhythm.
fn parse_timetable(body: &serde_json::Value) -> Timetable {
    let mut classes: Vec<ClassEntry> = Vec::new();
    let mut seen: BTreeSet<(u8, u8, String, String)> = BTreeSet::new();
    let mut periods_used: BTreeSet<u8> = BTreeSet::new();

    if let Some(akce) = body["rozvrhovaAkce"].as_array() {
        for a in akce {
            let typ = text(a, &["typAkce"]).unwrap_or("");
            if typ.to_lowercase().contains("zkou") {
                continue; // skip exams
            }
            let Some(entry) = parse_class(a) else {
                continue;
            };
            let key = (
                entry.day,
                entry.start_period,
                entry.subject.clone(),
                entry.kind.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            for p in entry.start_period..=entry.end_period {
                periods_used.insert(p);
            }
            classes.push(entry);
        }
    }

    classes.sort_by(|a, b| (a.day, a.start_period).cmp(&(b.day, b.start_period)));

    let min_period = periods_used.iter().min().copied().unwrap_or(1);
    let max_period = periods_used.iter().max().copied().unwrap_or(1);
    let periods = (min_period..=max_period)
        .map(|p| PeriodHeader {
            period: p,
            start: PERIOD_START.get(p as usize).unwrap_or(&"").to_string(),
        })
        .collect();

    Timetable {
        min_period,
        max_period,
        periods,
        classes,
    }
}

fn parse_class(akce: &serde_json::Value) -> Option<ClassEntry> {
    let day = day_index(akce["denZkr"].as_str().or_else(|| akce["den"].as_str())?)?;
    let start_period = akce["hodinaOd"].as_u64()? as u8;
    let end_period = akce["hodinaDo"].as_u64().unwrap_or(start_period as u64) as u8;
    let from = text(akce, &["hodinaSkutOd", "casOd"]).unwrap_or("");
    let to = text(akce, &["hodinaSkutDo", "casDo"]).unwrap_or("");
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
        day,
        day_label: day_short(day).to_string(),
        start_period,
        end_period,
        time: format!("{from}–{to}"),
        subject,
        kind: text(akce, &["typAkceZkr"]).unwrap_or("").to_string(),
        room,
    })
}

fn parse_courses(body: &serde_json::Value) -> Vec<Course> {
    let Some(list) = body["predmetStudenta"].as_array() else {
        return Vec::new();
    };
    let mut courses: Vec<Course> = list
        .iter()
        .filter_map(|p| {
            let code = match (p["katedra"].as_str(), p["zkratka"].as_str()) {
                (Some(dept), Some(zk)) => format!("{dept}/{zk}"),
                (_, Some(zk)) => zk.to_string(),
                _ => return None,
            };
            Some(Course {
                code,
                name: p["nazev"].as_str().unwrap_or("").to_string(),
                credits: p["kredity"].as_u64().unwrap_or(0),
                compulsory: p["statut"].as_str() == Some("A"),
            })
        })
        .collect();
    courses.sort_by(|a, b| b.credits.cmp(&a.credits));
    courses
}

/// Po=0 … Pá=4; accepts the abbreviated ("Po") or full ("Pondělí") day name.
fn day_index(name: &str) -> Option<u8> {
    match name {
        "Po" | "Pondělí" => Some(0),
        "Út" | "Úterý" => Some(1),
        "St" | "Středa" => Some(2),
        "Čt" | "Čtvrtek" => Some(3),
        "Pá" | "Pátek" => Some(4),
        _ => None,
    }
}

fn day_short(day: u8) -> &'static str {
    ["Po", "Út", "St", "Čt", "Pá"].get(day as usize).unwrap_or(&"?")
}

/// Teaching weeks (excludes the exam period): LS runs late Feb–mid May of the
/// calendar year after the academic year starts; ZS runs late Sep–mid Dec.
fn teaching_range(ay_start: i32, term: &str) -> (NaiveDate, NaiveDate) {
    let ymd = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
    match term {
        "LS" => (ymd(ay_start + 1, 2, 15), ymd(ay_start + 1, 5, 20)),
        _ => (ymd(ay_start, 9, 20), ymd(ay_start, 12, 20)),
    }
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

    super::store_secret("stag", &ticket)?;

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
