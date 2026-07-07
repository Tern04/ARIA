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
THEMES.reactor = THEMES.jarvis;

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
  ctx.strokeStyle = cfg.accent;
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
