mod email;
mod github;
mod hardware;

use tauri::AppHandle;

/// Read a secret from the OS keychain (service "ARIA").
/// Set via: security add-generic-password -s ARIA -a <account> -w <secret>
fn keychain_secret(account: &str) -> Result<String, String> {
    let entry = keyring::Entry::new("ARIA", account).map_err(|e| e.to_string())?;
    entry
        .get_password()
        .map_err(|_| format!("no '{account}' secret in keychain"))
}

pub fn spawn_all(app: &AppHandle) {
    hardware::spawn(app.clone());
    github::spawn(app.clone());
    email::spawn(app.clone());
}
