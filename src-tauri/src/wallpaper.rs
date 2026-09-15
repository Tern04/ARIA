//! The HUD's backdrop.
//!
//! On Linux the window has to be opaque (WebKitGTK on NVIDIA renders a
//! transparent window's see-through regions as flickering black — see
//! `CLAUDE.md`), so *something* must be painted behind the glass panels. That
//! started as the real desktop wallpaper (`window::desktop_background`); this
//! module generalises it into a user choice that also works on macOS and
//! Windows: the desktop wallpaper, a picked image or video file, or nothing.
//!
//! Only the *choice* lives here; large files are never base64'd through IPC.
//!
//! Images load over Tauri's asset protocol, with the chosen path added to the
//! asset scope at startup and on every change, so exactly one file is readable
//! and nothing else.
//!
//! **Video cannot use that path.** WebKitGTK plays media through GStreamer,
//! which does not know wry's custom `asset:` scheme — an `<img>` pointed at it
//! loads fine, a `<video>` fails instantly with `FormatError`. A `blob:` URL
//! built from the same bytes is worse: the element reports `readyState 4` and
//! fires `playing`, but `currentTime` never leaves 0. Measured, the only thing
//! that actually plays is HTTP with byte-range support — which is also the
//! only option that streams instead of buffering the whole file in memory.
//! Hence `server` below: a loopback-only, one-file media server.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Extensions the frontend should mount in a `<video>` rather than an `<img>`.
const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mkv", "mov", "m4v", "ogv", "ogg", "avi"];

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The real desktop wallpaper. Linux only — it is the only platform that
    /// resolves one (see `window::desktop_background`).
    Desktop,
    /// A file the user picked.
    File,
    /// No backdrop. On macOS/Windows this keeps the window genuinely
    /// transparent; on Linux it falls back to the flat colour in base.css.
    None,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Video,
}

/// How the media fills the window — maps straight onto CSS `object-fit`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Cover,
    Contain,
    Fill,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WallpaperConfig {
    pub source: Source,
    /// Absolute path when `source` is `File`.
    pub path: Option<String>,
    pub kind: Kind,
    pub fit: Fit,
    /// 0.0–1.0 black overlay, so a busy wallpaper doesn't fight the widgets.
    pub dim: f32,
    /// Blur radius in px, same reason.
    pub blur: u32,
    pub muted: bool,
    #[serde(rename = "loop")]
    pub looping: bool,
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        WallpaperConfig {
            // Linux needs an opaque backdrop, and the desktop wallpaper is the
            // least surprising one. Elsewhere the window is see-through and
            // should stay that way unless the user asks otherwise.
            source: if cfg!(target_os = "linux") {
                Source::Desktop
            } else {
                Source::None
            },
            path: None,
            kind: Kind::Image,
            fit: Fit::Cover,
            dim: 0.0,
            blur: 0,
            muted: true,
            looping: true,
        }
    }
}

impl WallpaperConfig {
    /// Clamp anything the frontend could get wrong, and keep `source`/`path`
    /// consistent so the UI can never end up pointing at nothing.
    fn sanitize(mut self) -> Self {
        self.dim = self.dim.clamp(0.0, 1.0);
        self.blur = self.blur.min(60);
        match &self.path {
            Some(p) if !p.is_empty() => self.kind = kind_of(Path::new(p)),
            _ => {
                self.path = None;
                if self.source == Source::File {
                    self.source = Source::None;
                }
            }
        }
        self
    }
}

/// Video or image, from the file extension. MPRIS-style content sniffing isn't
/// worth it here: the user picked the file through a filtered dialog.
fn kind_of(path: &Path) -> Kind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if VIDEO_EXTS.contains(&ext.as_str()) {
        Kind::Video
    } else {
        Kind::Image
    }
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("wallpaper.json"))
}

/// A missing or unreadable config is simply the default — unlike the account
/// collectors there is nothing here worth refusing to overwrite.
pub fn load(app: &AppHandle) -> WallpaperConfig {
    let Ok(path) = config_path(app) else {
        return WallpaperConfig::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<WallpaperConfig>(&raw).ok())
        .map(WallpaperConfig::sanitize)
        .unwrap_or_default()
}

fn save(app: &AppHandle, cfg: &WallpaperConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(app)?, json).map_err(|e| e.to_string())
}

/// Let the webview read exactly this file over the asset protocol. Called on
/// every change *and* at startup — the scope is runtime state, not config, so
/// it has to be re-granted each launch.
fn allow(app: &AppHandle, cfg: &WallpaperConfig) {
    if cfg.source != Source::File {
        return;
    }
    if let Some(path) = &cfg.path {
        if let Err(e) = app.asset_protocol_scope().allow_file(path) {
            eprintln!("wallpaper: could not grant asset access to {path}: {e}");
        }
    }
}

/// Re-grant asset access for the saved wallpaper at launch.
pub fn init(app: &AppHandle) {
    let cfg = load(app);
    allow(app, &cfg);
    if cfg.source == Source::File && cfg.kind == Kind::Video {
        if let Some(path) = &cfg.path {
            server::serve(Path::new(path));
        }
    }
}

/// Loopback media server for video wallpapers — see the module docs for why
/// this exists rather than the asset protocol.
///
/// It binds `127.0.0.1:0` (an ephemeral port, never a routable interface),
/// starts on first use and then serves exactly one file: whichever wallpaper is
/// currently configured. The path carries a per-process random token so other
/// local processes can't enumerate it, and any other request is a flat 404.
mod server {
    use std::io::{Read, Seek, SeekFrom};
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    /// One range chunk. WebKit asks for the whole file when it can, so this is
    /// only a ceiling on how much is held in memory at a time.
    const MAX_CHUNK: u64 = 8 * 1024 * 1024;

    struct Server {
        addr: SocketAddr,
        token: String,
        /// The single file being served, swapped when the wallpaper changes.
        file: Mutex<Option<PathBuf>>,
    }

    static SERVER: OnceLock<Option<Server>> = OnceLock::new();

    /// Random enough that a co-located process can't guess the URL. Not a
    /// secret — the file is the user's own wallpaper — just not enumerable.
    fn token() -> String {
        use std::hash::{BuildHasher, Hasher};
        // RandomState is seeded by the OS, which is all this needs.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        let a = h.finish();
        let mut h2 = std::collections::hash_map::RandomState::new().build_hasher();
        h2.write_u64(a);
        format!("{a:016x}{:016x}", h2.finish())
    }

    fn start() -> Option<Server> {
        let http = match tiny_http::Server::http("127.0.0.1:0") {
            Ok(s) => s,
            Err(e) => {
                eprintln!("wallpaper: could not start the media server: {e}");
                return None;
            }
        };
        let addr = http.server_addr().to_ip()?;
        let server = Server {
            addr,
            token: token(),
            file: Mutex::new(None),
        };
        let token = server.token.clone();
        std::thread::Builder::new()
            .name("aria-wallpaper-http".into())
            .spawn(move || {
                for request in http.incoming_requests() {
                    let path = current_file(&token, request.url());
                    if let Err(e) = respond(request, path) {
                        // A viewer that hangs up mid-seek is routine.
                        eprintln!("wallpaper media: {e}");
                    }
                }
            })
            .ok()?;
        Some(server)
    }

    fn instance() -> Option<&'static Server> {
        SERVER.get_or_init(start).as_ref()
    }

    /// The configured file, but only for the one URL we publish.
    fn current_file(token: &str, url: &str) -> Option<PathBuf> {
        let want = format!("/{token}");
        if url.split('?').next() != Some(want.as_str()) {
            return None;
        }
        instance()?.file.lock().ok()?.clone()
    }

    /// Publish `path` and return the URL the webview should load.
    pub fn serve(path: &Path) -> Option<String> {
        let s = instance()?;
        *s.file.lock().ok()? = Some(path.to_path_buf());
        Some(format!("http://{}/{}", s.addr, s.token))
    }

    /// Stop serving anything (the wallpaper was cleared or is now an image).
    pub fn clear() {
        if let Some(s) = instance() {
            if let Ok(mut f) = s.file.lock() {
                *f = None;
            }
        }
    }

    fn header(name: &str, value: &str) -> tiny_http::Header {
        // Both halves are ASCII literals or numbers we formatted ourselves.
        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
            .expect("static header is well-formed")
    }

    /// `bytes=START-[END]` — the only form WebKit's media player sends.
    fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
        let spec = value.trim().strip_prefix("bytes=")?;
        let (start, end) = spec.split_once('-')?;
        let start: u64 = start.trim().parse().ok()?;
        let end = match end.trim() {
            "" => len.saturating_sub(1),
            e => e.parse().ok()?,
        };
        let end = end.min(len.saturating_sub(1));
        (start <= end).then_some((start, end))
    }

    fn respond(request: tiny_http::Request, path: Option<PathBuf>) -> std::io::Result<()> {
        let Some(path) = path else {
            return request.respond(tiny_http::Response::empty(404));
        };
        let mut file = std::fs::File::open(&path)?;
        let len = file.metadata()?.len();
        let mime = mime_for(&path);

        let range = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Range"))
            .and_then(|h| parse_range(h.value.as_str(), len));

        let mut headers = vec![
            header("Content-Type", mime),
            // Without this WebKit will not even try to play the stream.
            header("Accept-Ranges", "bytes"),
            header("Cache-Control", "no-store"),
        ];

        match range {
            Some((start, end)) => {
                let take = (end - start + 1).min(MAX_CHUNK);
                let end = start + take - 1;
                headers.push(header(
                    "Content-Range",
                    &format!("bytes {start}-{end}/{len}"),
                ));
                file.seek(SeekFrom::Start(start))?;
                let body = file.take(take);
                request.respond(tiny_http::Response::new(
                    tiny_http::StatusCode(206),
                    headers,
                    body,
                    Some(take as usize),
                    None,
                ))
            }
            None => request.respond(tiny_http::Response::new(
                tiny_http::StatusCode(200),
                headers,
                file,
                Some(len as usize),
                None,
            )),
        }
    }

    fn mime_for(path: &Path) -> &'static str {
        match path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref()
        {
            Some("webm") => "video/webm",
            Some("mkv") => "video/x-matroska",
            Some("ogv") | Some("ogg") => "video/ogg",
            Some("mov") => "video/quicktime",
            Some("avi") => "video/x-msvideo",
            // mp4/m4v and anything else we let through the picker.
            _ => "video/mp4",
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn range_header_parsing() {
            assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
            // Open-ended is what WebKit sends first.
            assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
            // An end past EOF is clamped, not rejected.
            assert_eq!(parse_range("bytes=0-99999", 1000), Some((0, 999)));
            assert_eq!(parse_range("bytes=999-999", 1000), Some((999, 999)));
            // Nonsense must not panic or produce an inverted range.
            assert_eq!(parse_range("bytes=900-100", 1000), None);
            assert_eq!(parse_range("items=0-1", 1000), None);
            assert_eq!(parse_range("bytes=abc-", 1000), None);
            assert_eq!(parse_range("", 1000), None);
        }

        #[test]
        fn mime_follows_the_container() {
            assert_eq!(mime_for(Path::new("/a/loop.webm")), "video/webm");
            assert_eq!(mime_for(Path::new("/a/loop.MKV")), "video/x-matroska");
            assert_eq!(mime_for(Path::new("/a/loop.mp4")), "video/mp4");
        }

        #[test]
        fn only_the_token_path_resolves() {
            // Any other URL must not reach the file, whatever is configured.
            assert!(current_file("tok", "/other").is_none());
            assert!(current_file("tok", "/").is_none());
            assert!(current_file("tok", "/tok/../etc/passwd").is_none());
        }
    }
}

#[tauri::command]
pub fn wallpaper_config(app: AppHandle) -> WallpaperConfig {
    load(&app)
}

/// URL the `<video>` element should load, or None when the wallpaper isn't a
/// video. See the module docs for why this isn't just the asset protocol.
/// `path` serves a preset's own video instead of the saved wallpaper's; it is
/// held to the same rules — a real file, with a video extension.
#[tauri::command]
pub fn wallpaper_media_url(app: AppHandle, path: Option<String>) -> Option<String> {
    if let Some(path) = path {
        let p = PathBuf::from(path);
        if !p.is_file() || kind_of(&p) != Kind::Video {
            server::clear();
            return None;
        }
        return server::serve(&p);
    }
    let cfg = load(&app);
    if cfg.source != Source::File || cfg.kind != Kind::Video {
        server::clear();
        return None;
    }
    server::serve(Path::new(cfg.path.as_deref()?))
}

/// Validate a config and make its file readable by the webview, without
/// storing it as *the* wallpaper. Shared by `wallpaper_set` and the preview.
fn prepare(app: &AppHandle, config: WallpaperConfig) -> Result<WallpaperConfig, String> {
    let mut cfg = config.sanitize();
    if cfg.source == Source::File {
        let path = cfg.path.clone().unwrap_or_default();
        if !Path::new(&path).is_file() {
            return Err(format!("{path} is not a readable file"));
        }
        cfg.kind = kind_of(Path::new(&path));
    }
    allow(app, &cfg);
    Ok(cfg)
}

#[tauri::command]
pub fn wallpaper_set(app: AppHandle, config: WallpaperConfig) -> Result<WallpaperConfig, String> {
    let cfg = prepare(&app, config)?;
    save(&app, &cfg)?;
    Ok(cfg)
}

/// Show a wallpaper without adopting it: a board preset can carry its own
/// backdrop, which is worn while that preset is on the board and taken off
/// again afterwards. The saved wallpaper — the one the settings panel edits
/// and the one every launch starts from — is deliberately left alone, so the
/// backdrop a preset borrows can never become the one the user has to undo.
#[tauri::command]
pub fn wallpaper_preview(app: AppHandle, config: WallpaperConfig) -> Result<WallpaperConfig, String> {
    prepare(&app, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_comes_from_the_extension() {
        assert_eq!(kind_of(Path::new("/a/loop.MP4")), Kind::Video);
        assert_eq!(kind_of(Path::new("/a/clip.webm")), Kind::Video);
        assert_eq!(kind_of(Path::new("/a/shot.jpg")), Kind::Image);
        assert_eq!(kind_of(Path::new("/a/no-extension")), Kind::Image);
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = WallpaperConfig {
            source: Source::File,
            path: Some("/home/u/loop.mp4".into()),
            kind: Kind::Video,
            fit: Fit::Contain,
            dim: 0.4,
            blur: 8,
            muted: false,
            looping: true,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        // `loop` is a JS keyword-friendly name on the wire, not `looping`.
        assert!(json.contains("\"loop\":true"), "got {json}");
        assert!(json.contains("\"source\":\"file\""), "got {json}");
        let back: WallpaperConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.fit, Fit::Contain);
        assert_eq!(back.path.as_deref(), Some("/home/u/loop.mp4"));
    }

    #[test]
    fn sanitize_clamps_and_keeps_source_consistent() {
        let cfg = WallpaperConfig {
            dim: 9.0,
            blur: 9999,
            ..WallpaperConfig::default()
        }
        .sanitize();
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.blur, 60);

        // "file" with no path is not a usable state — fall back to none.
        let orphan = WallpaperConfig {
            source: Source::File,
            path: None,
            ..WallpaperConfig::default()
        }
        .sanitize();
        assert_eq!(orphan.source, Source::None);

        // The kind always follows the path, even if the caller lied.
        let lied = WallpaperConfig {
            source: Source::File,
            path: Some("/a/b.webm".into()),
            kind: Kind::Image,
            ..WallpaperConfig::default()
        }
        .sanitize();
        assert_eq!(lied.kind, Kind::Video);
    }

    #[test]
    fn a_bad_config_file_is_just_the_default() {
        assert!(serde_json::from_str::<WallpaperConfig>("{ not json").is_err());
        let d = WallpaperConfig::default();
        assert_eq!(d.path, None);
        assert!(
            d.muted,
            "a video backdrop must never surprise you with audio"
        );
    }
}
