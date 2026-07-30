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
| DISCORD | Voice channels across one or more servers, sorted by how much your group actually uses them (person-minutes tracked locally), who's in them (mute/deafen/streaming), plus online status and current game/song of picked friends | Discord Gateway, own bot token |

## The board

- 12×6 grid; every widget has S/M/L size presets and adapts its content to the size.
- Pencil button → edit mode: drag to move, cycle sizes, hide widgets into a tray, reset layout. Layout persists locally.
- Five themes (menu in the header): **Studio** (warm glass console), **JARVIS** (sci-fi cyan), **Porcelain** (light), **Nord** (arctic frost), **Terminal** (phosphor green).
- The HUD is click-through so it never steals input. Hold **⌥ Option** to interact with it (buttons, chips, edit mode). The pin button flips it above all windows temporarily.

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

### macOS

Everything works here — this is the primary platform.

- **GPU gauge** shells out to `powermetrics`, which needs root. Allow it without a password prompt:
  ```sh
  echo "$(whoami) ALL=(ALL) NOPASSWD: /usr/bin/powermetrics" | sudo tee /etc/sudoers.d/powermetrics
  ```
  Without this the GPU ring simply stays empty.
- **Now Playing**: the first playback poll triggers a macOS Automation prompt ("aria wants to control Music") — click OK once.
- Remember: hold **⌥** to click anything on the HUD.

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
- **NOW PLAYING** over MPRIS via `playerctl` — covers Spotify, VLC and any browser tab exposing a media session; transport buttons work, playlist chips are Apple-Music-only and don't render. Cover art is inlined (the CSP forbids remote images): Chromium-based browsers publish a local cover and Spotify an `https` one, so they show artwork and the real title; Firefox exposes very little for some sites (e.g. Netflix shows only "Netflix", no art). Needs `playerctl` installed.
- **SCREEN TIME** via a small Wayland client — the focused window from COSMIC's `zcosmic-toplevel-info` and idle from `ext-idle-notify`. COSMIC-specific for now (no active-window/idle path exists on plain GNOME/Wayland); the tracker simply records nothing elsewhere. Protocol bindings are generated from vendored, permissively-licensed XML in `src-tauri/protocols/`, so no GPL cosmic crate is pulled into this MIT project.

Still pending: the desktop layer (`_NET_WM_WINDOW_TYPE_DESKTOP`) — the window currently sits as a normal, opaque window (see the WebKitGTK/NVIDIA note in the repo). Secret storage uses the Secret Service, so GNOME Keyring or KWallet must be running.

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
6. Friends list: widget gear → **FRIENDS** — add with a user's ID (right-click user → **Copy User ID**) and any display name; applies live.

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
| layout / theme | `localStorage` | board arrangement, chosen theme |

¹ macOS: `~/Library/Application Support/com.aria.desktop/` — Windows/Linux use the platform-standard config/data dirs.

## License

MIT
