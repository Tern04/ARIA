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

function statRow(value, label) {
  const row = document.createElement("div");
  row.className = "stat-row";
  const val = document.createElement("span");
  val.className = "stat-value";
  val.textContent = value;
  row.append(val, ` ${label}`);
  return row;
}

function initCollectors() {
  const { listen } = window.__TAURI__.event;

  listen("hardware", (e) => {
    setGauge("cpu", e.payload.cpu);
    setGauge("ram", e.payload.ram);
  });

  listen("gpu", (e) => {
    setGauge("gpu", e.payload.gpu);
  });

  listen("interactive", (e) => {
    document.body.classList.toggle("interactive", e.payload === true);
  });

  listen("github", (e) => {
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

  listen("email", (e) => {
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
      return;
    }
    for (const acct of p.accounts) {
      const chip = document.createElement("span");
      chip.className = "mail-acct";
      const count = document.createElement("span");
      count.className = "mail-acct-count";
      count.textContent = acct.unread ?? "!";
      if (acct.unread === null) chip.title = "account unreachable";
      const label = document.createElement("span");
      label.className = "mail-acct-label";
      label.textContent = acct.label;
      chip.append(count, label);
      summary.append(chip);
    }
    for (const msg of p.messages) {
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
  });

  listen("screentime", (e) => {
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

  listen("music", (e) => {
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

  listen("stag", (e) => {
    const body = document.querySelector("#widget-stag .widget-body");
    const p = e.payload;
    body.replaceChildren();
    if (p.status === "connected") {
      const program = document.createElement("div");
      program.className = "stag-program";
      program.textContent = p.program;
      body.append(program);
      if (p.semester) {
        const semester = document.createElement("div");
        semester.className = "stag-semester";
        semester.textContent = p.semester;
        body.append(semester);
      }

      if (p.days.length === 0) {
        const span = document.createElement("span");
        span.className = "disconnected";
        span.textContent = "no classes this week";
        body.append(span);
      }
      for (const day of p.days) {
        const label = document.createElement("div");
        label.className = day.today ? "day-label day-label--today" : "day-label";
        label.textContent = day.label;
        body.append(label);
        for (const c of day.classes) {
          const row = document.createElement("div");
          row.className = "class-row";
          const time = document.createElement("span");
          time.className = "class-time";
          time.textContent = c.time;
          const subject = document.createElement("span");
          subject.className = "class-subject";
          subject.textContent = c.subject;
          const kind = document.createElement("span");
          kind.className = "class-kind";
          kind.textContent = c.kind;
          const room = document.createElement("span");
          room.className = "class-room";
          room.textContent = c.room;
          row.append(time, subject, kind, room);
          body.append(row);
        }
      }

      const examsLabel = document.createElement("div");
      examsLabel.className = "stag-section";
      examsLabel.textContent = "EXAMS";
      body.append(examsLabel);
      if (p.exams.length === 0) {
        const none = document.createElement("span");
        none.className = "disconnected";
        none.textContent = "no upcoming exams";
        body.append(none);
      }
      for (const ex of p.exams) {
        const row = document.createElement("div");
        row.className = "class-row";
        const date = document.createElement("span");
        date.className = "class-time";
        date.textContent = ex.date;
        const subject = document.createElement("span");
        subject.className = "class-subject";
        subject.textContent = ex.subject;
        row.append(date, subject);
        body.append(row);
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
  });
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

window.addEventListener("DOMContentLoaded", () => {
  updateClock();
  setInterval(updateClock, 1000);
  initGauges();
  initPinToggle();
  initCollectors();
  // Listeners are registered; let gated collectors start emitting.
  window.__TAURI__.core.invoke("frontend_ready");
});
