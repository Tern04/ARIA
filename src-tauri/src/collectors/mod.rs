pub mod crypto;
pub mod discord;
pub mod email;
pub mod github;
mod hardware;
pub mod music;
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

// Secret storage backend selection:
//   * Windows / Linux / macOS release -> OS credential store (keyring crate),
//     which encrypts at rest (DPAPI / Secret Service / Keychain).
//   * macOS DEBUG only -> encrypted local file (secrets_file), because
//     `tauri dev` re-signs the binary every rebuild, so macOS treats each
//     rebuild as a new app and re-prompts for keychain access no matter the
//     ACL. The file avoids the prompt storm; it is encrypted with a key
//     derived from this machine's hardware UUID so it is not plaintext on disk.
#[cfg(all(debug_assertions, target_os = "macos"))]
const USE_DEV_FILE: bool = true;

/// Read a secret for account (e.g. "github", "stag", "imap:<user>").
fn keychain_secret(account: &str) -> Result<String, String> {
    #[cfg(all(debug_assertions, target_os = "macos"))]
    if USE_DEV_FILE {
        return secrets_file::get(account);
    }
    keyring::Entry::new("ARIA", account)
        .map_err(|e| e.to_string())?
        .get_password()
        .map_err(|_| format!("no '{account}' secret in credential store"))
}

/// Persist a secret using the same backend split as `keychain_secret`.
fn store_secret(account: &str, value: &str) -> Result<(), String> {
    #[cfg(all(debug_assertions, target_os = "macos"))]
    if USE_DEV_FILE {
        return secrets_file::set(account, value);
    }
    keyring::Entry::new("ARIA", account)
        .map_err(|e| e.to_string())?
        .set_password(value)
        .map_err(|e| e.to_string())
}

/// Remove a secret (log-out) using the same backend split as `keychain_secret`.
fn delete_secret(account: &str) -> Result<(), String> {
    #[cfg(all(debug_assertions, target_os = "macos"))]
    if USE_DEV_FILE {
        return secrets_file::remove(account);
    }
    keyring::Entry::new("ARIA", account)
        .map_err(|e| e.to_string())?
        .delete_credential()
        .map_err(|e| e.to_string())
}

/// macOS dev-only secret store: a hardware-key-encrypted JSON map at
/// ~/.aria-dev-secrets.json (mode 0600). Sidesteps the tauri-dev keychain
/// prompt storm without leaving credentials in plaintext on disk.
#[cfg(all(debug_assertions, target_os = "macos"))]
mod secrets_file {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    const SALT: &[u8] = b"aria-hud-dev-secrets-v1";
    const NONCE_LEN: usize = 12;

    fn path() -> Result<PathBuf, String> {
        let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join(".aria-dev-secrets.json"))
    }

    /// Per-Mac key: SHA-256(salt || IOPlatformUUID). Never leaves the machine
    /// and requires no user input, so decryption is prompt-free.
    fn machine_key() -> Result<[u8; 32], String> {
        let out = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        let uuid = text
            .lines()
            .find(|l| l.contains("IOPlatformUUID"))
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().trim_matches('"').to_string())
            .ok_or("could not read IOPlatformUUID")?;
        let mut h = Sha256::new();
        h.update(SALT);
        h.update(uuid.as_bytes());
        Ok(h.finalize().into())
    }

    fn cipher() -> Result<ChaCha20Poly1305, String> {
        let key = machine_key()?;
        Ok(ChaCha20Poly1305::new(Key::from_slice(&key)))
    }

    fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::getrandom(&mut nonce).map_err(|e| e.to_string())?;
        let ct = cipher()?
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| "encrypt failed".to_string())?;
        let mut out = nonce.to_vec();
        out.extend(ct);
        Ok(out)
    }

    fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
        if data.len() <= NONCE_LEN {
            return Err("ciphertext too short".into());
        }
        let (nonce, ct) = data.split_at(NONCE_LEN);
        cipher()?
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|_| "decrypt failed".to_string())
    }

    fn load() -> Result<BTreeMap<String, String>, String> {
        let bytes = std::fs::read(path()?)
            .map_err(|_| "no dev secrets file (~/.aria-dev-secrets.json)".to_string())?;
        // Accept a legacy plaintext JSON file too, so the existing file
        // upgrades to encrypted on the next write without manual migration.
        let json = decrypt(&bytes).unwrap_or(bytes);
        serde_json::from_slice(&json).map_err(|e| e.to_string())
    }

    fn save(map: &BTreeMap<String, String>) -> Result<(), String> {
        let json = serde_json::to_vec(map).map_err(|e| e.to_string())?;
        let sealed = encrypt(&json)?;
        let p = path()?;
        std::fs::write(&p, sealed).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn get(account: &str) -> Result<String, String> {
        load()?
            .remove(account)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("no '{account}' secret configured"))
    }

    pub fn set(account: &str, value: &str) -> Result<(), String> {
        let mut map = load().unwrap_or_default();
        map.insert(account.to_string(), value.to_string());
        save(&map)
    }

    pub fn remove(account: &str) -> Result<(), String> {
        let mut map = load().unwrap_or_default();
        map.remove(account);
        save(&map)
    }

    /// Re-encrypt the file if it is still legacy plaintext (one-time upgrade).
    pub fn migrate_if_plaintext() {
        let Ok(bytes) = std::fs::read(match path() {
            Ok(p) => p,
            Err(_) => return,
        }) else {
            return;
        };
        // If it decrypts, it is already sealed — nothing to do.
        if decrypt(&bytes).is_ok() {
            return;
        }
        if let Ok(map) = serde_json::from_slice::<BTreeMap<String, String>>(&bytes) {
            if let Err(e) = save(&map) {
                eprintln!("secrets migrate failed: {e}");
            } else {
                eprintln!("dev secrets file encrypted");
            }
        }
    }
}

pub fn spawn_all(app: &AppHandle) {
    #[cfg(all(debug_assertions, target_os = "macos"))]
    secrets_file::migrate_if_plaintext();

    let (tx, rx) = watch::channel(false);
    app.manage(FrontendReady(tx));
    hardware::spawn(app.clone());
    screentime::spawn(app.clone());
    music::spawn(app.clone());
    github::spawn(app.clone(), rx.clone());
    crypto::spawn(app.clone(), rx.clone());
    discord::spawn(app.clone(), rx.clone());
    email::spawn(app.clone(), rx.clone());
    stag::spawn(app.clone(), rx);
}
