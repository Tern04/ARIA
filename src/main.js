import { drawGauge } from "./gauge.js";

function updateClock() {
  const now = new Date();
  document.getElementById("clock-time").textContent = now.toLocaleTimeString("en-GB");
  document.getElementById("clock-date").textContent = now.toLocaleDateString("en-CA");
}

function initGauges() {
  drawGauge(document.getElementById("gauge-cpu"), 0);
  drawGauge(document.getElementById("gauge-ram"), 0);
  drawGauge(document.getElementById("gauge-gpu"), 0);
}

// Smoothly tween each gauge from its current value to the new one so the
// rings sweep rather than snap.
const gaugeState = {};

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function setGauge(name, target) {
  const canvas = document.getElementById(`gauge-${name}`);
  const valEl = document.getElementById(`${name}-val`);
  const s = gaugeState[name] || (gaugeState[name] = { value: 0, raf: 0 });
  if (reduceMotion) {
    s.value = target;
    drawGauge(canvas, target);
    valEl.textContent = Math.round(target);
    return;
  }
  const from = s.value;
  const start = performance.now();
  const duration = 650;
  cancelAnimationFrame(s.raf);
  const step = (now) => {
    const t = Math.min(1, (now - start) / duration);
    const eased = 1 - Math.pow(1 - t, 3); // ease-out cubic
    const v = from + (target - from) * eased;
    s.value = v;
    drawGauge(canvas, v);
    valEl.textContent = Math.round(v);
    if (t < 1) s.raf = requestAnimationFrame(step);
  };
  s.raf = requestAnimationFrame(step);
}

function fmtDuration(secs) {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

function fmtCompact(v) {
  if (v >= 1e12) return `$${(v / 1e12).toFixed(2)}T`;
  if (v >= 1e9) return `$${(v / 1e9).toFixed(1)}B`;
  if (v >= 1e6) return `$${(v / 1e6).toFixed(0)}M`;
  if (v >= 1e3) return `$${(v / 1e3).toFixed(1)}K`;
  return `$${v.toFixed(0)}`;
}

function fmtRate(bps) {
  if (bps >= 1e6) return `${(bps / 1e6).toFixed(1)} MB/s`;
  if (bps >= 1e3) return `${Math.round(bps / 1e3)} kB/s`;
  return `${bps} B/s`;
}

function fmtUsd(v) {
  return "$" + v.toLocaleString("en-US", {
    minimumFractionDigits: v < 100 ? 2 : 0,
    maximumFractionDigits: v < 100 ? 2 : 0,
  });
}

// 7-day price history as a stroked polyline; CSS colours it via currentColor.
function sparkline(points, up) {
  const W = 100;
  const H = 26;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
  svg.setAttribute("preserveAspectRatio", "none");
  svg.classList.add("coin-spark", up ? "coin-spark--up" : "coin-spark--down");
  if (points.length < 2) return svg;
  const min = Math.min(...points);
  const max = Math.max(...points);
  const range = max - min || 1;
  const pad = 2;
  const pts = points
    .map((v, i) => {
      const x = (i / (points.length - 1)) * W;
      const y = pad + (1 - (v - min) / range) * (H - pad * 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  const line = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
  line.setAttribute("points", pts);
  line.setAttribute("fill", "none");
  line.setAttribute("stroke", "currentColor");
  line.setAttribute("stroke-width", "1.5");
  line.setAttribute("vector-effect", "non-scaling-stroke");
  svg.append(line);
  return svg;
}

// Crypto widget: last payload + selected period. The small size has no
// buttons and always shows the month; medium/large use the picked period.
let cryptoData = null;
const CRYPTO_PERIODS = [
  ["1d", "day"],
  ["1m", "month"],
  ["1y", "year"],
];
let cryptoPeriod = "1m";
try {
  const saved = localStorage.getItem("aria-crypto-period");
  if (CRYPTO_PERIODS.some(([k]) => k === saved)) cryptoPeriod = saved;
} catch {}

function renderCrypto() {
  const widget = document.getElementById("widget-crypto");
  const body = widget.querySelector(".widget-body");
  if (!cryptoData) return; // keep the loading placeholder until first emit
  body.replaceChildren();
  if (cryptoData.status !== "connected") {
    const span = document.createElement("span");
    span.className = "disconnected";
    span.textContent = cryptoData.reason;
    body.append(span);
    return;
  }

  const period = widget.dataset.size === "s" ? "1m" : cryptoPeriod;

  const bar = document.createElement("div");
  bar.className = "period-bar";
  for (const [key, label] of CRYPTO_PERIODS) {
    const btn = document.createElement("button");
    btn.className = key === cryptoPeriod ? "period-btn period-btn--active" : "period-btn";
    btn.textContent = key.toUpperCase();
    btn.title = `Change and chart over one ${label}`;
    btn.addEventListener("click", () => {
      cryptoPeriod = key;
      try {
        localStorage.setItem("aria-crypto-period", key);
      } catch {}
      renderCrypto();
    });
    bar.append(btn);
  }
  body.append(bar);

  for (const coin of cryptoData.coins) {
    const pct = coin[`change_${period}`];
    const spark = coin[`spark_${period}`];
    const up = pct >= 0;
    const row = document.createElement("div");
    row.className = "coin";
    const head = document.createElement("div");
    head.className = "coin-head";
    const sym = document.createElement("span");
    sym.className = "coin-sym";
    sym.textContent = coin.symbol;
    const price = document.createElement("span");
    price.className = "coin-price";
    price.textContent = fmtUsd(coin.price);
    const change = document.createElement("span");
    change.className = up ? "coin-change coin-change--up" : "coin-change coin-change--down";
    change.textContent = `${up ? "▲" : "▼"} ${Math.abs(pct).toFixed(2)}%`;
    change.title = `change over one ${CRYPTO_PERIODS.find(([k]) => k === period)[1]}`;
    head.append(sym, price, change);
    row.append(head);
    // Fundamentals sub-row from the same /markets payload (m+; l adds the
    // 24h range). Skip when the payload predates these fields.
    if (widget.dataset.size !== "s" && coin.market_cap > 0) {
      const sub = document.createElement("div");
      sub.className = "coin-sub";
      const bits = [
        `MCAP ${fmtCompact(coin.market_cap)}`,
        `VOL ${fmtCompact(coin.volume_24h)}`,
      ];
      if (widget.dataset.size === "l" && coin.high_24h > 0) {
        bits.push(`24H ${fmtCompact(coin.low_24h)}–${fmtCompact(coin.high_24h)}`);
      }
      sub.textContent = bits.join(" · ");
      row.append(sub);
    }
    row.append(sparkline(spark, up));
    body.append(row);
  }
}

// ── widget auth (log in / log out) ──────────────────────────
// Disconnected widgets render their login form inline; connected ones get a
// gear in the edit-mode controls that opens a log-out/reconfigure panel.
// While a panel is open its widget's render function must not clobber it.
let authPanelId = null;

function authInput(placeholder, type = "text") {
  const input = document.createElement("input");
  input.type = type;
  input.placeholder = placeholder;
  return input;
}

function authButton(label, onClick) {
  const btn = document.createElement("button");
  btn.className = "connect-btn";
  btn.textContent = label;
  btn.addEventListener("click", () => onClick(btn));
  return btn;
}

// Runs a backend command from a form button; errors land in `err` and the
// button re-enables so the user can fix the input and retry.
async function invokeAuth(btn, err, cmd, args) {
  btn.disabled = true;
  try {
    await window.__TAURI__.core.invoke(cmd, args);
    return true;
  } catch (e) {
    err.textContent = String(e);
    btn.disabled = false;
    return false;
  }
}

// Discord widget: last payload cached so size changes can re-render, plus
// the raw JSON so duplicate emits don't wipe the setup form mid-typing.
// The guild id is kept separately so the setup form stays prefilled even
// while disconnected (token-only re-auth must not require retyping it).
let discordData = null;
let discordLastJson = null;
let discordGuildId = "";

// Mic-off / headphones-off glyphs for voice occupants (static markup only).
const DC_ICONS = {
  mute: '<svg viewBox="0 0 24 24"><path fill="currentColor" d="M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-3.08A7 7 0 0 0 19 11h-2z"/><path stroke="currentColor" stroke-width="2.2" d="M4 3.5l16 17"/></svg>',
  deaf: '<svg viewBox="0 0 24 24"><path fill="currentColor" d="M12 3a9 9 0 0 0-9 9v7a2 2 0 0 0 2 2h3v-8H5v-1a7 7 0 0 1 14 0v1h-3v8h3a2 2 0 0 0 2-2v-7a9 9 0 0 0-9-9z"/><path stroke="currentColor" stroke-width="2.2" d="M4 3.5l16 17"/></svg>',
};

// tokenOptional: the gear panel keeps a stored token when the field is left
// empty; the inline (logged-out / never-configured) form has no token to keep.
function discordSetupForm(err, prefillGuild, tokenOptional) {
  const form = document.createElement("div");
  form.className = "auth-form";
  const token = authInput(
    tokenOptional ? "bot token (empty = keep current)" : "bot token",
    "password",
  );
  const guild = authInput("server ID");
  if (prefillGuild) guild.value = prefillGuild;
  const btn = authButton("CONNECT", async (b) => {
    const ok = await invokeAuth(b, err, "discord_setup", {
      token: token.value,
      guildId: guild.value,
    });
    if (ok) closeAuthPanel();
  });
  form.append(token, guild, btn);
  return form;
}

function renderDiscord() {
  if (authPanelId === "discord") return;
  const widget = document.getElementById("widget-discord");
  const body = widget.querySelector(".widget-body");
  if (!discordData) return; // keep the placeholder until first emit
  const p = discordData;
  body.replaceChildren();

  if (p.status !== "connected") {
    const span = document.createElement("span");
    span.className = "disconnected";
    span.textContent = p.reason;
    body.append(span);
    if (p.needs_setup) body.append(discordSetupForm(span, discordGuildId, false));
    return;
  }

  const size = widget.dataset.size;

  // Small: a glanceable headline — how many people are in voice, where.
  if (size === "s") {
    const total = p.voice.reduce((n, c) => n + c.occupants.length, 0);
    body.append(statRow(total, total === 1 ? "person in voice" : "in voice"));
    const top = p.voice.find((c) => c.occupants.length > 0);
    if (top) {
      const chan = document.createElement("div");
      chan.className = "dc-s-chan";
      chan.textContent = top.name;
      body.append(chan);
    }
    return;
  }

  // Voice channels arrive sorted by popularity; render them all and let
  // the fill-list fade clip whatever doesn't fit the current size.
  const list = document.createElement("div");
  list.className = "dc-voice fill-list";
  for (const chan of p.voice) {
    const live = chan.occupants.length > 0;
    const row = document.createElement("div");
    row.className = live ? "dc-voice-row dc-voice-row--live" : "dc-voice-row";
    const name = document.createElement("span");
    name.className = "dc-chan-name";
    name.textContent = chan.name;
    row.append(name);
    if (live) {
      const count = document.createElement("span");
      count.className = "dc-count";
      count.textContent = chan.occupants.length;
      row.append(count);
    }
    const occList = document.createElement("span");
    occList.className = "dc-occupants";
    if (!live) {
      occList.textContent = "—";
    }
    for (const o of chan.occupants) {
      const occ = document.createElement("span");
      occ.className = "dc-occ";
      occ.append(o.name);
      const flag = o.deaf ? "deaf" : o.mute ? "mute" : null;
      if (flag) {
        const ico = document.createElement("span");
        ico.className = "dc-occ-ico";
        ico.innerHTML = DC_ICONS[flag];
        ico.title = flag === "deaf" ? "deafened" : "muted";
        occ.append(ico);
      }
      if (o.streaming) {
        const liveTag = document.createElement("span");
        liveTag.className = "dc-live";
        liveTag.textContent = "LIVE";
        occ.append(liveTag);
      }
      if (size === "l" && o.activity) {
        const act = document.createElement("span");
        act.className = "dc-activity";
        act.textContent = o.activity;
        occ.title = o.activity;
        occ.append(act);
      }
      occList.append(occ);
    }
    row.append(occList);
    // Large: faint person-hours tag from the popularity tally.
    if (size === "l" && chan.score >= 60) {
      const hrs = document.createElement("span");
      hrs.className = "dc-chan-score";
      hrs.textContent = `${Math.round(chan.score / 60)} h`;
      hrs.title = "voice time tracked in this channel";
      row.append(hrs);
    }
    list.append(row);
  }

  // Large: channels left, friends as a full column right (status dot,
  // name, current game/song per row) — more detail than the m strip.
  if (size === "l" && p.friends.length > 0) {
    const cols = document.createElement("div");
    cols.className = "dc-cols";
    const left = document.createElement("div");
    left.className = "stag-col";
    left.append(list);
    const right = document.createElement("div");
    right.className = "stag-col";
    const label = document.createElement("div");
    label.className = "stag-section";
    label.textContent = "FRIENDS";
    const col = document.createElement("div");
    col.className = "dc-friend-col fill-list";
    for (const f of p.friends) {
      const row = document.createElement("div");
      row.className = "dc-friend-row";
      const dot = document.createElement("i");
      dot.className = `dc-dot dc-dot--${f.status}`;
      const name = document.createElement("span");
      name.className = "dc-friend-name";
      name.textContent = f.name;
      row.append(dot, name);
      if (f.activity) {
        const act = document.createElement("span");
        act.className = "dc-activity";
        act.textContent = f.activity;
        row.title = f.activity;
        row.append(act);
      }
      col.append(row);
    }
    right.append(label, col);
    cols.append(left, right);
    body.append(cols);
    return;
  }

  body.append(list);

  if (p.friends.length > 0) {
    const friends = document.createElement("div");
    friends.className = "dc-friends";
    for (const f of p.friends) {
      const chip = document.createElement("span");
      chip.className = "dc-friend";
      const dot = document.createElement("i");
      dot.className = `dc-dot dc-dot--${f.status}`;
      chip.append(dot, f.name);
      if (f.activity) {
        const act = document.createElement("span");
        act.textContent = f.activity;
        act.className = "dc-activity";
        chip.title = f.activity;
        chip.append(act);
      }
      friends.append(chip);
    }
    body.append(friends);
  }
}

// Caches for the auth panels and re-renders (github/email/stag mirror the
// discord pattern; crypto has its own above).
let githubData = null;
let githubLastJson = null;
let emailData = null;
let emailLastJson = null;
let stagData = null;

function mailAccountForm(err) {
  const form = document.createElement("div");
  form.className = "auth-form";
  const label = authInput("label (e.g. SEZNAM)");
  const host = authInput("imap host");
  const user = authInput("user / e-mail");
  const pass = authInput("password", "password");
  const btn = authButton("ADD ACCOUNT", async (b) => {
    const ok = await invokeAuth(b, err, "mail_add_account", {
      label: label.value,
      host: host.value,
      user: user.value,
      password: pass.value,
    });
    // The command refetches before resolving, so reopening shows the
    // updated account list immediately.
    if (ok && authPanelId === "email") openAuthPanel("email");
  });
  form.append(label, host, user, pass, btn);
  return form;
}

function closeAuthPanel() {
  const id = authPanelId;
  authPanelId = null;
  if (!id) return;
  ({ discord: renderDiscord, github: renderGithub, email: renderEmail, stag: renderStag })[
    id
  ]?.();
}

// The edit-mode gear opens this: a log-out/reconfigure panel in the widget
// body. Data rendering resumes when it closes.
function openAuthPanel(id) {
  authPanelId = id;
  const body = document.querySelector(`#widget-${id} .widget-body`);
  body.replaceChildren();

  const panel = document.createElement("div");
  panel.className = "auth-panel";
  const close = document.createElement("button");
  close.className = "auth-close";
  close.textContent = "×";
  close.title = "Close";
  close.addEventListener("click", closeAuthPanel);
  const err = document.createElement("span");
  err.className = "disconnected";
  panel.append(close);

  const logoutBtn = (cmd) =>
    authButton("LOG OUT", async (b) => {
      if (await invokeAuth(b, err, cmd, {})) closeAuthPanel();
    });

  if (id === "discord") {
    panel.append(discordSetupForm(err, discordGuildId, true));
    const out = logoutBtn("discord_logout");
    out.classList.add("auth-danger");
    const note = document.createElement("span");
    note.className = "auth-note";
    note.textContent = "log out deletes the stored bot token";
    panel.append(out, note);
  } else if (id === "github") {
    const form = document.createElement("div");
    form.className = "auth-form";
    const token = authInput("new personal access token", "password");
    form.append(
      token,
      authButton("SAVE", async (b) => {
        if (await invokeAuth(b, err, "github_setup", { token: token.value })) closeAuthPanel();
      }),
    );
    const out = logoutBtn("github_logout");
    out.classList.add("auth-danger");
    panel.append(form, out);
  } else if (id === "stag") {
    const out = logoutBtn("stag_logout");
    out.classList.add("auth-danger");
    panel.append(out);
  } else if (id === "email") {
    const list = document.createElement("div");
    list.className = "acct-list";
    for (const acct of emailData?.status === "connected" ? emailData.accounts : []) {
      const row = document.createElement("div");
      row.className = "acct-row";
      const name = document.createElement("span");
      name.className = "acct-name";
      name.textContent = acct.label;
      const who = document.createElement("span");
      who.className = "acct-user";
      who.textContent = acct.user;
      who.title = acct.host;
      const del = document.createElement("button");
      del.className = "w-btn";
      del.textContent = "×";
      del.title = "Remove account";
      del.addEventListener("click", async () => {
        del.disabled = true;
        try {
          await window.__TAURI__.core.invoke("mail_remove_account", { user: acct.user });
          if (authPanelId === "email") openAuthPanel("email");
        } catch (e) {
          err.textContent = String(e);
          del.disabled = false;
        }
      });
      row.append(name, who, del);
      list.append(row);
    }
    panel.append(list, mailAccountForm(err));
  }

  panel.append(err);
  body.append(panel);
}

function renderGithub() {
  if (authPanelId === "github") return;
  const body = document.querySelector("#widget-github .widget-body");
  if (!githubData) return;
  const p = githubData;
  body.replaceChildren();
  if (p.status !== "connected") {
    const span = document.createElement("span");
    span.className = "disconnected";
    span.textContent = p.reason;
    const form = document.createElement("div");
    form.className = "auth-form";
    const token = authInput("personal access token", "password");
    form.append(
      token,
      authButton("CONNECT", (b) => invokeAuth(b, span, "github_setup", { token: token.value })),
    );
    body.append(span, form);
    return;
  }
  const size = document.getElementById("widget-github").dataset.size;

  // Stats side by side so the row uses the full width.
  const stats = document.createElement("div");
  stats.className = "gh-stats";
  stats.append(
    statRow(p.notifications, "notifications"),
    statRow(p.open_prs, "open PRs"),
  );
  body.append(stats);

  // Large: notification titles (and open PRs when few of them) as
  // mail-style two-line rows.
  if (size === "l") {
    const items = [
      ...(p.notification_items ?? []).map((n) => ({
        title: n.title,
        tag: n.repo,
        sub: n.reason,
      })),
      ...((p.notification_items?.length ?? 0) < 3
        ? (p.pr_items ?? []).map((pr) => ({
            title: pr.title,
            tag: pr.repo,
            sub: `PR #${pr.number}`,
          }))
        : []),
    ];
    if (items.length > 0) {
      const list = document.createElement("div");
      list.className = "gh-list fill-list";
      for (const it of items) {
        const row = document.createElement("div");
        row.className = "gh-item";
        const head = document.createElement("div");
        head.className = "gh-item-head";
        const title = document.createElement("span");
        title.className = "gh-item-title";
        title.textContent = it.title;
        const tag = document.createElement("span");
        tag.className = "gh-item-tag";
        tag.textContent = it.tag;
        head.append(title, tag);
        const sub = document.createElement("div");
        sub.className = "gh-item-sub";
        sub.textContent = it.sub;
        row.append(head, sub);
        list.append(row);
      }
      body.append(list);
    }
  }

  const repos = size === "l" ? (p.recent_repos ?? []).slice(0, 2) : (p.recent_repos ?? []).slice(0, 1);
  for (const r of repos) {
    const repo = document.createElement("div");
    repo.className = "gh-repo";
    const name = document.createElement("span");
    name.className = "gh-repo-name";
    name.textContent = r.name;
    const when = document.createElement("span");
    when.className = "gh-repo-when";
    when.textContent = `pushed ${r.pushed_at}`;
    repo.append(name, when);
    body.append(repo);
  }
  if (p.contributions.length > 0) {
    const wall = document.createElement("div");
    wall.className = "gh-wall";
    for (const week of p.contributions) {
      for (let d = 0; d < 7; d++) {
        const cell = document.createElement("div");
        const n = week[d] ?? 0;
        cell.className =
          n === 0 ? "gh-cell" :
          n <= 2 ? "gh-cell gh-cell--1" :
          n <= 5 ? "gh-cell gh-cell--2" : "gh-cell gh-cell--3";
        if (week[d] === undefined) cell.className = "gh-cell gh-cell--pad";
        wall.append(cell);
      }
    }
    const total = document.createElement("div");
    total.className = "gh-total";
    total.textContent = `${p.total_contributions} contributions this year`;
    body.append(wall, total);
  }
}

// Today → time, this week → weekday, older → day.month.
function fmtMailTime(unixSecs) {
  const d = new Date(unixSecs * 1000);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay) {
    return d.toLocaleTimeString("cs-CZ", { hour: "2-digit", minute: "2-digit" });
  }
  if (now - d < 6 * 86400 * 1000) {
    return d.toLocaleDateString("cs-CZ", { weekday: "short" });
  }
  return d.toLocaleDateString("cs-CZ", { day: "numeric", month: "numeric" });
}

// null = show all accounts; otherwise the label to filter the mail list by.
let mailFilter = null;

function renderEmail() {
  if (authPanelId === "email") return;
  // The auth panel replaces the widget-body children, taking the fixed
  // summary/list scaffolding with it — rebuild when missing.
  let summary = document.getElementById("mail-summary");
  if (!summary) {
    const body = document.querySelector("#widget-email .widget-body");
    body.replaceChildren();
    summary = document.createElement("div");
    summary.className = "mail-summary";
    summary.id = "mail-summary";
    const ul = document.createElement("ul");
    ul.className = "mail-list";
    ul.id = "mail-list";
    body.append(summary, ul);
  }
  const list = document.getElementById("mail-list");
  if (!emailData) return;
  const p = emailData;
  summary.replaceChildren();
  list.replaceChildren();
  if (p.status !== "connected") {
    const span = document.createElement("span");
    span.className = "disconnected";
    span.textContent = p.reason;
    summary.append(span, mailAccountForm(span));
    mailFilter = null;
    return;
  }

  const renderList = () => {
    list.replaceChildren();
    list.classList.add("fill-list");
    const showTime =
      document.getElementById("widget-email").dataset.size === "l";
    const shown = p.messages.filter((m) => !mailFilter || m.account === mailFilter);
    for (const msg of shown) {
      const li = document.createElement("li");
      li.className = msg.unseen ? "mail-row mail-row--unseen" : "mail-row";
      const fromLine = document.createElement("div");
      fromLine.className = "mail-from-line";
      const from = document.createElement("span");
      from.className = "mail-from";
      from.textContent = msg.from;
      const tag = document.createElement("span");
      tag.className = "mail-acct-tag";
      tag.textContent =
        showTime && msg.timestamp > 0
          ? `${msg.account} · ${fmtMailTime(msg.timestamp)}`
          : msg.account;
      fromLine.append(from, tag);
      const subject = document.createElement("div");
      subject.className = "mail-subject";
      subject.textContent = msg.subject || "(no subject)";
      li.append(fromLine, subject);
      list.append(li);
    }
  };

  // Reset a stale filter if that account vanished from the config.
  if (mailFilter && !p.accounts.some((a) => a.label === mailFilter)) {
    mailFilter = null;
  }

  for (const acct of p.accounts) {
    const chip = document.createElement("button");
    chip.className =
      mailFilter === acct.label ? "mail-acct mail-acct--active" : "mail-acct";
    const count = document.createElement("span");
    count.className = "mail-acct-count";
    count.textContent = acct.unread ?? "!";
    if (acct.unread === null) chip.title = "account unreachable";
    const label = document.createElement("span");
    label.className = "mail-acct-label";
    label.textContent = acct.label;
    chip.append(count, label);
    chip.addEventListener("click", () => {
      // Toggle: click active account to clear the filter.
      mailFilter = mailFilter === acct.label ? null : acct.label;
      for (const c of summary.children) {
        c.classList.toggle(
          "mail-acct--active",
          mailFilter !== null && c === chip,
        );
      }
      renderList();
    });
    summary.append(chip);
  }
  renderList();
}

// Kind abbreviation → CSS modifier for colouring lecture vs seminar.
const CLASS_KIND = { "Př": "lecture", "Cv": "seminar", "Se": "seminar" };

// Monday-based weekday index of today (0 = Po … 6 = Ne).
function todayIndex() {
  return (new Date().getDay() + 6) % 7;
}

// Compact chips of today's classes — the s-size headline.
function buildTodayStrip(tt) {
  const wrap = document.createElement("div");
  wrap.className = "stag-today";
  const items = tt.classes.filter((c) => c.day === todayIndex());
  if (items.length === 0) {
    wrap.classList.add("stag-today--free");
    wrap.textContent = "no classes today";
    return wrap;
  }
  for (const c of items) {
    const chip = document.createElement("span");
    chip.className = `stag-today-chip tt-class--${CLASS_KIND[c.kind] ?? "other"}`;
    chip.textContent = `${c.subject} · ${c.kind} · ${c.time} · ${c.room}`;
    wrap.append(chip);
  }
  return wrap;
}

function buildTimetable(tt) {
  const grid = document.createElement("div");
  grid.className = "tt-grid";
  const cols = tt.max_period - tt.min_period + 1;
  // column 1 = day label, then one column per period.
  grid.style.gridTemplateColumns = `auto repeat(${cols}, minmax(0, 1fr))`;

  // header row: blank corner + period times
  const corner = document.createElement("div");
  corner.className = "tt-corner";
  grid.append(corner);
  for (const ph of tt.periods) {
    const h = document.createElement("div");
    h.className = "tt-head";
    h.style.gridColumn = ph.period - tt.min_period + 2;
    const num = document.createElement("span");
    num.className = "tt-head-num";
    num.textContent = ph.period;
    const t = document.createElement("span");
    t.className = "tt-head-time";
    t.textContent = ph.start;
    h.append(num, t);
    grid.append(h);
  }

  const dayNames = ["Po", "Út", "St", "Čt", "Pá"];
  for (let d = 0; d < 5; d++) {
    const label = document.createElement("div");
    label.className = d === todayIndex() ? "tt-day tt-day--today" : "tt-day";
    label.style.gridRow = d + 2;
    label.textContent = dayNames[d];
    grid.append(label);
  }

  for (const c of tt.classes) {
    const cell = document.createElement("div");
    cell.className = `tt-class tt-class--${CLASS_KIND[c.kind] ?? "other"}`;
    cell.style.gridRow = c.day + 2;
    cell.style.gridColumn = `${c.start_period - tt.min_period + 2} / span ${c.end_period - c.start_period + 1}`;
    const subj = document.createElement("span");
    subj.className = "tt-subj";
    subj.textContent = c.subject;
    const room = document.createElement("span");
    room.className = "tt-room";
    room.textContent = c.room;
    cell.append(subj, room);
    cell.title = `${c.subject} ${c.kind} · ${c.time} · ${c.room}`;
    grid.append(cell);
  }
  return grid;
}

function statRow(value, label) {
  const row = document.createElement("div");
  row.className = "stat-row";
  const val = document.createElement("span");
  val.className = "stat-value";
  val.textContent = value;
  row.append(val, ` ${label}`);
  return row;
}

// Returns once every listener is registered, so a gated collector's first
// emit can't race ahead of registration (listen() registers over async IPC).
function setStatus(id, online) {
  document.getElementById(id).classList.toggle("online", online);
}

let screentimeData = null;
function renderScreentime() {
  if (!screentimeData) return;
  const p = screentimeData;
  const size = document.getElementById("widget-screentime").dataset.size;
  const body = document.querySelector("#widget-screentime .widget-body");
  body.replaceChildren();
  const total = document.createElement("div");
  total.className = "st-total";
  total.textContent = fmtDuration(p.total);
  body.append(total);
  // Large: trend line against yesterday's saved tally.
  if (size === "l" && p.yesterday_total != null) {
    const d = p.total - p.yesterday_total;
    const delta = document.createElement("div");
    delta.className = "st-delta";
    delta.textContent =
      `yesterday ${fmtDuration(p.yesterday_total)} (${d >= 0 ? "+" : "−"}${fmtDuration(Math.abs(d))})`;
    body.append(delta);
  }
  const list = document.createElement("div");
  list.className = "st-list fill-list";
  const max = p.apps[0]?.secs || 1;
  for (const app of p.apps) {
    const wrap = document.createElement("div");
    wrap.className = "st-app";
    const row = document.createElement("div");
    row.className = "st-row";
    const name = document.createElement("span");
    name.className = "st-name";
    name.textContent = app.name;
    const time = document.createElement("span");
    time.className = "st-time";
    time.textContent = fmtDuration(app.secs);
    row.append(name, time);
    // share bar relative to the top app — a chart at zero data cost
    const bar = document.createElement("div");
    bar.className = "st-bar";
    const fill = document.createElement("div");
    fill.className = "st-bar-fill";
    fill.style.width = `${(app.secs / max) * 100}%`;
    bar.append(fill);
    wrap.append(row, bar);
    list.append(wrap);
  }
  body.append(list);
}

let musicData = null;
let musicSampledAt = 0; // wall clock of the last sample, for local advance

function fmtClock(secs) {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

// Advance the progress bar between polls using the wall clock; called by
// renderMusic and a 1 s ticker so the bar moves without new samples.
function updateMusicProgress() {
  const p = musicData;
  const wrap = document.getElementById("music-progress");
  if (!p || wrap.hidden) return;
  const dur = p.duration_secs;
  const elapsed = p.status === "playing" ? (Date.now() - musicSampledAt) / 1000 : 0;
  const pos = Math.min(p.position_secs + elapsed, dur);
  document.getElementById("music-progress-fill").style.width =
    dur > 0 ? `${(pos / dur) * 100}%` : "0%";
  document.getElementById("music-progress-time").textContent =
    dur > 0 ? `${fmtClock(pos)} / ${fmtClock(dur)}` : "";
}

function renderMusic() {
  if (!musicData) return;
  const p = musicData;
  const size = document.getElementById("widget-music").dataset.size;
  const title = document.getElementById("music-title");
  const artist = document.getElementById("music-artist");
  const album = document.getElementById("music-album");
  const art = document.getElementById("music-art");
  const progress = document.getElementById("music-progress");
  const controls = document.getElementById("music-controls");
  const playing = p.status === "playing";
  const active = playing || p.status === "paused";
  if (active) {
    title.textContent = p.title;
    artist.textContent = p.artist;
    // Album line; the source app tags along at l ("Album — Spotify").
    const albumText = [p.album, size === "l" ? p.app_name : null]
      .filter(Boolean)
      .join(" — ");
    album.textContent = albumText;
    album.hidden = !albumText;
    if (p.art) {
      art.src = p.art;
      art.hidden = false;
    } else {
      art.hidden = true;
      art.removeAttribute("src");
    }
    progress.hidden = !(p.duration_secs > 0);
    controls.classList.remove("music-controls--idle");
  } else {
    title.textContent = "Nothing playing";
    artist.textContent = "—";
    album.hidden = true;
    art.hidden = true;
    art.removeAttribute("src");
    progress.hidden = true;
    controls.classList.add("music-controls--idle");
  }
  // Play glyph when paused/stopped, pause glyph when playing.
  document.getElementById("music-playpause").classList.toggle("is-playing", playing);
  updateMusicProgress();
}

async function initCollectors() {
  const { listen } = window.__TAURI__.event;
  const pending = [];
  const on = (event, cb) => pending.push(listen(event, cb));

  on("hardware", (e) => {
    const p = e.payload;
    setGauge("cpu", p.cpu);
    setGauge("ram", p.ram);
    if (p.ram_total_gb > 0) {
      document.getElementById("hw-ram").textContent =
        `RAM ${p.ram_used_gb.toFixed(1)} / ${Math.round(p.ram_total_gb)} GB`;
    }
    document.getElementById("hw-net").textContent =
      `↓ ${fmtRate(p.net_rx_bps)} ↑ ${fmtRate(p.net_tx_bps)}`;
    const days = Math.floor(p.uptime_secs / 86400);
    document.getElementById("hw-up").textContent = days > 0
      ? `UP ${days}d ${Math.floor((p.uptime_secs % 86400) / 3600)}h`
      : `UP ${fmtDuration(p.uptime_secs)}`;
  });

  on("gpu", (e) => {
    setGauge("gpu", e.payload.gpu);
  });

  on("interactive", (e) => {
    document.body.classList.toggle("interactive", e.payload === true);
  });

  on("github", (e) => {
    setStatus("status-github", e.payload.status === "connected");
    const raw = JSON.stringify(e.payload);
    if (raw === githubLastJson) return; // don't wipe the PAT form mid-typing
    githubLastJson = raw;
    githubData = e.payload;
    renderGithub();
  });

  on("crypto", (e) => {
    cryptoData = e.payload;
    renderCrypto();
  });

  on("discord", (e) => {
    setStatus("status-discord", e.payload.status === "connected");
    if (e.payload.status === "connected") discordGuildId = e.payload.guild_id;
    // The collector suppresses duplicate emits, but guard here too so a
    // repeated payload never rebuilds the setup form mid-typing.
    const raw = JSON.stringify(e.payload);
    if (raw === discordLastJson) return;
    discordLastJson = raw;
    discordData = e.payload;
    renderDiscord();
  });

  on("email", (e) => {
    setStatus("status-mail", e.payload.status === "connected");
    const raw = JSON.stringify(e.payload);
    if (raw === emailLastJson) return; // don't wipe the add-account form
    emailLastJson = raw;
    emailData = e.payload;
    renderEmail();
  });

  on("screentime", (e) => {
    screentimeData = e.payload;
    renderScreentime();
  });

  on("music", (e) => {
    musicData = e.payload;
    musicSampledAt = Date.now();
    const active = musicData.status === "playing" || musicData.status === "paused";
    setStatus("status-music", active);
    renderMusic();
  });

  on("stag", (e) => {
    setStatus("status-stag", e.payload.status === "connected");
    stagData = e.payload;
    renderStag();
  });

  await Promise.all(pending);
}

function renderStag() {
  if (authPanelId === "stag") return;
  const body = document.querySelector("#widget-stag .widget-body");
  if (!stagData) return;
  const p = stagData;
  body.replaceChildren();
  if (p.status === "connected") {
    const header = document.createElement("div");
    header.className = "stag-header";
    const program = document.createElement("span");
    program.className = "stag-program";
    program.textContent = p.program;
    const meta = document.createElement("span");
    meta.className = "stag-meta";
    meta.textContent = `${p.semester} · ${p.total_credits} cr`;
    header.append(program, meta);
    body.append(header);

    const size = document.getElementById("widget-stag").dataset.size;

    // Small: header, today's classes, and the nearest exam.
    if (size === "s") {
      body.append(buildTodayStrip(p.timetable));
      if (p.exams?.length) {
        const next = document.createElement("div");
        next.className = "stag-next-exam";
        const ex = p.exams[0];
        next.textContent = `next exam · ${ex.subject} · ${ex.date}${ex.time ? ` ${ex.time}` : ""}`;
        body.append(next);
      }
      return;
    }

    body.append(buildTimetable(p.timetable));

    const coursesLabel = document.createElement("div");
    coursesLabel.className = "stag-section";
    coursesLabel.textContent = "COURSES";
    const courseList = document.createElement("div");
    courseList.className = "course-list fill-list";
    for (const c of p.courses) {
      const row = document.createElement("div");
      row.className = "course-row";
      const cr = document.createElement("span");
      cr.className = "course-cr";
      cr.textContent = c.credits;
      const code = document.createElement("span");
      code.className = c.compulsory ? "course-code course-code--req" : "course-code";
      code.textContent = c.code;
      const name = document.createElement("span");
      name.className = "course-name";
      name.textContent = c.name;
      const tag = document.createElement("span");
      tag.className = c.compulsory ? "course-tag course-tag--req" : "course-tag";
      tag.textContent = c.compulsory ? "req" : "opt";
      tag.title = c.compulsory ? "povinný" : "povinně volitelný";
      row.append(cr, code, name, tag);
      courseList.append(row);
    }

    // Large: two-column bottom — courses left, upcoming exams right.
    // The exams column always renders so the layout reads the same
    // even outside the exam period.
    if (size === "l") {
      const bottom = document.createElement("div");
      bottom.className = "stag-bottom";
      const coursesCol = document.createElement("div");
      coursesCol.className = "stag-col";
      coursesCol.append(coursesLabel, courseList);
      const examsCol = document.createElement("div");
      examsCol.className = "stag-col";
      const examsLabel = document.createElement("div");
      examsLabel.className = "stag-section";
      examsLabel.textContent = "EXAMS";
      const examList = document.createElement("div");
      examList.className = "exam-list fill-list";
      if (!p.exams?.length) {
        const empty = document.createElement("div");
        empty.className = "exam-empty";
        empty.textContent = "no upcoming exams";
        examList.append(empty);
      }
      for (const ex of p.exams ?? []) {
        const row = document.createElement("div");
        row.className = "exam-row";
        const date = document.createElement("span");
        date.className = "exam-date";
        date.textContent = ex.time ? `${ex.date} ${ex.time}` : ex.date;
        const subj = document.createElement("span");
        subj.className = "exam-subj";
        subj.textContent = ex.subject;
        const room = document.createElement("span");
        room.className = "exam-room";
        room.textContent = ex.room;
        row.append(date, subj, room);
        examList.append(row);
      }
      examsCol.append(examsLabel, examList);
      bottom.append(coursesCol, examsCol);
      body.append(bottom);
    } else {
      body.append(coursesLabel, courseList);
    }
  } else {
    const span = document.createElement("span");
    span.className = "disconnected";
    span.textContent = p.reason;
    const btn = document.createElement("button");
    btn.className = "connect-btn";
    btn.textContent = "CONNECT";
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      btn.textContent = "WAITING FOR LOGIN…";
      try {
        await window.__TAURI__.core.invoke("stag_login");
      } catch (err) {
        span.textContent = String(err);
        btn.disabled = false;
        btn.textContent = "CONNECT";
      }
    });
    body.append(span, btn);
  }
}

async function initMusicControls() {
  setInterval(updateMusicProgress, 1000);
  const controls = document.getElementById("music-controls");
  controls.addEventListener("click", async (e) => {
    const btn = e.target.closest(".music-btn");
    if (!btn) return;
    try {
      await window.__TAURI__.core.invoke("music_control", { action: btn.dataset.action });
    } catch (err) {
      console.error("music_control failed:", err);
    }
  });

  const list = document.getElementById("playlist-list");
  try {
    const names = await window.__TAURI__.core.invoke("music_playlists");
    for (const name of names) {
      const li = document.createElement("li");
      li.className = "playlist-item";
      const icon = document.createElement("span");
      icon.className = "playlist-shuffle";
      icon.innerHTML = '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M16 3h5v5h-2V6.4l-9 9-1.4-1.4 9-9H16zm-9.6 9.6L3 16.2 4.4 17.6 8 14zM17 16h-1.6l-2.5-2.5-1.4 1.4L14 17.6V19h-3v2h5v-3l1.6 1.6L21 17.8 17 13.8z"/></svg>';
      const label = document.createElement("span");
      label.className = "playlist-name";
      label.textContent = name;
      li.append(icon, label);
      li.addEventListener("click", async () => {
        try {
          await window.__TAURI__.core.invoke("music_play_playlist", { name });
        } catch (err) {
          console.error("music_play_playlist failed:", err);
        }
      });
      list.append(li);
    }
  } catch (err) {
    console.error("music_playlists failed:", err);
  }
}

function initPinToggle() {
  const btn = document.getElementById("pin-toggle");
  let above = false;
  btn.addEventListener("click", async () => {
    try {
      await window.__TAURI__.core.invoke("set_overlay", { above: !above });
      above = !above;
      btn.classList.toggle("active", above);
      btn.title = above ? "Return to desktop layer" : "Pin above windows";
    } catch (e) {
      console.error("set_overlay failed:", e);
    }
  });
}

// ── customizable layout ─────────────────────────────────────
// Phone-style widget board: a 12×6 cell grid where every widget
// occupies a cell rect. Size presets (s/m/l → [cols, rows]) work like
// Apple's widget sizes; content adapts via container queries in CSS.
// Edit mode (pencil in the header) allows drag-to-move, size cycling,
// and hiding widgets into a tray. Layout persists in localStorage.

const GRID_COLS = 12;
const GRID_ROWS = 6;

// home = [col, row, size] — the default board mirrors the original design.
const WIDGETS = {
  hardware:   { sizes: { s: [4, 1], m: [4, 2], l: [6, 2] }, home: [1, 1, "m"] },
  music:      { sizes: { s: [4, 1], m: [4, 2], l: [6, 3] }, home: [1, 3, "m"] },
  discord:    { sizes: { s: [4, 1], m: [4, 2], l: [5, 3] }, home: [1, 5, "m"] },
  stag:       { sizes: { s: [5, 2], m: [5, 4], l: [8, 4] }, home: [5, 1, "m"] },
  email:      { sizes: { s: [5, 1], m: [5, 2], l: [5, 4] }, home: [5, 5, "m"] },
  github:     { sizes: { s: [3, 1], m: [3, 2], l: [5, 3] }, home: [10, 1, "m"] },
  crypto:     { sizes: { s: [3, 1], m: [3, 2], l: [5, 2] }, home: [10, 3, "m"] },
  screentime: { sizes: { s: [3, 1], m: [3, 2], l: [3, 3] }, home: [10, 5, "m"] },
};

let layout = loadLayout();
normalizeLayout();

function defaultLayout() {
  const l = {};
  for (const [id, w] of Object.entries(WIDGETS)) {
    l[id] = { c: w.home[0], r: w.home[1], size: w.home[2], hidden: false };
  }
  return l;
}

function loadLayout() {
  const base = defaultLayout();
  try {
    const saved = JSON.parse(localStorage.getItem("aria-layout"));
    for (const id of Object.keys(base)) {
      if (saved?.[id] && WIDGETS[id].sizes[saved[id].size]) {
        Object.assign(base[id], saved[id]);
      }
    }
  } catch {}
  return base;
}

function saveLayout() {
  try {
    localStorage.setItem("aria-layout", JSON.stringify(layout));
  } catch {}
}

// Saved rects go stale when a widget's size presets change between
// versions: clamp each one back into the grid, relocate on collision,
// fall back to a smaller size, and hide (tray-recoverable) as a last
// resort — so applyLayout never renders an overlapping board.
function normalizeLayout() {
  let changed = false;
  for (const id of Object.keys(WIDGETS)) {
    const p = layout[id];
    if (p.hidden) continue;
    let placed = false;
    for (const size of [...new Set([p.size, "m", "s"])]) {
      const dims = WIDGETS[id].sizes[size];
      if (!dims) continue;
      const [w, h] = dims;
      const c = Math.min(Math.max(p.c, 1), GRID_COLS - w + 1);
      const r = Math.min(Math.max(p.r, 1), GRID_ROWS - h + 1);
      const spot = fits({ c, r, w, h }, id) ? [c, r] : findFreeSpot(w, h, id);
      if (spot) {
        changed ||= spot[0] !== p.c || spot[1] !== p.r || size !== p.size;
        Object.assign(p, { size, c: spot[0], r: spot[1] });
        placed = true;
        break;
      }
    }
    if (!placed) {
      p.hidden = true;
      changed = true;
    }
  }
  if (changed) saveLayout();
}

function widgetEl(id) {
  return document.getElementById(`widget-${id}`);
}

function rectOf(id) {
  const p = layout[id];
  const [w, h] = WIDGETS[id].sizes[p.size];
  return { c: p.c, r: p.r, w, h };
}

function overlaps(a, b) {
  return a.c < b.c + b.w && b.c < a.c + a.w && a.r < b.r + b.h && b.r < a.r + a.h;
}

function fits(rect, ignoreId) {
  if (rect.c < 1 || rect.r < 1) return false;
  if (rect.c + rect.w > GRID_COLS + 1 || rect.r + rect.h > GRID_ROWS + 1) return false;
  return Object.keys(WIDGETS).every(
    (id) => id === ignoreId || layout[id].hidden || !overlaps(rect, rectOf(id)),
  );
}

function findFreeSpot(w, h, ignoreId) {
  for (let r = 1; r <= GRID_ROWS - h + 1; r++) {
    for (let c = 1; c <= GRID_COLS - w + 1; c++) {
      if (fits({ c, r, w, h }, ignoreId)) return [c, r];
    }
  }
  return null;
}

function applyLayout() {
  const chips = document.getElementById("tray-chips");
  chips.replaceChildren();
  for (const id of Object.keys(WIDGETS)) {
    const el = widgetEl(id);
    const p = layout[id];
    if (p.hidden) {
      el.style.display = "none";
      const chip = document.createElement("button");
      chip.className = "tray-chip";
      chip.textContent = `+ ${el.querySelector(".widget-label").textContent}`;
      chip.addEventListener("click", () => showWidget(id));
      chips.append(chip);
    } else {
      const [w, h] = WIDGETS[id].sizes[p.size];
      el.style.display = "";
      el.style.gridArea = `${p.r} / ${p.c} / span ${h} / span ${w}`;
      el.dataset.size = p.size;
      const btn = el.querySelector(".w-size");
      if (btn) btn.textContent = p.size.toUpperCase();
    }
  }
  // Widget content ladders on the widget's size — re-render everything.
  renderCrypto();
  renderDiscord();
  renderGithub();
  renderEmail();
  renderStag();
  renderScreentime();
  renderMusic();
}

function showWidget(id) {
  const p = layout[id];
  // Prefer the size it was hidden at; fall back to smaller ones.
  for (const size of [...new Set([p.size, "m", "s"])]) {
    const dims = WIDGETS[id].sizes[size];
    if (!dims) continue;
    const spot = findFreeSpot(dims[0], dims[1], id);
    if (spot) {
      Object.assign(p, { hidden: false, size, c: spot[0], r: spot[1] });
      saveLayout();
      applyLayout();
      return;
    }
  }
}

function cycleSize(id) {
  const order = Object.keys(WIDGETS[id].sizes);
  const p = layout[id];
  // Walk the cycle and take the first size that fits somewhere, so the
  // button always responds even when the board is too full for one step
  // (e.g. m→l blocked on a packed board falls through to s).
  for (let i = 1; i < order.length; i++) {
    const next = order[(order.indexOf(p.size) + i) % order.length];
    const [w, h] = WIDGETS[id].sizes[next];
    // Stay anchored at the current cell, clamped to the grid; nudge if blocked.
    let c = Math.min(p.c, GRID_COLS - w + 1);
    let r = Math.min(p.r, GRID_ROWS - h + 1);
    if (!fits({ c, r, w, h }, id)) {
      const spot = findFreeSpot(w, h, id);
      if (!spot) continue;
      [c, r] = spot;
    }
    Object.assign(p, { size: next, c, r });
    saveLayout();
    applyLayout();
    return;
  }
}

function initDrag(id, el, grid) {
  el.addEventListener("pointerdown", (e) => {
    if (!document.body.classList.contains("editing")) return;
    if (e.target.closest(".w-controls")) return;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    el.classList.add("dragging");
    const box = grid.getBoundingClientRect();
    const { w, h } = rectOf(id);
    // Keep the grab point under the cursor while snapping to cells.
    const wBox = el.getBoundingClientRect();
    const offX = e.clientX - wBox.left;
    const offY = e.clientY - wBox.top;

    const onMove = (ev) => {
      const c = Math.round(((ev.clientX - offX - box.left) / box.width) * GRID_COLS) + 1;
      const r = Math.round(((ev.clientY - offY - box.top) / box.height) * GRID_ROWS) + 1;
      const rect = {
        c: Math.max(1, Math.min(c, GRID_COLS - w + 1)),
        r: Math.max(1, Math.min(r, GRID_ROWS - h + 1)),
        w,
        h,
      };
      const p = layout[id];
      if ((rect.c !== p.c || rect.r !== p.r) && fits(rect, id)) {
        p.c = rect.c;
        p.r = rect.r;
        applyLayout();
      }
    };
    const onUp = () => {
      el.classList.remove("dragging");
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", onUp);
      saveLayout();
    };
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", onUp);
  });
}

function initLayout() {
  const grid = document.querySelector(".hud-grid");
  const editBtn = document.getElementById("edit-toggle");
  const tray = document.getElementById("widget-tray");

  // Widgets with an account get a gear that opens their log-in/out panel.
  const AUTH_WIDGETS = new Set(["discord", "github", "email", "stag"]);

  for (const id of Object.keys(WIDGETS)) {
    const el = widgetEl(id);
    const controls = document.createElement("div");
    controls.className = "w-controls";
    if (AUTH_WIDGETS.has(id)) {
      const auth = document.createElement("button");
      auth.className = "w-btn w-auth";
      auth.textContent = "⚙";
      auth.title = "Account settings";
      auth.addEventListener("click", () => {
        // Leave edit mode so the panel is interactive (edit mode disables
        // pointer events inside widgets).
        document.body.classList.remove("editing");
        editBtn.classList.remove("active");
        tray.hidden = true;
        openAuthPanel(id);
      });
      controls.append(auth);
    }
    const size = document.createElement("button");
    size.className = "w-btn w-size";
    size.title = "Cycle size";
    size.addEventListener("click", () => cycleSize(id));
    const hide = document.createElement("button");
    hide.className = "w-btn w-hide";
    hide.textContent = "×";
    hide.title = "Hide widget";
    hide.addEventListener("click", () => {
      layout[id].hidden = true;
      saveLayout();
      applyLayout();
    });
    controls.append(size, hide);
    el.append(controls);
    initDrag(id, el, grid);
  }

  editBtn.addEventListener("click", () => {
    const editing = document.body.classList.toggle("editing");
    editBtn.classList.toggle("active", editing);
    tray.hidden = !editing;
  });

  document.getElementById("tray-reset").addEventListener("click", () => {
    layout = defaultLayout();
    saveLayout();
    applyLayout();
  });

  applyLayout();
}

const THEME_NAMES = ["studio", "jarvis", "porcelain"];

function redrawGauges() {
  for (const name of ["cpu", "ram", "gpu"]) {
    const s = gaugeState[name];
    drawGauge(document.getElementById(`gauge-${name}`), s ? s.value : 0);
  }
}

function applyTheme(name) {
  if (!THEME_NAMES.includes(name)) name = "studio";
  document.documentElement.dataset.theme = name;
  document.getElementById("theme-css").href = `theme-${name}.css`;
  try {
    localStorage.setItem("aria-theme", name);
  } catch {}
  for (const b of document.querySelectorAll("#theme-menu button")) {
    b.classList.toggle("active", b.dataset.theme === name);
  }
  redrawGauges();
}

function initThemePicker() {
  const btn = document.getElementById("theme-btn");
  const menu = document.getElementById("theme-menu");
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    menu.hidden = !menu.hidden;
  });
  menu.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-theme]");
    if (b) {
      applyTheme(b.dataset.theme);
      menu.hidden = true;
    }
  });
  document.addEventListener("click", () => {
    menu.hidden = true;
  });
  let saved = "studio";
  try {
    saved = localStorage.getItem("aria-theme") || "studio";
  } catch {}
  applyTheme(saved);
}

/* ── window placement: monitor picker, fill screen, move/resize ──
   Geometry persists in physical pixels (per-monitor-DPI safe) under
   "aria-window": { x, y, w, h, fill, preFill: {w, h} }. */

function loadWinState() {
  try {
    return JSON.parse(localStorage.getItem("aria-window")) || null;
  } catch {
    return null;
  }
}

function saveWinState(patch) {
  const s = { ...(loadWinState() || {}), ...patch };
  try {
    localStorage.setItem("aria-window", JSON.stringify(s));
  } catch {}
  return s;
}

// Work area excludes the taskbar/Dock; older API bundles lack it.
function workArea(m) {
  return m.workArea || { position: m.position, size: m.size };
}

async function placeCentered(m, w, h) {
  const W = window.__TAURI__.window;
  const wa = workArea(m);
  const cw = Math.min(w, wa.size.width);
  const ch = Math.min(h, wa.size.height);
  const win = W.getCurrentWindow();
  await win.setSize(new W.PhysicalSize(cw, ch));
  await win.setPosition(
    new W.PhysicalPosition(
      Math.round(wa.position.x + (wa.size.width - cw) / 2),
      Math.round(wa.position.y + (wa.size.height - ch) / 2),
    ),
  );
}

async function fillMonitor(m) {
  const W = window.__TAURI__.window;
  const wa = workArea(m);
  const win = W.getCurrentWindow();
  await win.setPosition(new W.PhysicalPosition(wa.position.x, wa.position.y));
  await win.setSize(new W.PhysicalSize(wa.size.width, wa.size.height));
}

async function restoreWindowState() {
  const s = loadWinState();
  if (!s) return; // first run: keep tauri.conf defaults
  const W = window.__TAURI__.window;
  try {
    const monitors = await W.availableMonitors();
    const cx = s.x + s.w / 2;
    const cy = s.y + s.h / 2;
    const target = monitors.find((m) => {
      const { position: p, size: z } = m;
      return cx >= p.x && cx < p.x + z.width && cy >= p.y && cy < p.y + z.height;
    });
    if (!target) {
      // Saved monitor is gone; land on the primary instead.
      const primary = await W.primaryMonitor();
      if (primary) {
        saveWinState({ fill: false });
        await placeCentered(primary, s.w, s.h);
      }
      return;
    }
    if (s.fill) {
      await fillMonitor(target);
    } else {
      const win = W.getCurrentWindow();
      await win.setPosition(new W.PhysicalPosition(s.x, s.y));
      await win.setSize(new W.PhysicalSize(s.w, s.h));
    }
  } catch (e) {
    console.error("window restore failed:", e);
  }
}

function initDisplayMenu() {
  const W = window.__TAURI__.window;
  const btn = document.getElementById("display-btn");
  const menu = document.getElementById("display-menu");
  const list = document.getElementById("display-monitors");
  const fillBtn = document.getElementById("fill-toggle");
  const autoBtn = document.getElementById("autostart-toggle");
  // No bundler, so no plugin guest JS — call the plugin commands directly.
  const autostart = (cmd) => window.__TAURI__.core.invoke(`plugin:autostart|${cmd}`);

  async function toggleAutostart() {
    try {
      const on = await autostart("is_enabled");
      await autostart(on ? "disable" : "enable");
      autoBtn.classList.toggle("active", !on);
    } catch (e) {
      console.error("autostart toggle failed:", e);
    }
  }

  async function moveTo(m) {
    if ((loadWinState() || {}).fill) {
      await fillMonitor(m);
      return;
    }
    // Keep the visual (logical) size when the target DPI differs.
    const win = W.getCurrentWindow();
    const size = await win.innerSize();
    const scale = await win.scaleFactor();
    await placeCentered(
      m,
      Math.round((size.width / scale) * m.scaleFactor),
      Math.round((size.height / scale) * m.scaleFactor),
    );
  }

  async function toggleFill() {
    const win = W.getCurrentWindow();
    const m = (await W.currentMonitor()) || (await W.primaryMonitor());
    if (!m) return;
    const s = loadWinState() || {};
    if (s.fill) {
      const scale = m.scaleFactor || 1;
      const pre = s.preFill || {
        w: Math.round(1280 * scale),
        h: Math.round(800 * scale),
      };
      saveWinState({ fill: false });
      await placeCentered(m, pre.w, pre.h);
    } else {
      const size = await win.innerSize();
      saveWinState({ fill: true, preFill: { w: size.width, h: size.height } });
      await fillMonitor(m);
    }
  }

  async function rebuild() {
    const [monitors, current] = await Promise.all([
      W.availableMonitors(),
      W.currentMonitor(),
    ]);
    list.replaceChildren();
    monitors.forEach((m, i) => {
      const b = document.createElement("button");
      b.textContent = `Display ${i + 1} — ${m.size.width}×${m.size.height}`;
      if (current && m.name === current.name) b.classList.add("active");
      b.addEventListener("click", () => moveTo(m));
      list.append(b);
    });
    fillBtn.classList.toggle("active", !!(loadWinState() || {}).fill);
    autostart("is_enabled")
      .then((on) => autoBtn.classList.toggle("active", !!on))
      .catch(() => {});
  }

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (menu.hidden) rebuild();
    menu.hidden = !menu.hidden;
  });
  fillBtn.addEventListener("click", toggleFill);
  autoBtn.addEventListener("click", toggleAutostart);
  document.addEventListener("click", () => {
    menu.hidden = true;
  });
}

function initWindowHandles() {
  const win = window.__TAURI__.window.getCurrentWindow();
  document.getElementById("win-move").addEventListener("pointerdown", (e) => {
    e.preventDefault();
    win.startDragging();
  });
  document.getElementById("win-resize").addEventListener("pointerdown", (e) => {
    e.preventDefault();
    win.startResizeDragging("SouthEast");
  });
}

// The OS owns move/resize drags (no completion callback), so geometry is
// captured from the window's own events, debounced.
function initWindowStateSaver() {
  const W = window.__TAURI__.window;
  const win = W.getCurrentWindow();
  let t = null;
  const queue = () => {
    clearTimeout(t);
    t = setTimeout(async () => {
      try {
        const pos = await win.outerPosition();
        const size = await win.innerSize();
        const patch = { x: pos.x, y: pos.y, w: size.width, h: size.height };
        const s = loadWinState() || {};
        if (s.fill) {
          // A size that no longer matches the work area means the user
          // resized manually and broke fill.
          const m = await W.currentMonitor();
          if (m) {
            const wa = workArea(m);
            if (size.width !== wa.size.width || size.height !== wa.size.height) {
              patch.fill = false;
            }
          }
        }
        saveWinState(patch);
      } catch {}
    }, 500);
  };
  win.onMoved(queue);
  win.onResized(queue);
}

window.addEventListener("DOMContentLoaded", async () => {
  restoreWindowState(); // async, fire-and-forget: reposition ASAP
  updateClock();
  setInterval(updateClock, 1000);
  // The interactivity gate is ⌥ on macOS, Alt elsewhere.
  if (!navigator.platform.includes("Mac")) {
    document.getElementById("mod-hint").textContent = "alt interact";
  }
  initThemePicker();
  initDisplayMenu();
  initWindowHandles();
  initWindowStateSaver();
  initLayout();
  initGauges();
  initPinToggle();
  initMusicControls();
  await initCollectors();
  // All listeners are now registered; let gated collectors start emitting.
  window.__TAURI__.core.invoke("frontend_ready");
});
