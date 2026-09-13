const START = -Math.PI / 2;       // top of circle
const SWEEP = 2 * Math.PI * 0.75; // 270° arc

// Per-theme gauge styling.
const THEMES = {
  studio: {
    accent: "#e8ac53",
    track: "rgba(255, 255, 255, 0.07)",
    r: 0.4,
    lw: 0.06,
    tip: "#fff4e2",
    glow: false,
  },
  jarvis: {
    accent: "#00d4ff",
    track: "rgba(0, 212, 255, 0.1)",
    r: 0.37,
    lw: 0.115,
    tip: null,
    glow: true,
  },
};
THEMES.porcelain = {
  accent: "#3450d2",
  track: "rgba(22, 24, 30, 0.09)",
  r: 0.4,
  lw: 0.06,
  tip: "#16181d",
  glow: false,
};
THEMES.nord = {
  accent: "#88c0d0",
  track: "rgba(216, 222, 233, 0.12)",
  r: 0.4,
  lw: 0.06,
  tip: "#eceff4",
  glow: false,
};
THEMES.aurora = {
  accent: "#3ee89b",
  // The arc shades from glacier cyan at rest to the curtain's green at full.
  accentFrom: "#4cc9e0",
  track: "rgba(190, 255, 225, 0.08)",
  r: 0.4,
  lw: 0.065,
  tip: "#e4f4ef",
  glow: true,
};
THEMES.scuderia = {
  accent: "#ff2800",
  // Rev lights: giallo at low load, into the red as the gauge fills.
  accentFrom: "#ffd500",
  track: "rgba(255, 236, 228, 0.08)",
  r: 0.4,
  lw: 0.075,
  tip: "#ffffff",
  glow: true,
};
THEMES.terminal = {
  accent: "#33ff66",
  track: "rgba(51, 255, 102, 0.12)",
  r: 0.4,
  lw: 0.05,
  tip: null,
  glow: true,
};

function themeConfig() {
  return THEMES[document.documentElement.dataset.theme] || THEMES.studio;
}

/**
 * Draw a circular gauge onto a canvas, styled to the active theme.
 * @param {HTMLCanvasElement} canvas
 * @param {number} value  0–100
 */
export function drawGauge(canvas, value) {
  const cfg = themeConfig();
  const ctx = canvas.getContext("2d");
  const w = canvas.width;
  const h = canvas.height;
  const cx = w / 2;
  const cy = h / 2;
  const r = w * cfg.r;
  const lineWidth = w * cfg.lw;
  const arcStart = START - SWEEP / 2 + Math.PI / 2;
  const filled = (SWEEP * Math.max(0, Math.min(100, value))) / 100;

  ctx.clearRect(0, 0, w, h);

  // track
  ctx.beginPath();
  ctx.arc(cx, cy, r, arcStart, arcStart + SWEEP);
  ctx.strokeStyle = cfg.track;
  ctx.lineWidth = lineWidth;
  ctx.lineCap = "round";
  ctx.stroke();

  if (value <= 0) return;

  // fill
  ctx.beginPath();
  ctx.arc(cx, cy, r, arcStart, arcStart + filled);
  if (cfg.accentFrom) {
    // Left-to-right matches the arc's sweep: it starts bottom-left and ends
    // bottom-right, so a fuller gauge reaches further into the accent.
    const grad = ctx.createLinearGradient(cx - r, 0, cx + r, 0);
    grad.addColorStop(0, cfg.accentFrom);
    grad.addColorStop(1, cfg.accent);
    ctx.strokeStyle = grad;
  } else {
    ctx.strokeStyle = cfg.accent;
  }
  ctx.lineWidth = lineWidth;
  ctx.lineCap = "round";
  if (cfg.glow) {
    ctx.shadowColor = cfg.accent;
    ctx.shadowBlur = 10;
  }
  ctx.stroke();
  ctx.shadowBlur = 0;

  // bright dot at the arc tip (studio)
  if (cfg.tip) {
    const tipA = arcStart + filled;
    ctx.beginPath();
    ctx.arc(cx + r * Math.cos(tipA), cy + r * Math.sin(tipA), lineWidth * 0.85, 0, Math.PI * 2);
    ctx.fillStyle = cfg.tip;
    ctx.fill();
  }
}
