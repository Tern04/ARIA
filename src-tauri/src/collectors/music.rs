use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Default)]
struct NowPlaying {
    title: String,
    artist: String,
    album: String,
    /// Friendly source-app name ("Spotify", "Chrome"…), when identifiable.
    app_name: Option<String>,
    position_secs: u64,
    duration_secs: u64,
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
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        sample_music()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
                ((pos / 10_000_000).max(0) as u64, (end / 10_000_000).max(0) as u64)
            })
            .unwrap_or((0, 0));
        let now = NowPlaying {
            album: props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default(),
            app_name: session
                .SourceAppUserModelId()
                .ok()
                .and_then(|id| friendly_app_name(&id.to_string())),
            position_secs,
            duration_secs,
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
#[cfg(any(target_os = "macos", target_os = "windows"))]
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

/// Linux (MPRIS) transport control lands with that machine.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let state = tauri::async_runtime::spawn_blocking(sample_music)
                .await
                .unwrap_or(MusicState::Stopped);
            if let Err(e) = app.emit("music", state) {
                eprintln!("music emit failed: {e}");
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

/// The Linux (MPRIS) variant lands with that machine.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn spawn(_app: AppHandle) {}

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
                position_secs: lines.next().and_then(|s| s.parse().ok()).unwrap_or(0),
                duration_secs: lines.next().and_then(|s| s.parse().ok()).unwrap_or(0),
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
