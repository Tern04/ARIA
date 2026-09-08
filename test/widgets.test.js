import { test } from "node:test";
import assert from "node:assert/strict";
import { WIDGETS, defaultLayout } from "../src/lib/widgets.js";
import { GRID_COLS, GRID_ROWS, normalizeLayout, overlaps, rectOf } from "../src/lib/layout.js";
import {
  BUILTIN_PRESETS,
  MAX_PRESETS,
  applyBuiltins,
  boardToLayout,
  sanitizeBoard,
} from "../src/lib/presets.js";

const entries = Object.entries(WIDGETS);

test("every preset is a third, a half or the whole board wide", () => {
  // The rule the whole size system rests on: mixed widths that are not
  // multiples of 3 leave slots no other widget can fill.
  for (const [id, w] of entries) {
    for (const [size, [cols]] of Object.entries(w.sizes)) {
      assert.ok([3, 6, 12].includes(cols), `${id}.${size} is ${cols} columns wide`);
    }
  }
});

test("every preset fits inside the grid", () => {
  for (const [id, w] of entries) {
    for (const [size, [cols, rows]] of Object.entries(w.sizes)) {
      assert.ok(cols <= GRID_COLS && rows <= GRID_ROWS, `${id}.${size} is ${cols}x${rows}`);
    }
  }
});

test("every widget's home size is one it actually offers", () => {
  for (const [id, w] of entries) {
    assert.ok(w.sizes[w.home[2]], `${id} homes at size ${w.home[2]}, which it has no preset for`);
  }
});

test("the default board is placed inside the grid and never overlaps", () => {
  const layout = defaultLayout();
  const placed = Object.keys(WIDGETS).filter((id) => !layout[id].hidden);
  for (const id of placed) {
    const r = rectOf(id, layout, WIDGETS);
    assert.ok(r.c >= 1 && r.c + r.w <= GRID_COLS + 1, `${id} runs off the side`);
    assert.ok(r.r >= 1 && r.r + r.h <= GRID_ROWS + 1, `${id} runs off the bottom`);
  }
  for (let i = 0; i < placed.length; i++) {
    for (let j = i + 1; j < placed.length; j++) {
      const [a, b] = [placed[i], placed[j]];
      assert.equal(
        overlaps(rectOf(a, layout, WIDGETS), rectOf(b, layout, WIDGETS)),
        false,
        `${a} overlaps ${b}`,
      );
    }
  }
});

test("normalizeLayout leaves a fresh board alone", () => {
  // If the homes ever stop being a valid board, normalizeLayout would quietly
  // relocate widgets on first run — this fails instead.
  const layout = defaultLayout();
  assert.equal(normalizeLayout(layout, WIDGETS), false);
  assert.deepEqual(layout, defaultLayout());
});

test("the default board fills the grid exactly", () => {
  const layout = defaultLayout();
  const used = Object.keys(WIDGETS)
    .filter((id) => !layout[id].hidden)
    .reduce((n, id) => {
      const r = rectOf(id, layout, WIDGETS);
      return n + r.w * r.h;
    }, 0);
  assert.equal(used, GRID_COLS * GRID_ROWS, "the default board should leave no empty cells");
});

test("the built-in presets are boards, not arbitrary coordinates", () => {
  for (const preset of BUILTIN_PRESETS) {
    const board = sanitizeBoard(preset.board, WIDGETS);
    assert.equal(
      Object.keys(board).length,
      Object.keys(preset.board).length,
      `${preset.name} names a widget or size that does not exist`,
    );
    assert.deepEqual(
      Object.keys(WIDGETS).filter((id) => !(id in board)),
      [],
      `${preset.name} omits a widget, which would reappear at its home cell`,
    );
    // Applying it must be a no-op for normalizeLayout: if the board needed
    // fixing, the preset would not restore the arrangement it promises.
    const layout = boardToLayout(board, defaultLayout());
    assert.equal(normalizeLayout(layout, WIDGETS), false, `${preset.name} does not lay out cleanly`);
  }
});

test("a shipped preset is seeded once per version, and a deleted one stays gone", () => {
  const [{ name, version }] = BUILTIN_PRESETS;
  const first = applyBuiltins([], null, WIDGETS);
  assert.deepEqual(
    first.presets.map((p) => p.name),
    [name],
  );
  assert.equal(first.seeded[name], version);

  // Deleted at the version it was seeded at: it must not come back.
  const afterDelete = applyBuiltins([], JSON.stringify(first.seeded), WIDGETS);
  assert.deepEqual(afterDelete.presets, []);

  // A revision does come back, replacing the stale copy rather than doubling it.
  const stale = [{ name, board: { gpu: { c: 1, r: 1, size: "s", hidden: false } } }];
  const revised = applyBuiltins(stale, JSON.stringify({ [name]: version - 1 }), WIDGETS);
  assert.equal(revised.presets.length, 1);
  assert.equal(revised.presets[0].board.gpu.size, BUILTIN_PRESETS[0].board.gpu.size);
});

test("seeding never evicts a preset the user saved", () => {
  const mine = Array.from({ length: MAX_PRESETS }, (_, i) => ({
    name: `mine ${i}`,
    board: { gpu: { c: 1, r: 1, size: "m", hidden: false } },
  }));
  const out = applyBuiltins(mine, null, WIDGETS);
  assert.deepEqual(
    out.presets.map((p) => p.name),
    mine.map((p) => p.name),
    "a full list is left alone",
  );
  assert.deepEqual(out.seeded, {}, "and is not recorded as seeded, so it can land later");
});

test("a legacy seeded flag does not block the current version", () => {
  // The first version of the seeder wrote the string "1", not a version map.
  const out = applyBuiltins([], "1", WIDGETS);
  assert.equal(out.presets.length, BUILTIN_PRESETS.length);
});
