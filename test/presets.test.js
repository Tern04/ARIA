import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MAX_PRESETS,
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
