import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MAX_PRESETS,
  promotePreset,
  sanitizeWallpaper,
  boardToLayout,
  matchingPreset,
  normalizeName,
  parsePresets,
  sanitizeBoard,
  withPreset,
  withoutPreset,
} from "../src/lib/presets.js";

const widgets = {
  a: { sizes: { s: [3, 1], m: [3, 2] }, home: [1, 1, "m"] },
  b: { sizes: { s: [3, 1], m: [3, 2] }, home: [4, 1, "m"] },
};
const base = {
  a: { c: 1, r: 1, size: "m", hidden: false },
  b: { c: 4, r: 1, size: "m", hidden: false },
};
const board = () => ({
  a: { c: 7, r: 3, size: "s", hidden: false },
  b: { c: 1, r: 1, size: "m", hidden: true },
});

test("names are trimmed, collapsed and capped", () => {
  assert.equal(normalizeName("  work   board "), "work board");
  assert.equal(normalizeName(""), "");
  assert.equal(normalizeName(null), "");
  assert.equal(normalizeName("x".repeat(50)).length, 24);
});

test("a widget that no longer exists is dropped", () => {
  const cleaned = sanitizeBoard({ ...board(), gone: { c: 1, r: 1, size: "m" } }, widgets);
  assert.deepEqual(Object.keys(cleaned).sort(), ["a", "b"]);
});

test("a size the widget no longer offers is dropped", () => {
  // 'l' was a preset when this was saved; the widget only has s and m now.
  const cleaned = sanitizeBoard({ a: { c: 1, r: 1, size: "l" } }, widgets);
  assert.deepEqual(cleaned, {});
});

test("only the four layout fields survive a hand-edited store", () => {
  const cleaned = sanitizeBoard({ a: { c: 2, r: 2, size: "m", hidden: 1, evil: "x" } }, widgets);
  assert.deepEqual(cleaned.a, { c: 2, r: 2, size: "m", hidden: true });
});

test("malformed storage parses to no presets rather than throwing", () => {
  assert.deepEqual(parsePresets("not json", widgets), []);
  assert.deepEqual(parsePresets(null, widgets), []);
  assert.deepEqual(parsePresets('{"not":"an array"}', widgets), []);
  assert.deepEqual(parsePresets("[]", widgets), []);
});

test("nameless, duplicate and fully-stale presets are skipped", () => {
  const raw = JSON.stringify([
    { name: "  ", board: board() },
    { name: "keep", board: board() },
    { name: "keep", board: board() },
    { name: "stale", board: { gone: { c: 1, r: 1, size: "m" } } },
  ]);
  assert.deepEqual(
    parsePresets(raw, widgets).map((p) => p.name),
    ["keep"],
  );
});

test("saving the same name overwrites in place rather than duplicating", () => {
  let presets = withPreset([], "work", board(), widgets);
  presets = withPreset(presets, "work", { a: { c: 1, r: 1, size: "m", hidden: false } }, widgets);
  assert.equal(presets.length, 1);
  assert.deepEqual(presets[0].board, { a: { c: 1, r: 1, size: "m", hidden: false } });
});

test("the oldest preset gives way at the cap", () => {
  let presets = [];
  for (let i = 0; i < MAX_PRESETS + 2; i++) {
    presets = withPreset(presets, `p${i}`, board(), widgets);
  }
  assert.equal(presets.length, MAX_PRESETS);
  assert.equal(presets[0].name, "p2", "p0 and p1 should have dropped off");
});

test("deleting uses the normalized name", () => {
  const presets = withPreset([], "work", board(), widgets);
  assert.deepEqual(withoutPreset(presets, "  work "), []);
});

test("a widget added after the preset was saved lands at its home", () => {
  // The preset predates widget b entirely.
  const layout = boardToLayout({ a: { c: 7, r: 3, size: "s", hidden: false } }, base);
  assert.deepEqual(layout.a, { c: 7, r: 3, size: "s", hidden: false });
  assert.deepEqual(layout.b, base.b, "b falls back to the default board");
});

test("boardToLayout does not alias the base board", () => {
  const layout = boardToLayout({}, base);
  layout.a.c = 99;
  assert.equal(base.a.c, 1);
});

test("matchingPreset names an exact match and nothing less", () => {
  const presets = withPreset([], "work", board(), widgets);
  assert.equal(matchingPreset(board(), presets), "work");
  const moved = board();
  moved.a.c = 8;
  assert.equal(matchingPreset(moved, presets), null);
  assert.equal(matchingPreset(board(), []), null);
});

test("a preset wallpaper is cleaned into the complete shape Rust expects", () => {
  // The Rust config struct has no serde defaults, so anything short of every
  // field is rejected at the IPC boundary and the preset silently shows the
  // ordinary wallpaper instead.
  const fields = ["source", "path", "kind", "fit", "dim", "blur", "muted", "loop"];
  const file = sanitizeWallpaper({ source: "file", path: "/w.jpg", dim: 9, blur: 999, evil: 1 });
  assert.deepEqual(Object.keys(file).sort(), [...fields].sort());
  assert.deepEqual([file.dim, file.blur, file.fit, file.kind], [1, 60, "cover", "image"]);
  assert.deepEqual(Object.keys(sanitizeWallpaper({ source: "desktop" })).sort(), [...fields].sort());

  assert.equal(sanitizeWallpaper({ source: "file" }), null, "file with no path is unusable");
  assert.equal(sanitizeWallpaper({ source: "nonsense" }), null);
  assert.equal(sanitizeWallpaper(null), null);
});

test("a preset keeps its wallpaper when the board is re-saved", () => {
  const wp = { source: "desktop", path: null, kind: "image", fit: "cover", dim: 0, blur: 0, muted: true, loop: true };
  let list = withPreset([], "work", board(), widgets, wp);
  assert.deepEqual(list[0].wallpaper, wp);
  list = withPreset(list, "work", board(), widgets); // re-saved after a tweak
  assert.deepEqual(list[0].wallpaper, wp);
  list = withPreset(list, "work", board(), widgets, null); // asked for none
  assert.equal("wallpaper" in list[0], false);
  // and it survives a round trip through storage
  const stored = parsePresets(JSON.stringify([{ name: "w", board: board(), wallpaper: wp }]), widgets);
  assert.deepEqual(stored[0].wallpaper, wp);
});

test("promoting moves a preset one place up, and the first one nowhere", () => {
  // The list order is the auto-switch priority, so this is how a board is told
  // to win: a game should take the board from a lecture and not the reverse.
  let presets = withPreset(withPreset([], "a", board(), widgets), "b", board(), widgets);
  presets = withPreset(presets, "c", board(), widgets);
  assert.deepEqual(promotePreset(presets, "c").map((p) => p.name), ["a", "c", "b"]);
  assert.deepEqual(promotePreset(presets, "a").map((p) => p.name), ["a", "b", "c"]);
  assert.deepEqual(promotePreset(presets, "nope"), presets, "an unknown name changes nothing");
  assert.deepEqual(presets.map((p) => p.name), ["a", "b", "c"], "the original is not mutated");
});
