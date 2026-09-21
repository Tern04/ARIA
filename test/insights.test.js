import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MIN_DAYS,
  MIN_SAMPLES,
  anomalies,
  coverage,
  cpuHeat,
  fmtHours,
  gpuHeat,
  mergeStats,
  packetLoss,
  pingNow,
  screenTime,
  split,
  trends,
  week,
} from "../src/lib/insights.js";

/* The report comes from history.rs as an array of day aggregates, oldest
   first. These helpers build one by hand: the point of the whole module is
   that a claim like "8 °C hotter at the same load" can be checked without a
   machine that is actually overheating. */

const stat = (avg, n = MIN_SAMPLES, max = avg) => ({ avg, max, n });
const day = (date, fields = {}) => ({
  date,
  samples: 1440,
  gpu_temp_by_load: {},
  ping_by_hour: {},
  screen_apps: [],
  ...fields,
});
/** `n` baseline days all alike, so only the last day varies in a test. */
const baselineOf = (n, fields) =>
  Array.from({ length: n }, (_, i) => day(`2026-09-0${i + 1}`, fields));

test("merging stats weights each by its sample count", () => {
  const merged = mergeStats([stat(10, 1), stat(20, 3, 25)]);
  assert.equal(merged.n, 4);
  assert.equal(merged.avg, 17.5, "not the unweighted 15");
  assert.equal(merged.max, 25);
});

test("merging nothing, or only empty stats, is null rather than zero", () => {
  assert.equal(mergeStats([]), null);
  assert.equal(mergeStats(null), null);
  assert.equal(mergeStats([null, undefined, { avg: 5, max: 5, n: 0 }]), null);
});

test("today is the last day and is never part of its own baseline", () => {
  const days = [day("2026-09-01"), day("2026-09-02"), day("2026-09-03")];
  const { today, baseline } = split(days);
  assert.equal(today.date, "2026-09-03");
  assert.deepEqual(baseline.map((d) => d.date), ["2026-09-01", "2026-09-02"]);
  assert.deepEqual(split([]), { today: null, baseline: [] });
});

/* ── GPU heat: the comparison the whole record exists for ──── */

test("the GPU is compared inside a load bucket, not across the day", () => {
  // Hotter today at 90-100% load, and busier too. Comparing daily averages
  // would only say "today was busier"; the bucket says it ran hot.
  const was = baselineOf(3, { gpu_temp_by_load: { 90: stat(70) } });
  const today = day("2026-09-04", { gpu_temp_by_load: { 90: stat(79, 10) } });
  const found = gpuHeat(today, was);
  assert.equal(found.id, "gpu-heat");
  assert.equal(found.level, "alert", "9°C is past the alert threshold");
  assert.match(found.text, /9°C hotter than usual at 90–100% load/);
});

test("a few degrees hotter at the same load is not worth saying", () => {
  const was = baselineOf(3, { gpu_temp_by_load: { 50: stat(70) } });
  assert.equal(gpuHeat(day("x", { gpu_temp_by_load: { 50: stat(73, 10) } }), was), null);
  const warm = gpuHeat(day("x", { gpu_temp_by_load: { 50: stat(75, 10) } }), was);
  assert.equal(warm.level, "warn", "5°C warns, and only 8°C alerts");
});

test("the worst bucket is reported, not an average of them", () => {
  const was = baselineOf(3, { gpu_temp_by_load: { 0: stat(35), 90: stat(70) } });
  // Idle is normal, load is 10° hot. Averaging the two would halve it.
  const today = day("x", { gpu_temp_by_load: { 0: stat(35, 10), 90: stat(80, 10) } });
  assert.match(gpuHeat(today, was).text, /10°C hotter than usual at 90–100% load/);
});

test("a bucket without enough history on either side says nothing", () => {
  // One short day of history. The baseline merges across days, so three such
  // days would be plenty — it is the total behind the bucket that must clear.
  const thin = [day("2026-09-01", { gpu_temp_by_load: { 90: stat(70, MIN_SAMPLES - 1) } })];
  assert.equal(gpuHeat(day("x", { gpu_temp_by_load: { 90: stat(85, 10) } }), thin), null);
  const enough = [...thin, day("2026-09-02", { gpu_temp_by_load: { 90: stat(70, 1) } })];
  assert.ok(gpuHeat(day("x", { gpu_temp_by_load: { 90: stat(85, 10) } }), enough), "one more sample and it is");

  const was = baselineOf(3, { gpu_temp_by_load: { 90: stat(70) } });
  const today = day("x", { gpu_temp_by_load: { 90: stat(85, 1) } });
  assert.equal(gpuHeat(today, was), null, "one hot sample today is not a trend");
  assert.equal(gpuHeat(day("x"), was), null, "and a machine with no GPU is silent");
});

/* ── the rest of the checks ─────────────────────────────────── */

test("the CPU is judged on its peak, against the usual peak", () => {
  const was = baselineOf(3, { cpu_temp: stat(60, MIN_SAMPLES, 78) });
  assert.equal(cpuHeat(day("x", { cpu_temp: stat(60, 10, 83) }), was), null, "5° is under the bar");
  const hot = cpuHeat(day("x", { cpu_temp: stat(60, 10, 85) }), was);
  assert.match(hot.text, /peaked at 85°C — 7°C above its usual peak/);
  assert.equal(hot.level, "warn");
  assert.equal(cpuHeat(day("x", { cpu_temp: stat(70, 10, 92) }), was).level, "alert");
});

test("latency is judged against the same hour, not the whole day", () => {
  // 21:00 is always slow here; tonight is no worse than usual for 21:00.
  const was = baselineOf(3, { ping_by_hour: { 21: stat(90), 6: stat(20) } });
  const today = day("x", { ping_by_hour: { 21: stat(95), 6: stat(22) } });
  assert.equal(pingNow(today, was, 21), null, "an evening that is always slow is not news");

  const bad = day("x", { ping_by_hour: { 6: stat(70) } });
  const found = pingNow(bad, was, 6);
  assert.match(found.text, /Ping 70 ms — usually 20 ms at this hour/);
  assert.match(found.detail, /3\.5× your normal for 06:00/);
  assert.equal(found.level, "warn", "50 ms over the usual still only warns");
  const worse = pingNow(day("x", { ping_by_hour: { 6: stat(85) } }), was, 6);
  assert.equal(worse.level, "alert", "65 ms over is past the alert threshold");
});

test("latency needs both a doubling and a margin a human would notice", () => {
  const was = baselineOf(3, { ping_by_hour: { 6: stat(8) } });
  // 3× normal, but 16 ms over 8 ms is nothing anyone can feel.
  assert.equal(pingNow(day("x", { ping_by_hour: { 6: stat(24) } }), was, 6), null);
  assert.equal(pingNow(day("x"), was, 6), null, "and an hour with no samples is silent");
});

test("packet loss is reported only when it is unusual for this connection", () => {
  const flaky = baselineOf(3, { loss_pct: 2 });
  assert.equal(packetLoss(day("x", { loss_pct: 4 }), flaky), null, "2× on a line that always drops");
  const clean = baselineOf(3, { loss_pct: 0.1 });
  const found = packetLoss(day("x", { loss_pct: 6 }), clean);
  assert.equal(found.level, "alert");
  assert.match(found.text, /Packet loss on 6\.0% of checks today/);
  assert.equal(packetLoss(day("x", { loss_pct: 0.5 }), clean), null, "under 1% is never reported");
});

test("screen time is only ever claimed as 'already', never as a finished day", () => {
  const was = baselineOf(3, { screen_secs: 4 * 3600 });
  const found = screenTime(day("x", { screen_secs: 7 * 3600 }), was);
  assert.match(found.text, /7h 00m on screen already — usually 4h 00m a day/);
  assert.equal(found.level, "info");
  assert.equal(screenTime(day("x", { screen_secs: 5 * 3600 }), was), null, "1.25× is not a story");
});

test("a day the machine was off does not drag the usual down", () => {
  // The off day is absent from screen_secs entirely; counting it as 0 would
  // halve the average and make an ordinary day look excessive.
  const was = [
    day("2026-09-01", { screen_secs: 8 * 3600 }),
    day("2026-09-02"),
    day("2026-09-03", { screen_secs: 8 * 3600 }),
  ];
  assert.equal(screenTime(day("x", { screen_secs: 9 * 3600 }), was), null);
});

/* ── what the widget actually shows ─────────────────────────── */

test("nothing is claimed until there are days to compare against", () => {
  const hot = { gpu_temp_by_load: { 90: stat(95, 10) } };
  assert.deepEqual(anomalies([day("2026-09-01", hot)]), [], "one day is not a baseline");
  assert.deepEqual(anomalies([day("2026-09-01"), day("2026-09-02", hot)]), []);
  assert.equal(coverage([day("a"), day("b")]).ready, false);
  assert.equal(coverage([day("a"), day("b"), day("c")]).ready, true);
  assert.deepEqual(coverage(null), { days: 0, ready: false });
});

test("anomalies come back worst first", () => {
  const was = baselineOf(MIN_DAYS, {
    gpu_temp_by_load: { 90: stat(70) },
    screen_secs: 3 * 3600,
  });
  const today = day("2026-09-09", {
    gpu_temp_by_load: { 90: stat(80, 10) }, // alert
    screen_secs: 9 * 3600, // info
  });
  const found = anomalies([...was, today]);
  assert.deepEqual(found.map((f) => f.id), ["gpu-heat", "screen-time"]);
  assert.deepEqual(found.map((f) => f.level), ["alert", "info"]);
});

test("a trend row is left out rather than shown as a dash", () => {
  const was = baselineOf(3, { cpu: stat(20), ping: stat(18) });
  const rows = trends([...was, day("x", { cpu: stat(35), ping: stat(19) })]);
  assert.deepEqual(rows.map((r) => r.label), ["CPU", "Ping"], "no GPU on this machine, no GPU row");
  const cpu = rows[0];
  assert.equal(cpu.text, "35%");
  assert.equal(cpu.usual, "20%");
  assert.equal(cpu.delta, 15);
  assert.equal(rows[1].text, "19.0 ms", "ping keeps a decimal");
});

test("trends needs a today, and survives having no baseline at all", () => {
  assert.deepEqual(trends([]), []);
  assert.deepEqual(trends([day("x", { cpu: stat(35) })]), [], "nothing to be usual yet");
});

test("the week draws only the days there are, scaled to the busiest", () => {
  const days = [
    day("2026-09-14", { screen_secs: 2 * 3600, screen_apps: [{ name: "firefox", secs: 7200 }] }),
    // 15th absent: the machine was off.
    day("2026-09-16", {
      screen_secs: 8 * 3600,
      screen_apps: [{ name: "firefox", secs: 3600 }, { name: "code", secs: 25200 }],
      gpu_temp: stat(60, MIN_SAMPLES, 81),
    }),
  ];
  const { bars, apps, total } = week(days);
  assert.equal(bars.length, 2, "an absent day is absent, not a zero bar");
  assert.deepEqual(bars.map((b) => b.share), [0.25, 1]);
  assert.equal(bars[0].label, "Mon");
  assert.equal(bars[1].gpuMax, 81);
  assert.deepEqual(apps, [
    { name: "code", secs: 25200 },
    { name: "firefox", secs: 10800 },
  ], "app time is summed across the week and ranked");
  assert.equal(total, 10 * 3600);
});

test("the week keeps to its limit and survives an empty record", () => {
  const days = Array.from({ length: 10 }, (_, i) => day(`2026-09-0${i}`, { screen_secs: 60 }));
  assert.equal(week(days).bars.length, 7);
  assert.equal(week(days, 3).bars.length, 3);
  const empty = week([]);
  assert.deepEqual(empty.bars, []);
  assert.deepEqual(empty.apps, []);
  assert.equal(empty.total, 0);
});

test("durations read as a person would say them", () => {
  assert.equal(fmtHours(0), "0m");
  assert.equal(fmtHours(2280), "38m");
  assert.equal(fmtHours(15120), "4h 12m");
  assert.equal(fmtHours(3600), "1h 00m");
  assert.equal(fmtHours(-5), "0m", "a clock that went backwards is not negative time");
});
