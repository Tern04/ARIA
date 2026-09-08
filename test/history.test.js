import { test } from "node:test";
import assert from "node:assert/strict";
import { autoScale, latest, peak, pushSample, seriesPaths } from "../src/lib/history.js";

test("pushSample keeps the window at its cap, oldest first out", () => {
  const s = [];
  for (let i = 0; i < 5; i++) pushSample(s, i, 3);
  assert.deepEqual(s, [2, 3, 4]);
});

test("a missing reading is a hole, not a zero", () => {
  const s = [];
  pushSample(s, undefined, 3);
  pushSample(s, NaN, 3);
  pushSample(s, null, 3);
  assert.deepEqual(s, [null, null, null]);
  assert.equal(peak(s), null);
  assert.equal(latest(s), null);
});

test("seriesPaths breaks the line at a gap instead of bridging it", () => {
  const paths = seriesPaths([10, 20, null, 30, 40], { len: 5, width: 100, height: 10 });
  assert.equal(paths.length, 2, "two unbroken runs");
  assert.equal(paths[0].split(" ").length, 2);
  assert.equal(paths[1].split(" ").length, 2);
});

test("the newest sample sits at the right edge while the window fills", () => {
  const [path] = seriesPaths([50, 50], { len: 10, width: 90, height: 10 });
  const xs = path.split(" ").map((p) => Number(p.split(",")[0]));
  assert.equal(xs[xs.length - 1], 90, "last sample is at the right edge");
  assert.equal(xs[0], 80, "the one before it is a slot to its left");
});

test("seriesPaths clamps out-of-range readings into the frame", () => {
  const [path] = seriesPaths([-20, 150], { len: 2, width: 10, height: 10, pad: 0 });
  const ys = path.split(" ").map((p) => Number(p.split(",")[1]));
  assert.deepEqual(ys, [10, 0], "floor at the bottom, ceiling at the top");
});

test("a single reading still draws something", () => {
  const [path] = seriesPaths([42], { len: 10 });
  assert.equal(path.split(" ").length, 2, "doubled into a visible dash");
});

test("autoScale pads the range and never collapses to zero height", () => {
  const flat = autoScale([12, 12, 12]);
  assert.ok(flat.max > flat.min, "a steady series still has a range");
  const varied = autoScale([10, 30]);
  assert.ok(varied.min < 10 && varied.max > 30, "padded on both sides");
  assert.ok(varied.min >= 0, "never goes below the floor");
});

test("autoScale with nothing measured yet is still a usable range", () => {
  const s = autoScale([null, null]);
  assert.ok(s.max > s.min);
});
