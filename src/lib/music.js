// Pure now-playing helpers, kept out of main.js so they can be unit-tested
// (`npm test`) without a DOM.

/// The backend samples a playing track every 3 s. Advancing the readout from
/// the wall clock bridges that gap, but the drift must be capped: if emits stop
/// (player crash, collector wedged, machine suspended) an uncapped counter runs
/// away and the widget shows a time that is pure fiction. Capped, it simply
/// freezes at the last known-good value until a real sample lands.
export const POLL_GRACE_SECS = 5;

export function fmtClock(secs) {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

/**
 * Where the progress readout should be right now.
 *
 * `hasBar` is false when the elapsed time is known but the total isn't — the
 * case for browser media sessions, which publish no track length. There is
 * nothing to draw a bar against, but a running elapsed time is still true and
 * still useful, so the caller shows the time alone.
 *
 * @param sample   the last `music` payload
 * @param anchorMs Date.now() when that payload arrived
 * @param nowMs    Date.now()
 * @returns {{visible: boolean, hasBar: boolean, pos: number, pct: number, label: string}}
 */
export function progressAt(sample, anchorMs, nowMs) {
  const hidden = { visible: false, hasBar: false, pos: 0, pct: 0, label: "" };
  if (!sample) return hidden;
  // A player that exposes no usable position at all (and none we could
  // reconstruct) — show nothing rather than a fake counter.
  if (!sample.position_known) return hidden;

  const dur = sample.duration_secs > 0 ? sample.duration_secs : 0;
  const drift =
    sample.status === "playing"
      ? Math.min(Math.max((nowMs - anchorMs) / 1000, 0), POLL_GRACE_SECS)
      : 0;
  let pos = Math.max(sample.position_secs, 0) + drift;
  if (dur) pos = Math.min(pos, dur);
  return {
    visible: true,
    hasBar: dur > 0,
    pos,
    pct: dur ? (pos / dur) * 100 : 0,
    label: dur ? `${fmtClock(pos)} / ${fmtClock(dur)}` : fmtClock(pos),
  };
}
