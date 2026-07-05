# ARIA

**Autonomous Reactive Intelligent Assistant** — a cross-platform desktop HUD that pulls live data from your university schedule, GitHub, email, and system hardware into a single sci-fi overlay window, styled after Stark Industries.

Runs on macOS, Windows, and Pop!_OS from one codebase. No browser, no Electron — built with Tauri so the binary stays small and the window can be transparent and frameless.

## What It Shows

- **Today's schedule** — pulls from STAG (ZČU university system) via REST API, shows today's classes and a countdown to the next exam
- **GitHub activity** — open pull requests and CI status for your repos, via the GitHub REST API
- **Unread email** — unread count from Seznam.cz via IMAP
- **Hardware** — CPU, RAM, disk usage via psutil; on Apple Silicon, GPU/CPU utilization from `powermetrics`
- **Screen time** — tracks active window to show where your time goes (custom per-OS implementation)
- **Now playing** — current Apple Music track, sourced from AppleScript on macOS, WinRT SMTC on Windows, and MPRIS/D-Bus on Linux

## Stack

| Layer | Tech |
|---|---|
| App shell | Tauri (Rust + WebView) |
| UI | HTML/CSS/SVG/Canvas, Orbitron/Rajdhani fonts |
| Data | Rust IPC commands + Python subprocesses |
| Cache | SQLite or JSON (local only, no cloud) |

The window is transparent and borderless — it sits on your desktop as an overlay, not a conventional app window.

## Setup

> Documentation will live here as modules are implemented.

## License

MIT
