import { test } from "node:test";
import assert from "node:assert/strict";
import {
  BREAK_MIN_MS,
  LOOKAHEAD_MS,
  MAX_ITEMS,
  fmtSpan,
  nextOnTimetable,
  noteSession,
  whatMatters,
} from "../src/lib/matters.js";

const NOW = new Date("2026-09-17T14:00:00").getTime();
const min = (n) => n * 60_000;
const texts = (sources, now = NOW) => whatMatters(sources, now).map((i) => i.text);

test("a quiet moment still gets a true line, not a filler one", () => {
  // The floor: with nothing demanding attention the strip reports the shape
  // of the day rather than going blank or inventing something.
  assert.deepEqual(texts({}), ["nothing else scheduled"]);
  assert.deepEqual(texts(undefined), ["nothing else scheduled"]);
  assert.deepEqual(
    texts({ github: { status: "connected", notifications: 0 } }),
    ["nothing else scheduled"],
    "a connected source with nothing in it adds nothing",
  );
});

test("a source that is down outranks everything it would have reported", () => {
  const sources = {
    offline: [{ widget: "email", label: "MAIL" }],
    agenda: [{ title: "KIV/PT", start: NOW + min(5), end: NOW + min(90) }],
  };
  const [first] = whatMatters(sources, NOW);
  assert.equal(first.text, "MAIL not connected");
  assert.equal(first.level, "alert");
  assert.equal(first.widget, "email", "and it points at the widget that went dark");
});

test("the clock beats everything that will wait", () => {
  const sources = {
    agenda: [{ title: "KIV/PT", start: NOW + min(22), end: NOW + min(90) }],
    insights: [{ id: "gpu-heat", level: "alert", text: "GPU 9°C hotter than usual" }],
    github: {
      status: "connected",
      notifications: 1,
      notification_items: [{ repo: "aria", reason: "review_requested" }],
    },
    notes: [{ text: "call the bank", due: NOW + min(10), done: false }],
  };
  assert.deepEqual(texts(sources), [
    "KIV/PT in 22 min",
    "GPU 9°C hotter than usual",
    "review requested on aria",
    "call the bank in 10 min",
    "nothing else scheduled",
  ]);
});

test("an event already running is reported as now, not dropped", () => {
  const sources = { agenda: [{ title: "KIV/PT", start: NOW - min(10), end: NOW + min(35) }] };
  assert.deepEqual(texts(sources), ["KIV/PT now", "nothing else scheduled"]);
});

test("an event past its end, or beyond the lookahead, is not news yet", () => {
  const over = { agenda: [{ title: "KIV/PT", start: NOW - min(90), end: NOW - min(30) }] };
  assert.deepEqual(texts(over), ["nothing else scheduled"], "an event that is over is over");
  // Beyond the lookahead it is not a demand yet, but it is still the shape of
  // the evening — so the floor reports it where the clock band will not.
  const far = { agenda: [{ title: "KIV/PT", start: NOW + LOOKAHEAD_MS + min(1), end: NOW + min(120) }] };
  assert.deepEqual(texts(far), ["free until 15:01 · KIV/PT"]);
  const edge = { agenda: [{ title: "KIV/PT", start: NOW + LOOKAHEAD_MS, end: NOW + min(120) }] };
  assert.deepEqual(texts(edge), ["KIV/PT in 60 min", "nothing else scheduled"]);
});

test("the soonest appointment leads, and ten minutes out starts warning", () => {
  const sources = {
    agenda: [
      { title: "late", start: NOW + min(40), end: NOW + min(90) },
      { title: "soon", start: NOW + min(8), end: NOW + min(50) },
    ],
  };
  const found = whatMatters(sources, NOW);
  assert.deepEqual(found.map((i) => i.text), [
    "soon in 8 min",
    "late in 40 min",
    "nothing else scheduled",
  ]);
  assert.deepEqual(found.map((i) => i.level), ["warn", "info", "info"]);
});

test("a review request is its own line, and is not double-counted", () => {
  const github = {
    status: "connected",
    notifications: 3,
    notification_items: [
      { repo: "aria", reason: "review_requested" },
      { repo: "aria", reason: "mention" },
    ],
  };
  assert.deepEqual(texts({ github }), [
    "review requested on aria",
    "2 notifications",
    "nothing else scheduled",
  ]);
});

test("one leftover notification is singular, and none leaves no line", () => {
  const one = { status: "connected", notifications: 1, notification_items: [] };
  assert.deepEqual(texts({ github: one }), ["1 notification", "nothing else scheduled"]);
  const exact = {
    status: "connected",
    notifications: 1,
    notification_items: [{ repo: "aria", reason: "review_requested" }],
  };
  assert.deepEqual(texts({ github: exact }), ["review requested on aria", "nothing else scheduled"]);
});

test("a disconnected source is silent rather than reporting zeroes", () => {
  const floor = ["nothing else scheduled"];
  assert.deepEqual(texts({ github: { status: "error", notifications: 9 } }), floor);
  assert.deepEqual(texts({ email: { status: "error", accounts: [{ label: "zcu", unread: 4 }] } }), floor);
  assert.deepEqual(texts({ discord: { status: "error", servers: [] } }), floor);
});

test("mail is counted per account, so you can tell which mailbox it is", () => {
  const email = {
    status: "connected",
    accounts: [
      { label: "zcu.cz", unread: 2 },
      { label: "gmail", unread: 0 },
    ],
  };
  assert.deepEqual(texts({ email }), ["2 unread from zcu.cz", "nothing else scheduled"]);
});

test("an account that failed to poll is not an account with no mail", () => {
  const email = { status: "connected", accounts: [{ label: "zcu.cz", unread: null }] };
  const [first] = whatMatters({ email }, NOW);
  assert.equal(first.text, "zcu.cz unreachable");
  assert.equal(first.level, "alert");
  assert.equal(first.widget, "email");
});

test("a note is due before it is overdue, and a done one is neither", () => {
  const notes = [
    { text: "call the bank", due: NOW - min(5), done: false },
    { text: "stretch", due: NOW + min(20), done: false },
    { text: "already did it", due: NOW - min(60), done: true },
    { text: "no reminder", due: null, done: false },
  ];
  const found = whatMatters({ notes }, NOW);
  assert.deepEqual(found.map((i) => i.text), [
    "call the bank — overdue",
    "stretch in 20 min",
    "nothing else scheduled",
  ]);
  assert.deepEqual(found.map((i) => i.level), ["warn", "info", "info"]);
});

test("voice reports the busiest channel across every server", () => {
  const discord = {
    status: "connected",
    servers: [
      { name: "friends", voice: [{ name: "General", occupants: [{}, {}] }, { name: "AFK", occupants: [] }] },
      { name: "uni", voice: [{ name: "Gaming", occupants: [{}, {}, {}] }] },
    ],
  };
  // The collector sorts within a server, not across them, so a busier
  // channel on the second server still has to win.
  assert.deepEqual(texts({ discord }), ["3 in Gaming", "nothing else scheduled"]);
});

test("an empty voice channel is not someone to join", () => {
  const quiet = { status: "connected", servers: [{ name: "friends", voice: [{ name: "AFK", occupants: [] }] }] };
  assert.deepEqual(texts({ discord: quiet }), ["nothing else scheduled"]);
  assert.deepEqual(texts({ discord: { status: "connected", servers: [] } }), ["nothing else scheduled"]);
});

test("the strip is capped, keeping the most serious", () => {
  const sources = {
    offline: [
      { widget: "email", label: "MAIL" },
      { widget: "github", label: "GITHUB" },
    ],
    agenda: [
      { title: "a", start: NOW + min(5), end: NOW + min(50) },
      { title: "b", start: NOW + min(9), end: NOW + min(50) },
      { title: "c", start: NOW + min(12), end: NOW + min(50) },
    ],
    notes: [{ text: "never seen", due: NOW + min(1), done: false }],
  };
  const found = whatMatters(sources, NOW);
  assert.equal(found.length, MAX_ITEMS);
  assert.equal(found.at(-1).text, "c in 12 min");
  assert.ok(!found.some((i) => i.text.startsWith("never seen")), "the lowest band is what gets cut");
});

test("equal-ranked items keep their order between ticks", () => {
  const sources = {
    email: {
      status: "connected",
      accounts: [
        { label: "zcu.cz", unread: 1 },
        { label: "gmail", unread: 9 },
      ],
    },
  };
  // Ranked by band and source order, never by the count — otherwise the strip
  // reshuffles under the cursor every time a number ticks.
  assert.deepEqual(texts(sources), [
    "1 unread from zcu.cz",
    "9 unread from gmail",
    "nothing else scheduled",
  ]);
});

test("every item names a widget to jump to", () => {
  const sources = {
    agenda: [{ title: "KIV/PT", start: NOW + min(5), end: NOW + min(50) }],
    insights: [{ id: "gpu-heat", level: "warn", text: "GPU warm" }],
    email: { status: "connected", accounts: [{ label: "zcu", unread: 1 }] },
  };
  for (const found of whatMatters(sources, NOW)) {
    assert.ok(found.widget, `"${found.text}" has nowhere to go`);
    assert.ok(["alert", "warn", "info"].includes(found.level), `"${found.text}" has no level`);
  }
});

/* ── the bands no widget covers ──────────────────────────────
   These are the reason the strip earns the slot: the widgets all report the
   day's totals and a weekly grid, and none of them answers "how long have I
   been sitting here" or "am I free tonight". */

test("a session is only a session once it has lasted", () => {
  const short = { session: { app: "Overwatch", since: NOW - min(9) } };
  assert.deepEqual(texts(short), ["nothing else scheduled"], "nine minutes is noise");
  const held = { session: { app: "Overwatch", since: NOW - min(72) } };
  assert.ok(texts(held).includes("1h 12m in Overwatch"));
});

test("time without a break nudges, then warns", () => {
  const quiet = whatMatters({ session: { activeSince: NOW - min(80) } }, NOW);
  assert.ok(!quiet.some((i) => i.text.includes("without a break")), "80 min is not a lecture");
  const nudge = whatMatters({ session: { activeSince: NOW - min(100) } }, NOW);
  const row = nudge.find((i) => i.text.includes("without a break"));
  assert.equal(row.text, "1h 40m without a break");
  assert.equal(row.level, "info");
  const warn = whatMatters({ session: { activeSince: NOW - min(180) } }, NOW);
  assert.equal(warn.find((i) => i.text.includes("without a break")).level, "warn");
});

/* `noteSession` — one `activity` tick at a time. The tracker says what the
   machine is doing; these decide what counts as a sitting and what ends it. */

/** Replay a run of ticks five seconds apart and return the final state. */
const replay = (ticks, start = NOW) => {
  let state = null;
  let at = start;
  for (const [presence, count] of ticks) {
    for (let i = 0; i < count; i++) {
      state = noteSession(state, presence, at);
      at += 5_000;
    }
  }
  return state;
};

test("a sitting starts with the first sign of use", () => {
  const s = noteSession(null, "input", NOW);
  assert.equal(s.activeSince, NOW);
  assert.equal(noteSession(s, "input", NOW + 5_000).activeSince, NOW, "and keeps its start");
});

test("watching something is being here, not being away", () => {
  // The whole reason presence is more than an idle flag: a film plays for two
  // hours without a keypress, and the old rule called every minute of it a
  // break. One hour of input, one of media, is one two-hour sitting.
  const s = replay([["input", 720], ["media", 720]]);
  assert.equal(s.activeSince, NOW, "media continues the sitting it found");
  const held = whatMatters({ session: { activeSince: s.activeSince } }, NOW + min(120));
  assert.equal(held.find((i) => i.text.includes("without a break")).text, "2h 00m without a break");
});

test("a pause is not a break, but leaving is", () => {
  // Four minutes away — the phone, the door — is the same sitting.
  const brief = replay([["input", 240], ["away", 48], ["input", 1]]);
  assert.equal(brief.activeSince, NOW, "a short absence does not restart the clock");

  // Past five minutes it is a break, and the sitting is over.
  const gone = replay([["input", 240], ["away", 61]]);
  assert.equal(gone.activeSince, 0);
  // Coming back starts a new one, timed from the return and not from the leaving.
  const back = noteSession(gone, "input", gone.at + 5_000);
  assert.equal(back.activeSince, gone.at + 5_000);
});

test("a session that cannot see a break never claims there wasn't one", () => {
  // No idle source: the machine being switched on is not evidence of anyone
  // sitting at it, so nothing is reported at all.
  assert.equal(replay([["unknown", 2000]]).activeSince, 0);
  const worked = replay([["input", 600]]);
  assert.equal(noteSession(worked, "unknown", worked.at + 5_000).activeSince, 0);
});

test("a gap in the ticks ends the sitting, because nobody was here for it", () => {
  // Suspend, or ARIA not running. The next tick arrives hours later and the
  // sitting must not swallow the hours in between.
  const before = replay([["input", 600]]);
  const after = noteSession(before, "input", before.at + BREAK_MIN_MS + 1);
  assert.equal(after.activeSince, before.at + BREAK_MIN_MS + 1);
});

test("the strip always has a floor, so it is never blank", () => {
  const [only] = whatMatters({}, NOW);
  assert.equal(only.text, "nothing else scheduled");
  assert.equal(only.level, "info");
});

test("free until the next thing, once it is past the clock band", () => {
  const agenda = [{ title: "band practice", start: NOW + min(180), end: NOW + min(240) }];
  assert.deepEqual(texts({ agenda }), ["free until 17:00 · band practice"]);
});

test("past the last thing today, the answer comes off the timetable", () => {
  // Thursday 14:00. This morning's class is over, so Friday's is next.
  const classes = [
    { day: 3, time: "08:00–09:30", subject: "KIV/PT" },
    { day: 4, time: "09:20–10:50", subject: "KIV/OS" },
  ];
  assert.deepEqual(texts({ classes }), ["next: KIV/OS tomorrow 09:20"]);
});

test("a class later the same day is named by its time alone", () => {
  const classes = [{ day: 3, time: "19:00–20:30", subject: "KIV/PT" }];
  assert.deepEqual(texts({ classes }), ["next: KIV/PT 19:00"]);
});

test("a class further out is named by its weekday", () => {
  const classes = [{ day: 0, time: "08:00–09:30", subject: "KIV/PT" }];
  // Thursday now, so Monday's class is four days away.
  const [only] = whatMatters({ classes }, NOW);
  assert.match(only.text, /^next: KIV\/PT Mon 08:00$/);
});

test("nextOnTimetable finds the next occurrence, not the next row", () => {
  const classes = [
    { day: 3, time: "08:00–09:30", subject: "earlier today" },
    { day: 3, time: "16:00–17:30", subject: "later today" },
  ];
  // 14:00 Thursday: this morning's class is over, so next week's is not the
  // answer either — this afternoon's is.
  assert.equal(nextOnTimetable(classes, NOW).subject, "later today");
  assert.equal(nextOnTimetable([], NOW), null);
  assert.equal(nextOnTimetable(null, NOW), null);
  assert.equal(nextOnTimetable([{ day: 3, time: "nonsense", subject: "x" }], NOW), null);
});

test("a class that has already started today rolls to the next week", () => {
  const classes = [{ day: 3, time: "08:00–09:30", subject: "KIV/PT" }];
  const found = nextOnTimetable(classes, NOW);
  assert.equal(found.subject, "KIV/PT");
  assert.ok(found.start > NOW + 6 * 86_400_000, "a week out, not this morning");
});

test("a repo left alone for days is worth a word; a fresh one is not", () => {
  const fresh = {
    status: "connected",
    recent_repos: [{ name: "aria", pushed_at: new Date(NOW - min(60)).toISOString() }],
  };
  assert.ok(!texts({ github: fresh }).some((t) => t.includes("pushed")));
  const stale = {
    status: "connected",
    recent_repos: [{ name: "aria", pushed_at: new Date(NOW - 3 * 86_400_000).toISOString() }],
  };
  assert.ok(texts({ github: stale }).includes("nothing pushed to aria in 3 days"));
  const junk = { status: "connected", recent_repos: [{ name: "aria", pushed_at: "not a date" }] };
  assert.ok(!texts({ github: junk }).some((t) => t.includes("pushed")));
});

test("spans read as spans, not as clock times", () => {
  assert.equal(fmtSpan(0), "0m");
  assert.equal(fmtSpan(min(45)), "45m");
  assert.equal(fmtSpan(min(130)), "2h 10m");
  assert.equal(fmtSpan(86_400_000), "1 day");
  assert.equal(fmtSpan(3 * 86_400_000), "3 days");
  assert.equal(fmtSpan(-5), "0m");
});

test("what you must act on still outranks what you are merely doing", () => {
  const sources = {
    agenda: [{ title: "KIV/PT", start: NOW + min(15), end: NOW + min(90) }],
    session: { app: "Overwatch", since: NOW - min(90), activeSince: NOW - min(200) },
  };
  const found = whatMatters(sources, NOW);
  assert.equal(found[0].text, "KIV/PT in 15 min", "the class leads");
  assert.ok(found.some((i) => i.text === "1h 30m in Overwatch"), "and the session still gets a line");
});
