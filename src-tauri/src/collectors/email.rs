use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const POLL: Duration = Duration::from_secs(300);

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum EmailState {
    Disconnected { reason: String },
    Connected { unread: u32 },
}

#[derive(Deserialize, Serialize)]
struct ImapConfig {
    host: String,
    user: String,
}

impl Default for ImapConfig {
    fn default() -> Self {
        Self {
            host: "imap.seznam.cz".into(),
            user: String::new(),
        }
    }
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let config = load_config(&app);
            let state = match config {
                Ok(cfg) => {
                    let fetched =
                        tauri::async_runtime::spawn_blocking(move || fetch_unread(&cfg)).await;
                    match fetched {
                        Ok(Ok(unread)) => EmailState::Connected { unread },
                        Ok(Err(reason)) => EmailState::Disconnected { reason },
                        Err(e) => EmailState::Disconnected { reason: e.to_string() },
                    }
                }
                Err(reason) => EmailState::Disconnected { reason },
            };
            if let Err(e) = app.emit("email", state) {
                eprintln!("email emit failed: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

/// Reads imap.json from the app config dir; writes a template on first run
/// so the user has a file to fill in.
fn load_config(app: &AppHandle) -> Result<ImapConfig, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let path = dir.join("imap.json");

    if !path.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let template = serde_json::to_string_pretty(&ImapConfig::default())
            .map_err(|e| e.to_string())?;
        std::fs::write(&path, template).map_err(|e| e.to_string())?;
        return Err("imap.json created — fill in user".into());
    }

    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let cfg: ImapConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if cfg.user.is_empty() {
        return Err("imap.json has no user set".into());
    }
    Ok(cfg)
}

/// Short-lived connection by design: connect, STATUS, logout.
fn fetch_unread(cfg: &ImapConfig) -> Result<u32, String> {
    let password = super::keychain_secret("imap")?;
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let client = imap::connect((cfg.host.as_str(), 993), cfg.host.as_str(), &tls)
        .map_err(|e| e.to_string())?;
    let mut session = client
        .login(&cfg.user, &password)
        .map_err(|(e, _)| e.to_string())?;
    let mailbox = session
        .status("INBOX", "(UNSEEN)")
        .map_err(|e| e.to_string())?;
    session.logout().ok();
    Ok(mailbox.unseen.unwrap_or(0))
}
