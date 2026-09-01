# ARIA

**Autonomous Reactive Intelligent Assistant** — a desktop HUD dashboard that lives on your wallpaper: below your windows, above the background, Rainmeter-style. One transparent, frameless [Tauri](https://tauri.app) window renders live widgets; all data collection runs in the Rust core. No browser, no Electron, no cloud.

Built around a student/dev workflow at ZČU, but every widget is optional and the board is fully rearrangeable.

## Widgets

| Widget | Data | Source |
|---|---|---|
| SYSTEM | CPU / RAM / GPU gauges, animated | `sysinfo`; GPU via `powermetrics` (Apple Silicon), PDH counters (Windows), `nvidia-smi` (Linux) |
| STAG / ZČU | Weekly timetable grid + course list with credits | STAG REST API (`stag-ws.zcu.cz`), CAS browser login |
| GITHUB | Notifications, open PRs, last-pushed repo, contribution wall | GitHub REST + GraphQL, personal access token |
| MAIL | Multi-account unread counts + recent messages, per-account filter | IMAP (read-only, never marks as seen) |
| CRYPTO | BTC/ETH price, change + sparkline over 1D / 1M / 1Y | CoinGecko (free, no key) |
| NOW PLAYING | Track info, transport controls, playlist shuffle chips | Apple Music (macOS), SMTC (Windows), MPRIS (Linux) |
| SCREEN TIME | Daily total + top apps by foreground time | Local tracking, idle-aware |
| TERMINAL | A real shell on a real PTY — `sudo`, `apt`, `vim`, `htop`, job control | `portable-pty` (Unix `openpty` / Windows ConPTY) + [xterm.js](https://xtermjs.org) |
| DISCORD | Voice channels across one or more servers, sorted by how much your group actually uses them (person-minutes tracked locally), who's in them (mute/deafen/streaming), plus online status and current game/song of picked friends | Discord Gateway, own bot token |

## The board

- 12×6 grid; every widget has S/M/L size presets and adapts its content to the size. (TERMINAL has only M and L — see below.)
- Pencil button → edit mode: drag to move, cycle sizes, hide widgets into a tray, reset layout. Layout persists locally.
- Five themes (menu in the header): **Studio** (warm glass console), **JARVIS** (sci-fi cyan), **Porcelain** (light), **Nord** (arctic frost), **Terminal** (phosphor green).
- On macOS and Windows the HUD is click-through so it never steals input: hold **⌥ Option** / **Alt** to interact with it (buttons, chips, edit mode). On **Linux** there is no gate — the HUD is always interactive, so the hint isn't shown. The pin button flips it above all windows temporarily.
- **Wallpaper** (display menu → *Wallpaper…*): pick any image or video as the HUD's backdrop, with fit, dim and blur. On Linux the default is your real desktop wallpaper (the window has to be opaque there — see the WebKitGTK note below); elsewhere the default is none, and setting one makes the window opaque behind the glass. Video is muted and looping, and pauses while the window is hidden.

## Terminal

Your login shell (`$SHELL -l`, PowerShell on Windows) on a genuine PTY, so it is
a terminal and not a command runner: password prompts, `apt` progress bars,
Ctrl-C, job control and `SIGWINCH` reflow all work. It starts in the tray —
drag it onto the board and the shell spawns; it is never started for a widget
you have not placed. The ⟳ in the widget's header (or `Ctrl+Shift+R`) throws the
current shell away and starts a clean one — back in your home directory, blank
screen, rc file and its fastfetch run again.

Your shell config comes along unchanged, fastfetch/neofetch ASCII art included.
Two things to know:

- `$TERM` is `xterm-256color`, not `xterm-kitty`. Truecolor and 256 colours
  work; kitty's **graphics protocol** does not, so a fastfetch image logo needs
  `--logo-type ascii`. `$ARIA_TERM` is set to `1` if you want to branch on it in
  your rc.
- **M is 74×17 cells** in the default window and 115×26 on a 1080p fullscreen
  board; **L** is 155×39 and 237×56. There is no S preset: the Pop!\_OS
  fastfetch block wants ~80×20 and anything smaller just clips.

| Key | Does |
|---|---|
| `Ctrl+Shift+C` / `V` | copy / paste (bare `Ctrl+C` is SIGINT), `Cmd+C` / `V` on macOS |
| `Ctrl+Shift+` `+` / `-` / `0` | font size up / down / reset |
| `Ctrl+Shift+R` | throw the shell away and start a clean one (`Cmd+R` on macOS) |
| `Enter` after the shell exits | start a new one |

xterm.js and its fit addon are committed under `src/vendor/` (no bundler, and
`script-src` is `'self'`) — see the README there for how to update them. They
also cost the CSP one directive: xterm creates `<style>` elements at runtime,
which `style-src 'self'` blocks, so `style-src` carries `'unsafe-inline'`.
`script-src` is untouched.

## Accounts & secrets

Each service widget has its own login: disconnected widgets show a login form inline, connected ones get a ⚙ gear in edit mode with log-out / reconfigure. Secrets (tokens, IMAP passwords) go to the OS credential store — Keychain / Credential Manager / Secret Service via the `keyring` crate. In macOS *dev* builds they live in `~/.aria-dev-secrets.json`, encrypted with a machine-bound key (Tauri dev re-signs every rebuild, which would otherwise trigger a Keychain prompt storm).

Nothing leaves your machine except the API calls to the services themselves.

## Setup

### Prerequisites (all platforms)

- [Rust](https://rustup.rs) (stable)
- [Node.js](https://nodejs.org) (for the Tauri CLI only — the frontend has no build step)
- Platform Tauri deps: see [tauri.app prerequisites](https://tauri.app/start/prerequisites/) (Xcode CLT on macOS, WebView2 on Windows, `webkit2gtk` etc. on Linux)

```sh
git clone https://github.com/Tern04/ARIA.git
cd ARIA
npm install
npm run tauri dev     # development
npm run tauri build   # release bundle
```

### Tests

```sh
npm test                                    # frontend logic (node --test)
cargo test --manifest-path src-tauri/Cargo.toml
```

Two dev helpers live in `scripts/dev/`, for the parts a unit test can't reach:

- `fake-mpris.py` publishes a fake MPRIS player so NOW PLAYING can be driven
  into its awkward states on demand — a 200-character title, a cover URL that
  404s, a position that errors, a position that never advances. Needs
  `python3-dbus` and `python3-gi`.
- `screentime-probe.sh` prints the session facilities the screen-time tracker
  picks its backend from, so "the tracker is broken" is distinguishable from
  "this session exposes nothing".

### macOS

Everything works here — this is the primary platform.

- **GPU gauge** shells out to `powermetrics`, which needs root. Allow it without a password prompt:
  ```sh
  echo "$(whoami) ALL=(ALL) NOPASSWD: /usr/bin/powermetrics" | sudo tee /etc/sudoers.d/powermetrics
  ```
  Without this the GPU ring simply stays empty.
- **Now Playing**: the first playback poll triggers a macOS Automation prompt ("aria wants to control Music") — click OK once.
- Remember: hold **⌥** to click anything on the HUD (macOS and Windows only).

### Windows

Fully implemented, pending verification on real hardware (developed and type-checked cross-target from macOS):

- desktop-layer placement via WorkerW/Progman parenting (Rainmeter technique), **Alt** instead of ⌥ for interaction
- NOW PLAYING reads the system media session (SMTC) — covers Spotify, browsers, Apple Music for Windows, anything the media keys control; transport buttons work, playlist chips are Apple-Music-only and don't render
- Screen time via foreground window + last-input idle detection
- GPU gauge from the "GPU Engine" performance counters (same source as Task Manager)
- Secrets go to Windows Credential Manager

If something misbehaves, it will be one of these — issues welcome.

### Linux (Pop!_OS / COSMIC + GNOME)

Cross-platform widgets work, plus:

- **GPU gauge** via `nvidia-smi` (NVIDIA cards; the gauge stays blank on other GPUs).
- **NOW PLAYING** over MPRIS via `playerctl` — covers Spotify, VLC and any browser tab exposing a media session; transport buttons work, playlist chips are Apple-Music-only and don't render. The player is pinned per sample, so several media sessions at once can't cross-wire the title and the position. A player that reports no position, or one that never advances, hides the progress bar instead of running a meaningless counter. Cover art is inlined (the CSP forbids remote images) and fetched off the poll path: Chromium-based browsers publish a local cover and Spotify an `https` one, so they show artwork and the real title; Firefox exposes very little for some sites (e.g. Netflix shows only "Netflix", no art). Needs `playerctl` installed.

  **Browser tabs get an elapsed time, but no progress bar.** Firefox's media session publishes no `mpris:length`, and its `Position` counts the whole listening session rather than the current track — measured against Apple Music web it ran past 19 000 s (5+ hours) while the track changed underneath it. That clock is sound though: it advances at exactly 1× and runs straight through a track change without a blip, so ARIA recovers the elapsed time within a track by noting where it stood when the title last changed. Join mid-track and no time is shown until the next track starts, because how far in you already are is genuinely unknowable. The track *length* is not recoverable at all — it is only knowable once the track has ended — so no bar is drawn rather than one clamped to a guess. Native players (Spotify, VLC, mpv) report both properly and get a real bar.
- **SCREEN TIME** picks a backend from the session:
  - **X11** (GNOME/Xorg and any other X11 WM) — focused window from `_NET_ACTIVE_WINDOW`, idle from MIT-SCREEN-SAVER. Full per-app breakdown.
  - **Wayland + COSMIC** (`zcosmic-toplevel-info`) or **wlroots-family** compositors — KDE Plasma, sway, Hyprland, wayfire (`zwlr-foreign-toplevel-management`). Full per-app breakdown, idle from `ext-idle-notify`.
  - **GNOME on Wayland** — Mutter implements `ext-idle-notify` but publishes no focused-window protocol at all, so the widget reports total screen time and says why there are no per-app rows.

  Protocol bindings are generated from vendored, permissively-licensed XML in `src-tauri/protocols/`, so no GPL cosmic crate is pulled into this MIT project.

- **Desktop layer** on **X11** (GNOME/Xorg and any other EWMH window manager): the HUD sits above the wallpaper, below every app window, on all workspaces, and out of the window switcher. It keeps keyboard focus, so the widget login forms still work.

  This is done with EWMH window *states* — `_NET_WM_STATE_BELOW`, `STICKY`, `SKIP_TASKBAR`, `SKIP_PAGER` — on an ordinary managed window, **not** the `_NET_WM_WINDOW_TYPE_DESKTOP` hint that Conky-style widgets use. GNOME Shell draws the wallpaper itself and never expected a client to claim that layer, so Mutter's placement path for a desktop-type window misbehaves: it pinned the window to the far edge of the combined virtual screen and re-applied a "new window" offset on top of the previous position at every relaunch (y drifted −74, then −148). Correcting the position from a move/resize watcher only fed the loop, because the correction is itself a move. The states above are supported, tested paths, and `BELOW` is a persistent layer rather than a per-raise decision, so nothing has to fight the WM to keep the HUD down.

  **On Wayland the HUD stays a normal window.** The mechanism there is `wlr-layer-shell`, which COSMIC implements natively but Mutter does not — so GNOME/Wayland has no route to a desktop layer at all.

Secret storage uses the Secret Service, so GNOME Keyring or KWallet must be running.

Note for GNOME/Xorg: Mutter auto-maximizes windows that ask for the whole work area, so *Fill screen* maximizes explicitly and the move/resize grips are disabled while it's on (a maximized X11 window ignores both).

**Video wallpapers on Linux** need a decoder for whatever you pick. WebM/VP8/VP9 works out of the box; **H.264 (most `.mp4` files) needs `gstreamer1.0-libav`**:

```sh
sudo apt install gstreamer1.0-libav
```

Without it the wallpaper panel reports that the video could not be played. Video is served to the webview over a loopback HTTP server on `127.0.0.1` rather than the asset protocol — WebKitGTK's media player runs on GStreamer, which cannot load wry's custom `asset:` scheme (an `<img>` loads, a `<video>` fails with `FormatError`) and mis-plays `blob:` URLs (`readyState` 4, `currentTime` stuck at 0). HTTP with byte ranges is the only path that actually plays, and the only one that streams a large file instead of buffering it whole. The server binds an ephemeral port on loopback and serves exactly one file behind a per-process random path.

## Connecting the services

### GitHub
Create a [personal access token](https://github.com/settings/tokens); the contribution wall additionally needs the `read:user` scope (degrades gracefully without it). Paste it into the GitHub widget.

### Mail
Add accounts straight in the MAIL widget: label, IMAP host, user, password (for providers with 2FA use an app password). Accounts land in `mail.json` (paths below), passwords in the credential store. IMAP access is read-only — unread flags are never touched.

### STAG (ZČU)
Click CONNECT in the widget — it opens the university CAS login in your browser and catches the ticket on localhost. Log out via the widget's gear.

### Discord
Needs a bot in the server(s) you want to watch (~10 min, free):

1. [discord.com/developers/applications](https://discord.com/developers/applications) → **New Application** → **Bot** tab → **Reset Token**, copy it.
2. Same tab: enable **Presence Intent** and **Server Members Intent** (privileged toggles).
3. Invite it with zero permissions: `https://discord.com/oauth2/authorize?client_id=<APP_ID>&scope=bot&permissions=0` — membership is all it needs. Give its role access to any restricted voice channels you want visible. Repeat per server.
4. In Discord enable Developer Mode (Settings → Advanced), right-click each server → **Copy Server ID**.
5. Paste token + server ID(s) (comma separated) into the widget.
6. Friends list: **edit mode (✎) → widget gear ⚙ → FRIENDS** — add with a user's ID (right-click user → **Copy User ID**) and any display name; applies live. Discord gives bots no access to your actual friends list, so this is deliberately hand-built. A person only shows a live status if they share one of the watched servers — presence arrives per guild — otherwise they stay `offline`.

Channel "popularity" accrues automatically — one point per person-minute in voice, stored locally, so your group's usual channels bubble to the top.

### Crypto
No setup; CoinGecko's free API.

## Files

| File | Where | What |
|---|---|---|
| `mail.json` | app config dir¹ | mail accounts (managed by the widget, hand-editable) |
| `discord.json` | app config dir¹ | server IDs + friends list (managed by the widget, hand-editable) |
| `discord-popularity.json` | app data dir¹ | per-channel person-minute tallies |
| `screentime/` | app data dir¹ | daily usage JSONs |
| layout / theme | `localStorage` | board arrangement, chosen theme, terminal font size |

¹ macOS: `~/Library/Application Support/com.aria.desktop/` — Windows/Linux use the platform-standard config/data dirs.

## License

MIT
