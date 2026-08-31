use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Poll cadence. A playing track drives a 1 s progress readout, so sampling it
/// every 10 s made every correction a visible jump; 3 s keeps the client-side
/// extrapolation short enough to be invisible. Nothing is moving otherwise, so
/// an idle/paused player is polled lazily.
const POLL_PLAYING: Duration = Duration::from_secs(3);
const POLL_IDLE: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Default)]
struct NowPlaying {
    title: String,
    artist: String,
    album: String,
    /// Friendly source-app name ("Spotify", "Chrome"…), when identifiable.
    app_name: Option<String>,
    /// Fractional seconds — truncating to whole seconds biased the readout up
    /// to 1 s low on every sample.
    position_secs: f64,
    duration_secs: u64,
    /// False when the player exposes no usable position — either the query
    /// fails outright, or it "succeeds" while never advancing (some browser
    /// media sessions just answer 0 forever). The widget hides the progress
    /// row rather than free-running a counter that means nothing.
    position_known: bool,
    /// Album art as a data: URL (Windows SMTC thumbnail; None elsewhere).
    art: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum MusicState {
    Stopped,
    Playing(NowPlaying),
    Paused(NowPlaying),
}

/// Transport actions the widget buttons can request. Re-samples and emits the
/// new state right after so the widget updates instantly instead of waiting
/// for the next poll.
#[tauri::command]
pub async fn music_control(app: AppHandle, action: String) -> Result<(), String> {
    let state = tauri::async_runtime::spawn_blocking(move || {
        control(&action)?;
        // Skipping loads a new track; give Music a moment before sampling.
        if action == "next" || action == "previous" {
            std::thread::sleep(Duration::from_millis(500));
        }
        Ok::<MusicState, String>(sample_now())
    })
    .await
    .map_err(|e| e.to_string())??;
    let _ = app.emit("music", state);
    Ok(())
}

/// Names of the user's real playlists (macOS only; empty elsewhere).
#[tauri::command]
pub async fn music_playlists() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(playlists)
        .await
        .map_err(|e| e.to_string())?
}

/// Shuffle-play a playlist by name, then emit the new now-playing state.
#[tauri::command]
pub async fn music_play_playlist(app: AppHandle, name: String) -> Result<(), String> {
    let state = tauri::async_runtime::spawn_blocking(move || {
        play_playlist(&name)?;
        // Music needs a moment to load the first track before the new title
        // and player state are readable.
        std::thread::sleep(Duration::from_millis(500));
        Ok::<MusicState, String>(sample_now())
    })
    .await
    .map_err(|e| e.to_string())??;
    let _ = app.emit("music", state);
    Ok(())
}

fn sample_now() -> MusicState {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        sample_music()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        MusicState::Stopped
    }
}

#[cfg(target_os = "macos")]
fn control(action: &str) -> Result<(), String> {
    if !music_is_running() {
        return Ok(()); // nothing to control; never launch Music ourselves
    }
    let command = match action {
        "playpause" => "playpause",
        "next" => "next track",
        "previous" => "previous track",
        other => return Err(format!("unknown music action: {other}")),
    };
    let script = format!("tell application \"Music\" to {command}");
    let out = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "macos")]
fn playlists() -> Result<Vec<String>, String> {
    if !music_is_running() {
        return Ok(Vec::new()); // don't launch Music just to enumerate
    }
    // special kind "none" excludes the library/Downloaded/etc. special
    // playlists; a `whose` clause with a bare `none` keyword errors, so filter
    // in the loop. Newline-delimited so names with commas stay intact.
    const SCRIPT: &str = r#"tell application "Music"
    set out to ""
    repeat with p in user playlists
        if (special kind of p as text) is "none" then set out to out & (name of p) & linefeed
    end repeat
    return out
end tell"#;
    let out = run_osascript(SCRIPT)?;
    Ok(out.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}

#[cfg(target_os = "macos")]
fn play_playlist(name: &str) -> Result<(), String> {
    // Strip quotes/backslashes so the name can't break out of the AppleScript
    // string literal.
    let safe: String = name.chars().filter(|c| *c != '"' && *c != '\\').collect();
    let script = format!(
        "tell application \"Music\"\n    set shuffle enabled to true\n    play playlist \"{safe}\"\nend tell"
    );
    run_osascript(&script).map(|_| ())
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Result<String, String> {
    let out = std::process::Command::new("osascript")
        .args(["-e", script])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Windows: whatever app owns the system media session (Spotify, a browser,
/// Apple Music for Windows…) — the same source the media keys control.
#[cfg(target_os = "windows")]
fn control(action: &str) -> Result<(), String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;

    let run = || -> windows::core::Result<()> {
        let mgr = Manager::RequestAsync()?.join()?;
        let Ok(session) = mgr.GetCurrentSession() else {
            return Ok(()); // nothing playing anywhere; nothing to control
        };
        match action {
            "playpause" => session.TryTogglePlayPauseAsync()?.join()?,
            "next" => session.TrySkipNextAsync()?.join()?,
            "previous" => session.TrySkipPreviousAsync()?.join()?,
            _ => unreachable!(),
        };
        Ok(())
    };
    match action {
        "playpause" | "next" | "previous" => run().map_err(|e| e.to_string()),
        other => Err(format!("unknown music action: {other}")),
    }
}

#[cfg(target_os = "windows")]
fn sample_music() -> MusicState {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };

    let sample = || -> windows::core::Result<MusicState> {
        let mgr = Manager::RequestAsync()?.join()?;
        let Ok(session) = mgr.GetCurrentSession() else {
            return Ok(MusicState::Stopped);
        };
        let status = session.GetPlaybackInfo()?.PlaybackStatus()?;
        let props = session.TryGetMediaPropertiesAsync()?.join()?;
        let title = props.Title()?.to_string();
        let artist = props.Artist()?.to_string();
        if title.is_empty() {
            return Ok(MusicState::Stopped);
        }
        // Sources update the timeline sporadically (browsers especially), so
        // Position is stale by up to seconds; extrapolate from its own
        // LastUpdatedTime while playing, or the widget's bar jumps backward.
        let (position_secs, duration_secs) = session
            .GetTimelineProperties()
            .map(|t| {
                // Apple Music reports Position/EndTime against a running
                // *session* timeline, not the current track: StartTime is the
                // cumulative offset where this track begins. Spotify, Opera and
                // the browsers leave StartTime at 0, so they worked; Apple Music
                // did not — ignoring StartTime showed the running session total
                // (67:24 / 71:02) that never reset between tracks. Rebase both
                // values onto StartTime so the bar is per-track.
                let start = t.StartTime().map(|s| s.Duration).unwrap_or(0);
                let mut pos = t.Position().map(|p| p.Duration).unwrap_or(0) - start;
                let end = t.EndTime().map(|e| e.Duration).unwrap_or(0) - start;
                if status == Status::Playing {
                    if let (Ok(updated), Ok(since_unix)) = (
                        t.LastUpdatedTime(),
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH),
                    ) {
                        // UniversalTime: 100 ns ticks since 1601-01-01 UTC.
                        const UNIX_EPOCH_1601: i64 = 116_444_736_000_000_000;
                        let now = UNIX_EPOCH_1601 + (since_unix.as_nanos() / 100) as i64;
                        // Extrapolation only bridges the ~10 s poll gap. Some
                        // sources set LastUpdatedTime once at play and never
                        // refresh Position, so cap the drift just past one poll
                        // interval instead of letting it stack.
                        const MAX_DRIFT: i64 = 15 * 10_000_000; // 15 s in ticks
                        pos += (now - updated.UniversalTime).clamp(0, MAX_DRIFT);
                    }
                }
                if end > 0 {
                    pos = pos.min(end);
                }
                (
                    pos.max(0) as f64 / 10_000_000.0,
                    (end / 10_000_000).max(0) as u64,
                )
            })
            .unwrap_or((0.0, 0));
        let now = NowPlaying {
            album: props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default(),
            app_name: session
                .SourceAppUserModelId()
                .ok()
                .and_then(|id| friendly_app_name(&id.to_string())),
            position_secs,
            duration_secs,
            // SMTC always carries a timeline; a zero length means the source
            // published none, which is the same thing as "no position".
            position_known: duration_secs > 0,
            art: thumbnail_data_url(&props, &title, &artist),
            title,
            artist,
        };
        Ok(match status {
            Status::Playing => MusicState::Playing(now),
            Status::Paused => MusicState::Paused(now),
            _ => MusicState::Stopped,
        })
    };
    sample().unwrap_or_else(|e| {
        eprintln!("music smtc: {e}");
        MusicState::Stopped
    })
}

/// SMTC source ids are AUMIDs ("Spotify.exe", "MSEdge",
/// "AppleInc.AppleMusicWin_…!App"); reduce the common ones to a label.
#[cfg(target_os = "windows")]
fn friendly_app_name(aumid: &str) -> Option<String> {
    let id = aumid.to_lowercase();
    let name = if id.contains("spotify") {
        "Spotify"
    } else if id.contains("applemusic") {
        "Apple Music"
    } else if id.contains("msedge") {
        "Edge"
    } else if id.contains("chrome") {
        "Chrome"
    } else if id.contains("firefox") {
        "Firefox"
    } else if id.contains("opera") {
        "Opera"
    } else if id.contains("vlc") {
        "VLC"
    } else if id.contains("zune") || id.contains("media") {
        "Media Player"
    } else {
        return None;
    };
    Some(name.to_string())
}

/// Read the SMTC thumbnail into a data: URL. Encoded once per track — the
/// poll runs every 10 s and the art is by far the heaviest field.
#[cfg(target_os = "windows")]
fn thumbnail_data_url(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
    title: &str,
    artist: &str,
) -> Option<String> {
    use std::sync::Mutex;
    use windows::Storage::Streams::DataReader;

    static CACHE: Mutex<Option<((String, String), Option<String>)>> = Mutex::new(None);
    let key = (title.to_string(), artist.to_string());
    if let Ok(cache) = CACHE.lock() {
        if let Some((k, art)) = cache.as_ref() {
            if *k == key {
                return art.clone();
            }
        }
    }

    let read = || -> windows::core::Result<Option<String>> {
        let stream = props.Thumbnail()?.OpenReadAsync()?.join()?;
        let size = stream.Size()?;
        if size == 0 || size > 1_500_000 {
            return Ok(None); // absent or unreasonably large
        }
        let reader = DataReader::CreateDataReader(&stream)?;
        reader.LoadAsync(size as u32)?.join()?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes)?;
        let mime = stream
            .ContentType()
            .map(|c| c.to_string())
            .unwrap_or_else(|_| "image/jpeg".into());
        Ok(Some(format!("data:{mime};base64,{}", base64(&bytes))))
    };
    let art = read().unwrap_or(None);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((key, art.clone()));
    }
    art
}

/// Plain base64 (RFC 4648) — small enough not to warrant a dependency.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ABC[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ABC[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Linux (MPRIS): drive whichever player playerctl selects (the same source
/// the media keys control). `play-pause` toggles; skips move track.
#[cfg(target_os = "linux")]
fn control(action: &str) -> Result<(), String> {
    let verb = match action {
        "playpause" => "play-pause",
        "next" => "next",
        "previous" => "previous",
        other => return Err(format!("unknown music action: {other}")),
    };
    // No player running is not an error — nothing to control, like macOS.
    let _ = run_playerctl(&[verb]);
    Ok(())
}

/// Other Unix (no MPRIS tooling assumed) — inert, like the old stub.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn control(_action: &str) -> Result<(), String> {
    Ok(())
}

/// Playlists are an Apple Music concept; SMTC/MPRIS have no equivalent, so
/// the widget simply renders no playlist chips elsewhere.
#[cfg(not(target_os = "macos"))]
fn playlists() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[cfg(not(target_os = "macos"))]
fn play_playlist(_name: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let state = tauri::async_runtime::spawn_blocking(sample_music)
                .await
                .unwrap_or(MusicState::Stopped);
            let next = match state {
                MusicState::Playing(_) => POLL_PLAYING,
                _ => POLL_IDLE,
            };
            if let Err(e) = app.emit("music", state) {
                eprintln!("music emit failed: {e}");
            }
            tokio::time::sleep(next).await;
        }
    });
}

/// Other Unix without MPRIS tooling — no now-playing source.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn spawn(_app: AppHandle) {}

/// Linux now-playing over MPRIS via `playerctl` — the same source the media
/// keys drive, so it covers Spotify, VLC and any browser tab (Firefox, Opera…)
/// exposing a media session. When `playerctl` is missing or no player is
/// running the commands fail and we report Stopped, exactly like the old stub.
/// One `playerctl metadata` line, split out so it can be unit-tested.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq)]
struct Meta {
    status: String,
    player: String,
    title: String,
    artist: String,
    album: String,
    length_us: u64,
    art_url: String,
}

/// Parse the unit-separator-delimited sample line. Returns None when the line
/// describes nothing playable, so the caller can report Stopped.
#[cfg(target_os = "linux")]
fn parse_metadata_line(line: &str) -> Option<Meta> {
    let mut f = line.split('\x1f');
    let status = f.next().unwrap_or("").trim().to_string();
    let player = f.next().unwrap_or("").trim().to_string();
    let title = f.next().unwrap_or("").trim().to_string();
    let artist = f.next().unwrap_or("").trim().to_string();
    let album = f.next().unwrap_or("").trim().to_string();
    let length_us: u64 = f.next().unwrap_or("").trim().parse().unwrap_or(0);
    let art_url = f.next().unwrap_or("").trim().to_string();
    if title.is_empty() || !matches!(status.as_str(), "Playing" | "Paused") {
        return None;
    }
    Some(Meta { status, player, title, artist, album, length_us, art_url })
}

/// Pick which MPRIS player to report on.
///
/// `playerctl` with no `--player` answers from whichever bus it finds first,
/// and prefers `playerctld` when that daemon is running — a proxy that keeps
/// answering for a player which has since gone away, so the widget can end up
/// showing a track from hours ago. Choose deliberately instead: something
/// actually Playing beats something merely Paused, and the proxy is skipped so
/// we always talk to a real player.
#[cfg(target_os = "linux")]
fn pick_player() -> Option<String> {
    let list = run_playerctl(&["-l"]).ok()?;
    let mut paused = None;
    for name in list.lines().map(str::trim).filter(|n| !n.is_empty()) {
        if name == "playerctld" {
            continue; // the proxy, not a player
        }
        match run_playerctl(&["--player", name, "status"]).as_deref() {
            Ok("Playing") => return Some(name.to_string()),
            Ok("Paused") if paused.is_none() => paused = Some(name.to_string()),
            _ => {}
        }
    }
    paused
}

#[cfg(target_os = "linux")]
fn sample_music() -> MusicState {
    // One unit-separator-delimited line keeps every field from one consistent
    // sample; \x1f never appears in titles. Position isn't metadata, so it is
    // fetched separately below.
    const FMT: &str = "{{status}}\x1f{{playerName}}\x1f{{title}}\x1f{{artist}}\x1f{{album}}\x1f{{mpris:length}}\x1f{{mpris:artUrl}}";
    let Some(player) = pick_player() else {
        return MusicState::Stopped; // no player / playerctl absent
    };
    let Ok(line) = run_playerctl(&["--player", &player, "metadata", "--format", FMT]) else {
        return MusicState::Stopped;
    };
    let Some(m) = parse_metadata_line(&line) else {
        return MusicState::Stopped;
    };

    // Resolve the cover *before* reading the position. Art can involve disk or
    // network I/O, and anything between the position read and the emit shows up
    // as the widget's clock starting behind. `art_for` never blocks — a cache
    // miss is fetched on a background thread and picked up by the next poll.
    let art = art_for(&m.art_url, &m.title, &m.artist);

    // `playerctl position` prints float seconds, read from the same player the
    // metadata came from so several MPRIS buses (a browser tab plus Spotify)
    // can't cross-wire the title and the clock.
    let duration_secs = m.length_us / 1_000_000;
    let sampled = run_playerctl(&["--player", &player, "position"])
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite());
    // Both of these carry state across samples, so they must run every time,
    // not only when their answer gets used.
    let live = sampled.is_some_and(|p| position_is_live(&m.title, &m.artist, &m.status, p));
    let derived = sampled.and_then(|p| track_elapsed(&player, &m.title, &m.artist, p));

    let (mut position_secs, position_known) = match sampled {
        // The player has no Position property at all.
        None => (0.0, false),
        // A player that reports a real track-relative position: trust it.
        Some(p) if duration_secs > 0 && position_fits_track(p, duration_secs) => {
            (p.max(0.0), live)
        }
        // Otherwise the position can't belong to this track (or there's no
        // length to check it against), so recover the elapsed time from the
        // session clock — see track_elapsed.
        Some(_) => match derived {
            Some(elapsed) => (elapsed.max(0.0), live),
            None => (0.0, false),
        },
    };
    // Only once the value is known to belong to this track: clamp so the bar
    // can't overshoot when the two calls straddle a tick.
    if position_known && duration_secs > 0 {
        position_secs = position_secs.min(duration_secs as f64);
    }

    let now = NowPlaying {
        app_name: friendly_player_name(&m.player),
        position_secs,
        duration_secs,
        position_known,
        // The CSP only allows `data:` images, so everything is inlined:
        // Chromium browsers publish a local file:// cover, Spotify an https one.
        art,
        title: m.title,
        artist: m.artist,
        album: m.album,
    };
    if m.status == "Playing" {
        MusicState::Playing(now)
    } else {
        MusicState::Paused(now)
    }
}

/// Recovers a track-relative elapsed time from a session-cumulative clock.
///
/// Players like Firefox report a position that counts the whole listening
/// session rather than the current track, and publish no track length at all.
/// The clock itself is sound though — measured against Apple Music web it
/// advances at exactly 1x and runs straight through a track change without a
/// blip (…19618, 19620, [title changes], 19622, 19624…). So the elapsed time
/// within a track is simply the distance from where that clock stood when the
/// title last changed.
///
/// The catch is that it only works from a change we actually witnessed:
/// joining mid-track says nothing about how far in we already are, so that
/// reports unknown rather than a confident and wrong 0:00. The *length* of a
/// track is genuinely not recoverable this way — it can only be known once the
/// track has ended, which is too late to draw a bar with.
#[cfg(target_os = "linux")]
struct SessionClock {
    player: String,
    track: (String, String),
    /// Where the session clock stood when this track started.
    anchor: f64,
    /// False until a track change is observed, i.e. until `anchor` is real.
    anchored: bool,
}

#[cfg(target_os = "linux")]
fn track_elapsed(player: &str, title: &str, artist: &str, position: f64) -> Option<f64> {
    static CLOCK: std::sync::Mutex<Option<SessionClock>> = std::sync::Mutex::new(None);
    let mut clock = CLOCK.lock().ok()?;
    let track = (title.to_string(), artist.to_string());

    if let Some(c) = clock.as_mut() {
        if c.player == player && c.track == track {
            // A backwards jump means the session clock itself restarted (the
            // player was relaunched); re-anchor and admit we don't know.
            if position < c.anchor {
                c.anchor = position;
                c.anchored = false;
            }
            return c.anchored.then_some(position - c.anchor);
        }
    }

    // A different track. If we were already following this player then the
    // change we just saw *is* the anchor, and elapsed starts from zero.
    let anchored = clock.as_ref().is_some_and(|c| c.player == player);
    *clock = Some(SessionClock {
        player: player.to_string(),
        track,
        anchor: position,
        anchored,
    });
    anchored.then_some(0.0)
}

/// Slack for the gap between reading the metadata and reading the position.
#[cfg(target_os = "linux")]
const POSITION_OVERSHOOT_TOLERANCE: f64 = 2.0;

/// Whether a reported position can plausibly belong to the current track.
///
/// Browser media sessions are the problem case: Firefox playing Apple Music
/// reports a `Position` that counts the whole *listening session* rather than
/// the track (observed at 18 870 s — five hours — advancing at 1× and never
/// resetting between tracks), and publishes no `mpris:length` at all. Clamping
/// such a value into the last known duration is worse than useless: it turns
/// nonsense into a believable readout, which is how the widget ended up
/// showing the running time of a film watched hours earlier.
///
/// With no duration there is nothing to contradict, so the value is taken at
/// face value — the progress row is hidden anyway.
#[cfg(target_os = "linux")]
fn position_fits_track(position: f64, duration_secs: u64) -> bool {
    duration_secs == 0 || position <= duration_secs as f64 + POSITION_OVERSHOOT_TOLERANCE
}

/// A playing track whose position isn't keeping up with the wall clock this
/// many samples running isn't really reporting a position. At the 3 s playing
/// poll that is ~9 s of "progress" that went nowhere, which no working player
/// does — but a stuck media session does it forever.
#[cfg(target_os = "linux")]
const STALE_POSITION_SAMPLES: u8 = 3;

/// Samples closer together than this can't be judged — the difference is in
/// the noise of two `playerctl` process spawns.
#[cfg(target_os = "linux")]
const MIN_JUDGEABLE_GAP: f64 = 0.5;

/// A playing track must advance at roughly wall-clock rate. A generous
/// fraction of it absorbs slow polls and playback rates below 1×, while still
/// catching a position that is standing still (or going backwards).
///
/// Note this can't be an equality check: `playerctl position` interpolates
/// from when it read the property, so even a player frozen at zero answers
/// 0.000004, 0.000006, … — never the same number twice.
#[cfg(target_os = "linux")]
const MIN_ADVANCE_RATIO: f64 = 0.25;

/// Pure half of `position_is_live`: the new consecutive-stall count.
#[cfg(target_os = "linux")]
fn stall_count(prev_pos: f64, pos: f64, elapsed: f64, prev_stalls: u8) -> u8 {
    if pos - prev_pos < elapsed * MIN_ADVANCE_RATIO {
        prev_stalls.saturating_add(1)
    } else {
        0
    }
}

#[cfg(target_os = "linux")]
struct PositionSample {
    title: String,
    artist: String,
    pos: f64,
    at: std::time::Instant,
    stalls: u8,
}

/// Whether the position we just read is actually moving. Paused tracks are
/// exempt (holding still is the whole point), as is any track change.
#[cfg(target_os = "linux")]
fn position_is_live(title: &str, artist: &str, status: &str, pos: f64) -> bool {
    static LAST: std::sync::Mutex<Option<PositionSample>> = std::sync::Mutex::new(None);
    let Ok(mut last) = LAST.lock() else {
        return true; // never hide the bar over a lock we couldn't take
    };
    if status != "Playing" {
        *last = None; // resuming starts a fresh judgement
        return true;
    }
    let now = std::time::Instant::now();
    let stalls = match last.as_ref() {
        Some(p) if p.title == title && p.artist == artist => {
            let elapsed = now.duration_since(p.at).as_secs_f64();
            if elapsed < MIN_JUDGEABLE_GAP {
                return p.stalls < STALE_POSITION_SAMPLES; // keep the old baseline
            }
            stall_count(p.pos, pos, elapsed, p.stalls)
        }
        _ => 0,
    };
    *last = Some(PositionSample {
        title: title.to_string(),
        artist: artist.to_string(),
        pos,
        at: now,
        stalls,
    });
    stalls < STALE_POSITION_SAMPLES
}

/// MPRIS player-bus names ("firefox", "spotify", "chromium") → a display label.
#[cfg(target_os = "linux")]
fn friendly_player_name(player: &str) -> Option<String> {
    let id = player.to_lowercase();
    let name = if id.contains("spotify") {
        "Spotify"
    } else if id.contains("firefox") {
        "Firefox"
    } else if id.contains("opera") {
        "Opera"
    } else if id.contains("chromium") {
        "Chromium"
    } else if id.contains("chrome") {
        "Chrome"
    } else if id.contains("vlc") {
        "VLC"
    } else if id.contains("mpv") {
        "mpv"
    } else if id.is_empty() {
        return None;
    } else {
        // Unknown player — Title-case the bus name so it still reads sensibly.
        let mut c = player.chars();
        return c
            .next()
            .map(|first| first.to_uppercase().collect::<String>() + c.as_str());
    };
    Some(name.to_string())
}

/// Cover art as an inlined `data:` URL (the CSP forbids remote images). Handles
/// the three shapes MPRIS players publish: an already-inlined `data:` URI, a
/// local `file://` path (Chromium caches the cover to disk), or a remote
/// `http(s)` URL (Spotify, some sites).
///
/// Never blocks: a cache hit returns immediately, a miss starts a background
/// fetch and returns None so the caller can go on to sample the position and
/// emit. The following poll (≤3 s) picks up the cached result. Art used to be
/// fetched inline, which parked the poll for up to 5 s *between* reading the
/// position and emitting it — the widget's clock then started that far behind.
#[cfg(target_os = "linux")]
fn art_for(art_url: &str, title: &str, artist: &str) -> Option<String> {
    if art_url.is_empty() {
        return None;
    }
    if art_url.starts_with("data:") {
        return Some(art_url.to_string()); // already inlined
    }
    let key = (title.to_string(), artist.to_string());
    if let Some(hit) = art_cache_get(&key) {
        return hit;
    }
    spawn_art_fetch(art_url.to_string(), key);
    None
}

/// Bounded per-track cache. A single entry made alternating tracks (a queue
/// bouncing between two, or a paused player re-sampled next to a playing one)
/// re-download the cover on every poll.
#[cfg(target_os = "linux")]
const ART_CACHE_MAX: usize = 8;

#[cfg(target_os = "linux")]
type ArtKey = (String, String);

#[cfg(target_os = "linux")]
static ART_CACHE: std::sync::Mutex<Vec<(ArtKey, Option<String>)>> =
    std::sync::Mutex::new(Vec::new());

/// `Some(entry)` when the track has been resolved (the inner Option is None if
/// it genuinely has no usable cover); `None` when it has never been fetched.
#[cfg(target_os = "linux")]
fn art_cache_get(key: &ArtKey) -> Option<Option<String>> {
    let cache = ART_CACHE.lock().ok()?;
    cache.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[cfg(target_os = "linux")]
fn art_cache_put(key: ArtKey, art: Option<String>) {
    if let Ok(mut cache) = ART_CACHE.lock() {
        cache.retain(|(k, _)| *k != key);
        cache.push((key, art));
        let overflow = cache.len().saturating_sub(ART_CACHE_MAX);
        cache.drain(..overflow);
    }
}

/// Fetch a cover off the poll thread, de-duplicated so a slow host can't stack
/// one request per poll for the same track.
#[cfg(target_os = "linux")]
fn spawn_art_fetch(art_url: String, key: ArtKey) {
    static IN_FLIGHT: std::sync::Mutex<Vec<ArtKey>> = std::sync::Mutex::new(Vec::new());
    {
        let Ok(mut flight) = IN_FLIGHT.lock() else { return };
        if flight.contains(&key) {
            return;
        }
        flight.push(key.clone());
    }
    std::thread::spawn(move || {
        let art = match fetch_art_bytes(&art_url) {
            Ok((mime, bytes)) => Some(format!("data:{mime};base64,{}", base64(&bytes))),
            Err(e) => {
                // Cached as None either way, so this can't retry-loop; but a
                // silently missing cover is impossible to diagnose otherwise.
                eprintln!("music art: {art_url}: {e}");
                None
            }
        };
        art_cache_put(key.clone(), art);
        if let Ok(mut flight) = IN_FLIGHT.lock() {
            flight.retain(|k| *k != key);
        }
    });
}

/// Load the raw cover bytes + mime for a `file://` or `http(s)` art URL.
/// The error is returned rather than swallowed so a cover that never appears
/// can be explained (see `spawn_art_fetch`).
#[cfg(target_os = "linux")]
fn fetch_art_bytes(art_url: &str) -> Result<(String, Vec<u8>), String> {
    const MAX: usize = 3_000_000; // guard against an unreasonably large cover
    let bytes: Vec<u8> = if let Some(path) = art_url.strip_prefix("file://") {
        let decoded = percent_decode(path);
        let data = std::fs::read(&decoded).map_err(|e| e.to_string())?;
        if data.is_empty() || data.len() > MAX {
            return Err(format!("{} bytes is not a usable cover", data.len()));
        }
        data
    } else if art_url.starts_with("http") {
        // This runs on its own thread, so the async client is driven to
        // completion with a private single-threaded runtime rather than
        // borrowing Tauri's (block_on from a non-runtime thread is not safe to
        // rely on, and a cover fetch has no business occupying a worker).
        let url = art_url.to_string();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async move {
            // Bounded so a slow/hanging cover host can't wedge the fetch.
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
            // Without this an error page is happily base64'd and handed to the
            // <img> as a bogus JPEG.
            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }
            let data = resp.bytes().await.map_err(|e| e.to_string())?;
            if data.is_empty() || data.len() > MAX {
                return Err(format!("{} bytes is not a usable cover", data.len()));
            }
            Ok(data.to_vec())
        })?
    } else {
        return Err("unsupported art URL scheme".into());
    };
    Ok((sniff_image_mime(&bytes).to_string(), bytes))
}

/// Mime from magic bytes; MPRIS art URLs carry no reliable content-type.
#[cfg(target_os = "linux")]
fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/jpeg" // JPEG or unknown — browsers sniff anyway
    }
}

/// Decode %XX escapes in a file:// path. MPRIS paths are otherwise plain UTF-8.
#[cfg(target_os = "linux")]
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run `playerctl` with the given args, returning trimmed stdout on success.
/// A non-zero exit (no player, playerctl not installed) becomes an error the
/// callers treat as "nothing playing".
#[cfg(target_os = "linux")]
fn run_playerctl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("playerctl")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "macos")]
fn sample_music() -> MusicState {
    // Telling a non-running app via AppleScript would launch it; check the
    // process list first so ARIA never opens Music on its own.
    if !music_is_running() {
        return MusicState::Stopped;
    }

    const SCRIPT: &str = r#"tell application "Music"
    set pstate to (player state as text)
    if pstate is "playing" or pstate is "paused" then
        return pstate & linefeed & (name of current track) & linefeed & (artist of current track) & linefeed & (album of current track) & linefeed & ((player position as integer) as text) & linefeed & ((duration of current track as integer) as text)
    else
        return "stopped"
    end if
end tell"#;

    let out = match std::process::Command::new("osascript")
        .args(["-e", SCRIPT])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            eprintln!("music osascript failed: {e}");
            return MusicState::Stopped;
        }
    };
    if !out.status.success() {
        // Most likely the user declined the Automation permission prompt.
        eprintln!(
            "music osascript error: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return MusicState::Stopped;
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    match (lines.next(), lines.next(), lines.next()) {
        (Some(state @ ("playing" | "paused")), Some(title), Some(artist)) if !title.is_empty() => {
            let now = NowPlaying {
                album: lines.next().unwrap_or("").to_string(),
                app_name: Some("Music".into()),
                position_secs: lines.next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
                duration_secs: lines.next().and_then(|s| s.parse().ok()).unwrap_or(0),
                position_known: true, // AppleScript always reports player position
                art: artwork_data_url(title, artist),
                title: title.to_string(),
                artist: artist.to_string(),
            };
            if state == "playing" {
                MusicState::Playing(now)
            } else {
                MusicState::Paused(now)
            }
        }
        _ => MusicState::Stopped,
    }
}

/// Current track's album art as a data: URL. AppleScript can't hand raw image
/// bytes back through stdout intact, so it writes them to a temp file we read
/// and encode. Cached per track — the poll runs every 10 s and the art is by
/// far the heaviest field.
#[cfg(target_os = "macos")]
fn artwork_data_url(title: &str, artist: &str) -> Option<String> {
    use std::sync::Mutex;

    static CACHE: Mutex<Option<((String, String), Option<String>)>> = Mutex::new(None);
    let key = (title.to_string(), artist.to_string());
    if let Ok(cache) = CACHE.lock() {
        if let Some((k, art)) = cache.as_ref() {
            if *k == key {
                return art.clone();
            }
        }
    }

    let art = read_artwork();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((key, art.clone()));
    }
    art
}

#[cfg(target_os = "macos")]
fn read_artwork() -> Option<String> {
    let mut path = std::env::temp_dir();
    path.push("aria-music-art.tmp");
    let posix = path.to_str()?;
    // temp_dir is ours, so the path has no quotes to escape out of the literal.
    let script = format!(
        r#"tell application "Music"
    if player state is stopped then return ""
    if (count of artworks of current track) is 0 then return ""
    set d to raw data of artwork 1 of current track
end tell
try
    set fh to open for access (POSIX file "{posix}") with write permission
    set eof fh to 0
    write d to fh
    close access fh
on error
    try
        close access (POSIX file "{posix}")
    end try
    return ""
end try
return "ok""#
    );
    if run_osascript(&script).ok()? != "ok" {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    if bytes.is_empty() || bytes.len() > 3_000_000 {
        return None; // absent or unreasonably large
    }
    // Apple stores PNG or JPEG artwork; sniff the magic bytes for the mime.
    let mime = if bytes.starts_with(b"\x89PNG") { "image/png" } else { "image/jpeg" };
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

#[cfg(target_os = "macos")]
fn music_is_running() -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.processes()
        .values()
        .any(|p| p.name() == std::ffi::OsStr::new("Music"))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    const FIELDS: usize = 7;

    fn line(parts: &[&str]) -> String {
        parts.join("\x1f")
    }

    #[test]
    fn parses_a_full_sample() {
        let m = parse_metadata_line(&line(&[
            "Playing",
            "spotify",
            "Some Track",
            "Some Artist",
            "Some Album",
            "215000000",
            "https://i.scdn.co/image/abc",
        ]))
        .expect("a playing track parses");
        assert_eq!(m.status, "Playing");
        assert_eq!(m.player, "spotify");
        assert_eq!(m.title, "Some Track");
        assert_eq!(m.length_us, 215_000_000);
        assert_eq!(m.art_url, "https://i.scdn.co/image/abc");
    }

    #[test]
    fn long_titles_survive_intact() {
        // The field separator is the reason titles are not truncated or
        // re-split; a 500-char video title must come through byte-for-byte.
        let title = "A ".repeat(250);
        let m = parse_metadata_line(&line(&[
            "Playing",
            "firefox",
            title.trim_end(),
            "",
            "",
            "0",
            "",
        ]))
        .expect("long titles parse");
        assert_eq!(m.title, title.trim_end());
        assert!(m.artist.is_empty());
    }

    #[test]
    fn rejects_nothing_playable() {
        // Stopped, and no title at all, both mean "report Stopped".
        assert!(parse_metadata_line(&line(&["Stopped", "", "", "", "", "0", ""])).is_none());
        assert!(parse_metadata_line(&line(&["Playing", "vlc", "", "", "", "0", ""])).is_none());
        assert!(parse_metadata_line("").is_none());
        // A truncated line must not panic, just fail the title check.
        assert!(parse_metadata_line("Playing").is_none());
    }

    #[test]
    fn missing_length_is_zero_not_an_error() {
        // Firefox publishes no mpris:length for many sites.
        let m = parse_metadata_line(&line(&["Paused", "firefox", "Netflix", "", "", "", ""]))
            .expect("a paused track with no length still parses");
        assert_eq!(m.length_us, 0);
        assert_eq!(m.status, "Paused");
    }

    #[test]
    fn format_string_and_parser_agree_on_field_count() {
        // Guards against adding a field to one and not the other.
        assert_eq!(line(&["a"; FIELDS]).split('\x1f').count(), FIELDS);
    }

    #[test]
    fn session_clock_recovers_elapsed_across_a_track_change() {
        // Replays the measured Firefox trace: one continuous 1x clock, no
        // reset at the boundary (…19618, 19620, [title changes], 19622…).
        let p = "firefox.instance_1_1414";

        // Joining mid-track tells us nothing about how far in we are.
        assert_eq!(track_elapsed(p, "A Different World", "Korn", 19618.0), None);
        assert_eq!(track_elapsed(p, "A Different World", "Korn", 19620.0), None);

        // The title change is the anchor: elapsed restarts from zero...
        assert_eq!(track_elapsed(p, "HALLELUYAH", "Yzomandias", 19622.0), Some(0.0));
        // ...and then tracks the session clock exactly.
        assert_eq!(track_elapsed(p, "HALLELUYAH", "Yzomandias", 19624.0), Some(2.0));
        assert_eq!(track_elapsed(p, "HALLELUYAH", "Yzomandias", 19680.0), Some(58.0));

        // The next change re-anchors, it does not accumulate.
        assert_eq!(track_elapsed(p, "Whistle", "Flo Rida", 19700.0), Some(0.0));
        assert_eq!(track_elapsed(p, "Whistle", "Flo Rida", 19705.0), Some(5.0));

        // A backwards jump means the player restarted its clock: re-anchor and
        // stop claiming to know, rather than reporting a negative elapsed.
        assert_eq!(track_elapsed(p, "Whistle", "Flo Rida", 3.0), None);
        assert_eq!(track_elapsed(p, "Whistle", "Flo Rida", 9.0), None);

        // A different player starts over with no anchor of its own.
        assert_eq!(track_elapsed("spotify", "Whistle", "Flo Rida", 42.0), None);
    }

    #[test]
    fn a_session_cumulative_position_is_rejected() {
        // Firefox playing Apple Music reports the whole listening session, not
        // the track: 18_870 s against a 44-minute film. Clamping that to the
        // duration is what produced a believable but entirely wrong readout.
        assert!(!position_fits_track(18_870.0, 2679));
        // Normal playback is fine, including right at the end.
        assert!(position_fits_track(0.0, 2679));
        assert!(position_fits_track(2679.0, 2679));
        // A little overshoot is just the gap between the two playerctl calls.
        assert!(position_fits_track(2680.0, 2679));
        assert!(!position_fits_track(2690.0, 2679));
        // Nothing to contradict without a duration — the row is hidden anyway.
        assert!(position_fits_track(18_870.0, 0));
    }

    #[test]
    fn a_healthy_track_never_accrues_stalls() {
        // 3 s of wall clock, 3 s of progress.
        assert_eq!(stall_count(30.0, 33.0, 3.0, 0), 0);
        // Even at half speed, which is well inside the tolerance.
        assert_eq!(stall_count(30.0, 31.5, 3.0, 2), 0, "one good sample clears the count");
    }

    #[test]
    fn a_frozen_position_accrues_stalls() {
        // What --frozen-position produces: playerctl interpolates from its own
        // read, so the value creeps by microseconds and is never twice the
        // same — an equality check would miss this entirely.
        let mut stalls = 0;
        for (prev, now) in [(0.000004, 0.000006), (0.000006, 0.000005), (0.000005, 0.000009)] {
            stalls = stall_count(prev, now, 3.0, stalls);
        }
        assert_eq!(stalls, STALE_POSITION_SAMPLES);
        assert!(stalls >= STALE_POSITION_SAMPLES, "the progress row gets hidden");
    }

    #[test]
    fn a_backwards_jump_counts_as_a_stall() {
        assert_eq!(stall_count(120.0, 5.0, 3.0, 0), 1);
    }

    #[test]
    fn stall_count_saturates_rather_than_wrapping() {
        assert_eq!(stall_count(0.0, 0.0, 3.0, u8::MAX), u8::MAX);
    }

    #[test]
    fn a_paused_track_may_hold_its_position_forever() {
        for _ in 0..STALE_POSITION_SAMPLES * 3 {
            assert!(position_is_live("Held", "Artist", "Paused", 61.0));
        }
    }

    #[test]
    fn changing_track_resets_the_judgement() {
        for _ in 0..STALE_POSITION_SAMPLES + 2 {
            let _ = position_is_live("First", "A", "Playing", 0.0);
        }
        assert!(
            position_is_live("Second", "A", "Playing", 0.0),
            "a new track starts with a clean slate"
        );
    }

    #[test]
    fn rapid_samples_are_not_judged() {
        // Two samples microseconds apart say nothing about whether playback is
        // advancing; a burst of them must not hide a working progress bar.
        for _ in 0..STALE_POSITION_SAMPLES * 4 {
            assert!(position_is_live("Fast", "A", "Playing", 12.0));
        }
    }

    #[test]
    fn friendly_player_names() {
        assert_eq!(friendly_player_name("spotify").as_deref(), Some("Spotify"));
        assert_eq!(
            friendly_player_name("firefox.instance_1_2").as_deref(),
            Some("Firefox")
        );
        assert_eq!(friendly_player_name("").as_deref(), None);
        // Unknown players are title-cased rather than dropped.
        assert_eq!(friendly_player_name("audacious").as_deref(), Some("Audacious"));
    }

    #[test]
    fn sniffs_image_mime_from_magic_bytes() {
        assert_eq!(sniff_image_mime(b"\x89PNG\r\n\x1a\n"), "image/png");
        assert_eq!(sniff_image_mime(b"GIF89a"), "image/gif");
        assert_eq!(sniff_image_mime(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
        assert_eq!(sniff_image_mime(b"\xff\xd8\xff"), "image/jpeg");
        assert_eq!(sniff_image_mime(b""), "image/jpeg"); // unknown -> let the browser sniff
    }

    #[test]
    fn art_cache_is_bounded_and_lru_by_insertion() {
        for i in 0..ART_CACHE_MAX + 4 {
            art_cache_put((format!("t{i}"), "a".into()), Some(format!("data:{i}")));
        }
        let len = ART_CACHE.lock().unwrap().len();
        assert_eq!(len, ART_CACHE_MAX, "cache must not grow without bound");
        // The oldest entries were evicted, the newest survive.
        assert!(art_cache_get(&("t0".into(), "a".into())).is_none());
        let newest = ART_CACHE_MAX + 3;
        assert_eq!(
            art_cache_get(&((format!("t{newest}")), "a".into())),
            Some(Some(format!("data:{newest}")))
        );
        // Re-inserting a key moves it, it does not duplicate.
        art_cache_put((format!("t{newest}"), "a".into()), None);
        assert_eq!(ART_CACHE.lock().unwrap().len(), ART_CACHE_MAX);
        assert_eq!(art_cache_get(&(format!("t{newest}"), "a".into())), Some(None));
    }

    #[test]
    fn already_inlined_art_is_passed_through_without_fetching() {
        let uri = "data:image/png;base64,AAAA";
        assert_eq!(art_for(uri, "t", "a").as_deref(), Some(uri));
        assert_eq!(art_for("", "t", "a"), None);
    }

    #[test]
    fn percent_decodes_file_urls() {
        assert_eq!(percent_decode("/tmp/a%20b.png"), "/tmp/a b.png");
        assert_eq!(percent_decode("/tmp/plain.png"), "/tmp/plain.png");
    }
}
