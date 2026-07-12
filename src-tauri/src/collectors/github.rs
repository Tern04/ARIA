use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(120);
const API_VERSION: &str = "2022-11-28";
// Enough columns to fill the widget without dominating it.
const CALENDAR_WEEKS: usize = 18;

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum GithubState {
    Disconnected {
        reason: String,
    },
    Connected {
        notifications: usize,
        open_prs: u64,
        recent_repo: Option<RecentRepo>,
        contributions: Vec<Vec<u32>>,
        total_contributions: u64,
    },
}

#[derive(Serialize, Clone)]
struct RecentRepo {
    name: String,
    pushed_at: String,
}

pub fn spawn(app: AppHandle, mut ready: tokio::sync::watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.wait_for(|r| *r).await;
        let client = match reqwest::Client::builder().user_agent("aria-hud").build() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("github: failed to build http client: {e}");
                return;
            }
        };
        loop {
            let state = poll(&client)
                .await
                .unwrap_or_else(|reason| GithubState::Disconnected { reason });
            if let Err(e) = app.emit("github", state) {
                eprintln!("github emit failed: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

async fn poll(client: &reqwest::Client) -> Result<GithubState, String> {
    let token = super::keychain_secret("github").map_err(|_| "not connected".to_string())?;

    let (notifications, prs, repos, calendar) = tokio::join!(
        get(client, &token, "https://api.github.com/notifications?per_page=50"),
        get(
            client,
            &token,
            "https://api.github.com/search/issues?q=is%3Aopen+is%3Apr+author%3A%40me",
        ),
        get(
            client,
            &token,
            "https://api.github.com/user/repos?sort=pushed&per_page=1",
        ),
        contribution_calendar(client, &token),
    );

    let notifications = notifications?.as_array().map(|a| a.len()).unwrap_or(0);
    let open_prs = prs?["total_count"].as_u64().unwrap_or(0);

    let recent_repo = repos.ok().and_then(|r| {
        let repo = r.as_array()?.first()?.clone();
        Some(RecentRepo {
            name: repo["name"].as_str()?.to_string(),
            pushed_at: repo["pushed_at"]
                .as_str()
                .and_then(|t| t.split('T').next())
                .unwrap_or("")
                .to_string(),
        })
    });

    // The calendar needs read:user scope; degrade to an empty wall without it.
    let (contributions, total_contributions) = calendar.unwrap_or_default();

    Ok(GithubState::Connected {
        notifications,
        open_prs,
        recent_repo,
        contributions,
        total_contributions,
    })
}

/// Contribution wall (the green squares) is only exposed via GraphQL.
async fn contribution_calendar(
    client: &reqwest::Client,
    token: &str,
) -> Result<(Vec<Vec<u32>>, u64), String> {
    let query = serde_json::json!({
        "query": "{ viewer { contributionsCollection { contributionCalendar { \
                   totalContributions weeks { contributionDays { contributionCount } } } } } }"
    });
    let body: serde_json::Value = client
        .post("https://api.github.com/graphql")
        .bearer_auth(token)
        .json(&query)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let calendar = &body["data"]["viewer"]["contributionsCollection"]["contributionCalendar"];
    let total = calendar["totalContributions"].as_u64().unwrap_or(0);
    let weeks = calendar["weeks"]
        .as_array()
        .ok_or_else(|| "no calendar in response".to_string())?;

    let contributions: Vec<Vec<u32>> = weeks
        .iter()
        .rev()
        .take(CALENDAR_WEEKS)
        .rev()
        .map(|w| {
            w["contributionDays"]
                .as_array()
                .map(|days| {
                    days.iter()
                        .map(|d| d["contributionCount"].as_u64().unwrap_or(0) as u32)
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();

    Ok((contributions, total))
}

/// Store a PAT from the widget's login form and refresh immediately.
#[tauri::command]
pub async fn github_setup(app: AppHandle, token: String) -> Result<(), String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("token is required".into());
    }
    super::store_secret("github", &token)?;
    let client = reqwest::Client::builder()
        .user_agent("aria-hud")
        .build()
        .map_err(|e| e.to_string())?;
    let state = poll(&client)
        .await
        .unwrap_or_else(|reason| GithubState::Disconnected { reason });
    app.emit("github", state).map_err(|e| e.to_string())
}

/// Log out: forget the PAT. The poll loop keeps re-emitting disconnected.
/// Best-effort delete: an already-missing secret must not block logging out.
#[tauri::command]
pub fn github_logout(app: AppHandle) -> Result<(), String> {
    if let Err(e) = super::delete_secret("github") {
        eprintln!("github: secret delete: {e}");
    }
    let state = GithubState::Disconnected {
        reason: "not connected".into(),
    };
    app.emit("github", state).map_err(|e| e.to_string())
}

async fn get(client: &reqwest::Client, token: &str, url: &str) -> Result<serde_json::Value, String> {
    client
        .get(url)
        .bearer_auth(token)
        .header("X-GitHub-Api-Version", API_VERSION)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}
