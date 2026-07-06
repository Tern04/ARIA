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

function setGauge(name, value) {
  drawGauge(document.getElementById(`gauge-${name}`), value);
  document.getElementById(`${name}-val`).textContent = Math.round(value);
}

function fmtDuration(secs) {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

// null = show all accounts; otherwise the label to filter the mail list by.
let mailFilter = null;

// Kind abbreviation → CSS modifier for colouring lecture vs seminar.
const CLASS_KIND = { "Př": "lecture", "Cv": "seminar", "Se": "seminar" };

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
    label.className = "tt-day";
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
async function initCollectors() {
  const { listen } = window.__TAURI__.event;
  const pending = [];
  const on = (event, cb) => pending.push(listen(event, cb));

  on("hardware", (e) => {
    setGauge("cpu", e.payload.cpu);
    setGauge("ram", e.payload.ram);
  });

  on("gpu", (e) => {
    setGauge("gpu", e.payload.gpu);
  });

  on("interactive", (e) => {
    document.body.classList.toggle("interactive", e.payload === true);
  });

  on("github", (e) => {
    const body = document.querySelector("#widget-github .widget-body");
    const p = e.payload;
    body.replaceChildren();
    if (p.status !== "connected") {
      const span = document.createElement("span");
      span.className = "disconnected";
      span.textContent = p.reason;
      body.append(span);
      return;
    }
    body.append(
      statRow(p.notifications, "notifications"),
      statRow(p.open_prs, "open PRs"),
    );
    if (p.recent_repo) {
      const repo = document.createElement("div");
      repo.className = "gh-repo";
      const name = document.createElement("span");
      name.className = "gh-repo-name";
      name.textContent = p.recent_repo.name;
      const when = document.createElement("span");
      when.className = "gh-repo-when";
      when.textContent = `pushed ${p.recent_repo.pushed_at}`;
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
  });

  on("email", (e) => {
    const p = e.payload;
    const summary = document.getElementById("mail-summary");
    const list = document.getElementById("mail-list");
    summary.replaceChildren();
    list.replaceChildren();
    if (p.status !== "connected") {
      const span = document.createElement("span");
      span.className = "disconnected";
      span.textContent = p.reason;
      summary.append(span);
      mailFilter = null;
      return;
    }

    const renderList = () => {
      list.replaceChildren();
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
        tag.textContent = msg.account;
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
  });

  on("screentime", (e) => {
    const body = document.querySelector("#widget-screentime .widget-body");
    const p = e.payload;
    body.replaceChildren();
    const total = document.createElement("div");
    total.className = "st-total";
    total.textContent = fmtDuration(p.total);
    body.append(total);
    for (const app of p.apps) {
      const row = document.createElement("div");
      row.className = "st-row";
      const name = document.createElement("span");
      name.className = "st-name";
      name.textContent = app.name;
      const time = document.createElement("span");
      time.className = "st-time";
      time.textContent = fmtDuration(app.secs);
      row.append(name, time);
      body.append(row);
    }
  });

  on("music", (e) => {
    const p = e.payload;
    const title = document.getElementById("music-title");
    const artist = document.getElementById("music-artist");
    if (p.status === "playing") {
      title.textContent = p.title;
      artist.textContent = p.artist;
    } else {
      title.textContent = "--";
      artist.textContent = "--";
    }
  });

  on("stag", (e) => {
    const body = document.querySelector("#widget-stag .widget-body");
    const p = e.payload;
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

      body.append(buildTimetable(p.timetable));

      const coursesLabel = document.createElement("div");
      coursesLabel.className = "stag-section";
      coursesLabel.textContent = "COURSES";
      body.append(coursesLabel);
      const courseList = document.createElement("div");
      courseList.className = "course-list";
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
      body.append(courseList);
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
  });

  await Promise.all(pending);
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

window.addEventListener("DOMContentLoaded", async () => {
  updateClock();
  setInterval(updateClock, 1000);
  initGauges();
  initPinToggle();
  await initCollectors();
  // All listeners are now registered; let gated collectors start emitting.
  window.__TAURI__.core.invoke("frontend_ready");
});
