// TERMINAL widget: xterm.js in front of a real PTY (see src-tauri/terminal.rs).
//
// The shell is started lazily — the widget ships hidden, and spawning a login
// shell for a widget nobody put on the board would be rude. Everything below
// therefore assumes `start()` may never run.

import { Terminal } from "../vendor/xterm.mjs";
import { FitAddon } from "../vendor/addon-fit.mjs";

// Per-theme palettes, keyed by theme name like gauge.js does. Kept in JS rather
// than read from CSS custom properties on purpose: xterm wants a colour object,
// and the theme stylesheet is swapped by `href` — so custom properties are
// still the *old* theme's for a frame or two after a switch.
const DARK_ANSI = {
  black: "#2a2e37", red: "#e57b63", green: "#6bc39a", yellow: "#e8ac53",
  blue: "#6a9fd8", magenta: "#b98ad0", cyan: "#6fc4c9", white: "#d6dae3",
  brightBlack: "#5f636d", brightRed: "#ff9a82", brightGreen: "#8ad9b6",
  brightYellow: "#f5c877", brightBlue: "#8dbcf0", brightMagenta: "#d0a8e4",
  brightCyan: "#92dde1", brightWhite: "#f3f5f8",
};

const PALETTES = {
  studio: {
    foreground: "#eceae3", cursor: "#e8ac53", cursorAccent: "#13151b",
    selectionBackground: "rgba(232, 172, 83, 0.28)", ...DARK_ANSI,
  },
  jarvis: {
    foreground: "#cce8f0", cursor: "#00d4ff", cursorAccent: "#000814",
    selectionBackground: "rgba(0, 212, 255, 0.3)",
    ...DARK_ANSI,
    black: "#0a2532", blue: "#4fa8d8", cyan: "#00d4ff", white: "#cce8f0",
    brightBlack: "#4a7a8a", brightBlue: "#7fd0f5", brightCyan: "#aef4ff",
    brightWhite: "#eafcff",
  },
  porcelain: {
    // The only light theme: the whole palette has to darken or nothing is
    // legible on paper-white glass.
    foreground: "#16181d", cursor: "#3450d2", cursorAccent: "#f6f5f1",
    selectionBackground: "rgba(52, 80, 210, 0.22)",
    black: "#16181d", red: "#b32d1e", green: "#177a4b", yellow: "#8a5a00",
    blue: "#3450d2", magenta: "#8b3aa8", cyan: "#0f6d78", white: "#555b66",
    brightBlack: "#8a909b", brightRed: "#c23a2b", brightGreen: "#1d8f58",
    brightYellow: "#a06a00", brightBlue: "#4a63de", brightMagenta: "#a04ac0",
    brightCyan: "#12808d", brightWhite: "#2c313a",
  },
  nord: {
    foreground: "#eceff4", cursor: "#88c0d0", cursorAccent: "#2e3440",
    selectionBackground: "rgba(136, 192, 208, 0.28)",
    black: "#3b4252", red: "#bf616a", green: "#a3be8c", yellow: "#ebcb8b",
    blue: "#81a1c1", magenta: "#b48ead", cyan: "#88c0d0", white: "#e5e9f0",
    brightBlack: "#4c566a", brightRed: "#bf616a", brightGreen: "#a3be8c",
    brightYellow: "#ebcb8b", brightBlue: "#81a1c1", brightMagenta: "#b48ead",
    brightCyan: "#8fbcbb", brightWhite: "#eceff4",
  },
  terminal: {
    // Phosphor CRT: everything bends towards green, but the semantic colours
    // stay distinguishable or `ls` and diffs turn to mush.
    foreground: "#baffcf", cursor: "#33ff66", cursorAccent: "#040a06",
    selectionBackground: "rgba(51, 255, 102, 0.28)",
    black: "#0a2413", red: "#ff5f56", green: "#33ff66", yellow: "#ffbd2e",
    blue: "#4ecf9a", magenta: "#7dffa2", cyan: "#4ecf76", white: "#baffcf",
    brightBlack: "#2b8049", brightRed: "#ff8078", brightGreen: "#7dffa2",
    brightYellow: "#ffd464", brightBlue: "#7dffc9", brightMagenta: "#aaffc4",
    brightCyan: "#7dffa2", brightWhite: "#e6fff0",
  },
};

// Matches the user's kitty stack; falls back to the webfont the rest of the
// HUD already loads. No Nerd Font is bundled — nothing here needs one.
const FONT_STACK = '"Fira Mono", "JetBrains Mono", ui-monospace, monospace';
const DEFAULT_FONT_SIZE = 12;
const MIN_FONT_SIZE = 7;
const MAX_FONT_SIZE = 28;
const FIT_DEBOUNCE = 80;

const IS_MAC = navigator.platform.includes("Mac");

let term = null;
let fitAddon = null;
let host = null;
let sessionId = null;
let starting = false;
let exited = false;
let fitTimer = 0;
let restarting = false;
/** Output that arrived before `term_open` returned the id it belongs to. */
let earlyOutput = [];

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

export function initTerminal() {
  host = document.getElementById("term-host");
  host.addEventListener("mousedown", () => term?.focus());
  document.getElementById("term-restart").addEventListener("click", () => {
    if (term) restartShell();
  });
  // A page reload (dev) would otherwise orphan the shell.
  window.addEventListener("beforeunload", () => {
    if (sessionId !== null) invoke("term_close", { id: sessionId });
  });
}

/** Called from applyLayout(): start on first appearance, refit afterwards. */
export function syncTerminal() {
  const widget = document.getElementById("widget-terminal");
  if (!widget || widget.style.display === "none") return;
  if (!term && !starting) start();
  else scheduleFit();
}

/** Called from applyTheme(), alongside redrawGauges(). */
export function applyTerminalTheme(name) {
  if (!term) return;
  term.options.theme = themeFor(name);
}

function themeFor(name) {
  // `background` is transparent so the glass panel (and the wallpaper behind
  // it) shows through, the way every other widget's surface does.
  return { background: "#00000000", ...(PALETTES[name] || PALETTES.studio) };
}

function currentTheme() {
  return document.documentElement.dataset.theme || "studio";
}

function loadFontSize() {
  const n = Number(localStorage.getItem("aria-term-fontsize"));
  return Number.isFinite(n) && n >= MIN_FONT_SIZE && n <= MAX_FONT_SIZE ? n : DEFAULT_FONT_SIZE;
}

async function start() {
  starting = true;
  try {
    term = new Terminal({
      fontFamily: FONT_STACK,
      fontSize: loadFontSize(),
      lineHeight: 1.15,
      theme: themeFor(currentTheme()),
      allowTransparency: true,
      cursorBlink: true,
      scrollback: 5000,
      // ⌥ on macOS types accented characters; as Meta it drives readline's
      // word motions instead, which is what a terminal is for.
      macOptionIsMeta: true,
    });
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(host);
    term.attachCustomKeyEventHandler(handleKey);
    fitAddon.fit();

    const { listen } = window.__TAURI__.event;
    // Registered before term_open: listen() is async IPC, and the shell starts
    // printing (a prompt, fastfetch) within a frame of being spawned — the
    // first flush would be lost in the gap. Output that beats the id home is
    // queued and replayed below.
    await listen("term-output", (e) => {
      if (sessionId === null) earlyOutput.push(e.payload);
      else if (e.payload.id === sessionId) term.write(e.payload.data);
    });
    await listen("term-exit", (e) => {
      if (e.payload.id !== sessionId) return;
      exited = true;
      term.write("\r\n\x1b[2m[process exited — press Enter for a new shell]\x1b[0m\r\n");
    });

    sessionId = await invoke("term_open", { cols: term.cols, rows: term.rows });
    for (const p of earlyOutput) if (p.id === sessionId) term.write(p.data);
    earlyOutput = [];

    term.onData(onData);
    new ResizeObserver(scheduleFit).observe(document.getElementById("widget-terminal"));
    term.focus();
  } catch (e) {
    if (term) term.write(`\r\n\x1b[31mcould not start a shell: ${e}\x1b[0m\r\n`);
    else host.textContent = `could not start a shell: ${e}`;
  } finally {
    starting = false;
  }
}

function onData(data) {
  if (exited) {
    // Any Enter restarts; anything else is ignored, so stray keys on a dead
    // session don't silently do nothing forever.
    if (data.includes("\r")) restartShell();
    return;
  }
  if (sessionId !== null) invoke("term_write", { id: sessionId, data });
}

/**
 * Throw the current shell away and start a fresh one: back in the home
 * directory, clean environment, blank screen, rc file (and its fastfetch) run
 * again. Also the recovery path when the shell has exited on its own, where
 * closing a session whose child is already dead is a no-op.
 */
async function restartShell() {
  if (restarting) return;
  restarting = true;
  const previous = sessionId;
  // Cleared first so the output listener stops writing the old shell's
  // trailing bytes into a terminal that is about to belong to a new one.
  sessionId = null;
  exited = false;
  earlyOutput = [];
  try {
    if (previous !== null) await invoke("term_close", { id: previous });
    term.reset();
    sessionId = await invoke("term_open", { cols: term.cols, rows: term.rows });
    // Same race as the first start: the new shell prints before invoke()
    // returns, so replay whatever the listener queued for this id.
    for (const p of earlyOutput) if (p.id === sessionId) term.write(p.data);
    earlyOutput = [];
    term.focus();
  } catch (e) {
    exited = true;
    term.write(`\r\n\x1b[31mcould not start a shell: ${e}\x1b[0m\r\n`);
  } finally {
    restarting = false;
  }
}

function scheduleFit() {
  clearTimeout(fitTimer);
  fitTimer = setTimeout(fitNow, FIT_DEBOUNCE);
}

function fitNow() {
  // A hidden widget has no dimensions; fitting against them would resize the
  // PTY to garbage and reflow the shell's output for a size nobody can see.
  if (!term || !host.clientWidth || !host.clientHeight) return;
  fitAddon.fit();
  if (sessionId !== null) {
    invoke("term_resize", { id: sessionId, cols: term.cols, rows: term.rows });
  }
}

// Ctrl+Shift+<key>, because the bare Ctrl+<key> forms belong to the shell:
// Ctrl+C is SIGINT, Ctrl+R is reverse-search. Matches kitty's bindings.
const SHORTCUTS = {
  KeyC: () => copy(),
  KeyV: () => paste(),
  KeyR: () => restartShell(),
  Equal: () => setFontSize(term.options.fontSize + 1),
  Minus: () => setFontSize(term.options.fontSize - 1),
  Digit0: () => setFontSize(DEFAULT_FONT_SIZE),
};

/** Returns false to swallow the key, true to let xterm send it to the shell. */
function handleKey(e) {
  if (e.type !== "keydown") return true;
  const combo = IS_MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey && e.shiftKey;
  if (!combo) return true;
  const action = SHORTCUTS[e.code];
  if (!action) return true;
  // The webview claims some of these for itself — Ctrl+Shift+R is a hard
  // reload, ⌘R a reload — and returning false only stops *xterm* from sending
  // the key on. Without this the HUD reloads instead of restarting the shell.
  e.preventDefault();
  action();
  return false;
}

function setFontSize(next) {
  const size = Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, next));
  term.options.fontSize = size;
  try {
    localStorage.setItem("aria-term-fontsize", String(size));
  } catch {}
  fitNow();
}

async function copy() {
  const sel = term.getSelection();
  if (!sel) return;
  try {
    await navigator.clipboard.writeText(sel);
  } catch {
    notice("clipboard copy was blocked");
  }
}

async function paste() {
  try {
    const text = await navigator.clipboard.readText();
    // term.paste() rather than writing the text straight to the PTY: it adds
    // the bracketed-paste markers, so pasting a multi-line block into a shell
    // that asked for them lands as text to edit instead of running line by
    // line the moment it arrives.
    if (text) term.paste(text);
  } catch {
    notice("clipboard paste was blocked");
  }
}

/** A dim one-liner in the terminal itself — there is no other UI to put it in. */
function notice(msg) {
  term.write(`\r\n\x1b[2m${msg}\x1b[0m\r\n`);
}
