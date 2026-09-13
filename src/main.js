import { drawGauge } from "./gauge.js";
import { progressAt } from "./lib/music.js";
import { HISTORY_LEN, autoScale, latest, peak, pushSample, seriesPaths } from "./lib/history.js";
import { applyTerminalTheme, initTerminal, syncTerminal } from "./lib/terminal.js";
import { WIDGETS, defaultLayout } from "./lib/widgets.js";
import {
  applyBuiltins,
  boardToLayout,
  matchingPreset,
  normalizeName,
  parsePresets,
  withPreset,
  withoutPreset,
} from "./lib/presets.js";
import {
  GRID_COLS,
  GRID_ROWS,
  fits as fitsIn,
  findFreeSpot as findFreeSpotIn,
  normalizeLayout as normalizeLayoutIn,
  rectOf as rectOfIn,
} from "./lib/layout.js";

// Platform gates. macOS is the only place the ⌥ interactivity gate exists;
// Linux needs the opaque-background workaround (see base.css and CLAUDE.md)
// and the Mutter auto-maximize dance in the window helpers below.
const IS_MAC = navigator.platform.includes("Mac");
const IS_LINUX = navigator.platform.includes("Linux");

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
  const guild = authInput("server ID(s), comma separated");
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
  const servers = p.servers ?? [];

  // Small: a glanceable headline — how many people are in voice, where.
  if (size === "s") {
    const allChans = servers.flatMap((s) => s.voice);
    const total = allChans.reduce((n, c) => n + c.occupants.length, 0);
    body.append(statRow(total, total === 1 ? "person in voice" : "in voice"));
    const top = allChans.find((c) => c.occupants.length > 0);
    if (top) {
      const chan = document.createElement("div");
      chan.className = "dc-s-chan";
      chan.textContent = top.name;
      body.append(chan);
    }
    return;
  }

  // Channels arrive per server, sorted by popularity; render them all and
  // let the fill-list fade clip whatever doesn't fit. With more than one
  // server each group gets a faint server-name label.
  const list = document.createElement("div");
  list.className = "dc-voice fill-list";
  for (const server of servers) {
    if (servers.length > 1) {
      const label = document.createElement("div");
      label.className = "stag-section";
      label.textContent = server.name.toUpperCase();
      list.append(label);
    }
    for (const chan of server.voice) {
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
  }

  // Discord gives bots no way to read *your* friends list, so the watched
  // people are added by hand. Without this the widget just showed nothing and
  // looked broken.
  const friendsHint = () => {
    const hint = document.createElement("div");
    hint.className = "dc-friends-hint";
    hint.textContent = "no friends tracked — add them via ✎ edit mode → ⚙";
    hint.title =
      "Right-click a user in Discord → Copy User ID, then add them in the widget's settings. " +
      "They must share a watched server, and the bot needs the Presence intent.";
    return hint;
  };

  // Large: channels left, friends as a full column right (status dot,
  // name, current game/song per row) — more detail than the m strip.
  if (size === "l") {
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
    if (p.friends.length === 0) col.append(friendsHint());
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

  if (p.friends.length === 0) {
    body.append(friendsHint());
  } else {
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
let calData = null;
let calLastJson = null;

// Number of feed colours defined in CSS (.cal-dot--0 … --N-1); the collector's
// color index is taken modulo this.
const CAL_COLORS = 6;

function calFeedForm(err) {
  const form = document.createElement("div");
  form.className = "auth-form";
  const label = authInput("label (e.g. FAMILY)");
  const url = authInput("secret iCal URL", "password");
  const btn = authButton("ADD CALENDAR", async (b) => {
    const ok = await invokeAuth(b, err, "cal_add_feed", {
      label: label.value,
      url: url.value,
    });
    // The command refetches before resolving, so reopening shows the new feed.
    if (ok && authPanelId === "calendar") openAuthPanel("calendar");
  });
  form.append(label, url, btn);
  return form;
}

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
  ({
    discord: renderDiscord,
    github: renderGithub,
    email: renderEmail,
    stag: renderStag,
    calendar: renderCalendar,
  })[id]?.();
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

    // Friends manager: mirrors the mail account list, but reads the config
    // file so it works even while the gateway is disconnected.
    const label = document.createElement("div");
    label.className = "stag-section";
    label.textContent = "FRIENDS";
    const list = document.createElement("div");
    list.className = "acct-list";
    window.__TAURI__.core
      .invoke("discord_list_friends")
      .then((friends) => {
        for (const f of friends) {
          const row = document.createElement("div");
          row.className = "acct-row";
          const name = document.createElement("span");
          name.className = "acct-name";
          name.textContent = f.name;
          const who = document.createElement("span");
          who.className = "acct-user";
          who.textContent = f.id;
          const del = document.createElement("button");
          del.className = "w-btn";
          del.textContent = "×";
          del.title = "Remove friend";
          del.addEventListener("click", async () => {
            del.disabled = true;
            try {
              await window.__TAURI__.core.invoke("discord_remove_friend", { id: f.id });
              if (authPanelId === "discord") openAuthPanel("discord");
            } catch (e) {
              err.textContent = String(e);
              del.disabled = false;
            }
          });
          row.append(name, who, del);
          list.append(row);
        }
      })
      .catch((e) => {
        err.textContent = String(e);
      });
    const addForm = document.createElement("div");
    addForm.className = "auth-form";
    const fid = authInput("user ID (right-click user → Copy User ID)");
    const fname = authInput("display name");
    addForm.append(
      fid,
      fname,
      authButton("ADD FRIEND", async (b) => {
        const ok = await invokeAuth(b, err, "discord_add_friend", {
          id: fid.value,
          name: fname.value,
        });
        if (ok) openAuthPanel("discord");
      }),
    );
    panel.append(label, list, addForm);

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
  } else if (id === "calendar") {
    const list = document.createElement("div");
    list.className = "acct-list";
    for (const feed of calData?.status === "connected" ? calData.feeds : []) {
      const row = document.createElement("div");
      row.className = "acct-row";
      const dot = document.createElement("span");
      dot.className = `cal-dot cal-dot--${feed.color % CAL_COLORS}`;
      const name = document.createElement("span");
      name.className = "acct-name";
      name.textContent = feed.label;
      const status = document.createElement("span");
      status.className = "acct-user";
      status.textContent = feed.ok ? "" : "error";
      const del = document.createElement("button");
      del.className = "w-btn";
      del.textContent = "×";
      del.title = "Remove calendar";
      del.addEventListener("click", async () => {
        del.disabled = true;
        try {
          await window.__TAURI__.core.invoke("cal_remove_feed", { label: feed.label });
          if (authPanelId === "calendar") openAuthPanel("calendar");
        } catch (e) {
          err.textContent = String(e);
          del.disabled = false;
        }
      });
      row.append(dot, name, status, del);
      list.append(row);
    }
    panel.append(list, calFeedForm(err));
    const note = document.createElement("span");
    note.className = "auth-note";
    note.textContent = "Google Calendar → Settings → Integrate calendar → Secret address in iCal format";
    panel.append(note);
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

// Backends that can report *how long* the session was active but not *which*
// window had focus. GNOME on Wayland is the big one — Mutter implements
// ext-idle-notify but exposes no toplevel/focus protocol at all.
const ST_NO_APP_BREAKDOWN = {
  "idle-only": "no per-app breakdown on this compositor",
  unsupported: "no window or idle source on this session",
};

function renderScreentime() {
  if (!screentimeData) return;
  const p = screentimeData;
  const size = document.getElementById("widget-screentime").dataset.size;
  const body = document.querySelector("#widget-screentime .widget-body");
  // On a backend that can't name the focused window, say so — otherwise a
  // permanent bare "0m" is indistinguishable from a broken tracker.
  const note = ST_NO_APP_BREAKDOWN[p.source];
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
  if (note) {
    const why = document.createElement("div");
    why.className = "st-note";
    why.textContent = note;
    body.append(why);
    return; // there is no app list to draw
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

// Advance the progress bar between polls using the wall clock; called by
// renderMusic and a 1 s ticker so the bar moves without new samples. The
// drift cap and the "player has no position" case live in lib/music.js.
function updateMusicProgress() {
  const p = musicData;
  const wrap = document.getElementById("music-progress");
  const active = !!p && (p.status === "playing" || p.status === "paused");
  const r = active
    ? progressAt(p, musicSampledAt, Date.now())
    : { visible: false };
  wrap.hidden = !r.visible;
  if (!r.visible) return;
  // No total to measure against (browser media sessions publish no track
  // length) — show the running time on its own rather than an empty bar.
  wrap.classList.toggle("music-progress--no-total", !r.hasBar);
  document.getElementById("music-progress-fill").style.width = `${r.pct}%`;
  document.getElementById("music-progress-time").textContent = r.label;
}

function renderMusic() {
  if (!musicData) return;
  const p = musicData;
  const size = document.getElementById("widget-music").dataset.size;
  const title = document.getElementById("music-title");
  const artist = document.getElementById("music-artist");
  const album = document.getElementById("music-album");
  const art = document.getElementById("music-art");
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
      // A cover that fails to decode would otherwise leave an invisible
      // 72–140 px hole where the art should be.
      art.onerror = () => {
        art.hidden = true;
      };
      art.src = p.art;
      art.hidden = false;
    } else {
      art.hidden = true;
      art.removeAttribute("src");
    }
    controls.classList.remove("music-controls--idle");
  } else {
    title.textContent = "Nothing playing";
    artist.textContent = "—";
    album.hidden = true;
    art.hidden = true;
    art.removeAttribute("src");
    controls.classList.add("music-controls--idle");
  }
  // Play glyph when paused/stopped, pause glyph when playing.
  document.getElementById("music-playpause").classList.toggle("is-playing", playing);
  updateMusicProgress();
}

// ── telemetry widgets: GPU / THERMALS / PERF / PING ─────────
// The four gaming-loadout widgets. They share one payload cache and one
// history, because they read the same three collectors on the same beat: the
// GPU widget's temperature and the THERMALS row are the same reading, and
// splitting them would let the two disagree by a tick.

let hwData = null;
let gpuData = null;
let latencyData = null;
// When each payload last arrived. A collector that stops answering (nvidia-smi
// missing, so the GPU loop backs off for two minutes) must leave a gap in the
// graph, not a flat line at its last value — which would read as a genuine
// steady load.
let hwAt = 0;
let gpuAt = 0;
const STALE_MS = 8000;

const HISTORY = { cpu: [], gpu: [], ram: [], vram: [] };

const SVG_NS = "http://www.w3.org/2000/svg";
const SPARK_W = 100;
const SPARK_H = 24;

// Temperatures are shown against what the part is happy at, not a raw scale:
// [warm, hot] in °C. Drives run much cooler than silicon, hence their own band.
const TEMP_BANDS = { cpu: [70, 85], gpu: [72, 86], drive: [55, 70] };
// A bar from 0 °C would sit two-thirds full at idle and barely move, so the
// meters span the range a running machine actually occupies.
const TEMP_FLOOR = 30;
const TEMP_CEIL = 100;

function tempClass(kind, t) {
  const [warm, hot] = TEMP_BANDS[kind];
  if (t >= hot) return "is-hot";
  if (t >= warm) return "is-warm";
  return "";
}

/** °C as a fraction of the meter, clamped to the band it can usefully show. */
function tempPct(t) {
  return Math.max(0, Math.min(100, ((t - TEMP_FLOOR) / (TEMP_CEIL - TEMP_FLOOR)) * 100));
}

const dash = "—";
const orDash = (v, fmt) => (v === null || v === undefined ? dash : fmt(v));

/**
 * Repaint a sparkline in place: one polyline per unbroken run of readings,
 * each optionally over a filled area dropped to the baseline. The fill is what
 * makes a trace read as a load at a glance rather than as a squiggle — but it
 * is drawn per run, so a gap stays a gap instead of being floored to zero.
 */
function drawSpark(svg, values, opts = {}) {
  const { fill = false, ...geom } = opts;
  const runs = seriesPaths(values, { width: SPARK_W, height: SPARK_H, ...geom });
  const shapes = [];
  for (const points of runs) {
    if (fill) {
      const xs = points.split(" ").map((pt) => pt.split(",")[0]);
      const area = document.createElementNS(SVG_NS, "polygon");
      area.setAttribute("points", `${points} ${xs[xs.length - 1]},${SPARK_H} ${xs[0]},${SPARK_H}`);
      area.setAttribute("fill", "currentColor");
      area.setAttribute("opacity", "0.16");
      shapes.push(area);
    }
    const line = document.createElementNS(SVG_NS, "polyline");
    line.setAttribute("points", points);
    line.setAttribute("fill", "none");
    line.setAttribute("stroke", "currentColor");
    line.setAttribute("stroke-width", "1.5");
    line.setAttribute("vector-effect", "non-scaling-stroke");
    shapes.push(line);
  }
  svg.replaceChildren(...shapes);
}

function setMeter(el, pct) {
  el.style.width = `${Math.max(0, Math.min(100, pct))}%`;
}

function renderGpu() {
  const body = document.getElementById("widget-gpu").querySelector(".widget-body");
  const note = document.getElementById("gpu-note");
  if (!gpuData) {
    // Kept as "waiting" until the collector has had a chance to answer;
    // initTelemetry decides when silence means "this machine has no telemetry".
    return;
  }
  note.hidden = true;
  body.classList.remove("is-empty");
  const d = gpuData;

  document.getElementById("gpu-name").textContent = d.name ?? "GPU";
  document.getElementById("gpu-load").textContent = Math.round(d.gpu);
  setMeter(document.getElementById("gpu-load-fill"), d.gpu);
  document.getElementById("gpu-load-val").textContent = `${Math.round(d.gpu)}%`;

  const vram = document.getElementById("gpu-vram-fill");
  const vramVal = document.getElementById("gpu-vram-val");
  if (d.vram_total_mb) {
    const used = d.vram_used_mb ?? 0;
    setMeter(vram, (used / d.vram_total_mb) * 100);
    // GB, because a card's VRAM is sold in GB and 5.9/6 reads faster than
    // 6041/6144 mid-match.
    vramVal.textContent = `${(used / 1024).toFixed(1)}/${Math.round(d.vram_total_mb / 1024)}G`;
  } else {
    setMeter(vram, 0);
    vramVal.textContent = dash;
  }

  const temp = document.getElementById("gpu-temp");
  temp.textContent = orDash(d.temp_c, (v) => `${Math.round(v)}°`);
  temp.className = `stat-val ${d.temp_c === null ? "" : tempClass("gpu", d.temp_c)}`;
  document.getElementById("gpu-power").textContent = orDash(d.power_w, (v) =>
    d.power_limit_w ? `${Math.round(v)}/${Math.round(d.power_limit_w)}W` : `${Math.round(v)}W`,
  );
  document.getElementById("gpu-clock").textContent = orDash(d.clock_mhz, (v) => `${v}MHz`);
  document.getElementById("gpu-memclock").textContent = orDash(d.mem_clock_mhz, (v) => `${v}MHz`);
  document.getElementById("gpu-fan").textContent = orDash(d.fan_pct, (v) => `${Math.round(v)}%`);
  renderGpuSpark();
}

/** The large GPU panel's load curve. Shares the graph widget's history, so the
 *  two never disagree about what the GPU was doing a minute ago. */
function renderGpuSpark() {
  drawSpark(document.querySelector(".gpu-spark"), HISTORY.gpu, {
    min: 0,
    max: 100,
    len: HISTORY_LEN,
    fill: true,
  });
}

function renderThermals() {
  const rows = {
    cpu: hwData?.cpu_temp ?? null,
    gpu: gpuData?.temp_c ?? null,
    drive: hwData?.drive_temp ?? null,
  };
  for (const [kind, t] of Object.entries(rows)) {
    const row = document.querySelector(`.th-row[data-sensor="${kind}"]`);
    const fill = row.querySelector(".meter-fill");
    const val = row.querySelector(".th-val");
    if (t === null) {
      // No sensor is not 0 °C: say so, and leave the bar empty.
      row.className = "th-row is-absent";
      setMeter(fill, 0);
      val.textContent = dash;
      continue;
    }
    row.className = `th-row ${tempClass(kind, t)}`;
    setMeter(fill, tempPct(t));
    val.textContent = `${Math.round(t)}°C`;
  }
  document.getElementById("th-fan").textContent =
    `FAN ${orDash(gpuData?.fan_pct ?? null, (v) => `${Math.round(v)}%`)}`;
  document.getElementById("th-clock").textContent =
    `CLK ${hwData?.cpu_freq_mhz ? `${(hwData.cpu_freq_mhz / 1000).toFixed(1)} GHz` : dash}`;
}

const PERF_LABEL = {
  cpu: (v) => `${Math.round(v)}%`,
  gpu: (v) => `${Math.round(v)}%`,
  ram: (v) => `${Math.round(v)}%`,
  vram: (v) => `${Math.round(v)}%`,
};

function renderPerf() {
  for (const row of document.querySelectorAll("#pf-body .pf-row")) {
    const key = row.dataset.series;
    const values = HISTORY[key];
    drawSpark(row.querySelector(".pf-spark"), values, {
      min: 0,
      max: 100,
      len: HISTORY_LEN,
      fill: true,
    });
    const now = latest(values);
    const high = peak(values);
    row.querySelector(".pf-val").textContent =
      now === null ? dash : `${PERF_LABEL[key](now)}${high === null ? "" : ` ↑${Math.round(high)}`}`;
  }
}

function renderLatency() {
  const widget = document.getElementById("widget-latency");
  const d = latencyData;
  if (!d) return;
  document.getElementById("pg-host").textContent = d.host;
  const ms = document.getElementById("pg-ms");
  // A lost packet reads as a dash, and the widget goes red: for a game that is
  // worse news than a high-but-arriving ping, so it must not look like zero.
  ms.textContent = d.last_ms === null ? dash : d.last_ms.toFixed(d.last_ms < 10 ? 1 : 0);
  const hot = d.last_ms === null || d.last_ms >= 100 || d.loss_pct >= 20;
  widget.classList.toggle("is-hot", hot);
  widget.classList.toggle("is-warm", !hot && (d.last_ms >= 50 || d.loss_pct > 0));
  drawSpark(document.querySelector(".pg-spark"), d.samples, {
    ...autoScale(d.samples),
    len: d.samples.length > 30 ? d.samples.length : 30,
    fill: true,
  });
  document.getElementById("pg-avg").textContent =
    `avg ${orDash(d.avg_ms, (v) => `${v.toFixed(v < 10 ? 1 : 0)}ms`)}`;
  document.getElementById("pg-jitter").textContent =
    `jit ${orDash(d.jitter_ms, (v) => `${v.toFixed(1)}ms`)}`;
  document.getElementById("pg-loss").textContent = `loss ${Math.round(d.loss_pct)}%`;
}

const PING_HOST_KEY = "aria-ping-host";

/**
 * One clock for the history graph, rather than pushing from each collector's
 * own event: the CPU and GPU loops tick independently, so appending on arrival
 * would let the two traces drift apart and mean different times at the same x.
 */
function sampleHistory() {
  const fresh = (at) => Date.now() - at < STALE_MS;
  const hw = fresh(hwAt) ? hwData : null;
  const gpu = fresh(gpuAt) ? gpuData : null;
  pushSample(HISTORY.cpu, hw?.cpu ?? null);
  pushSample(HISTORY.ram, hw?.ram ?? null);
  pushSample(HISTORY.gpu, gpu?.gpu ?? null);
  pushSample(
    HISTORY.vram,
    gpu?.vram_total_mb ? ((gpu.vram_used_mb ?? 0) / gpu.vram_total_mb) * 100 : null,
  );
  renderPerf();
  renderGpuSpark();
}

function initTelemetry() {
  const form = document.getElementById("pg-form");
  const input = document.getElementById("pg-input");
  let host = null;
  try {
    host = localStorage.getItem(PING_HOST_KEY);
  } catch {}
  if (host) {
    input.value = host;
    // The collector starts on its default, so a saved host has to be re-applied
    // every launch.
    window.__TAURI__.core.invoke("latency_set_host", { host }).catch((e) => console.error(e));
  }
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const next = input.value.trim();
    if (!next) return;
    try {
      await window.__TAURI__.core.invoke("latency_set_host", { host: next });
      localStorage.setItem(PING_HOST_KEY, next);
      input.classList.remove("is-bad");
    } catch (err) {
      console.error("ping host rejected:", err);
      input.classList.add("is-bad");
    }
  });

  sampleHistory();
  setInterval(sampleHistory, 2000);

  // Silence past this point is an answer: no NVIDIA card, or no driver tool.
  setTimeout(() => {
    if (gpuData) return;
    const note = document.getElementById("gpu-note");
    note.textContent = "no GPU telemetry on this machine";
    note.hidden = false;
    document.getElementById("widget-gpu").querySelector(".widget-body").classList.add("is-empty");
    document.getElementById("gpu-name").textContent = dash;
  }, 15000);
}

async function initCollectors() {
  const { listen } = window.__TAURI__.event;
  const pending = [];
  const on = (event, cb) => pending.push(listen(event, cb));

  on("hardware", (e) => {
    const p = e.payload;
    hwData = p;
    hwAt = Date.now();
    renderThermals();
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
    gpuData = e.payload;
    gpuAt = Date.now();
    setGauge("gpu", e.payload.gpu);
    renderGpu();
    renderThermals();
  });

  on("latency", (e) => {
    latencyData = e.payload;
    renderLatency();
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

  on("calendar", (e) => {
    setStatus("status-cal", e.payload.status === "connected");
    const raw = JSON.stringify(e.payload);
    if (raw === calLastJson) return; // don't wipe the feed form mid-typing
    calLastJson = raw;
    calData = e.payload;
    renderCalendar();
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

// Group a flat, time-sorted event list into day sections. Each section gets a
// faint day header; rows then only carry the time, so the eye reads down a day
// at a glance.
function buildAgenda(events, limit) {
  const list = document.createElement("div");
  list.className = "cal-agenda fill-list";
  if (events.length === 0) {
    const empty = document.createElement("div");
    empty.className = "cal-empty";
    empty.textContent = "nothing scheduled";
    list.append(empty);
    return list;
  }
  let lastDay = null;
  for (const e of events.slice(0, limit)) {
    if (e.day_key !== lastDay) {
      lastDay = e.day_key;
      const sep = document.createElement("div");
      sep.className = "cal-day-sep";
      sep.textContent = e.day_label;
      list.append(sep);
    }
    const row = document.createElement("div");
    row.className = "cal-row";
    const when = document.createElement("span");
    when.className = "cal-when";
    when.textContent = e.all_day ? "all day" : e.time_label;
    const dot = document.createElement("span");
    dot.className = `cal-dot cal-dot--${e.color % CAL_COLORS}`;
    dot.title = e.feed;
    const title = document.createElement("span");
    title.className = "cal-title";
    title.textContent = e.title;
    row.append(when, dot, title);
    if (e.location) {
      const loc = document.createElement("span");
      loc.className = "cal-loc";
      loc.textContent = e.location;
      row.append(loc);
    }
    row.title = `${e.day_label}${e.time_label ? " " + e.time_label : ""} · ${e.title}${e.location ? " · " + e.location : ""}`;
    list.append(row);
  }
  return list;
}

// Current-month grid, Monday-first, with a dot on every day that has events
// and today ringed. Feeds the large size's left column.
function calMonthGrid(dayCounts) {
  const counts = new Map(dayCounts.map((d) => [d.date, d.count]));
  const now = new Date();
  const year = now.getFullYear();
  const month = now.getMonth();
  const grid = document.createElement("div");
  grid.className = "cal-month";
  const title = document.createElement("div");
  title.className = "cal-month-title";
  title.textContent = now.toLocaleDateString(undefined, { month: "long", year: "numeric" });
  grid.append(title);
  const cells = document.createElement("div");
  cells.className = "cal-month-grid";
  for (const d of ["M", "T", "W", "T", "F", "S", "S"]) {
    const h = document.createElement("span");
    h.className = "cal-month-dow";
    h.textContent = d;
    cells.append(h);
  }
  // Monday-based leading offset for the 1st of the month.
  const lead = (new Date(year, month, 1).getDay() + 6) % 7;
  for (let i = 0; i < lead; i++) cells.append(document.createElement("span"));
  const days = new Date(year, month + 1, 0).getDate();
  for (let d = 1; d <= days; d++) {
    const key = `${year}-${String(month + 1).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
    const cell = document.createElement("span");
    cell.className = "cal-month-day";
    if (d === now.getDate()) cell.classList.add("cal-month-day--today");
    if (counts.has(key)) cell.classList.add("cal-month-day--has");
    cell.textContent = d;
    cells.append(cell);
  }
  grid.append(cells);
  return grid;
}

function renderCalendar() {
  if (authPanelId === "calendar") return;
  const body = document.querySelector("#widget-calendar .widget-body");
  if (!calData) return;
  const p = calData;
  body.replaceChildren();

  if (p.status !== "connected") {
    const span = document.createElement("span");
    span.className = "disconnected";
    span.textContent = p.reason;
    const err = document.createElement("span");
    err.className = "disconnected";
    body.append(span, calFeedForm(err), err);
    return;
  }

  const size = document.getElementById("widget-calendar").dataset.size;
  const events = p.events ?? [];

  // Small: today's headline and the next thing coming up.
  if (size === "s") {
    const head = document.createElement("div");
    head.className = "cal-s-head";
    const date = document.createElement("span");
    date.className = "cal-s-date";
    date.textContent = new Date().toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
    const count = document.createElement("span");
    count.className = "cal-s-count";
    count.textContent = p.today_count === 1 ? "1 today" : `${p.today_count} today`;
    head.append(date, count);
    body.append(head);
    const next = events[0];
    if (next) {
      const row = document.createElement("div");
      row.className = "cal-s-next";
      const when = document.createElement("span");
      when.className = "cal-when";
      when.textContent = `${next.day_label}${next.time_label ? " " + next.time_label : ""}`;
      const dot = document.createElement("span");
      dot.className = `cal-dot cal-dot--${next.color % CAL_COLORS}`;
      const title = document.createElement("span");
      title.className = "cal-title";
      title.textContent = next.title;
      row.append(when, dot, title);
      body.append(row);
    } else {
      const empty = document.createElement("div");
      empty.className = "cal-empty";
      empty.textContent = "nothing scheduled";
      body.append(empty);
    }
    return;
  }

  // Large: month grid beside a taller agenda.
  if (size === "l") {
    const wrap = document.createElement("div");
    wrap.className = "cal-l";
    const left = document.createElement("div");
    left.className = "cal-col";
    left.append(calMonthGrid(p.day_counts ?? []));
    const right = document.createElement("div");
    right.className = "cal-col";
    const label = document.createElement("div");
    label.className = "stag-section";
    label.textContent = "UPCOMING";
    right.append(label, buildAgenda(events, 40));
    wrap.append(left, right);
    body.append(wrap);
    return;
  }

  // Medium: just the agenda.
  body.append(buildAgenda(events, 40));
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

let layout = loadLayout();
normalizeLayout();

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

// The placement rules live in lib/layout.js so they can be unit-tested; these
// bind them to this module's board state.
function normalizeLayout() {
  if (normalizeLayoutIn(layout, WIDGETS)) saveLayout();
}

function widgetEl(id) {
  return document.getElementById(`widget-${id}`);
}

const rectOf = (id) => rectOfIn(id, layout, WIDGETS);
const fits = (rect, ignoreId) => fitsIn(rect, ignoreId, layout, WIDGETS);
const findFreeSpot = (w, h, ignoreId) => findFreeSpotIn(w, h, ignoreId, layout, WIDGETS);

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
  renderCalendar();
  renderGpu();
  renderThermals();
  renderPerf();
  renderLatency();
  syncTerminal();
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
  const AUTH_WIDGETS = new Set(["discord", "github", "email", "stag", "calendar"]);
  // Slow-polling collectors get a poll-now button.
  const REFRESH_WIDGETS = { email: "email_refresh", github: "github_refresh", crypto: "crypto_refresh", stag: "stag_refresh", calendar: "cal_refresh" };

  for (const id of Object.keys(WIDGETS)) {
    const el = widgetEl(id);
    const controls = document.createElement("div");
    controls.className = "w-controls";
    if (REFRESH_WIDGETS[id]) {
      const refresh = document.createElement("button");
      refresh.className = "w-btn w-refresh";
      refresh.textContent = "↻";
      refresh.title = "Refresh now";
      refresh.addEventListener("click", async () => {
        // Leave edit mode so the incoming re-render is visible right away.
        document.body.classList.remove("editing");
        editBtn.classList.remove("active");
        tray.hidden = true;
        try {
          await window.__TAURI__.core.invoke(REFRESH_WIDGETS[id]);
        } catch (e) {
          console.error("refresh failed:", e);
        }
      });
      controls.append(refresh);
    }
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
    if (editing) refreshFilled(); // the window grips only show in edit mode
  });

  document.getElementById("tray-reset").addEventListener("click", () => {
    layout = defaultLayout();
    saveLayout();
    applyLayout();
  });

  applyLayout();
}

/* ── board presets ──────────────────────────────────────────
   Named snapshots of the layout, so a board arranged for coding and one
   arranged for a lecture day are both one click away. The list logic lives
   in lib/presets.js; this is the menu around it. */

const PRESETS_KEY = "aria-presets";
const SEEDED_KEY = "aria-presets-seeded";
let presets = [];

function persistPresets() {
  try {
    localStorage.setItem(PRESETS_KEY, JSON.stringify(presets));
  } catch {}
}

function initPresetMenu() {
  const btn = document.getElementById("preset-btn");
  const menu = document.getElementById("preset-menu");
  const list = document.getElementById("preset-list");
  const saveBtn = document.getElementById("preset-save");
  const form = document.getElementById("preset-new");
  const input = document.getElementById("preset-name");

  try {
    const stored = parsePresets(localStorage.getItem(PRESETS_KEY), WIDGETS);
    // The shipped presets are seeded per version, so one the user deletes stays
    // deleted while a revised one still reaches a board that has the old copy.
    const seeding = applyBuiltins(stored, localStorage.getItem(SEEDED_KEY), WIDGETS);
    presets = seeding.presets;
    if (presets !== stored) persistPresets();
    localStorage.setItem(SEEDED_KEY, JSON.stringify(seeding.seeded));
  } catch {}

  const closeNaming = () => {
    form.hidden = true;
    saveBtn.hidden = false;
    input.value = "";
  };

  const render = () => {
    list.replaceChildren();
    if (!presets.length) {
      const empty = document.createElement("div");
      empty.className = "preset-empty";
      empty.textContent = "No presets yet. Arrange the board, then save it.";
      list.append(empty);
      return;
    }
    // Marked active only on an exact match, so the tick means "the board is
    // this preset" rather than "this is the one you last clicked".
    const active = matchingPreset(layout, presets);
    for (const preset of presets) {
      const row = document.createElement("div");
      row.className = "preset-row";
      const apply = document.createElement("button");
      apply.className = "preset-apply";
      const label = document.createElement("span");
      label.className = "preset-label";
      label.textContent = preset.name;
      apply.append(label);
      apply.title = `Apply "${preset.name}"`;
      if (preset.name === active) apply.classList.add("active");
      apply.addEventListener("click", () => {
        layout = boardToLayout(preset.board, defaultLayout());
        // A preset can go stale — a widget's size preset may have changed
        // shape since it was saved — so it is normalized like any other
        // layout before being shown.
        normalizeLayout();
        saveLayout();
        applyLayout();
        menu.hidden = true;
        closeNaming();
      });
      const del = document.createElement("button");
      del.className = "preset-del";
      del.textContent = "×";
      del.title = `Delete "${preset.name}"`;
      del.setAttribute("aria-label", `Delete preset ${preset.name}`);
      del.addEventListener("click", () => {
        presets = withoutPreset(presets, preset.name);
        persistPresets();
        render();
      });
      row.append(apply, del);
      list.append(row);
    }
  };

  saveBtn.addEventListener("click", () => {
    saveBtn.hidden = true;
    form.hidden = false;
    // Offer the current preset's name so re-saving after a tweak is one Enter.
    input.value = matchingPreset(layout, presets) ?? "";
    input.focus();
    input.select();
  });

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    if (!normalizeName(input.value)) return;
    presets = withPreset(presets, input.value, layout, WIDGETS);
    persistPresets();
    closeNaming();
    render();
  });

  input.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeNaming();
  });

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    document.getElementById("theme-menu").hidden = true;
    document.getElementById("display-menu").hidden = true;
    document.getElementById("wallpaper-panel").hidden = true;
    if (menu.hidden) {
      closeNaming();
      render();
    }
    menu.hidden = !menu.hidden;
  });
  // Clicks inside must not reach the document handler below — typing a name
  // would otherwise dismiss the menu on the first click into the field.
  menu.addEventListener("click", (e) => e.stopPropagation());
  menu.addEventListener("pointerdown", (e) => e.stopPropagation());
  document.addEventListener("click", () => {
    menu.hidden = true;
    closeNaming();
  });
}

const THEME_NAMES = ["studio", "jarvis", "porcelain", "nord", "terminal"];

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
  applyTerminalTheme(name);
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
   "aria-window": { x, y, w, h, fill, preFill: {w, h}, mon: {x, y, w, h} }.
   `mon` is the monitor the window was last on, by geometry. */

// Wayland gives a client no say over, or knowledge of, its own position:
// setPosition() is dropped and outerPosition() is always {0,0}. Monitor
// geometry *is* real there, so placement goes by monitor instead, through
// window::place_on_monitor. Resolved once; everything below awaits it.
const onWayland = IS_LINUX
  ? window.__TAURI__.core.invoke("display_server").then((d) => d === "wayland").catch(() => false)
  : Promise.resolve(false);

// Monitor names are the EDID model string, so two identical panels share one
// name — which lit every entry in the picker at once. Geometry is unique.
function sameMonitor(a, b) {
  return (
    !!a && !!b &&
    a.position.x === b.position.x && a.position.y === b.position.y &&
    a.size.width === b.size.width && a.size.height === b.size.height
  );
}

function monitorRect(m) {
  return { x: m.position.x, y: m.position.y, w: m.size.width, h: m.size.height };
}

function matchesRect(m, r) {
  return !!r && m.position.x === r.x && m.position.y === r.y &&
    m.size.width === r.w && m.size.height === r.h;
}

// Wayland: move onto monitor m (maximized if fill), then restore a plain size.
async function placeOnMonitorWayland(m, fill, w, h) {
  const W = window.__TAURI__.window;
  const monitors = await W.availableMonitors();
  const index = monitors.findIndex((x) => sameMonitor(x, m));
  if (index < 0) return;
  await window.__TAURI__.core.invoke("place_on_monitor", { index, fill });
  if (!fill && w && h) {
    const wa = workArea(m);
    await W.getCurrentWindow().setSize(
      new W.PhysicalSize(Math.min(w, wa.size.width), Math.min(h, wa.size.height)),
    );
  }
}

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

// GNOME/Mutter auto-maximizes any window that asks for (nearly) the whole work
// area — exactly what fillMonitor does. A maximized X11 window then ignores
// setSize() and gtk_window_begin_move_drag(), so "Fill screen" looks stuck and
// the ✥ MOVE grip goes dead. Every geometry change drops maximization first.
async function unmaximize(win) {
  try {
    if (await win.isMaximized()) await win.unmaximize();
  } catch {}
}

async function isFilled() {
  if ((loadWinState() || {}).fill) return true;
  try {
    return await window.__TAURI__.window.getCurrentWindow().isMaximized();
  } catch {
    return false;
  }
}

async function placeCentered(m, w, h) {
  const W = window.__TAURI__.window;
  const wa = workArea(m);
  const cw = Math.min(w, wa.size.width);
  const ch = Math.min(h, wa.size.height);
  const win = W.getCurrentWindow();
  await unmaximize(win);
  await win.setSize(new W.PhysicalSize(cw, ch));
  await win.setPosition(
    new W.PhysicalPosition(
      Math.round(wa.position.x + (wa.size.width - cw) / 2),
      Math.round(wa.position.y + (wa.size.height - ch) / 2),
    ),
  );
}

async function fillMonitor(m) {
  if (await onWayland) return placeOnMonitorWayland(m, true);
  const W = window.__TAURI__.window;
  const wa = workArea(m);
  const win = W.getCurrentWindow();
  await unmaximize(win);
  await win.setPosition(new W.PhysicalPosition(wa.position.x, wa.position.y));
  if (IS_LINUX) {
    // Ask for maximization explicitly rather than letting Mutter infer it from
    // the size, so the state is one unmaximize() can undo.
    try {
      await win.maximize();
      return;
    } catch {}
  }
  await win.setSize(new W.PhysicalSize(wa.size.width, wa.size.height));
}

async function restoreWindowState() {
  const s = loadWinState();
  if (!s) return; // first run: keep tauri.conf defaults
  const W = window.__TAURI__.window;
  try {
    const monitors = await W.availableMonitors();
    const wayland = await onWayland;
    const cx = s.x + s.w / 2;
    const cy = s.y + s.h / 2;
    // The remembered monitor first; the saved centre point is the fallback for
    // state written before `mon` existed. Wayland's saved x/y are only ever
    // derived from `mon` (see the saver), so the fallback is sound there too.
    const target =
      monitors.find((m) => matchesRect(m, s.mon)) ||
      monitors.find((m) => {
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
    } else if (wayland) {
      await placeOnMonitorWayland(target, false, s.w, s.h);
    } else {
      const win = W.getCurrentWindow();
      // A window the WM left maximized ignores both calls below.
      await unmaximize(win);
      await win.setPosition(new W.PhysicalPosition(s.x, s.y));
      await win.setSize(new W.PhysicalSize(s.w, s.h));
    }
  } catch (e) {
    console.error("window restore failed:", e);
  }
  await refreshFilled();
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
  // The plugin registers the *running* exe. In dev that is the debug build,
  // which loads its UI from the tauri-dev server — at login there is no
  // server, so the window would come up as a WebView2 connection error
  // (plus a console window). Dev pages are served from a ported origin;
  // the installed app runs from tauri.localhost with no port.
  const devBuild = location.port !== "";
  if (devBuild) {
    autoBtn.disabled = true;
    autoBtn.title = "available in the installed app (a dev build would fail at login)";
  }

  async function toggleAutostart() {
    if (devBuild) return;
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
    } else {
      // Keep the visual (logical) size when the target DPI differs.
      const win = W.getCurrentWindow();
      const size = await win.innerSize();
      const scale = await win.scaleFactor();
      const w = Math.round((size.width / scale) * m.scaleFactor);
      const h = Math.round((size.height / scale) * m.scaleFactor);
      if (await onWayland) await placeOnMonitorWayland(m, false, w, h);
      else await placeCentered(m, w, h);
    }
    // Wayland never reports a move, so the saver would not record this.
    saveWinState({ mon: monitorRect(m) });
  }

  async function toggleFill() {
    const win = W.getCurrentWindow();
    const m = (await W.currentMonitor()) || (await W.primaryMonitor());
    if (!m) return;
    const s = loadWinState() || {};
    const scale = m.scaleFactor || 1;
    const fallback = { w: Math.round(1280 * scale), h: Math.round(800 * scale) };
    // Trust the real window state, not the saved flag alone: Mutter can
    // maximize us behind our back, and a stale flag used to capture the
    // fullscreen size as preFill — which left the window stuck filled forever.
    if (await isFilled()) {
      const pre = s.preFill || fallback;
      saveWinState({ fill: false });
      await placeCentered(m, pre.w, pre.h);
    } else {
      const size = await win.innerSize();
      const wa = workArea(m);
      // Never remember a "restore" size that is itself the filled size.
      const pre =
        size.width >= wa.size.width && size.height >= wa.size.height
          ? s.preFill || fallback
          : { w: size.width, h: size.height };
      saveWinState({ fill: true, preFill: pre });
      await fillMonitor(m);
    }
    await refreshFilled();
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
      if (sameMonitor(m, current)) b.classList.add("active");
      b.addEventListener("click", () => moveTo(m));
      list.append(b);
    });
    fillBtn.classList.toggle("active", await isFilled());
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

// Cached so the grip handlers can decide synchronously — see the mousedown
// comment below; an await between the click and startDragging() loses the grab.
let winFilled = false;

async function refreshFilled() {
  winFilled = await isFilled();
  for (const id of ["win-move", "win-resize"]) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.toggle("is-disabled", winFilled);
    el.title = winFilled
      ? "Turn off Fill screen to move or resize the window"
      : id === "win-move"
        ? "Move window"
        : "Resize window";
  }
}

function initWindowHandles() {
  const win = window.__TAURI__.window.getCurrentWindow();
  // mousedown, not pointerdown, and no preventDefault(): GTK's
  // begin_move_drag()/begin_resize_drag() need the live button grab, which
  // preventDefault() plus the async IPC hop can lose on X11.
  document.getElementById("win-move").addEventListener("mousedown", (e) => {
    if (e.button !== 0 || winFilled) return;
    win.startDragging();
  });
  document.getElementById("win-resize").addEventListener("mousedown", (e) => {
    if (e.button !== 0 || winFilled) return;
    win.startResizeDragging("SouthEast");
  });
  refreshFilled();
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
        const size = await win.innerSize();
        const cur = await W.currentMonitor();
        const patch = { w: size.width, h: size.height };
        if (cur) patch.mon = monitorRect(cur);
        if (!(await onWayland)) {
          const pos = await win.outerPosition();
          patch.x = pos.x;
          patch.y = pos.y;
        } else if (cur) {
          // outerPosition() is a constant {0,0} here. Record the window as
          // centred on its monitor, so an X11 session restoring this state
          // lands on the same screen.
          patch.x = Math.round(cur.position.x + (cur.size.width - size.width) / 2);
          patch.y = Math.round(cur.position.y + (cur.size.height - size.height) / 2);
        }
        const s = loadWinState() || {};
        // A size that no longer matches the work area means the user resized
        // manually and broke fill — unless the window is genuinely maximized,
        // in which case the geometry can legitimately differ by the frame.
        if (s.fill && !(await win.isMaximized().catch(() => false))) {
          const m = await W.currentMonitor();
          if (m) {
            const wa = workArea(m);
            if (size.width !== wa.size.width || size.height !== wa.size.height) {
              patch.fill = false;
            }
          }
        }
        saveWinState(patch);
        await refreshFilled();
      } catch {}
    }, 500);
  };
  win.onMoved(queue);
  win.onResized(queue);
}

// ── wallpaper ───────────────────────────────────────────────────────────────
// The HUD's backdrop. On Linux it must exist at all: the window is opaque
// there (WebKitGTK+NVIDIA renders transparency as flickering black — see
// CLAUDE.md), so something has to be painted behind the glass. The default is
// the real desktop wallpaper; the user can instead pick any image or video, or
// turn it off.
//
// Seam alignment (desktop source only): the layer is sized to the whole monitor
// and shifted by the window's offset within it, so the image lines up
// pixel-for-pixel with the desktop showing around the window — the border gap
// and the panel strip a filled window can't cover both disappear. That needs
// the window's absolute position, which only X11 gives; on Wayland (COSMIC)
// outerPosition() and the monitor position both report {0,0} and the work area
// reports no panel inset, so we cover-fit the window instead. A picked file has
// nothing to line up with, so it always cover-fits.

let wallpaperCfg = null;

async function setupWallpaper() {
  const { core } = window.__TAURI__;
  try {
    wallpaperCfg = await core.invoke("wallpaper_config");
  } catch (e) {
    console.error("wallpaper_config failed:", e);
    wallpaperCfg = { source: IS_LINUX ? "desktop" : "none", fit: "cover", dim: 0, blur: 0 };
  }
  try {
    await applyWallpaper(wallpaperCfg);
  } catch (e) {
    showWallpaperError(e?.message || e);
  }
  watchWallpaperSources();
}

// Live source: the desktop wallpaper rotates (COSMIC every ~5 min) and can be
// changed by hand, so the backdrop follows it instead of going stale.
function watchWallpaperSources() {
  const { event } = window.__TAURI__;
  try {
    event.listen("desktop-background", (e) => {
      if (wallpaperCfg?.source === "desktop") setWallpaperMedia(e.payload, "image");
    });
  } catch {}
  // Stop decoding video while the window is minimised or fully occluded.
  document.addEventListener("visibilitychange", () => {
    const video = document.getElementById("wallpaper-video");
    if (video.hidden) return;
    if (document.hidden) video.pause();
    else video.play().catch(() => {});
  });
}

/** Report a wallpaper problem in the settings panel (and the console). */
function showWallpaperError(message) {
  console.error("wallpaper:", message);
  const el = document.getElementById("wp-error");
  if (!el) return;
  el.textContent = String(message);
  el.hidden = !message;
}

/**
 * Point the underlay at an image or a video.
 *
 * Images come over Tauri's asset protocol. Video cannot: WebKitGTK plays media
 * through GStreamer, which doesn't know wry's custom `asset:` scheme (an <img>
 * loads, a <video> fails instantly with FormatError), and a blob: URL built
 * from the same bytes is worse — the element reports readyState 4 and fires
 * `playing` while currentTime stays pinned at 0. Video therefore comes from the
 * loopback media server in wallpaper.rs, which is also what lets a large file
 * stream instead of being buffered whole.
 */
async function setWallpaperMedia(src, kind) {
  const img = document.getElementById("wallpaper-img");
  const video = document.getElementById("wallpaper-video");

  const clearVideo = () => {
    video.hidden = true;
    video.removeAttribute("src");
    video.load(); // drop the decoder
  };

  if (!src) {
    img.hidden = true;
    img.removeAttribute("src");
    clearVideo();
    return;
  }
  if (kind === "video") {
    img.hidden = true;
    img.removeAttribute("src");
    const url = await window.__TAURI__.core.invoke("wallpaper_media_url");
    if (!url) throw new Error("could not serve the video file");
    // A codec the system can't decode fails here, not at invoke time — on
    // Linux H.264 needs gstreamer1.0-libav, which is not installed by default.
    video.onerror = () => {
      showWallpaperError(
        video.error?.message ||
          "this video could not be played — the system may be missing a decoder for it",
      );
    };
    video.src = url;
    video.hidden = false;
    video.play().catch(() => {});
    return;
  }
  clearVideo();
  img.src = src;
  img.hidden = false;
}


async function applyWallpaper(cfg) {
  const { core } = window.__TAURI__;
  const root = document.documentElement;
  wallpaperCfg = cfg;

  root.style.setProperty("--wallpaper-fit", cfg.fit || "cover");
  root.style.setProperty("--wallpaper-dim", String(cfg.dim ?? 0));
  root.style.setProperty("--wallpaper-blur", `${cfg.blur ?? 0}px`);
  // A blur samples past the edges and would feather in the backdrop; scaling
  // up by roughly the bleed hides it.
  root.style.setProperty("--wallpaper-blur-scale", String((cfg.blur ?? 0) / 400));

  const video = document.getElementById("wallpaper-video");
  video.muted = cfg.muted !== false;
  video.loop = cfg.loop !== false;

  if (cfg.source === "file" && cfg.path) {
    await setWallpaperMedia(core.convertFileSrc(cfg.path), cfg.kind);
    return;
  }
  if (cfg.source === "desktop") {
    let uri = null;
    try {
      uri = await core.invoke("desktop_background");
    } catch {}
    await setWallpaperMedia(uri, "image");
    return;
  }
  await setWallpaperMedia(null);
}

function initWallpaperMenu() {
  const { core } = window.__TAURI__;
  const openBtn = document.getElementById("wallpaper-btn");
  const panel = document.getElementById("wallpaper-panel");
  const err = document.getElementById("wp-error");
  const pathEl = document.getElementById("wp-path");
  const dim = document.getElementById("wp-dim");
  const blur = document.getElementById("wp-blur");
  const muted = document.getElementById("wp-muted");

  // The desktop wallpaper is only resolvable on Linux (window::desktop_background).
  document.getElementById("wp-source-desktop").disabled = !IS_LINUX;

  const showError = (e) => {
    if (e) return showWallpaperError(e?.message || e);
    err.textContent = "";
    err.hidden = true;
  };

  const sync = () => {
    const c = wallpaperCfg || {};
    for (const b of panel.querySelectorAll("#wp-source button")) {
      b.classList.toggle("active", b.dataset.source === c.source);
    }
    for (const b of panel.querySelectorAll("#wp-fit button")) {
      b.classList.toggle("active", b.dataset.fit === (c.fit || "cover"));
    }
    pathEl.textContent = c.source === "file" && c.path ? c.path : "";
    dim.value = String(Math.round((c.dim ?? 0) * 100));
    blur.value = String(c.blur ?? 0);
    muted.checked = c.muted !== false;
    document.getElementById("wp-mute-row").hidden = c.kind !== "video" || c.source !== "file";
  };

  // Persist through Rust, which validates the path, grants asset access to it
  // and returns the config it actually stored.
  const commit = async (patch) => {
    showError(null);
    try {
      const saved = await core.invoke("wallpaper_set", {
        config: { ...wallpaperCfg, ...patch },
      });
      await applyWallpaper(saved);
      sync();
    } catch (e) {
      showError(e);
    }
  };

  panel.addEventListener("click", async (e) => {
    const btn = e.target.closest("button");
    if (!btn || btn.disabled) return;
    if (btn.dataset.fit) return commit({ fit: btn.dataset.fit });
    if (!btn.dataset.source) return;
    if (btn.dataset.source !== "file") return commit({ source: btn.dataset.source });
    // No bundler, so no plugin guest JS — call the plugin command directly.
    let picked = null;
    try {
      picked = await core.invoke("plugin:dialog|open", {
        options: {
          title: "Choose a wallpaper",
          multiple: false,
          directory: false,
          filters: [
            {
              name: "Images and video",
              extensions: [
                "jpg", "jpeg", "png", "webp", "gif", "bmp", "avif",
                "mp4", "webm", "mkv", "mov", "m4v", "ogv",
              ],
            },
          ],
        },
      });
    } catch (e) {
      return showError(e);
    }
    // The plugin returns a path, or {path} depending on version; and null on cancel.
    const path = typeof picked === "string" ? picked : picked?.path;
    if (path) await commit({ source: "file", path });
  });

  dim.addEventListener("input", () => {
    document.documentElement.style.setProperty("--wallpaper-dim", String(dim.value / 100));
  });
  dim.addEventListener("change", () => commit({ dim: Number(dim.value) / 100 }));
  blur.addEventListener("input", () => {
    document.documentElement.style.setProperty("--wallpaper-blur", `${blur.value}px`);
    document.documentElement.style.setProperty(
      "--wallpaper-blur-scale",
      String(Number(blur.value) / 400),
    );
  });
  blur.addEventListener("change", () => commit({ blur: Number(blur.value) }));
  muted.addEventListener("change", () => commit({ muted: muted.checked }));

  openBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    document.getElementById("display-menu").hidden = true;
    showError(null);
    sync();
    panel.hidden = !panel.hidden;
  });
  // The document-level click that closes the pickers must not fire for clicks
  // inside the panel — dragging a slider would otherwise dismiss it.
  panel.addEventListener("click", (e) => e.stopPropagation());
  panel.addEventListener("pointerdown", (e) => e.stopPropagation());
  document.addEventListener("click", () => {
    panel.hidden = true;
  });
}


window.addEventListener("DOMContentLoaded", async () => {
  restoreWindowState(); // async, fire-and-forget: reposition ASAP
  updateClock();
  setInterval(updateClock, 1000);
  // The interactivity gate is ⌥ on macOS and Alt on Windows. Linux has no
  // gate at all (window::spawn_interactivity_watch is a no-op there and never
  // emits "interactive"), so the hint would be a lie and the accent state
  // would never light — mark the HUD interactive and drop the hint.
  const modHint = document.getElementById("mod-hint");
  if (IS_LINUX) {
    modHint.hidden = true;
    document.body.classList.add("interactive");
  } else if (!IS_MAC) {
    modHint.textContent = "alt interact";
  }
  // Linux/WebKitGTK transparency workaround (see base.css and CLAUDE.md):
  // WebKitGTK on NVIDIA renders a transparent window's see-through regions as
  // flickering black garbage. So on Linux the HUD is *opaque* — we paint the
  // desktop wallpaper behind the glass instead of letting the window show
  // through. `.is-linux` also drops backdrop-filter (a no-op over a would-be
  // transparent surface). The solid fallback colour lives in base.css for when
  // the wallpaper can't be resolved (e.g. COSMIC's rotating folder).
  if (IS_LINUX) {
    document.documentElement.classList.add("is-linux");
  }
  setupWallpaper();
  initThemePicker();
  initDisplayMenu();
  initWallpaperMenu();
  initWindowHandles();
  initWindowStateSaver();
  // Before initLayout(): its applyLayout() calls syncTerminal(), which starts
  // the shell if the terminal is on the board.
  initTerminal();
  initLayout();
  initPresetMenu(); // after initLayout(): reads the board it just built
  initGauges();
  initPinToggle();
  initMusicControls();
  initTelemetry();
  await initCollectors();
  // All listeners are now registered; let gated collectors start emitting.
  window.__TAURI__.core.invoke("frontend_ready");
});
