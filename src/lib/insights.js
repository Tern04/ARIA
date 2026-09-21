// Reading the record: what today looks like next to the days before it.
//
// The collector in history.rs keeps a sample a minute and sums each day; this
// turns those sums into the handful of sentences worth saying. Pure functions
// over the report array (oldest day first), so every threshold below is
// testable without a machine that is actually overheating.
//
// Two rules run through all of it:
//
//   - Compare like with like. A GPU at 90 % load is hotter than one at 10 %,
//     and a home connection is worse at 21:00 than at 06:00, so heat is
//     compared inside a load bucket and latency inside an hour of the day.
//     Anything else just reports that today was busier.
//   - Say nothing without evidence. Every check needs a minimum number of
//     samples on both sides, and a day that is missing a metric is skipped
//     rather than counted as zero — otherwise a laptop that was asleep drags
//     every average down and everything looks alarming.

/** Samples a comparison needs on each side before it is worth making. */
export const MIN_SAMPLES = 20;
/** Days of history before "usual" means anything. */
export const MIN_DAYS = 2;

/** Weighted merge of several `{avg, max, n}` stats into one. */
export function mergeStats(stats) {
  const list = (stats ?? []).filter((s) => s && s.n > 0);
  if (!list.length) return null;
  const n = list.reduce((sum, s) => sum + s.n, 0);
  return {
    avg: list.reduce((sum, s) => sum + s.avg * s.n, 0) / n,
    max: list.reduce((m, s) => Math.max(m, s.max), -Infinity),
    n,
  };
}

/** Today (the last day in the report) and the days it is judged against. */
export function split(days) {
  const list = days ?? [];
  return { today: list[list.length - 1] ?? null, baseline: list.slice(0, -1) };
}

const pick = (days, key) => mergeStats(days.map((d) => d[key]));

/**
 * The GPU running hotter than it used to *at the same load* — the tell for
 * dust, a failing fan, or a warmer room. Buckets are compared one by one and
 * the worst is reported, so a single hot bucket is not averaged away by the
 * idle ones.
 */
export function gpuHeat(today, baseline) {
  if (!today?.gpu_temp_by_load) return null;
  let worst = null;
  for (const [bucket, now] of Object.entries(today.gpu_temp_by_load)) {
    if (!now || now.n < MIN_SAMPLES / 4) continue;
    const was = mergeStats(baseline.map((d) => d.gpu_temp_by_load?.[bucket]));
    if (!was || was.n < MIN_SAMPLES) continue;
    const delta = now.avg - was.avg;
    if (!worst || delta > worst.delta) worst = { bucket: Number(bucket), delta, now, was };
  }
  if (!worst || worst.delta < 4) return null;
  return {
    id: "gpu-heat",
    level: worst.delta >= 8 ? "alert" : "warn",
    text: `GPU ${worst.delta.toFixed(0)}°C hotter than usual at ${worst.bucket}–${worst.bucket + 10}% load`,
    detail: `${worst.now.avg.toFixed(0)}°C now, ${worst.was.avg.toFixed(0)}°C usually — dust, fan or room`,
  };
}

/**
 * Latency bad *for this hour*. An evening that is always slow is not news;
 * an evening twice its own normal is.
 */
export function pingNow(today, baseline, hour) {
  const now = today?.ping_by_hour?.[String(hour)];
  if (!now) return null;
  const was = mergeStats(baseline.map((d) => d.ping_by_hour?.[String(hour)]));
  if (!was || was.n < MIN_SAMPLES) return null;
  if (now.avg < was.avg * 2 || now.avg - was.avg < 20) return null;
  return {
    id: "ping-hour",
    level: now.avg - was.avg >= 60 ? "alert" : "warn",
    text: `Ping ${now.avg.toFixed(0)} ms — usually ${was.avg.toFixed(0)} ms at this hour`,
    detail: `${(now.avg / was.avg).toFixed(1)}× your normal for ${String(hour).padStart(2, "0")}:00`,
  };
}

/** Packet loss that is not normal for this connection. */
export function packetLoss(today, baseline) {
  if (today?.loss_pct == null || today.loss_pct < 1) return null;
  const was = baseline.map((d) => d.loss_pct).filter((v) => v != null);
  const usual = was.length ? was.reduce((a, b) => a + b, 0) / was.length : 0;
  if (was.length < MIN_DAYS || today.loss_pct < Math.max(usual * 3, 2)) return null;
  return {
    id: "packet-loss",
    level: today.loss_pct >= 5 ? "alert" : "warn",
    text: `Packet loss on ${today.loss_pct.toFixed(1)}% of checks today`,
    detail: `usually ${usual.toFixed(1)}%`,
  };
}

/** The CPU peaking hotter than it has been peaking. */
export function cpuHeat(today, baseline) {
  const now = today?.cpu_temp;
  const was = pick(baseline, "cpu_temp");
  if (!now || !was || was.n < MIN_SAMPLES || now.n < MIN_SAMPLES / 4) return null;
  const delta = now.max - was.max;
  if (delta < 6) return null;
  return {
    id: "cpu-heat",
    level: now.max >= 90 ? "alert" : "warn",
    text: `CPU peaked at ${now.max.toFixed(0)}°C — ${delta.toFixed(0)}°C above its usual peak`,
    detail: `usual peak ${was.max.toFixed(0)}°C`,
  };
}

/**
 * More screen time than usual. Deliberately only ever reported as "already",
 * never as a full-day comparison: today is a part-day and the baseline days
 * are whole ones, so the honest claim is that today has *already* passed them.
 */
export function screenTime(today, baseline) {
  const secs = today?.screen_secs;
  const past = baseline.map((d) => d.screen_secs).filter((v) => v != null && v > 0);
  if (secs == null || past.length < MIN_DAYS) return null;
  const usual = past.reduce((a, b) => a + b, 0) / past.length;
  if (secs < usual * 1.5 || secs - usual < 3600) return null;
  return {
    id: "screen-time",
    level: "info",
    text: `${fmtHours(secs)} on screen already — usually ${fmtHours(usual)} a day`,
    detail: `${(secs / usual).toFixed(1)}× your daily average`,
  };
}

/** Everything worth saying right now, most serious first. */
export function anomalies(days, now = Date.now()) {
  const { today, baseline } = split(days);
  if (!today || baseline.length < MIN_DAYS) return [];
  const hour = new Date(now).getHours();
  const found = [
    gpuHeat(today, baseline),
    cpuHeat(today, baseline),
    pingNow(today, baseline, hour),
    packetLoss(today, baseline),
    screenTime(today, baseline),
  ].filter(Boolean);
  const rank = { alert: 0, warn: 1, info: 2 };
  return found.sort((a, b) => rank[a.level] - rank[b.level]);
}

/**
 * The widget's steady rows: today's number, the usual one, and which way it
 * has moved. Rows whose metric this machine does not report are left out
 * rather than shown as dashes.
 */
export function trends(days) {
  const { today, baseline } = split(days);
  if (!today) return [];
  const rows = [];
  const add = (label, nowStat, wasStat, unit, digits = 0) => {
    if (!nowStat || !wasStat) return;
    rows.push({
      label,
      now: nowStat.avg,
      was: wasStat.avg,
      delta: nowStat.avg - wasStat.avg,
      unit,
      text: `${nowStat.avg.toFixed(digits)}${unit}`,
      usual: `${wasStat.avg.toFixed(digits)}${unit}`,
    });
  };
  add("CPU", today.cpu, pick(baseline, "cpu"), "%");
  add("GPU", today.gpu, pick(baseline, "gpu"), "%");
  add("GPU °C", today.gpu_temp, pick(baseline, "gpu_temp"), "°");
  add("Ping", today.ping, pick(baseline, "ping"), " ms", 1);
  if (today.screen_secs != null) {
    const past = baseline.map((d) => d.screen_secs).filter((v) => v != null && v > 0);
    if (past.length) {
      const usual = past.reduce((a, b) => a + b, 0) / past.length;
      rows.push({
        label: "Screen",
        now: today.screen_secs,
        was: usual,
        delta: today.screen_secs - usual,
        unit: "s",
        text: fmtHours(today.screen_secs),
        usual: fmtHours(usual),
      });
    }
  }
  return rows;
}

/**
 * The week at a glance: one bar per day of screen time, and what that time
 * went on. Days the machine was off are absent from the report, so they are
 * absent here too rather than drawn as an honest-looking zero.
 */
export function week(days, limit = 7) {
  const list = (days ?? []).slice(-limit);
  const peak = Math.max(1, ...list.map((d) => d.screen_secs ?? 0));
  const apps = new Map();
  for (const day of list) {
    for (const a of day.screen_apps ?? []) apps.set(a.name, (apps.get(a.name) ?? 0) + a.secs);
  }
  return {
    bars: list.map((d) => ({
      date: d.date,
      label: new Date(`${d.date}T12:00:00`).toLocaleDateString("en-GB", { weekday: "short" }),
      secs: d.screen_secs ?? 0,
      share: (d.screen_secs ?? 0) / peak,
      gpuMax: d.gpu_temp?.max ?? null,
    })),
    apps: [...apps.entries()]
      .map(([name, secs]) => ({ name, secs }))
      .sort((a, b) => b.secs - a.secs)
      .slice(0, 5),
    total: list.reduce((sum, d) => sum + (d.screen_secs ?? 0), 0),
  };
}

/** "4h 12m", "38m" — the widget's one time format. */
export function fmtHours(secs) {
  const total = Math.max(0, Math.round(secs / 60));
  const h = Math.floor(total / 60);
  const m = total % 60;
  return h ? `${h}h ${String(m).padStart(2, "0")}m` : `${m}m`;
}

/** How much history there is, for the widget's "still collecting" state. */
export function coverage(days) {
  const list = days ?? [];
  return { days: list.length, ready: list.length > MIN_DAYS };
}
