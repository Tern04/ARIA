import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ENTER_MS,
  EXIT_MS,
  automationAgenda,
  initialState,
  isGameApp,
  overridden,
  parseAutomation,
  sanitizeTrigger,
  step,
  triggerReason,
} from "../src/lib/automation.js";

const order = ["Gaming", "Coding", "Study"];
const cfg = (rules, enabled = true) => ({ enabled, rules });
const GAME = "Steam_app_730";

/** Run the engine over a list of [ms, signals] ticks, collecting actions. */
function run(settings, ticks, state = initialState(null, 0)) {
  const actions = [];
  for (const [now, signals] of ticks) {
    const out = step(state, settings, order, { app: null, gpu: null, agenda: [], ...signals }, now);
    state = out.state;
    if (out.action) actions.push({ at: now, ...out.action });
  }
  return { state, actions };
}

/** Ticks every 5 s from `from` to `to` inclusive with the same signals. */
const span = (from, to, signals) => {
  const out = [];
  for (let t = from; t <= to; t += 5_000) out.push([t, signals]);
  return out;
};

test("steam game windows are recognised, other apps are not", () => {
  assert.ok(isGameApp("Steam_app_730"));
  assert.ok(isGameApp("steam_app_1091500"));
  assert.ok(!isGameApp("Steam"));
  assert.ok(!isGameApp("steam_app_"));
  assert.ok(!isGameApp(null));
});

test("an unwritten or malformed store parses to no rules, not to defaults", () => {
  // Shipped rules arrive with their shipped preset (see presets.js), so they
  // must not be defaulted in here as well — that would resurrect a trigger the
  // user cleared.
  assert.deepEqual(parseAutomation(null), { enabled: true, rules: {} });
  assert.deepEqual(parseAutomation("garbage"), { enabled: true, rules: {} });
  assert.deepEqual(parseAutomation('{"enabled":true,"rules":{}}'), { enabled: true, rules: {} });
  assert.equal(parseAutomation('{"enabled":false,"rules":{}}').enabled, false);
});

test("triggers are cleaned field by field", () => {
  assert.equal(sanitizeTrigger({ kind: "nope" }), null);
  assert.equal(sanitizeTrigger(null), null);
  assert.deepEqual(sanitizeTrigger({ kind: "game", evil: 1 }), { kind: "game" });
  assert.deepEqual(sanitizeTrigger({ kind: "app", apps: " code , firefox,,code" }), {
    kind: "app",
    apps: ["code", "firefox"],
  });
  assert.deepEqual(sanitizeTrigger({ kind: "class", lead: 7 }), { kind: "class", lead: 15 });
  assert.deepEqual(sanitizeTrigger({ kind: "class", lead: 30 }), { kind: "class", lead: 30 });
});

test("app triggers match case-insensitively on part of the name", () => {
  const t = { kind: "app", apps: ["code"] };
  assert.equal(triggerReason(t, { app: "Code" }, 0), "Code in focus");
  assert.equal(triggerReason(t, { app: "Code-oss" }, 0), "Code-oss in focus");
  assert.equal(triggerReason(t, { app: "Firefox" }, 0), null);
  assert.equal(triggerReason({ kind: "app", apps: [] }, { app: "Code" }, 0), null);
});

test("a class trigger holds from the lead time until the class ends", () => {
  const t = { kind: "class", lead: 15 };
  const agenda = [{ title: "KIV/PT", start: 60 * 60_000, end: 150 * 60_000 }];
  assert.equal(triggerReason(t, { agenda }, 44 * 60_000), null);
  assert.equal(triggerReason(t, { agenda }, 45 * 60_000), "KIV/PT in 15 min");
  assert.equal(triggerReason(t, { agenda }, 90 * 60_000), "KIV/PT now");
  assert.equal(triggerReason(t, { agenda }, 150 * 60_000), null);
});

test("a game engages only after it has held for the enter delay", () => {
  const { actions } = run(cfg({ Gaming: { kind: "game" } }), span(0, 20_000, { app: GAME }));
  assert.equal(actions.length, 1);
  assert.equal(actions[0].type, "apply");
  assert.equal(actions[0].preset, "Gaming");
  assert.equal(actions[0].at, ENTER_MS.game);
});

test("a launcher flashing a game window does not switch the board", () => {
  const ticks = [[0, { app: GAME }], [2_000, { app: "Steam" }], ...span(5_000, 30_000, { app: "Steam" })];
  assert.deepEqual(run(cfg({ Gaming: { kind: "game" } }), ticks).actions, []);
});

test("alt-tabbing out of a game briefly keeps the board", () => {
  const ticks = [
    ...span(0, 60_000, { app: GAME }),
    ...span(65_000, 100_000, { app: "Discord" }), // 40 s away, under EXIT_MS
    ...span(105_000, 200_000, { app: GAME }),
  ];
  const { actions } = run(cfg({ Gaming: { kind: "game" } }), ticks);
  assert.deepEqual(actions.map((a) => a.type), ["apply"]);
});

test("the board is restored once the trigger has been gone for the exit delay", () => {
  const ticks = [...span(0, 30_000, { app: GAME }), ...span(35_000, 200_000, { app: "Firefox" })];
  const { actions, state } = run(cfg({ Gaming: { kind: "game" } }), ticks);
  assert.deepEqual(actions.map((a) => a.type), ["apply", "restore"]);
  assert.equal(actions[1].at, 30_000 + EXIT_MS);
  assert.equal(state.active, null);
});

test("a higher-ranked rule takes over; a lower-ranked one waits", () => {
  const rules = cfg({ Gaming: { kind: "game" }, Study: { kind: "app", apps: ["okular"] } });
  // Study first, then a game: Gaming outranks it and takes over.
  let { actions } = run(rules, [...span(0, 20_000, { app: "Okular" }), ...span(25_000, 50_000, { app: GAME })]);
  assert.deepEqual(actions.map((a) => a.preset ?? a.type), ["Study", "Gaming"]);
  // Game first, then Study: the game's rule is still held, so nothing moves.
  ({ actions } = run(rules, [...span(0, 20_000, { app: GAME }), ...span(25_000, 50_000, { app: "Okular" })]));
  assert.deepEqual(actions.map((a) => a.preset ?? a.type), ["Gaming"]);
});

test("a lower-ranked rule inherits the board without flashing the old one", () => {
  const rules = cfg({ Gaming: { kind: "game" }, Study: { kind: "app", apps: ["okular"] } });
  const ticks = [...span(0, 20_000, { app: GAME }), ...span(25_000, 200_000, { app: "Okular" })];
  const { actions } = run(rules, ticks);
  assert.deepEqual(actions.map((a) => a.preset ?? a.type), ["Gaming", "Study"]);
});

test("taking the board back by hand keeps automation out until the trigger clears", () => {
  const rules = cfg({ Gaming: { kind: "game" } });
  let { state } = run(rules, span(0, 20_000, { app: GAME }));
  assert.equal(state.active, "Gaming");
  state = overridden(state);
  assert.equal(state.active, null);
  // Still in the game: no re-apply.
  let out = run(rules, span(25_000, 120_000, { app: GAME }), state);
  assert.deepEqual(out.actions, []);
  // Leave long enough to clear, then come back: it engages again.
  out = run(rules, [...span(125_000, 200_000, { app: "Firefox" }), ...span(205_000, 230_000, { app: GAME })], out.state);
  assert.deepEqual(out.actions.map((a) => a.type), ["apply"]);
});

test("disabling automation or deleting the preset hands the board back", () => {
  const rules = { Gaming: { kind: "game" } };
  let { state } = run(cfg(rules), span(0, 20_000, { app: GAME }));
  let out = step(state, cfg(rules, false), order, { app: GAME, agenda: [] }, 25_000);
  assert.equal(out.action?.type, "restore");
  out = step(state, cfg(rules), ["Coding"], { app: GAME, agenda: [] }, 25_000);
  assert.equal(out.action?.type, "restore");
});

test("a rule restored from a previous run gets the full exit grace", () => {
  const state = initialState({ active: "Gaming", suppressed: [] }, 1_000_000);
  const { actions } = run(cfg({ Gaming: { kind: "game" } }), span(1_000_000, 1_000_000 + EXIT_MS - 5_000, {}), state);
  assert.deepEqual(actions, []);
});

test("the agenda places today's classes and skips all-day events", () => {
  // Wednesday 16 Sep 2026, 09:00 local.
  const now = new Date(2026, 8, 16, 9, 0).getTime();
  const stag = {
    status: "connected",
    timetable: {
      classes: [
        { day: 2, time: "10:15–11:55", subject: "KIV/PT" },
        { day: 3, time: "10:15–11:55", subject: "Thursday" },
        { day: 2, time: "garbage", subject: "Bad" },
      ],
    },
  };
  const calendar = {
    status: "connected",
    events: [
      { title: "Dentist", all_day: false, start: new Date(2026, 8, 16, 14, 0).getTime() / 1000 },
      { title: "Holiday", all_day: true, start: now / 1000 },
    ],
  };
  const agenda = automationAgenda(stag, calendar, now);
  assert.deepEqual(agenda.map((e) => e.title), ["KIV/PT", "Dentist"]);
  assert.equal(agenda[0].start, new Date(2026, 8, 16, 10, 15).getTime());
  assert.equal(agenda[0].end, new Date(2026, 8, 16, 11, 55).getTime());
  assert.equal(agenda[1].end - agenda[1].start, 3_600_000);
  assert.deepEqual(automationAgenda({ status: "disconnected" }, null, now), []);
});
