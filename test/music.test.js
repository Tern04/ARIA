import { test } from "node:test";
import assert from "node:assert/strict";
import { fmtClock, progressAt, POLL_GRACE_SECS } from "../src/lib/music.js";

const T0 = 1_700_000_000_000;
const playing = (over = {}) => ({
  status: "playing",
  position_secs: 30,
  duration_secs: 200,
  position_known: true,
  ...over,
});

test("fmtClock pads seconds and floors fractions", () => {
  assert.equal(fmtClock(0), "0:00");
  assert.equal(fmtClock(9.9), "0:09");
  assert.equal(fmtClock(61), "1:01");
  assert.equal(fmtClock(600), "10:00");
  assert.equal(fmtClock(3599), "59:59");
  // Tracks longer than an hour keep counting in minutes rather than wrapping.
  assert.equal(fmtClock(3661), "61:01");
});

test("a playing track advances from the wall clock", () => {
  const r = progressAt(playing(), T0, T0 + 4000);
  assert.equal(r.visible, true);
  assert.equal(r.hasBar, true);
  assert.equal(r.pos, 34);
  assert.equal(r.pct, 17);
  assert.equal(r.label, "0:34 / 3:20");
});

test("drift is capped so a stalled collector cannot run the clock away", () => {
  // This is the "the time just adds up" bug: with no cap, a sample that stops
  // being refreshed keeps counting until it hits the track length.
  const tenMinutesLater = T0 + 10 * 60 * 1000;
  const r = progressAt(playing(), T0, tenMinutesLater);
  assert.equal(
    r.pos,
    30 + POLL_GRACE_SECS,
    "position freezes one grace window past the last real sample",
  );
  assert.ok(r.pos < 200, "and nowhere near the end of the track");
});

test("a paused track does not advance at all", () => {
  const r = progressAt(playing({ status: "paused" }), T0, T0 + 60_000);
  assert.equal(r.pos, 30);
});

test("a clock that went backwards does not rewind the bar", () => {
  // NTP steps and suspend/resume can move Date.now() behind the anchor.
  const r = progressAt(playing(), T0, T0 - 5000);
  assert.equal(r.pos, 30);
});

test("players with no usable position show nothing", () => {
  // Chromium-based browsers report a constant 0 rather than erroring.
  const r = progressAt(playing({ position_known: false }), T0, T0 + 1000);
  assert.equal(r.visible, false);
  assert.equal(r.label, "");
});

test("a known elapsed with no duration shows the time but no bar", () => {
  // Browser media sessions publish no mpris:length; the elapsed time is
  // reconstructed from the session clock, so it is still worth showing.
  const r = progressAt(playing({ duration_secs: 0, position_secs: 95 }), T0, T0 + 3000);
  assert.equal(r.visible, true);
  assert.equal(r.hasBar, false, "nothing to draw a bar against");
  assert.equal(r.label, "1:38", "elapsed only, no total");
  assert.equal(r.pct, 0);
  // Missing entirely behaves the same as zero.
  assert.equal(progressAt(playing({ duration_secs: undefined }), T0, T0).hasBar, false);
});

test("an unbounded elapsed is not clamped to anything", () => {
  // With no duration there is no ceiling; a long track must keep counting.
  const r = progressAt(playing({ duration_secs: 0, position_secs: 4000 }), T0, T0);
  assert.equal(r.pos, 4000);
  assert.equal(r.label, "66:40");
});

test("position never overshoots the duration", () => {
  const r = progressAt(playing({ position_secs: 199 }), T0, T0 + 30_000);
  assert.equal(r.pos, 200);
  assert.equal(r.pct, 100);
});

test("a negative position from the player is clamped to zero", () => {
  const r = progressAt(playing({ position_secs: -3, status: "paused" }), T0, T0);
  assert.equal(r.pos, 0);
});

test("no sample at all is simply hidden", () => {
  assert.equal(progressAt(null, T0, T0).visible, false);
  assert.equal(progressAt(undefined, T0, T0).visible, false);
});
