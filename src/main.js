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

function initCollectors() {
  const { listen } = window.__TAURI__.event;

  listen("hardware", (e) => {
    setGauge("cpu", e.payload.cpu);
    setGauge("ram", e.payload.ram);
  });

  listen("github", (e) => {
    const body = document.querySelector("#widget-github .widget-body");
    const p = e.payload;
    if (p.status === "connected") {
      body.innerHTML = `
        <div class="stat-row"><span class="stat-value">${p.notifications}</span> notifications</div>
        <div class="stat-row"><span class="stat-value">${p.open_prs}</span> open PRs</div>`;
    } else {
      body.innerHTML = `<span class="disconnected">${p.reason}</span>`;
    }
  });

  listen("email", (e) => {
    const p = e.payload;
    const count = document.getElementById("email-count");
    if (p.status === "connected") {
      count.textContent = p.unread;
    } else {
      count.textContent = "--";
      count.title = p.reason;
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
});
