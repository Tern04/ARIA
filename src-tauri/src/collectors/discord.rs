use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const GATEWAY: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
// GUILDS | GUILD_MEMBERS | GUILD_VOICE_STATES | GUILD_PRESENCES. The latter
// two are privileged intents and must be toggled on the bot's page in the
// Developer Portal, otherwise the gateway closes with 4014.
const INTENTS: u64 = (1 << 0) | (1 << 1) | (1 << 7) | (1 << 8);
const SETUP_RETRY: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const POPULARITY_TICK: Duration = Duration::from_secs(60);
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Bumped by discord_setup so the collector re-attempts immediately even if
/// the user re-submits credentials identical to ones that already failed.
static SETUP_GEN: AtomicU64 = AtomicU64::new(0);

/// Kicks the live gateway session on setup/logout so it drops the old
/// credentials instead of staying connected with them. notify_one stores a
/// permit, so a notification can't be lost between select! iterations.
static RECONFIG: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[derive(Serialize, Deserialize, Clone)]
struct Friend {
    id: String,
    name: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct DiscordConfig {
    guild_id: String,
    friends: Vec<Friend>,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
enum DiscordState {
    Disconnected {
        reason: String,
        needs_setup: bool,
    },
    Connected {
        guild_id: String,
        voice: Vec<VoiceChannel>,
        friends: Vec<FriendStatus>,
    },
}

#[derive(Serialize, Clone, PartialEq)]
struct VoiceChannel {
    id: String,
    name: String,
    occupants: Vec<Occupant>,
    score: u64,
}

#[derive(Serialize, Clone, PartialEq)]
struct Occupant {
    name: String,
    mute: bool,
    deaf: bool,
    streaming: bool,
    /// Current game / ♪ song from the presence map (large sizes show it).
    activity: Option<String>,
}

#[derive(Serialize, Clone, PartialEq)]
struct FriendStatus {
    name: String,
    status: String, // online | idle | dnd | offline
    activity: Option<String>,
}

/// Live voice state of one user, from VOICE_STATE_UPDATE / GUILD_CREATE.
struct VoiceState {
    channel: String,
    mute: bool,
    deaf: bool,
    streaming: bool,
}

struct PresenceState {
    status: String,
    activity: Option<String>,
}

/// Live gateway view of the configured guild.
#[derive(Default)]
struct Guild {
    channels: BTreeMap<String, String>,     // voice channel id -> name
    voice: HashMap<String, VoiceState>,     // user id -> voice state
    presence: HashMap<String, PresenceState>, // user id -> status + activity
    names: HashMap<String, String>,         // user id -> display name
    scores: HashMap<String, u64>,           // channel id -> person-minutes, persisted
    loaded: bool,                           // GUILD_CREATE for our guild processed
}

pub fn spawn(app: AppHandle, mut ready: tokio::sync::watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.wait_for(|r| *r).await;
        let mut last: Option<DiscordState> = None;
        let mut backoff = BACKOFF_MIN;
        loop {
            let (token, cfg) = match credentials(&app) {
                Ok(v) => v,
                Err(reason) => {
                    let state = DiscordState::Disconnected {
                        reason,
                        needs_setup: true,
                    };
                    emit_changed(&app, &mut last, state);
                    tokio::time::sleep(SETUP_RETRY).await;
                    continue;
                }
            };

            // Drain any stale reconfigure permit (from a logout while no
            // session was running) so a fresh session isn't instantly killed.
            let _ = tokio::time::timeout(Duration::from_millis(1), RECONFIG.notified()).await;

            let started = std::time::Instant::now();
            let (reason, needs_setup) = session(&app, &token, &cfg, &mut last).await;
            if reason == "reconfiguring" {
                // discord_setup / discord_logout already emitted the right
                // state; jump straight to reloading credentials.
                last = None;
                continue;
            }
            emit_changed(
                &app,
                &mut last,
                DiscordState::Disconnected { reason, needs_setup },
            );

            if needs_setup {
                // Auth-shaped failure: re-identifying with the same
                // credentials cannot succeed, so wait until they change
                // (or the user re-submits the setup form).
                let failed = (token, cfg.guild_id);
                let gen0 = SETUP_GEN.load(Ordering::Relaxed);
                loop {
                    tokio::time::sleep(SETUP_RETRY).await;
                    if SETUP_GEN.load(Ordering::Relaxed) != gen0 {
                        // Explicit retry: drop dedup state so the outcome
                        // is re-emitted even if it is the same error.
                        last = None;
                        break;
                    }
                    let current = credentials(&app).ok().map(|(t, c)| (t, c.guild_id));
                    if current != Some(failed.clone()) {
                        break;
                    }
                }
                backoff = BACKOFF_MIN;
                continue;
            }

            // A session that held for a while means the credentials are fine
            // and this was a network blip — start the backoff over.
            if started.elapsed() > Duration::from_secs(60) {
                backoff = BACKOFF_MIN;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    });
}

fn emit_changed(app: &AppHandle, last: &mut Option<DiscordState>, state: DiscordState) {
    if last.as_ref() == Some(&state) {
        return;
    }
    if let Err(e) = app.emit("discord", &state) {
        eprintln!("discord emit failed: {e}");
    }
    *last = Some(state);
}

/// One gateway connection: identify, then translate dispatch events into
/// emitted state until the connection dies. Returns (reason, needs_setup).
async fn session(
    app: &AppHandle,
    token: &str,
    cfg: &DiscordConfig,
    last: &mut Option<DiscordState>,
) -> (String, bool) {
    let ws = match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(GATEWAY)).await {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => return (format!("gateway connect failed: {e}"), false),
        Err(_) => return ("gateway connect timed out".into(), false),
    };
    let (mut tx, mut rx) = ws.split();

    // Hello (op 10) carries the heartbeat interval; Identify goes out after.
    let hb = loop {
        match tokio::time::timeout(CONNECT_TIMEOUT, rx.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = match serde_json::from_str(&t) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v["op"].as_u64() == Some(10) {
                    match v["d"]["heartbeat_interval"].as_u64() {
                        Some(ms) => break Duration::from_millis(ms),
                        None => return ("hello without heartbeat_interval".into(), false),
                    }
                }
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return (format!("gateway read failed: {e}"), false),
            Ok(None) => return ("gateway closed before hello".into(), false),
            Err(_) => return ("no hello from gateway".into(), false),
        }
    };

    let identify = json!({
        "op": 2,
        "d": {
            "token": token,
            "intents": INTENTS,
            "properties": { "os": std::env::consts::OS, "browser": "aria", "device": "aria" },
        }
    });
    if let Err(e) = tx.send(Message::Text(identify.to_string().into())).await {
        return (format!("identify send failed: {e}"), false);
    }

    let mut guild = Guild {
        scores: load_scores(app),
        ..Guild::default()
    };
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + hb, hb);
    let mut sampler = tokio::time::interval_at(
        tokio::time::Instant::now() + POPULARITY_TICK,
        POPULARITY_TICK,
    );
    let mut last_seq: Option<u64> = None;
    let mut acked = true;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if !acked {
                    return ("heartbeat not acknowledged — reconnecting".into(), false);
                }
                acked = false;
                let beat = json!({ "op": 1, "d": last_seq });
                if let Err(e) = tx.send(Message::Text(beat.to_string().into())).await {
                    return (format!("heartbeat send failed: {e}"), false);
                }
            }
            _ = sampler.tick() => {
                if guild.loaded && sample_popularity(app, &mut guild) {
                    emit_changed(app, last, build_state(&guild, cfg));
                }
            }
            _ = RECONFIG.notified() => {
                return ("reconfiguring".into(), false);
            }
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let v: Value = match serde_json::from_str(&t) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    match v["op"].as_u64() {
                        // Dispatch: mutate guild state, emit if it changed.
                        Some(0) => {
                            if let Some(s) = v["s"].as_u64() {
                                last_seq = Some(s);
                            }
                            if let Err(e) = dispatch(&v, cfg, &mut guild) {
                                return e;
                            }
                            if guild.loaded {
                                emit_changed(app, last, build_state(&guild, cfg));
                            }
                        }
                        // Server-requested heartbeat: answer immediately.
                        Some(1) => {
                            let beat = json!({ "op": 1, "d": last_seq });
                            if let Err(e) = tx.send(Message::Text(beat.to_string().into())).await {
                                return (format!("heartbeat send failed: {e}"), false);
                            }
                        }
                        // Reconnect / Invalid Session: drop and re-identify.
                        // (RESUME skipped on purpose; traffic is tiny.)
                        Some(7) => return ("gateway requested reconnect".into(), false),
                        Some(9) => return ("session invalidated by gateway".into(), false),
                        Some(11) => acked = true,
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let (code, why) = frame
                        .map(|f| (u16::from(f.code), f.reason.to_string()))
                        .unwrap_or((0, String::new()));
                    return match code {
                        4004 => ("invalid bot token".into(), true),
                        4013 | 4014 => (
                            "privileged intents missing — enable Presence Intent and \
                             Server Members Intent on the bot's Developer Portal page"
                                .into(),
                            false,
                        ),
                        _ => (format!("gateway closed: {code} {why}").trim().into(), false),
                    };
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return (format!("gateway read failed: {e}"), false),
                None => return ("gateway stream ended".into(), false),
            }
        }
    }
}

fn dispatch(v: &Value, cfg: &DiscordConfig, g: &mut Guild) -> Result<(), (String, bool)> {
    let d = &v["d"];
    let gid = cfg.guild_id.as_str();
    match v["t"].as_str().unwrap_or("") {
        "READY" => {
            let member = d["guilds"]
                .as_array()
                .is_some_and(|gs| gs.iter().any(|x| x["id"] == gid));
            if !member {
                return Err((
                    "bot is not in that server — check the server ID and the invite".into(),
                    true,
                ));
            }
        }
        "GUILD_CREATE" if d["id"] == gid => {
            // Small guild: channels, voice states, members and (online)
            // presences all arrive inlined in this one event. Guilds past
            // the large threshold would need Request Guild Members (op 8).
            g.channels.clear();
            for c in d["channels"].as_array().into_iter().flatten() {
                if c["type"].as_u64() == Some(2) {
                    if let (Some(id), Some(name)) = (c["id"].as_str(), c["name"].as_str()) {
                        g.channels.insert(id.into(), name.into());
                    }
                }
            }
            g.voice.clear();
            for vs in d["voice_states"].as_array().into_iter().flatten() {
                apply_voice_state(g, vs);
            }
            for m in d["members"].as_array().into_iter().flatten() {
                if let (Some(u), Some(n)) = (m["user"]["id"].as_str(), member_name(m)) {
                    g.names.insert(u.into(), n);
                }
            }
            for p in d["presences"].as_array().into_iter().flatten() {
                apply_presence(g, p);
            }
            g.loaded = true;
        }
        "VOICE_STATE_UPDATE" if d["guild_id"] == gid => {
            apply_voice_state(g, d);
        }
        "PRESENCE_UPDATE" if d["guild_id"] == gid => {
            apply_presence(g, d);
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" if d["guild_id"] == gid => {
            if let Some(id) = d["id"].as_str() {
                // An UPDATE can change a channel's type, so re-check it.
                if d["type"].as_u64() == Some(2) {
                    if let Some(name) = d["name"].as_str() {
                        g.channels.insert(id.into(), name.into());
                    }
                } else {
                    g.channels.remove(id);
                }
            }
        }
        "CHANNEL_DELETE" if d["guild_id"] == gid => {
            if let Some(id) = d["id"].as_str() {
                g.channels.remove(id);
            }
        }
        "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" if d["guild_id"] == gid => {
            if let (Some(u), Some(n)) = (d["user"]["id"].as_str(), member_name(d)) {
                g.names.insert(u.into(), n);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Shared by GUILD_CREATE voice_states entries and VOICE_STATE_UPDATE — the
/// payload shape is the same. `mute`/`deaf` fold in server-side mutes.
fn apply_voice_state(g: &mut Guild, vs: &Value) {
    let Some(u) = vs["user_id"].as_str() else {
        return;
    };
    if let Some(n) = member_name(&vs["member"]) {
        g.names.insert(u.into(), n);
    }
    match vs["channel_id"].as_str() {
        Some(c) => {
            let b = |k: &str| vs[k].as_bool().unwrap_or(false);
            g.voice.insert(
                u.into(),
                VoiceState {
                    channel: c.into(),
                    mute: b("self_mute") || b("mute"),
                    deaf: b("self_deaf") || b("deaf"),
                    streaming: b("self_stream"),
                },
            );
        }
        None => {
            g.voice.remove(u);
        }
    }
}

fn apply_presence(g: &mut Guild, p: &Value) {
    if let (Some(u), Some(s)) = (p["user"]["id"].as_str(), p["status"].as_str()) {
        g.presence.insert(
            u.into(),
            PresenceState {
                status: s.into(),
                activity: activity_label(p),
            },
        );
    }
}

/// First real activity (type 4 = custom status, skipped). Spotify (type 2,
/// "listening") shows the track title instead of just "Spotify".
fn activity_label(p: &Value) -> Option<String> {
    let a = p["activities"]
        .as_array()?
        .iter()
        .find(|a| a["type"].as_u64() != Some(4))?;
    if a["type"].as_u64() == Some(2) {
        if let Some(song) = a["details"].as_str() {
            return Some(format!("♪ {song}"));
        }
    }
    a["name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

/// Display name preference: server nick, then global name, then username.
fn member_name(m: &Value) -> Option<String> {
    m["nick"]
        .as_str()
        .or_else(|| m["user"]["global_name"].as_str())
        .or_else(|| m["user"]["username"].as_str())
        .map(str::to_string)
}

fn build_state(g: &Guild, cfg: &DiscordConfig) -> DiscordState {
    let display = |id: &str| {
        cfg.friends
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.clone())
            .or_else(|| g.names.get(id).cloned())
            .unwrap_or_else(|| format!("user…{}", &id[id.len().saturating_sub(4)..]))
    };

    let mut voice: Vec<VoiceChannel> = g
        .channels
        .iter()
        .map(|(id, name)| {
            // Sorted so payload comparison is stable across HashMap orders.
            let mut occupants: Vec<Occupant> = g
                .voice
                .iter()
                .filter(|(_, v)| v.channel == *id)
                .map(|(u, v)| Occupant {
                    name: display(u),
                    mute: v.mute,
                    deaf: v.deaf,
                    streaming: v.streaming,
                    activity: g.presence.get(u).and_then(|p| p.activity.clone()),
                })
                .collect();
            occupants.sort_by(|a, b| a.name.cmp(&b.name));
            VoiceChannel {
                id: id.clone(),
                name: name.clone(),
                occupants,
                score: g.scores.get(id).copied().unwrap_or(0),
            }
        })
        .collect();
    voice.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.occupants.len().cmp(&a.occupants.len()))
            .then(a.name.cmp(&b.name))
    });

    let friends = cfg
        .friends
        .iter()
        .map(|f| {
            let (status, activity) = g
                .presence
                .get(&f.id)
                .map(|p| (p.status.clone(), p.activity.clone()))
                .unwrap_or_else(|| ("offline".into(), None));
            FriendStatus {
                name: f.name.clone(),
                status,
                activity,
            }
        })
        .collect();

    DiscordState::Connected {
        guild_id: cfg.guild_id.clone(),
        voice,
        friends,
    }
}

/// Adds one person-minute per current occupant to their channel's tally and
/// persists. Returns true when any tally changed. This sampling approach
/// loses at most one minute on a crash, unlike transition accounting.
fn sample_popularity(app: &AppHandle, g: &mut Guild) -> bool {
    let mut changed = false;
    for v in g.voice.values() {
        if g.channels.contains_key(&v.channel) {
            *g.scores.entry(v.channel.clone()).or_insert(0) += 1;
            changed = true;
        }
    }
    if changed {
        if let Err(e) = save_scores(app, &g.scores) {
            eprintln!("discord: popularity save failed: {e}");
        }
    }
    changed
}

fn scores_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("discord-popularity.json"))
}

fn load_scores(app: &AppHandle) -> HashMap<String, u64> {
    scores_path(app)
        .and_then(|p| std::fs::read_to_string(p).map_err(|e| e.to_string()))
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        .unwrap_or_default()
}

fn save_scores(app: &AppHandle, scores: &HashMap<String, u64>) -> Result<(), String> {
    let path = scores_path(app)?;
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string(scores).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("discord.json"))
}

/// Reads discord.json from the app config dir; writes a template on first run
/// (mirrors mail.json). guild_id is filled by the widget's setup form; the
/// friends list ([{id, name}]) is hand-edited — names are what the widget
/// shows, ids come from right-click → Copy User ID in Discord.
fn load_config(app: &AppHandle) -> Result<DiscordConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        let template = DiscordConfig {
            guild_id: String::new(),
            friends: vec![Friend {
                id: String::new(),
                name: String::new(),
            }],
        };
        let json = serde_json::to_string_pretty(&template).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut cfg: DiscordConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    cfg.friends.retain(|f| !f.id.is_empty());
    if cfg.guild_id.is_empty() {
        return Err("enter bot token + server ID".into());
    }
    Ok(cfg)
}

fn credentials(app: &AppHandle) -> Result<(String, DiscordConfig), String> {
    let cfg = load_config(app)?;
    let token = super::keychain_secret("discord")
        .map_err(|_| "enter bot token + server ID".to_string())?;
    Ok((token, cfg))
}

/// Invoked by the widget's setup form: store the token, set the guild id
/// (keeping hand-added friends), and nudge the collector to retry now.
/// An empty token keeps the stored one, so switching servers is one field.
#[tauri::command]
pub async fn discord_setup(app: AppHandle, token: String, guild_id: String) -> Result<(), String> {
    let token = token.trim().to_string();
    let guild_id = guild_id.trim().to_string();
    if guild_id.is_empty() {
        return Err("server ID is required".into());
    }
    if !guild_id.chars().all(|c| c.is_ascii_digit()) {
        return Err("server ID must be numeric (right-click server → Copy Server ID)".into());
    }
    if token.is_empty() {
        super::keychain_secret("discord")
            .map_err(|_| "no stored token — paste the bot token".to_string())?;
    } else {
        super::store_secret("discord", &token)?;
    }

    let path = config_path(&app)?;
    let mut cfg: DiscordConfig = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    cfg.guild_id = guild_id;
    if cfg.friends.is_empty() {
        cfg.friends.push(Friend {
            id: String::new(),
            name: String::new(),
        });
    }
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;

    SETUP_GEN.fetch_add(1, Ordering::Relaxed);
    RECONFIG.notify_one();
    let state = DiscordState::Disconnected {
        reason: "connecting…".into(),
        needs_setup: false,
    };
    if let Err(e) = app.emit("discord", &state) {
        eprintln!("discord emit failed: {e}");
    }
    Ok(())
}

/// Log out: forget the bot token and drop the live gateway session.
/// Best-effort delete: an already-missing secret must not block logging out.
#[tauri::command]
pub fn discord_logout(app: AppHandle) -> Result<(), String> {
    if let Err(e) = super::delete_secret("discord") {
        eprintln!("discord: secret delete: {e}");
    }
    SETUP_GEN.fetch_add(1, Ordering::Relaxed);
    RECONFIG.notify_one();
    let state = DiscordState::Disconnected {
        reason: "logged out".into(),
        needs_setup: true,
    };
    if let Err(e) = app.emit("discord", &state) {
        eprintln!("discord emit failed: {e}");
    }
    Ok(())
}
