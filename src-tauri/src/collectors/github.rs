use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(120);
const API_VERSION: &str = "2022-11-28";

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum GithubState {
    Disconnected { reason: String },
    Connected { notifications: usize, open_prs: u64 },
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
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
    let token = super::keychain_secret("github")?;

    let notifications: serde_json::Value =
        get(client, &token, "https://api.github.com/notifications?per_page=50").await?;
    let notifications = notifications.as_array().map(|a| a.len()).unwrap_or(0);

    let prs: serde_json::Value = get(
        client,
        &token,
        "https://api.github.com/search/issues?q=is%3Aopen+is%3Apr+author%3A%40me",
    )
    .await?;
    let open_prs = prs["total_count"].as_u64().unwrap_or(0);

    Ok(GithubState::Connected { notifications, open_prs })
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
