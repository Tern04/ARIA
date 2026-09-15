// Board presets: named snapshots of the widget layout.
//
// Pure functions over a preset list, so the awkward cases can be tested
// without a DOM or localStorage — and they are the whole difficulty here. A
// preset outlives the board it was saved from: it may name a widget that has
// since been removed, omit one that did not exist yet, or pin a size preset
// whose shape has changed. None of those may produce a broken board.

import { DEFAULT_LEAD } from "./automation.js";

export const MAX_PRESETS = 8;
export const MAX_NAME = 24;

/**
 * Presets shipped with the app, seeded into a board that has not had them.
 * The telemetry widgets are hidden by default — a loadout is only useful if it
 * is one click away, and hunting four chips out of the tray and sizing each by
 * hand is not that.
 *
 * "Gaming" gives the top half of the board to GPU and the history graph, and
 * the middle band to the four things worth a whole tile mid-session: thermals,
 * the CPU gauges, the player and voice chat. Ping and the notification strip
 * run along the bottom as one-line headlines. Every widget is listed, hidden
 * ones included, so nothing reappears at its home cell and displaces the
 * arrangement.
 *
 * `version` is what lets a shipped preset be revised: it is seeded again when
 * its version has moved on, and never otherwise — so a preset deleted at the
 * version it was seeded at stays deleted.
 */
export const BUILTIN_PRESETS = [
  {
    name: "Gaming",
    version: 2,
    trigger: { kind: "game" },
    board: {
      gpu:        { c: 1, r: 1, size: "l", hidden: false },
      perf:       { c: 7, r: 1, size: "l", hidden: false },
      thermals:   { c: 1, r: 4, size: "m", hidden: false },
      hardware:   { c: 4, r: 4, size: "m", hidden: false },
      music:      { c: 7, r: 4, size: "m", hidden: false },
      discord:    { c: 10, r: 4, size: "m", hidden: false },
      latency:    { c: 1, r: 6, size: "s", hidden: false },
      crypto:     { c: 4, r: 6, size: "s", hidden: false },
      email:      { c: 7, r: 6, size: "s", hidden: false },
      github:     { c: 10, r: 6, size: "s", hidden: false },
      // The board is full at 72 cells, so the four M tiles cost the screen-time
      // headline: it is the one that reports on the session rather than to it.
      screentime: { c: 1, r: 1, size: "s", hidden: true },
      stag:       { c: 1, r: 1, size: "m", hidden: true },
      terminal:   { c: 7, r: 1, size: "m", hidden: true },
      calendar:   { c: 1, r: 1, size: "m", hidden: true },
      notes:      { c: 1, r: 1, size: "m", hidden: true },
    },
  },
  {
    // The editor is somewhere else — this is the board beside it: the shell at
    // half width, what the repo is doing, what the machine is doing, and a
    // place to write down the thing you must not forget after this function.
    name: "Coding",
    version: 1,
    trigger: { kind: "app", apps: ["jetbrains", "code"] },
    board: {
      terminal:   { c: 1, r: 1, size: "m", hidden: false },
      github:     { c: 7, r: 1, size: "m", hidden: false },
      notes:      { c: 10, r: 1, size: "m", hidden: false },
      perf:       { c: 7, r: 3, size: "m", hidden: false },
      thermals:   { c: 10, r: 3, size: "m", hidden: false },
      hardware:   { c: 1, r: 5, size: "m", hidden: false },
      music:      { c: 4, r: 5, size: "m", hidden: false },
      email:      { c: 7, r: 5, size: "s", hidden: false },
      discord:    { c: 7, r: 6, size: "s", hidden: false },
      crypto:     { c: 10, r: 5, size: "s", hidden: false },
      latency:    { c: 10, r: 6, size: "s", hidden: false },
      stag:       { c: 1, r: 1, size: "m", hidden: true },
      calendar:   { c: 1, r: 1, size: "m", hidden: true },
      gpu:        { c: 1, r: 1, size: "m", hidden: true },
      screentime: { c: 1, r: 1, size: "s", hidden: true },
    },
  },
  {
    // The lecture-day board: the week, what is on today, and the notes for it
    // side by side — the three things a class actually needs — with mail and
    // the repo below them and no telemetry at all.
    name: "Study",
    version: 1,
    trigger: { kind: "class", lead: DEFAULT_LEAD },
    board: {
      stag:       { c: 1, r: 1, size: "m", hidden: false },
      calendar:   { c: 7, r: 1, size: "m", hidden: false },
      notes:      { c: 10, r: 1, size: "m", hidden: false },
      email:      { c: 7, r: 3, size: "m", hidden: false },
      github:     { c: 10, r: 3, size: "m", hidden: false },
      screentime: { c: 1, r: 5, size: "m", hidden: false },
      music:      { c: 4, r: 5, size: "m", hidden: false },
      hardware:   { c: 7, r: 5, size: "s", hidden: false },
      discord:    { c: 7, r: 6, size: "s", hidden: false },
      crypto:     { c: 10, r: 5, size: "s", hidden: false },
      latency:    { c: 10, r: 6, size: "s", hidden: false },
      terminal:   { c: 7, r: 1, size: "m", hidden: true },
      gpu:        { c: 1, r: 1, size: "m", hidden: true },
      thermals:   { c: 1, r: 1, size: "m", hidden: true },
      perf:       { c: 1, r: 1, size: "m", hidden: true },
    },
  },
];

/**
 * Fold the shipped presets into the stored list. `seededRaw` is the stored
 * record of what has already been seeded, as a name → version map; the return
 * carries the updated record so the caller can persist it.
 *
 * A shipped preset is added when its version has not been seeded yet, and
 * replaces a same-named one — that is how a revision reaches a board that
 * already has the old copy. It is never added over a full list: the user's own
 * arrangements outrank ours, and `withPreset` would evict the oldest.
 *
 * `triggers` carries the auto-switch rule of each preset that was actually
 * seeded this time, for the caller to fold into its rules. It rides along with
 * the preset rather than being seeded separately so that a shipped rule
 * arrives exactly once, with the board it belongs to: a trigger the user then
 * changes or clears is theirs, and is never written back over.
 */
export function applyBuiltins(presets, seededRaw, widgets) {
  let seeded = {};
  try {
    const parsed = JSON.parse(seededRaw);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) seeded = parsed;
  } catch {}
  let next = presets;
  const record = { ...seeded };
  const triggers = {};
  for (const builtin of BUILTIN_PRESETS) {
    const name = normalizeName(builtin.name);
    if (record[name] === builtin.version) continue;
    const replacing = next.some((p) => p.name === name);
    // Not recorded as seeded, so it lands the day a slot frees up.
    if (!replacing && next.length >= MAX_PRESETS) continue;
    next = withPreset(next, name, builtin.board, widgets);
    record[name] = builtin.version;
    if (builtin.trigger) triggers[name] = builtin.trigger;
  }
  return { presets: next, seeded: record, triggers };
}

/**
 * A preset may carry its own backdrop, worn while that preset is on the board.
 * Cleaned field by field like a board: a preset is stored data, and stored
 * data is never handed to the wallpaper layer as it was found.
 */
export function sanitizeWallpaper(raw) {
  if (!raw || !["desktop", "file", "none"].includes(raw.source)) return null;
  const num = (v, min, max, fallback) => {
    const n = Number(v);
    return Number.isFinite(n) ? Math.min(Math.max(n, min), max) : fallback;
  };
  // Every field the Rust side's config carries, always: it deserializes into
  // a struct with no defaults, so a half-filled object is rejected outright.
  const wp = {
    source: raw.source,
    path: null,
    kind: raw.kind === "video" ? "video" : "image",
    fit: ["cover", "contain", "fill"].includes(raw.fit) ? raw.fit : "cover",
    dim: num(raw.dim, 0, 1, 0),
    blur: Math.round(num(raw.blur, 0, 60, 0)),
    muted: raw.muted !== false,
    loop: raw.loop !== false,
  };
  if (wp.source !== "file") return wp;
  const path = String(raw.path ?? "");
  // "file" with no path is not a usable state; Rust's sanitize says the same.
  if (!path) return null;
  return { ...wp, path };
}

/** Trim a user-typed name to something storable; "" when nothing is left. */
export function normalizeName(raw) {
  return String(raw ?? "")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, MAX_NAME);
}

/**
 * Drop everything that no longer makes sense: widgets that have been removed,
 * and sizes a widget no longer offers. Entries that survive are copied field
 * by field, so a hand-edited localStorage cannot smuggle extra keys into the
 * layout. Widgets simply missing from a preset are not an error — they are
 * filled from the default board by `boardToLayout`.
 */
export function sanitizeBoard(board, widgets) {
  const out = {};
  for (const [id, p] of Object.entries(board ?? {})) {
    if (!widgets[id] || !p || !widgets[id].sizes[p.size]) continue;
    const c = Math.trunc(Number(p.c));
    const r = Math.trunc(Number(p.r));
    if (!Number.isFinite(c) || !Number.isFinite(r)) continue;
    out[id] = { c, r, size: p.size, hidden: !!p.hidden };
  }
  return out;
}

/** Parse the stored JSON, discarding anything malformed rather than throwing. */
export function parsePresets(raw, widgets) {
  let data;
  try {
    data = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(data)) return [];
  const seen = new Set();
  const out = [];
  for (const entry of data) {
    const name = normalizeName(entry?.name);
    if (!name || seen.has(name)) continue;
    const board = sanitizeBoard(entry?.board, widgets);
    // A preset that kept nothing would silently apply as "the default board",
    // which is not what its name promises.
    if (!Object.keys(board).length) continue;
    seen.add(name);
    const wallpaper = sanitizeWallpaper(entry?.wallpaper);
    out.push(wallpaper ? { name, board, wallpaper } : { name, board });
    if (out.length >= MAX_PRESETS) break;
  }
  return out;
}

/**
 * Add or overwrite a preset. Oldest gives way once the cap is reached.
 *
 * Re-saving a preset keeps the wallpaper it already had, unless one is passed:
 * saving a rearranged board is about the board, and silently dropping the
 * backdrop with it would be a second, unasked-for change.
 */
export function withPreset(presets, name, board, widgets, wallpaper) {
  const clean = normalizeName(name);
  if (!clean) return presets;
  const existing = presets.find((p) => p.name === clean);
  const wp = sanitizeWallpaper(wallpaper === undefined ? existing?.wallpaper : wallpaper);
  const rest = presets.filter((p) => p.name !== clean);
  const entry = { name: clean, board: sanitizeBoard(board, widgets) };
  const next = [...rest, wp ? { ...entry, wallpaper: wp } : entry];
  return next.slice(Math.max(0, next.length - MAX_PRESETS));
}

/** Give a preset its own wallpaper, or take it away with null. */
export function withPresetWallpaper(presets, name, wallpaper) {
  const clean = normalizeName(name);
  const wp = sanitizeWallpaper(wallpaper);
  return presets.map((p) => {
    if (p.name !== clean) return p;
    const { wallpaper: _drop, ...rest } = p;
    return wp ? { ...rest, wallpaper: wp } : rest;
  });
}

export function withoutPreset(presets, name) {
  return presets.filter((p) => p.name !== normalizeName(name));
}

/**
 * A complete layout from a preset: the default board with the preset laid over
 * it, so a widget added since the preset was saved arrives at its home cell
 * instead of being undefined. The caller still runs normalizeLayout, which
 * resolves any collision that creates.
 */
export function boardToLayout(board, base) {
  const layout = {};
  for (const [id, home] of Object.entries(base)) {
    layout[id] = board[id] ? { ...home, ...board[id] } : { ...home };
  }
  return layout;
}

/** Name of the preset the board currently matches exactly, else null. */
export function matchingPreset(layout, presets) {
  return presets.find((p) => sameBoard(layout, p.board))?.name ?? null;
}

function sameBoard(layout, board) {
  const ids = Object.keys(layout);
  if (ids.length !== Object.keys(board).length) return false;
  return ids.every((id) => {
    const a = layout[id];
    const b = board[id];
    return b && a.c === b.c && a.r === b.r && a.size === b.size && !!a.hidden === !!b.hidden;
  });
}
