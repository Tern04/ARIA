use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum MusicState {
    Stopped,
    Playing { title: String, artist: String },
    Paused { title: String, artist: String },
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
    #[cfg(target_os = "macos")]
    {
        sample_music()
    }
    #[cfg(not(target_os = "macos"))]
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

/// Windows (SMTC) and Linux (MPRIS) transport control land with those machines.
#[cfg(not(target_os = "macos"))]
fn control(_action: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn playlists() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[cfg(not(target_os = "macos"))]
fn play_playlist(_name: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
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

/// Windows (SMTC) and Linux (MPRIS) variants land with those machines.
#[cfg(not(target_os = "macos"))]
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
        return pstate & linefeed & (name of current track) & linefeed & (artist of current track)
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
        (Some("playing"), Some(title), Some(artist)) if !title.is_empty() => MusicState::Playing {
            title: title.to_string(),
            artist: artist.to_string(),
        },
        (Some("paused"), Some(title), Some(artist)) if !title.is_empty() => MusicState::Paused {
            title: title.to_string(),
            artist: artist.to_string(),
        },
        _ => MusicState::Stopped,
    }
}

#[cfg(target_os = "macos")]
fn music_is_running() -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.processes()
        .values()
        .any(|p| p.name() == std::ffi::OsStr::new("Music"))
}
