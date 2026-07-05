mod email;
mod github;
mod hardware;
mod music;
mod screentime;
pub mod stag;

use tauri::{AppHandle, Manager};
use tokio::sync::watch;

/// Collectors with slow poll cycles hold their first emit until the webview
/// has registered its listeners, otherwise the initial state is lost.
pub struct FrontendReady(watch::Sender<bool>);

#[tauri::command]
pub fn frontend_ready(state: tauri::State<FrontendReady>) {
    let _ = state.0.send(true);
}

/// Read a secret from the OS keychain (service "ARIA").
/// Set via: security add-generic-password -s ARIA -a <account> -w <secret>
fn keychain_secret(account: &str) -> Result<String, String> {
    let entry = keyring::Entry::new("ARIA", account).map_err(|e| e.to_string())?;
    entry
        .get_password()
        .map_err(|_| format!("no '{account}' secret in keychain"))
}

pub fn spawn_all(app: &AppHandle) {
    let (tx, rx) = watch::channel(false);
    app.manage(FrontendReady(tx));
    hardware::spawn(app.clone());
    screentime::spawn(app.clone());
    music::spawn(app.clone());
    github::spawn(app.clone(), rx.clone());
    email::spawn(app.clone(), rx.clone());
    stag::spawn(app.clone(), rx);
}
