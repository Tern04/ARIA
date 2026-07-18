use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(120);
const API_VERSION: &str = "2022-11-28";
// Enough columns to fill the widget; the wall stretches to fit.
const CALENDAR_WEEKS: usize = 26;
// Item lists rendered at the large size.
const NOTIF_ITEMS: usize = 5;
const PR_ITEMS: usize = 3;

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum GithubState {
    Disconnected {
        reason: String,
    },
    Connected {
        notifications: usize,
        notification_items: Vec<NotificationItem>,
        open_prs: u64,
        pr_items: Vec<PrItem>,
        recent_repos: Vec<RecentRepo>,
        contributions: Vec<Vec<u32>>,
        total_contributions: u64,
    },
}

#[derive(Serialize, Clone)]
struct NotificationItem {
    title: String,
    repo: String,
    reason: String,
}

#[derive(Serialize, Clone)]
struct PrItem {
    title: String,
    repo: String,
    number: u64,
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
            "https://api.github.com/user/repos?sort=pushed&per_page=3",
        ),
        contribution_calendar(client, &token),
    );

    let notifications = notifications?;
    let notifications = notifications.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let notification_items = notifications
        .iter()
        .take(NOTIF_ITEMS)
        .map(|n| NotificationItem {
            title: n["subject"]["title"].as_str().unwrap_or("?").to_string(),
            repo: n["repository"]["name"].as_str().unwrap_or("").to_string(),
            reason: n["reason"].as_str().unwrap_or("").replace('_', " "),
        })
        .collect();
    let notifications = notifications.len();

    let prs = prs?;
    let open_prs = prs["total_count"].as_u64().unwrap_or(0);
    let pr_items = prs["items"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .take(PR_ITEMS)
        .map(|p| PrItem {
            title: p["title"].as_str().unwrap_or("?").to_string(),
            repo: p["repository_url"]
                .as_str()
                .and_then(|u| u.rsplit('/').next())
                .unwrap_or("")
                .to_string(),
            number: p["number"].as_u64().unwrap_or(0),
        })
        .collect();

    let recent_repos = repos
        .ok()
        .and_then(|r| {
            Some(
                r.as_array()?
                    .iter()
                    .filter_map(|repo| {
                        Some(RecentRepo {
                            name: repo["name"].as_str()?.to_string(),
                            pushed_at: repo["pushed_at"]
                                .as_str()
                                .and_then(|t| t.split('T').next())
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();

    // The calendar needs read:user scope; degrade to an empty wall without it.
    let (contributions, total_contributions) = calendar.unwrap_or_default();

    Ok(GithubState::Connected {
        notifications,
        notification_items,
        open_prs,
        pr_items,
        recent_repos,
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
