use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const POLL: Duration = Duration::from_secs(300);
const RECENT_PER_ACCOUNT: u32 = 5;
const RECENT_SHOWN: usize = 5;

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum EmailState {
    Disconnected { reason: String },
    Connected { accounts: Vec<AccountSummary>, messages: Vec<MailMessage> },
}

#[derive(Serialize, Clone)]
struct AccountSummary {
    label: String,
    unread: Option<u32>, // None when this account errored this poll
}

#[derive(Serialize, Clone)]
struct MailMessage {
    account: String,
    from: String,
    subject: String,
    unseen: bool,
    #[serde(skip)]
    timestamp: i64,
}

#[derive(Deserialize, Serialize, Clone)]
struct ImapAccount {
    label: String,
    host: String,
    user: String,
}

#[derive(Deserialize, Serialize)]
struct MailConfig {
    accounts: Vec<ImapAccount>,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            accounts: vec![ImapAccount {
                label: "MAIL".into(),
                host: "imap.example.com".into(),
                user: String::new(),
            }],
        }
    }
}

pub fn spawn(app: AppHandle, mut ready: tokio::sync::watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.wait_for(|r| *r).await;
        loop {
            let state = match load_config(&app) {
                Ok(cfg) => {
                    tauri::async_runtime::spawn_blocking(move || fetch_all(&cfg))
                        .await
                        .unwrap_or_else(|e| EmailState::Disconnected { reason: e.to_string() })
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

fn fetch_all(cfg: &MailConfig) -> EmailState {
    let mut accounts = Vec::new();
    let mut messages: Vec<MailMessage> = Vec::new();
    for account in &cfg.accounts {
        match fetch_inbox(account) {
            Ok((unread, mut msgs)) => {
                accounts.push(AccountSummary {
                    label: account.label.clone(),
                    unread: Some(unread),
                });
                messages.append(&mut msgs);
            }
            Err(e) => {
                eprintln!("mail {}: {e}", account.label);
                accounts.push(AccountSummary {
                    label: account.label.clone(),
                    unread: None,
                });
            }
        }
    }
    messages.sort_by_key(|m| std::cmp::Reverse(m.timestamp));
    messages.truncate(RECENT_SHOWN);
    EmailState::Connected { accounts, messages }
}

/// Reads mail.json from the app config dir; writes a template on first run
/// so the user has a file to fill in.
fn load_config(app: &AppHandle) -> Result<MailConfig, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let path = dir.join("mail.json");

    if !path.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let template =
            serde_json::to_string_pretty(&MailConfig::default()).map_err(|e| e.to_string())?;
        std::fs::write(&path, template).map_err(|e| e.to_string())?;
        return Err("mail.json created — fill in accounts".into());
    }

    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let cfg: MailConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if cfg.accounts.iter().all(|a| a.user.is_empty()) {
        return Err("mail.json has no accounts configured".into());
    }
    Ok(cfg)
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
