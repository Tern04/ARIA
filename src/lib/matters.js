// What matters now: one ranked line across every widget.
//
// The header used to carry six connection dots. They were green all day and
// told you nothing you would act on, while the things you *would* act on —
// a class in twenty minutes, a review waiting on you — were scattered across
// nine widgets, each of which had to be found and read. This turns that
// around: the widgets keep the detail, and the header carries whichever item
// is most worth knowing about right now.
//
// Pure over already-digested inputs, so the ranking is testable without a
// timetable server or a mail account. The caller does the digesting it
// already does for other reasons (`automationAgenda`, `anomalies`) and owns
// the cycling; this module only decides what is worth saying and in what
// order.
//
// The rules, in the order they are applied:
//
//   1. A source that is *down* outranks its own content, because the content
//      is missing rather than empty. A dark MAIL widget reading "0 unread"
//      is a lie the dots used to catch, and this is where that job went.
//   2. Anything with a clock on it, soonest first. A class you can still
//      walk to is worth more than a PR that will wait.
//   3. Anything unusual — the baselines from insights.js. Not "new", which
//      is merely traffic, but "not how this machine normally behaves".
//   4. Anything asked *of you*: a review request, an unread message.
//   5. Anything you asked of yourself: a note that is due.
//   6. What this session actually looks like — how long you have been in one
//      app, how long since you last stopped. No widget shows either: they all
//      report totals for the day, and a four-hour total says nothing about
//      whether you have stood up in the last four hours.
//   7. The shape of the day: what you are free until, or what is next. The
//      timetable widget draws a grid; nobody reads a grid to find out that
//      they are free until nine.
//
// Bands 6 and 7 are why the strip is worth its slot. Bands 1-5 are things
// the widgets already know, ranked; these are things none of them say, and
// band 7 always has an answer — so the strip has a floor and is never a
// blank space where information used to be.
//
// Ties inside a band keep source order, so the strip does not reshuffle
// between ticks when nothing has changed.

/** How far ahead an appointment starts being worth a line. */
export const LOOKAHEAD_MS = 60 * 60_000;
/** Most items the strip will cycle through, worst first. */
export const MAX_ITEMS = 5;

const BAND = { offline: 0, clock: 1, unusual: 2, asked: 3, self: 4, session: 5, shape: 6 };

/** Below this a focus session is noise, not a session. */
export const FOCUS_MIN_MS = 20 * 60_000;
/** Time at the machine without a break worth mentioning, then worth a nudge. */
export const BREAK_NUDGE_MS = 90 * 60_000;
export const BREAK_WARN_MS = 150 * 60_000;
/**
 * Away this long is a break. Anything shorter is the same sitting: answering
 * the door does not rest your eyes, and a rule that reset on every two-minute
 * gap would never let the counter reach the ninety minutes it exists to
 * report.
 */
export const BREAK_MIN_MS = 5 * 60_000;
/**
 * No word from the tracker for this long means the machine was asleep or ARIA
 * was not running. Either way nobody was sitting here, so it ends the sitting
 * the same way a break does.
 */
export const SESSION_GAP_MS = BREAK_MIN_MS;
/** A repo untouched this long is worth a word. */
export const STALE_PUSH_MS = 2 * 24 * 3600_000;

/**
 * Fold one `activity` tick into the running session.
 *
 * The tracker answers a narrower question than this one: it says whether the
 * machine is *in use right now* ("input", "media"), not in use ("away"), or
 * on a session that cannot tell ("unknown"). Turning that into "how long
 * since your last break" is this function's job, and it is where two wrong
 * answers get fixed:
 *
 *   - **A pause is not a break.** Use stopping for two minutes used to reset
 *     the sitting outright, so a real four-hour session reported as four
 *     short ones and the nudge never came.
 *   - **Unknown is not zero.** Where nothing can report idleness, the old
 *     rule read "never idle" and the counter simply grew from whenever ARIA
 *     started — a machine merely left switched on claimed to have been worked
 *     at all night. Now the sitting is not claimed at all, and the header
 *     says nothing rather than something false.
 *
 * `state` is `{ activeSince, awaySince, at }` and is returned, not mutated.
 * `activeSince` of 0 means "no sitting to report".
 */
export function noteSession(state, presence, now) {
  const prev = state ?? { activeSince: 0, awaySince: 0, at: 0 };
  const using = presence === "input" || presence === "media";
  // Nothing knowable, or a gap where the clock ran without us: start over.
  if (presence === "unknown") return { activeSince: 0, awaySince: 0, at: now };
  if (prev.at && now - prev.at >= SESSION_GAP_MS) {
    return { activeSince: using ? now : 0, awaySince: using ? 0 : now, at: now };
  }
  if (using) return { activeSince: prev.activeSince || now, awaySince: 0, at: now };
  const awaySince = prev.awaySince || now;
  // The gap counts as part of the sitting until it is long enough to be a
  // break; at that point the sitting is over and the next tick of use starts
  // a fresh one.
  const broken = now - awaySince >= BREAK_MIN_MS;
  return { activeSince: broken ? 0 : prev.activeSince, awaySince, at: now };
}

/**
 * `sources` carries what the caller already has:
 *   offline  [{ widget, label }]      sources reporting not-connected
 *   agenda   [{ title, start, end }]  from automationAgenda()
 *   insights [{ id, level, text }]    from anomalies()
 *   github   the GITHUB payload
 *   email    the MAIL payload
 *   notes    the parsed note list
 *   discord  the DISCORD payload
 *   session  { app, since, activeSince } — the live focus, from `activity`
 *   classes  the STAG timetable entries, for "what is next"
 */
export function whatMatters(sources = {}, now = Date.now()) {
  const items = [
    ...offlineItems(sources.offline),
    ...agendaItems(sources.agenda, now),
    ...insightItems(sources.insights),
    ...githubItems(sources.github),
    ...mailItems(sources.email),
    ...noteItems(sources.notes, now),
    ...discordItems(sources.discord),
    ...staleItems(sources.github, now),
    ...sessionItems(sources.session, now),
    ...shapeItems(sources.agenda, sources.classes, now),
  ];
  // A stable sort by band only — never by text — so an item keeps its place
  // while its number changes, and the strip does not jump under the cursor.
  return items
    .map((item, i) => ({ item, i }))
    .sort((a, b) => a.item.band - b.item.band || a.i - b.i)
    .map(({ item }) => item)
    .slice(0, MAX_ITEMS);
}

const item = (band, widget, text, level) => ({ band, widget, text, level });

function offlineItems(offline) {
  return (offline ?? []).map((s) =>
    item(BAND.offline, s.widget, `${s.label} not connected`, "alert"),
  );
}

/**
 * The next thing on the clock. Something already running is reported as
 * "now" rather than skipped: walking in late is exactly when you want to be
 * told, and it stops the strip going quiet during the hour it matters most.
 */
function agendaItems(agenda, now) {
  return (agenda ?? [])
    .filter((e) => Number.isFinite(e.start) && now < e.end && e.start - now <= LOOKAHEAD_MS)
    .sort((a, b) => a.start - b.start)
    .map((e) => {
      const mins = Math.ceil((e.start - now) / 60_000);
      const when = mins > 0 ? `in ${mins} min` : "now";
      return item(BAND.clock, "stag", `${e.title} ${when}`, mins > 0 && mins <= 10 ? "warn" : "info");
    });
}

/** The baselines already decided what is unusual; keep their wording. */
function insightItems(insights) {
  return (insights ?? []).map((f) =>
    item(BAND.unusual, "trends", f.text, f.level === "alert" ? "alert" : "warn"),
  );
}

/**
 * A review request is someone waiting on you, which a notification count is
 * not, so it gets its own line and the count is only a fallback.
 */
function githubItems(github) {
  if (github?.status !== "connected") return [];
  const out = [];
  const reviews = (github.notification_items ?? []).filter((n) => n.reason === "review_requested");
  for (const r of reviews.slice(0, 2)) {
    out.push(item(BAND.asked, "github", `review requested on ${r.repo}`, "warn"));
  }
  const rest = (github.notifications ?? 0) - reviews.length;
  if (rest > 0) out.push(item(BAND.asked, "github", `${rest} ${plural(rest, "notification")}`, "info"));
  return out;
}

/**
 * Per account, because "7 unread" across three mailboxes hides which one is
 * the university. An account that failed to poll reports `null`, which is
 * not zero and is reported as its own line.
 */
function mailItems(email) {
  if (email?.status !== "connected") return [];
  const out = [];
  for (const acct of email.accounts ?? []) {
    if (acct.unread == null) {
      out.push(item(BAND.offline, "email", `${acct.label} unreachable`, "alert"));
    } else if (acct.unread > 0) {
      out.push(item(BAND.asked, "email", `${acct.unread} unread from ${acct.label}`, "info"));
    }
  }
  return out;
}

/** Overdue outranks merely due; neither outranks a class you are late for. */
function noteItems(notes, now) {
  return (notes ?? [])
    .filter((n) => !n.done && n.due != null && n.due - now <= LOOKAHEAD_MS)
    .sort((a, b) => a.due - b.due)
    .slice(0, 2)
    .map((n) =>
      n.due <= now
        ? item(BAND.self, "notes", `${n.text} — overdue`, "warn")
        : item(BAND.self, "notes", `${n.text} in ${Math.ceil((n.due - now) / 60_000)} min`, "info"),
    );
}

/**
 * Who is in voice, which is the one Discord fact worth acting on. Channels
 * arrive per server, so they are flattened before the busiest is picked —
 * the collector sorts within a server, not across them.
 */
function discordItems(discord) {
  if (discord?.status !== "connected") return [];
  const busiest = (discord.servers ?? [])
    .flatMap((srv) => srv.voice ?? [])
    .filter((c) => (c.occupants?.length ?? 0) > 0)
    .sort((a, b) => b.occupants.length - a.occupants.length)[0];
  if (!busiest) return [];
  const n = busiest.occupants.length;
  return [item(BAND.self, "discord", `${n} in ${busiest.name}`, "info")];
}

/**
 * A repo you have not pushed to in a while. The GITHUB widget names the
 * last-pushed repo but not *when*, and "3 days ago" is the half that tells
 * you whether you have dropped something.
 */
function staleItems(github, now) {
  if (github?.status !== "connected") return [];
  const recent = (github.recent_repos ?? [])[0];
  const at = Date.parse(recent?.pushed_at ?? "");
  if (!Number.isFinite(at) || now - at < STALE_PUSH_MS) return [];
  return [item(BAND.session, "github", `nothing pushed to ${recent.name} in ${fmtSpan(now - at)}`, "info")];
}

/**
 * What this session looks like, which is the thing no widget can say. SCREEN
 * TIME reports the day's total; that a total of four hours was one unbroken
 * sitting, or that the last hour of it went to one game, is nowhere on the
 * board.
 *
 *   session.app        the focused app, and `since` when it took focus
 *   session.activeSince  when the current sitting began — see `noteSession`.
 *                        Undefined when there is no sitting to report, which
 *                        includes every session that cannot detect a break.
 */
function sessionItems(session, now) {
  const out = [];
  const app = session?.app;
  const held = app && Number.isFinite(session.since) ? now - session.since : 0;
  if (app && held >= FOCUS_MIN_MS) {
    out.push(item(BAND.session, "screentime", `${fmtSpan(held)} in ${app}`, "info"));
  }
  const awake = Number.isFinite(session?.activeSince) ? now - session.activeSince : 0;
  if (awake >= BREAK_NUDGE_MS) {
    out.push(
      item(
        BAND.session,
        "screentime",
        `${fmtSpan(awake)} without a break`,
        awake >= BREAK_WARN_MS ? "warn" : "info",
      ),
    );
  }
  return out;
}

/**
 * The floor: what the rest of the day looks like. Always has an answer, so
 * the strip always has something true to say rather than going blank.
 *
 * "Free until" counts anything still to come today that the clock band has
 * not already picked up; past the last one it looks to the timetable for the
 * next class on any day, which is the question a weekly grid is worst at
 * answering.
 */
function shapeItems(agenda, classes, now) {
  const later = (agenda ?? [])
    .filter((e) => Number.isFinite(e.start) && e.start - now > LOOKAHEAD_MS)
    .sort((a, b) => a.start - b.start)[0];
  if (later) {
    return [item(BAND.shape, "calendar", `free until ${clock(later.start)} · ${later.title}`, "info")];
  }
  const next = nextOnTimetable(classes, now);
  if (next) {
    const days = dayGap(now, next.start);
    const when = days === 0 ? clock(next.start) : days === 1 ? `tomorrow ${clock(next.start)}` : weekdayAt(next.start);
    return [item(BAND.shape, "stag", `next: ${next.subject} ${when}`, "info")];
  }
  return [item(BAND.shape, "calendar", "nothing else scheduled", "info")];
}

/**
 * The next class on a weekly timetable, searched forward a week from now.
 * `classes` is the STAG entries: `day` is 0 = Monday, `time` is "HH:MM–HH:MM".
 * A timetable repeats, so "next" means the next occurrence, not the next row.
 */
export function nextOnTimetable(classes, now) {
  const list = classes ?? [];
  if (!list.length) return null;
  const today = new Date(now);
  const weekday = (today.getDay() + 6) % 7;
  let best = null;
  for (let ahead = 0; ahead <= 7; ahead++) {
    const day = (weekday + ahead) % 7;
    for (const c of list) {
      if (c.day !== day) continue;
      const m = /^(\d{1,2}):(\d{2})/.exec(String(c.time ?? "").trim());
      if (!m) continue;
      const d = new Date(today.getFullYear(), today.getMonth(), today.getDate() + ahead, +m[1], +m[2]);
      const start = d.getTime();
      if (start <= now) continue;
      if (!best || start < best.start) best = { subject: c.subject, start };
    }
    if (best) return best;
  }
  return null;
}

/** Whole days between two instants by calendar date, not by 24-hour blocks. */
function dayGap(now, then) {
  const a = new Date(now);
  const b = new Date(then);
  const day = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  return Math.round((day(b) - day(a)) / 86_400_000);
}

const clock = (t) =>
  new Date(t).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });

const weekdayAt = (t) =>
  `${new Date(t).toLocaleDateString("en-GB", { weekday: "short" })} ${clock(t)}`;

/** "3 days", "2h 10m", "45m" — a span, not a clock time. */
export function fmtSpan(ms) {
  const mins = Math.max(0, Math.round(ms / 60_000));
  if (mins >= 1440) {
    const days = Math.floor(mins / 1440);
    return `${days} ${plural(days, "day")}`;
  }
  const h = Math.floor(mins / 60);
  return h ? `${h}h ${String(mins % 60).padStart(2, "0")}m` : `${mins}m`;
}

function plural(n, word) {
  return n === 1 ? word : `${word}s`;
}
