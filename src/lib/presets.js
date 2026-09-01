// Board presets: named snapshots of the widget layout.
//
// Pure functions over a preset list, so the awkward cases can be tested
// without a DOM or localStorage — and they are the whole difficulty here. A
// preset outlives the board it was saved from: it may name a widget that has
// since been removed, omit one that did not exist yet, or pin a size preset
// whose shape has changed. None of those may produce a broken board.

export const MAX_PRESETS = 8;
export const MAX_NAME = 24;

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
    out.push({ name, board });
    if (out.length >= MAX_PRESETS) break;
  }
  return out;
}

/** Add or overwrite a preset. Oldest gives way once the cap is reached. */
export function withPreset(presets, name, board, widgets) {
  const clean = normalizeName(name);
  if (!clean) return presets;
  const rest = presets.filter((p) => p.name !== clean);
  const next = [...rest, { name: clean, board: sanitizeBoard(board, widgets) }];
  return next.slice(Math.max(0, next.length - MAX_PRESETS));
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
