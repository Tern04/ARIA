// Rolling telemetry history for the live graphs, and the geometry that turns
// it into a sparkline. Pure functions, so `npm test` can check the awkward
// parts — a half-full buffer, a dropped sample, a flat series — without a DOM.
//
// A series is a plain array of numbers with `null` for "no reading": a lost
// ping, or a GPU that answered on one tick and not the next. Nulls are holes
// in the line, never zeros, because a zero is a claim (the GPU was idle) and a
// hole is the truth (nobody knows).

/** 90 samples at one per 2 s ≈ 3 minutes — a match's worth of scrollback. */
export const HISTORY_LEN = 90;

/** Append one reading, dropping the oldest once the window is full. */
export function pushSample(values, v, len = HISTORY_LEN) {
  values.push(Number.isFinite(v) ? v : null);
  if (values.length > len) values.splice(0, values.length - len);
  return values;
}

/** Highest reading in the window, or null while nothing has arrived. */
export function peak(values) {
  const got = values.filter((v) => v !== null);
  return got.length ? Math.max(...got) : null;
}

/** Most recent reading, or null when the latest tick had none. */
export function latest(values) {
  return values.length ? values[values.length - 1] : null;
}

/**
 * Polyline point strings for a series, one per unbroken run of readings — a
 * gap ends a run rather than being bridged by a straight line across it, which
 * would invent data.
 *
 * The newest sample sits at the right edge and a partly-filled window trails
 * off to the left, so the trace grows leftwards as it fills instead of
 * stretching a two-sample line across the whole widget.
 */
export function seriesPaths(values, opts = {}) {
  const { width = 100, height = 26, min = 0, max = 100, pad = 1, len = HISTORY_LEN } = opts;
  const span = Math.max(1, len - 1);
  const range = max - min || 1;
  const offset = Math.max(0, len - values.length);
  const runs = [];
  let run = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (run.length) runs.push(run);
      run = [];
      return;
    }
    const x = ((i + offset) / span) * width;
    const clamped = Math.max(min, Math.min(max, v));
    const y = pad + (1 - (clamped - min) / range) * (height - pad * 2);
    run.push(`${x.toFixed(1)},${y.toFixed(1)}`);
  });
  if (run.length) runs.push(run);
  // A lone reading has no line to draw; doubling it gives the polyline a
  // visible one-pixel dash instead of nothing at all.
  return runs.map((r) => (r.length === 1 ? `${r[0]} ${r[0]}` : r.join(" ")));
}

/**
 * Scale for a series with no natural ceiling (ping times). Padded by a tenth
 * so the trace does not ride the frame, and never flat: a perfectly steady
 * 12 ms would otherwise divide by a zero range and draw off the top.
 */
export function autoScale(values, { floor = 0 } = {}) {
  const got = values.filter((v) => v !== null);
  if (!got.length) return { min: floor, max: floor + 1 };
  const lo = Math.min(...got);
  const hi = Math.max(...got);
  const padding = Math.max((hi - lo) * 0.1, hi * 0.05, 1);
  return { min: Math.max(floor, lo - padding), max: hi + padding };
}
