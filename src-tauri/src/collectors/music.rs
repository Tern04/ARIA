use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const POLL: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum MusicState {
    Stopped,
    Playing { title: String, artist: String },
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
    if player state is playing then
        return (name of current track) & linefeed & (artist of current track)
    else
        return ""
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
    match (lines.next(), lines.next()) {
        (Some(title), Some(artist)) if !title.is_empty() => MusicState::Playing {
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
