//! A real terminal for the TERMINAL widget.
//!
//! The shell runs on a genuine PTY rather than piped stdio, because that is
//! the difference between "a command runner" and a terminal: `sudo` refuses to
//! read a password unless it is on a TTY, `apt` wants one for its progress bar
//! and prompts, and job control, Ctrl-C and `SIGWINCH` reflow all need a
//! controlling terminal. `portable-pty` gives us one on every platform (Unix
//! `openpty`, Windows ConPTY).
//!
//! Output plumbing is deliberately two-staged. A reader thread does nothing but
//! move bytes from the PTY into a shared buffer; a flusher thread drains that
//! buffer on a fixed tick and emits one `term-output` event per tick. Emitting
//! per read instead would put a JSON IPC hop on every 8 KB of a `cat`, which
//! drowns the webview. See `flush_loop` for why the reader stops reading
//! instead of dropping bytes when the buffer runs high.

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// One PTY read. Bigger than a pipe's 64 KB would help nothing: the flush tick,
/// not the read size, sets how often the webview hears from us.
const READ_BUF: usize = 8 * 1024;
/// Coalescing window. ~1 frame: fast enough to feel instant while typing, slow
/// enough that a flood becomes ~60 events/s instead of thousands.
const FLUSH: Duration = Duration::from_millis(16);
/// Backpressure threshold — see `read_loop`.
const HIGH_WATER: usize = 512 * 1024;
/// How long the reader naps while waiting for the flusher to catch up.
const BACKOFF: Duration = Duration::from_millis(5);

#[derive(Clone, Serialize)]
struct Output {
    id: u32,
    data: String,
}

#[derive(Clone, Serialize)]
struct Exit {
    id: u32,
}

/// Shared between the reader and the flusher: the bytes in flight, plus the
/// reader's "the shell is gone" signal.
#[derive(Default)]
struct Pipe {
    buf: Mutex<Vec<u8>>,
    eof: AtomicBool,
}

struct Session {
    /// Input goes through a channel to a dedicated writer thread so that
    /// `term_write` can never block the caller — Tauri runs sync commands on
    /// the main thread, and a large paste into a shell that has stopped
    /// reading would otherwise freeze the whole HUD.
    input: Sender<Vec<u8>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

#[derive(Default)]
pub struct Terminals {
    sessions: Mutex<HashMap<u32, Session>>,
    next_id: AtomicU32,
}

pub fn init(app: &AppHandle) {
    app.manage(Terminals::default());
}

/// The shell to run, configured as an interactive login shell.
///
/// Login (`-l`) rather than plain interactive because PATH additions usually
/// live in `.zprofile`/`.profile`, and a terminal that can't find the tools you
/// installed is useless.
#[cfg(unix)]
fn shell_command() -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = CommandBuilder::new(shell);
    cmd.arg("-l");
    cmd
}

#[cfg(windows)]
fn shell_command() -> CommandBuilder {
    // pwsh (7+) if it is on PATH, else the in-box Windows PowerShell.
    let exe = if which_pwsh() { "pwsh.exe" } else { "powershell.exe" };
    let mut cmd = CommandBuilder::new(exe);
    cmd.arg("-NoLogo");
    cmd
}

#[cfg(windows)]
fn which_pwsh() -> bool {
    std::process::Command::new("where")
        .arg("pwsh.exe")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    let key = "HOME";
    #[cfg(windows)]
    let key = "USERPROFILE";
    std::env::var_os(key).map(std::path::PathBuf::from)
}

/// Open a shell on a new PTY. Returns the session id the other commands take.
#[tauri::command]
pub fn term_open(
    app: AppHandle,
    state: tauri::State<Terminals>,
    cols: u16,
    rows: u16,
) -> Result<u32, String> {
    let size = PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = portable_pty::native_pty_system()
        .openpty(size)
        .map_err(|e| format!("openpty failed: {e}"))?;

    let mut cmd = shell_command();
    // xterm.js speaks xterm-256color; anything inherited from the terminal ARIA
    // was launched from (`xterm-kitty`, say) would advertise capabilities we do
    // not have.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // Lets a shell rc branch on "am I in the HUD?" the way it might branch on
    // $KITTY_WINDOW_ID — e.g. a smaller fastfetch logo in here.
    cmd.env("ARIA_TERM", "1");
    // If ARIA was launched from kitty, its shell integration variables are in
    // our environment and would make the new shell load integration for a
    // terminal it is not running in.
    for k in ["KITTY_WINDOW_ID", "KITTY_PID", "KITTY_INSTALLATION_DIR", "KITTY_SHELL_INTEGRATION"] {
        cmd.env_remove(k);
    }
    if let Some(home) = home_dir() {
        cmd.cwd(home);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to start shell: {e}"))?;
    // The slave fd must go now: while we still hold it the PTY never reaches
    // EOF, so an exited shell would look like a live one that has gone quiet.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("pty reader failed: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("pty writer failed: {e}"))?;

    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let pipe = Arc::new(Pipe::default());

    std::thread::spawn({
        let pipe = pipe.clone();
        move || read_loop(reader, pipe)
    });
    std::thread::spawn({
        let app = app.clone();
        move || flush_loop(app, id, pipe)
    });

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || write_loop(writer, rx));

    state.sessions.lock().unwrap().insert(
        id,
        Session {
            input: tx,
            master: pair.master,
            child,
        },
    );
    Ok(id)
}

/// Move bytes from the PTY into the shared buffer until the shell exits.
///
/// When the buffer is already deep, this *stops reading* rather than dropping
/// or truncating. The unread bytes back up in the kernel's PTY buffer, which
/// blocks the writing program — real terminal flow control. Dropping bytes
/// instead would cut escape sequences in half and leave the screen wrecked
/// long after the flood ended.
fn read_loop(mut reader: Box<dyn Read + Send>, pipe: Arc<Pipe>) {
    let mut chunk = [0u8; READ_BUF];
    loop {
        if pipe.buf.lock().unwrap().len() >= HIGH_WATER {
            std::thread::sleep(BACKOFF);
            continue;
        }
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => pipe.buf.lock().unwrap().extend_from_slice(&chunk[..n]),
        }
    }
    pipe.eof.store(true, Ordering::Release);
}

/// Drain the buffer on a fixed tick and emit one event per tick.
fn flush_loop(app: AppHandle, id: u32, pipe: Arc<Pipe>) {
    loop {
        let eof = pipe.eof.load(Ordering::Acquire);
        let taken = {
            let mut buf = pipe.buf.lock().unwrap();
            // At EOF nothing will ever complete a dangling sequence, so take
            // the lot and let the lossy conversion mark it.
            let take = if eof { buf.len() } else { utf8_safe_prefix(&buf) };
            buf.drain(..take).collect::<Vec<u8>>()
        };
        if !taken.is_empty() {
            let data = String::from_utf8_lossy(&taken).into_owned();
            let _ = app.emit("term-output", Output { id, data });
        }
        if eof {
            let _ = app.emit("term-exit", Exit { id });
            return;
        }
        std::thread::sleep(FLUSH);
    }
}

fn write_loop(mut writer: Box<dyn Write + Send>, rx: mpsc::Receiver<Vec<u8>>) {
    while let Ok(data) = rx.recv() {
        if writer.write_all(&data).is_err() || writer.flush().is_err() {
            return;
        }
    }
}

/// How many bytes of `buf` can be handed to the webview now.
///
/// A read can land mid-character, so a trailing *incomplete* UTF-8 sequence is
/// held back for the next chunk to complete — otherwise every multi-byte glyph
/// unlucky enough to straddle a read would arrive as replacement characters.
/// Bytes that are outright invalid are a different case: nothing will ever
/// complete them, so they are passed through (and lossily converted) rather
/// than wedging the stream forever.
fn utf8_safe_prefix(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        Ok(_) => buf.len(),
        Err(e) => match e.error_len() {
            None => e.valid_up_to(),
            Some(_) => buf.len(),
        },
    }
}

#[tauri::command]
pub fn term_write(state: tauri::State<Terminals>, id: u32, data: String) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions.get(&id).ok_or("no such terminal session")?;
    session
        .input
        .send(data.into_bytes())
        .map_err(|_| "terminal has exited".to_string())
}

#[tauri::command]
pub fn term_resize(
    state: tauri::State<Terminals>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions.get(&id).ok_or("no such terminal session")?;
    session
        .master
        .resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn term_close(state: tauri::State<Terminals>, id: u32) -> Result<(), String> {
    let Some(mut session) = state.sessions.lock().unwrap().remove(&id) else {
        return Ok(()); // already gone; closing twice is not an error
    };
    let _ = session.child.kill();
    let _ = session.child.wait();
    Ok(())
}

/// Exercises the real plumbing: a shell on a real PTY, the reader thread, the
/// UTF-8 boundary handling and EOF detection. Only the Tauri `emit` is left
/// out, since that needs a running app.
#[cfg(all(test, unix))]
mod pty_tests {
    use super::*;

    #[test]
    fn a_shell_runs_a_command_and_then_reaches_eof() {
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
            .expect("openpty");
        // /bin/sh, not $SHELL: the user's rc (fastfetch, a fancy prompt) would
        // make this test's output depend on the machine running it.
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.env("TERM", "xterm-256color");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn shell");
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().expect("reader");
        let mut writer = pair.master.take_writer().expect("writer");
        let pipe = Arc::new(Pipe::default());
        std::thread::spawn({
            let pipe = pipe.clone();
            move || read_loop(reader, pipe)
        });

        // A multi-byte character proves bytes survive the buffer intact.
        writer.write_all(b"printf 'ARIA-OK-\\304\\215\\n'; exit\n").expect("write");
        writer.flush().expect("flush");

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !pipe.eof.load(Ordering::Acquire) {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("shell never exited; PTY did not reach EOF");
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let buf = pipe.buf.lock().unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("ARIA-OK-\u{10d}"), "shell output was: {text:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::utf8_safe_prefix;

    #[test]
    fn passes_complete_ascii_and_utf8() {
        assert_eq!(utf8_safe_prefix(b"hello"), 5);
        assert_eq!(utf8_safe_prefix("čau".as_bytes()), 4);
        assert_eq!(utf8_safe_prefix(b""), 0);
    }

    #[test]
    fn holds_back_a_split_character() {
        // "ab" + the first byte of a two-byte 'č': the tail must wait.
        let mut buf = b"ab".to_vec();
        buf.push(0xC4);
        assert_eq!(utf8_safe_prefix(&buf), 2);

        // A three-byte glyph split after one and after two bytes.
        let art = "█".as_bytes(); // E2 96 88
        assert_eq!(utf8_safe_prefix(&art[..1]), 0);
        assert_eq!(utf8_safe_prefix(&art[..2]), 0);
        assert_eq!(utf8_safe_prefix(art), 3);
    }

    #[test]
    fn does_not_stall_on_invalid_bytes() {
        // A lone continuation byte can never become valid; holding it back
        // would stop the terminal forever, so it goes through.
        let buf = vec![b'o', b'k', 0x80];
        assert_eq!(utf8_safe_prefix(&buf), 3);
    }

    #[test]
    fn resumes_after_a_completed_split() {
        let mut buf = b"x".to_vec();
        buf.push(0xC4); // partial
        let n = utf8_safe_prefix(&buf);
        assert_eq!(n, 1);
        buf.drain(..n);
        buf.push(0x8D); // completes 'č'
        assert_eq!(utf8_safe_prefix(&buf), 2);
    }
}
