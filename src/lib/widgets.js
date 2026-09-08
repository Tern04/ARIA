// The widget catalogue: what size presets each widget offers, and where it
// sits on a fresh board. Extracted from main.js so the placement can be
// checked by `npm test` without a DOM — a default board that overlaps or
// overflows would otherwise only show up as a scrambled HUD on first run.

/**
 * Size presets, on a strict column rhythm: every preset is 3, 6 or 12 columns
 * wide — a third, a half, or the whole 12-column board. Nothing is 4, 5 or 8
 * wide any more, because those never line up with anything else: a 5-wide
 * widget beside a 3-wide one leaves a 4-column slot that only fits a widget
 * nobody has. Heights stay free; differing heights just stack.
 *
 *   S = [3,1]  one-line headline    M = [3,2]  the standard tile
 *   L = [6,3]  double-wide feature
 *
 * STAG and TERMINAL are the two exceptions, and only on the width they cannot
 * give up: a five-day timetable and an 80-column shell genuinely do not fit in
 * a third of the board. They are still 6 or 12 wide, so they tile with the
 * rest — STAG just takes a taller M, since a week compressed into two rows is
 * unreadable.
 *
 * `home` = [col, row, size] on a fresh board. Those coordinates fill the grid
 * exactly (see the test), so the defaults are a finished board rather than an
 * arbitrary scattering.
 */
export const WIDGETS = {
  // STAG and TERMINAL share a footprint so they sit level side by side, and
  // so an L of either is the top two-thirds of the board with a strip of
  // widgets still visible underneath — a full-board terminal would leave
  // nothing to glance at, which is the whole point of the HUD.
  stag:       { sizes: { s: [3, 2], m: [6, 4], l: [12, 4] }, home: [1, 1, "m"] },
  terminal:   { sizes: { m: [6, 4], l: [12, 4] }, home: [7, 1, "m"] },
  hardware:   { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [1, 5, "m"] },
  music:      { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [4, 5, "m"] },
  // The bottom-right strip is one row tall, which is exactly what S is for:
  // the glanceable headline (unread count, who is in voice, the current
  // price) with the list dropped.
  email:      { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [7, 5, "s"] },
  github:     { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [10, 5, "s"] },
  discord:    { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [7, 6, "s"] },
  crypto:     { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [10, 6, "s"] },
  // The default board is full, so everything below starts in the tray; drag
  // one in (or shrink something) to place it. The four telemetry widgets are
  // deliberately not on the default board: they are a gaming loadout, not
  // everyday chrome, and the "Gaming" preset places all four at once.
  screentime: { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [1, 1, "m"], defaultHidden: true },
  calendar:   { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [1, 1, "m"], defaultHidden: true },
  gpu:        { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [1, 1, "m"], defaultHidden: true },
  thermals:   { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [1, 1, "m"], defaultHidden: true },
  perf:       { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [1, 1, "m"], defaultHidden: true },
  latency:    { sizes: { s: [3, 1], m: [3, 2], l: [6, 3] }, home: [1, 1, "m"], defaultHidden: true },
};

/** A fresh board: every widget at its home cell and size. */
export function defaultLayout() {
  const layout = {};
  for (const [id, w] of Object.entries(WIDGETS)) {
    layout[id] = { c: w.home[0], r: w.home[1], size: w.home[2], hidden: !!w.defaultHidden };
  }
  return layout;
}
