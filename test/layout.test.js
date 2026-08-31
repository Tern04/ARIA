import { test } from "node:test";
import assert from "node:assert/strict";
import {
  GRID_COLS,
  GRID_ROWS,
  fits,
  findFreeSpot,
  normalizeLayout,
  overlaps,
  rectOf,
} from "../src/lib/layout.js";

const widgets = {
  a: { sizes: { s: [3, 1], m: [4, 2], l: [6, 4] } },
  b: { sizes: { s: [3, 1], m: [4, 2] } },
};
const board = (over = {}) => ({
  a: { c: 1, r: 1, size: "m", hidden: false },
  b: { c: 5, r: 1, size: "m", hidden: false },
  ...over,
});

test("overlaps is edge-exclusive", () => {
  const a = { c: 1, r: 1, w: 4, h: 2 };
  assert.equal(overlaps(a, { c: 5, r: 1, w: 4, h: 2 }), false, "adjacent columns");
  assert.equal(overlaps(a, { c: 1, r: 3, w: 4, h: 2 }), false, "adjacent rows");
  assert.equal(overlaps(a, { c: 4, r: 2, w: 4, h: 2 }), true, "one shared cell");
  assert.equal(overlaps(a, a), true);
});

test("rectOf reads the widget's current size preset", () => {
  assert.deepEqual(rectOf("a", board(), widgets), { c: 1, r: 1, w: 4, h: 2 });
  assert.deepEqual(rectOf("a", board({ a: { c: 2, r: 3, size: "s" } }), widgets), {
    c: 2,
    r: 3,
    w: 3,
    h: 1,
  });
});

test("fits rejects anything outside the grid", () => {
  const l = board();
  assert.equal(fits({ c: 0, r: 1, w: 1, h: 1 }, null, l, widgets), false);
  assert.equal(fits({ c: 1, r: 0, w: 1, h: 1 }, null, l, widgets), false);
  assert.equal(fits({ c: GRID_COLS, r: 1, w: 2, h: 1 }, null, l, widgets), false);
  assert.equal(fits({ c: 1, r: GRID_ROWS, w: 1, h: 2 }, null, l, widgets), false);
  // Exactly flush with the far edge is fine.
  assert.equal(fits({ c: GRID_COLS, r: GRID_ROWS, w: 1, h: 1 }, null, l, widgets), true);
});

test("fits ignores the widget being moved and hidden widgets", () => {
  const l = board();
  const onTopOfA = { c: 1, r: 1, w: 4, h: 2 };
  assert.equal(fits(onTopOfA, null, l, widgets), false);
  assert.equal(fits(onTopOfA, "a", l, widgets), true, "a widget may occupy its own cells");
  assert.equal(
    fits(onTopOfA, null, board({ a: { c: 1, r: 1, size: "m", hidden: true } }), widgets),
    true,
    "a widget in the tray takes no space",
  );
});

test("findFreeSpot scans rows before columns and reports failure", () => {
  const l = board();
  assert.deepEqual(findFreeSpot(4, 2, null, l, widgets), [9, 1]);
  // Nothing that wide exists anywhere.
  assert.equal(findFreeSpot(GRID_COLS + 1, 1, null, l, widgets), null);
});

test("normalizeLayout clamps an out-of-bounds rect back into the grid", () => {
  const l = board({ a: { c: 99, r: 99, size: "m", hidden: false } });
  assert.equal(normalizeLayout(l, widgets), true, "reports that it moved something");
  assert.ok(l.a.c >= 1 && l.a.c + 4 <= GRID_COLS + 1);
  assert.ok(l.a.r >= 1 && l.a.r + 2 <= GRID_ROWS + 1);
});

test("normalizeLayout leaves a valid board untouched", () => {
  const l = board();
  assert.equal(normalizeLayout(l, widgets), false);
  assert.deepEqual(l, board());
});

test("normalizeLayout falls back to a smaller preset before hiding", () => {
  // 'a' at l is 6x4; fill the board so only a small slot is left.
  const crowded = {
    a: { c: 1, r: 1, size: "l", hidden: false },
    b: { c: 1, r: 5, size: "m", hidden: false },
  };
  const wide = {
    a: { sizes: { s: [3, 1], m: [4, 2], l: [12, 5] } },
    b: { sizes: { m: [4, 2] } },
  };
  assert.equal(normalizeLayout(crowded, wide), true);
  assert.notEqual(crowded.a.size, "l", "the oversized preset was abandoned");
  assert.equal(crowded.a.hidden, false, "and it did not need to be hidden");
});

test("normalizeLayout hides a widget that cannot be placed at any size", () => {
  const l = { a: { c: 1, r: 1, size: "m", hidden: false } };
  // Every preset is wider than the grid.
  const impossible = { a: { sizes: { m: [GRID_COLS + 1, 1] } } };
  assert.equal(normalizeLayout(l, impossible), true);
  assert.equal(l.a.hidden, true, "hidden is recoverable from the tray, overlapping is not");
});

test("normalizeLayout resolves an overlap rather than rendering one", () => {
  const l = board({ b: { c: 2, r: 1, size: "m", hidden: false } });
  normalizeLayout(l, widgets);
  assert.equal(
    overlaps(rectOf("a", l, widgets), rectOf("b", l, widgets)),
    false,
    "no two visible widgets may share a cell",
  );
});
