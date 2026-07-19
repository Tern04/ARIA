use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const POLL: Duration = Duration::from_secs(300);
// Enough rows to fill the large widget size; smaller sizes clip the rest.
const RECENT_PER_ACCOUNT: u32 = 12;
const RECENT_SHOWN: usize = 12;

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum EmailState {
    Disconnected { reason: String },
    Connected { accounts: Vec<AccountSummary>, messages: Vec<MailMessage> },
}

#[derive(Serialize, Clone)]
struct AccountSummary {
    label: String,
    user: String,
    host: String,
    unread: Option<u32>, // None when this account errored this poll
}

#[derive(Serialize, Clone)]
struct MailMessage {
    account: String,
    from: String,
    subject: String,
    unseen: bool,
    /// Unix seconds; the widget renders a relative day/time at l.
    timestamp: i64,
}

#[derive(Deserialize, Serialize, Clone)]
struct ImapAccount {
    label: String,
    host: String,
    user: String,
}

#[derive(Deserialize, Serialize, Default)]
struct MailConfig {
    accounts: Vec<ImapAccount>,
}

/// Wakes the poll loop early (widget refresh button).
static REFRESH: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[tauri::command]
pub fn email_refresh() {
    REFRESH.notify_one();
}

pub fn spawn(app: AppHandle, mut ready: tokio::sync::watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.wait_for(|r| *r).await;
        loop {
            if let Err(e) = refresh(&app).await {
                eprintln!("email emit failed: {e}");
            }
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = REFRESH.notified() => {}
            }
        }
    });
}

fn fetch_all(cfg: &MailConfig) -> EmailState {
    let mut accounts = Vec::new();
    let mut messages: Vec<MailMessage> = Vec::new();
    for account in &cfg.accounts {
        let summary = |unread| AccountSummary {
            label: account.label.clone(),
            user: account.user.clone(),
            host: account.host.clone(),
            unread,
        };
        match fetch_inbox(account) {
            Ok((unread, mut msgs)) => {
                accounts.push(summary(Some(unread)));
                messages.append(&mut msgs);
            }
            Err(e) => {
                eprintln!("mail {}: {e}", account.label);
                accounts.push(summary(None));
            }
        }
    }
    messages.sort_by_key(|m| std::cmp::Reverse(m.timestamp));
    messages.truncate(RECENT_SHOWN);
    EmailState::Connected { accounts, messages }
}

fn config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("mail.json"))
}

/// Reads mail.json from the app config dir. Accounts are managed from the
/// widget's login form (mail_add_account / mail_remove_account); hand-editing
/// the file still works.
fn load_config(app: &AppHandle) -> Result<MailConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Err("no mail accounts".into());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut cfg: MailConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    cfg.accounts.retain(|a| !a.user.is_empty());
    if cfg.accounts.is_empty() {
        return Err("no mail accounts".into());
    }
    Ok(cfg)
}

fn save_config(app: &AppHandle, cfg: &MailConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(app)?, json).map_err(|e| e.to_string())
}

/// Config for the mutation path: a missing file is an empty config, but a
/// corrupt one is a hard error — rewriting it would wipe existing accounts.
fn read_config_for_update(app: &AppHandle) -> Result<MailConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(MailConfig::default());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("mail.json is invalid, fix it first: {e}"))
}

/// Fetch all accounts and emit — shared by the poll loop and the commands.
async fn refresh(app: &AppHandle) -> Result<(), String> {
    let state = match load_config(app) {
        Ok(cfg) => tauri::async_runtime::spawn_blocking(move || fetch_all(&cfg))
            .await
            .unwrap_or_else(|e| EmailState::Disconnected {
                reason: e.to_string(),
            }),
        Err(reason) => EmailState::Disconnected { reason },
    };
    app.emit("email", state).map_err(|e| e.to_string())
}

/// Add (or replace) an account from the widget form: password to the secret
/// store, account to mail.json, then refresh immediately.
#[tauri::command]
pub async fn mail_add_account(
    app: AppHandle,
    label: String,
    host: String,
    user: String,
    password: String,
) -> Result<(), String> {
    let label = label.trim().to_string();
    let host = host.trim().to_string();
    let user = user.trim().to_string();
    if label.is_empty() || host.is_empty() || user.is_empty() || password.is_empty() {
        return Err("all fields are required".into());
    }
    let mut cfg = read_config_for_update(&app)?;
    super::store_secret(&format!("imap:{user}"), &password)?;
    cfg.accounts.retain(|a| a.user != user);
    cfg.accounts.push(ImapAccount { label, host, user });
    save_config(&app, &cfg)?;
    refresh(&app).await
}

#[tauri::command]
pub async fn mail_remove_account(app: AppHandle, user: String) -> Result<(), String> {
    let mut cfg = read_config_for_update(&app)?;
    cfg.accounts.retain(|a| a.user != user);
    save_config(&app, &cfg)?;
    // Best-effort: a missing secret must not block removing the account.
    if let Err(e) = super::delete_secret(&format!("imap:{user}")) {
        eprintln!("mail: secret delete for {user}: {e}");
    }
    refresh(&app).await
}

/// Short-lived connection by design: connect, read-only fetch, logout.
/// The password lives in the keychain under account "imap:<user>".
fn fetch_inbox(account: &ImapAccount) -> Result<(u32, Vec<MailMessage>), String> {
    let password = super::keychain_secret(&format!("imap:{}", account.user))?;
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let client = imap::connect((account.host.as_str(), 993), account.host.as_str(), &tls)
        .map_err(|e| e.to_string())?;
    let mut session = client
        .login(&account.user, &password)
        .map_err(|(e, _)| e.to_string())?;

    // EXAMINE keeps the mailbox read-only so fetching never flips \Seen.
    let mailbox = session.examine("INBOX").map_err(|e| e.to_string())?;

    // Some servers (Seznam) return empty FLAGS in FETCH responses, so unseen
    // state comes from SEARCH instead.
    let unseen_set = session.search("UNSEEN").map_err(|e| e.to_string())?;
    let unread = unseen_set.len() as u32;

    let mut messages = Vec::new();
    if mailbox.exists > 0 {
        let start = mailbox.exists.saturating_sub(RECENT_PER_ACCOUNT - 1).max(1);
        let fetches = session
            .fetch(format!("{start}:{}", mailbox.exists), "ENVELOPE")
            .map_err(|e| e.to_string())?;
        for f in fetches.iter() {
            let Some(envelope) = f.envelope() else {
                continue;
            };
            messages.push(MailMessage {
                account: account.label.clone(),
                from: sender_of(envelope),
                subject: decode_word(envelope.subject),
                unseen: unseen_set.contains(&f.message),
                timestamp: parse_date(envelope.date),
            });
        }
    }

    session.logout().ok();
    Ok((unread, messages))
}

/// RFC2822 date to unix seconds; undated messages sort last.
fn parse_date(raw: Option<&[u8]>) -> i64 {
    let Some(raw) = raw else { return 0 };
    let text = String::from_utf8_lossy(raw);
    chrono::DateTime::parse_from_rfc2822(text.trim())
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

/// Display name if present, otherwise mailbox@host.
fn sender_of(env: &imap_proto::types::Envelope) -> String {
    let Some(addr) = env.from.as_ref().and_then(|list| list.first()) else {
        return "unknown".into();
    };
    let name = decode_word(addr.name);
    if !name.is_empty() {
        return name;
    }
    let mailbox = decode_word(addr.mailbox);
    let host = decode_word(addr.host);
    if mailbox.is_empty() {
        "unknown".into()
    } else {
        format!("{mailbox}@{host}")
    }
}

/// Headers arrive RFC2047-encoded (=?UTF-8?B?...?=) for anything non-ASCII.
fn decode_word(raw: Option<&[u8]>) -> String {
    let raw = match raw {
        Some(r) => r,
        None => return String::new(),
    };
    // Real-world senders (Steam) exceed the RFC's 75-char encoded-word limit;
    // decode anyway instead of failing.
    rfc2047_decoder::Decoder::new()
        .too_long_encoded_word_strategy(rfc2047_decoder::RecoverStrategy::Decode)
        .decode(raw)
        .unwrap_or_else(|_| String::from_utf8_lossy(raw).into_owned())
        .trim()
        .to_string()
}
