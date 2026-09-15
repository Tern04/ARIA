// Auto-switching: presets that apply themselves when something happens — a
// game takes focus, a class is about to start — and hand the board back when
// it stops.
//
// Pure functions over plain data, like presets.js, because the difficulty is
// all in the timing rather than the DOM. A board that flips the moment you
// alt-tab to Discord mid-match, or that fights you after you rearranged it by
// hand, is worse than no automation at all. So:
//
//   - a trigger must hold for a moment before it engages (`ENTER_MS`), and
//     must be gone for a while before it lets go (`EXIT_MS`);
//   - rules are ranked by their preset's position in the preset list, and a
//     higher-ranked one takes over from a lower-ranked one, never the reverse;
//   - when the last rule lets go, the board the user had before automation
//     took over comes back — not some fixed default;
//   - touching the board by hand while a rule is in charge ends automation for
//     that rule until its trigger has gone away and come back.

/** Trigger kinds, in the order the editor offers them. */
export const TRIGGER_KINDS = ["game", "app", "gpu", "class"];

/** How long a trigger must hold before it engages, per kind. */
export const ENTER_MS = {
  // A game window is unambiguous, but launchers flash one briefly.
  game: 5_000,
  // Deliberately slower: an app you pass through on the way elsewhere is not
  // an activity.
  app: 10_000,
  // A load spike is not a session — shader compilation, a video export frame.
  gpu: 30_000,
  // The lead time is already the delay.
  class: 0,
};

/** How long a trigger must be gone before its rule lets go of the board. */
export const EXIT_MS = 60_000;

export const GPU_BUSY = 80;
export const LEAD_CHOICES = [5, 10, 15, 30];
export const DEFAULT_LEAD = 15;
const MAX_APPS = 8;
const MAX_APP_LEN = 40;

/** Proton and Steam-launched games carry a `steam_app_<id>` window class. */
export function isGameApp(app) {
  return /^steam_app_\d+$/i.test(String(app ?? ""));
}

/**
 * A trigger from untrusted storage or the editor, or null when nothing usable
 * is left. Only the fields its kind uses survive.
 */
export function sanitizeTrigger(raw) {
  if (!raw || !TRIGGER_KINDS.includes(raw.kind)) return null;
  switch (raw.kind) {
    case "app": {
      const list = Array.isArray(raw.apps) ? raw.apps : String(raw.apps ?? "").split(",");
      const apps = [
        ...new Set(list.map((a) => String(a ?? "").trim().slice(0, MAX_APP_LEN)).filter(Boolean)),
      ].slice(0, MAX_APPS);
      // An app rule with no apps can never fire; keep it rather than drop it,
      // so the editor does not lose the kind while the user is still typing.
      return { kind: "app", apps };
    }
    case "class": {
      const lead = Number(raw.lead);
      return { kind: "class", lead: LEAD_CHOICES.includes(lead) ? lead : DEFAULT_LEAD };
    }
    default:
      return { kind: raw.kind };
  }
}

/**
 * The stored automation settings. Shipped rules are not defaulted in here —
 * they ride along with the shipped presets (see applyBuiltins), so a trigger
 * the user changed or cleared is never written back over.
 */
export function parseAutomation(raw) {
  let data = null;
  try {
    data = JSON.parse(raw);
  } catch {}
  if (!data || typeof data !== "object" || Array.isArray(data)) {
    return { enabled: true, rules: {} };
  }
  const rules = {};
  for (const [name, trigger] of Object.entries(data.rules ?? {})) {
    const clean = sanitizeTrigger(trigger);
    if (clean) rules[name] = clean;
  }
  return { enabled: data.enabled !== false, rules };
}

/**
 * Why a trigger holds right now, as a short human reason, or null when it
 * does not.
 *
 * `signals`:
 *   app     name of the focused app (never ARIA itself), or null
 *   gpu     GPU load in percent, or null when unknown or stale
 *   agenda  [{ title, start, end }] in epoch ms
 */
export function triggerReason(trigger, signals, now) {
  switch (trigger?.kind) {
    case "game":
      return isGameApp(signals.app) ? `${signals.app} in focus` : null;
    case "app": {
      const app = String(signals.app ?? "").toLowerCase();
      if (!app) return null;
      const hit = trigger.apps.some((a) => app.includes(a.toLowerCase()));
      return hit ? `${signals.app} in focus` : null;
    }
    case "gpu":
      return signals.gpu != null && signals.gpu >= GPU_BUSY
        ? `GPU at ${Math.round(signals.gpu)}%`
        : null;
    case "class": {
      const lead = trigger.lead * 60_000;
      const hit = (signals.agenda ?? []).find((e) => e.start - lead <= now && now < e.end);
      if (!hit) return null;
      const mins = Math.ceil((hit.start - now) / 60_000);
      return mins > 0 ? `${hit.title} in ${mins} min` : `${hit.title} now`;
    }
    default:
      return null;
  }
}

/** Fresh engine state. `active` and `suppressed` are what gets persisted. */
export function initialState(saved, now) {
  const active = typeof saved?.active === "string" ? saved.active : null;
  const suppressed = Array.isArray(saved?.suppressed)
    ? saved.suppressed.filter((n) => typeof n === "string")
    : [];
  return {
    active,
    reason: null,
    suppressed,
    since: {},
    // A rule restored from a previous run gets the whole exit grace, rather
    // than letting go on the first tick before its signals have arrived.
    lastSeen: active ? { [active]: now } : {},
  };
}

/**
 * Advance the engine by one tick.
 *
 * `order` is the preset names in priority order (the preset list); rules for
 * names not in it are ignored, so deleting a preset retires its rule.
 *
 * Returns `{ state, action }`, where action is null, `{ type: "apply", preset,
 * reason }` or `{ type: "restore" }`. The caller owns the board: it snapshots
 * the layout before the first apply and puts it back on restore.
 */
export function step(prev, { enabled, rules }, order, signals, now) {
  const state = {
    ...prev,
    since: { ...prev.since },
    lastSeen: { ...prev.lastSeen },
    suppressed: [...prev.suppressed],
  };
  const ranked = order.filter((name) => rules[name]);
  const reasons = {};

  for (const name of ranked) {
    const reason = enabled ? triggerReason(rules[name], signals, now) : null;
    if (reason) {
      state.since[name] ??= now;
      state.lastSeen[name] = now;
      reasons[name] = reason;
    } else {
      delete state.since[name];
    }
  }
  // Forget timing for rules that no longer exist.
  for (const name of Object.keys(state.since)) if (!rules[name]) delete state.since[name];

  const released = (name) =>
    !rules[name] || !ranked.includes(name) || !enabled ||
    (!reasons[name] && now - (state.lastSeen[name] ?? -Infinity) >= EXIT_MS);

  // A suppressed rule is eligible again once its trigger has fully gone away.
  state.suppressed = state.suppressed.filter((name) => !released(name));

  const engaged = ranked.find(
    (name) =>
      reasons[name] &&
      now - state.since[name] >= ENTER_MS[rules[name].kind] &&
      !state.suppressed.includes(name),
  );

  if (state.active) {
    const activeRank = ranked.indexOf(state.active);
    const stillHeld = !released(state.active);
    if (reasons[state.active]) state.reason = reasons[state.active];
    if (engaged && engaged !== state.active && (!stillHeld || ranked.indexOf(engaged) < activeRank)) {
      state.active = engaged;
      state.reason = reasons[engaged];
      return { state, action: { type: "apply", preset: engaged, reason: state.reason } };
    }
    if (stillHeld) return { state, action: null };
    // Another rule is holding but not engaged yet: keep the board until it
    // is, rather than flashing the old board up for a few seconds between.
    if (ranked.some((name) => reasons[name] && !state.suppressed.includes(name))) {
      return { state, action: null };
    }
    state.active = null;
    state.reason = null;
    return { state, action: { type: "restore" } };
  }

  if (engaged) {
    state.active = engaged;
    state.reason = reasons[engaged];
    return { state, action: { type: "apply", preset: engaged, reason: state.reason } };
  }
  return { state, action: null };
}

/**
 * The user took the board back while a rule was in charge. That rule stays
 * out until its trigger has gone away, and the pre-automation snapshot is the
 * caller's to discard — the board the user just made is the one to keep.
 */
export function overridden(prev) {
  if (!prev.active) return prev;
  return {
    ...prev,
    suppressed: [...new Set([...prev.suppressed, prev.active])],
    active: null,
    reason: null,
  };
}

/**
 * Today's timetable and calendar as agenda entries for the `class` trigger.
 *
 * STAG classes are a weekly grid (`day` 0 = Monday, `time` "HH:MM–HH:MM"), so
 * they are placed on the date of `now`. Calendar events carry only a start;
 * an hour is assumed. All-day events never trigger — they are not something
 * you get ready for fifteen minutes ahead.
 */
export function automationAgenda(stag, calendar, now) {
  const out = [];
  const today = new Date(now);
  const weekday = (today.getDay() + 6) % 7;
  if (stag?.status === "connected") {
    for (const c of stag.timetable?.classes ?? []) {
      if (c.day !== weekday) continue;
      const m = /^(\d{1,2}):(\d{2})\s*[–-]\s*(\d{1,2}):(\d{2})$/.exec(String(c.time ?? "").trim());
      if (!m) continue;
      const at = (h, min) =>
        new Date(today.getFullYear(), today.getMonth(), today.getDate(), +h, +min).getTime();
      out.push({ title: c.subject, start: at(m[1], m[2]), end: at(m[3], m[4]) });
    }
  }
  if (calendar?.status === "connected") {
    for (const e of calendar.events ?? []) {
      if (e.all_day || !Number.isFinite(e.start)) continue;
      const start = e.start * 1000;
      out.push({ title: e.title, start, end: start + 3_600_000 });
    }
  }
  return out;
}
