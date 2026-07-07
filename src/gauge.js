const ACCENT = "#e8ac53";
const TRACK = "rgba(255, 255, 255, 0.07)";
const START = -Math.PI / 2;       // top of circle
const SWEEP = 2 * Math.PI * 0.75; // 270° arc

/**
 * Draw a clean circular gauge ring onto a canvas element.
 * @param {HTMLCanvasElement} canvas
 * @param {number} value  0–100
 */
export function drawGauge(canvas, value) {
  const ctx = canvas.getContext("2d");
  const w = canvas.width;
  const h = canvas.height;
  const cx = w / 2;
  const cy = h / 2;
  const r = w * 0.4;
  const lineWidth = w * 0.06;
  const arcStart = START - SWEEP / 2 + Math.PI / 2;
  const filled = (SWEEP * Math.max(0, Math.min(100, value))) / 100;

  ctx.clearRect(0, 0, w, h);

  // track
  ctx.beginPath();
  ctx.arc(cx, cy, r, arcStart, arcStart + SWEEP);
  ctx.strokeStyle = TRACK;
  ctx.lineWidth = lineWidth;
  ctx.lineCap = "round";
  ctx.stroke();

  // fill
  if (value > 0) {
    ctx.beginPath();
    ctx.arc(cx, cy, r, arcStart, arcStart + filled);
    ctx.strokeStyle = ACCENT;
    ctx.lineWidth = lineWidth;
    ctx.lineCap = "round";
    ctx.stroke();

    // bright dot at the arc tip
    const tip = arcStart + filled;
    const tx = cx + r * Math.cos(tip);
    const ty = cy + r * Math.sin(tip);
    ctx.beginPath();
    ctx.arc(tx, ty, lineWidth * 0.85, 0, Math.PI * 2);
    ctx.fillStyle = "#fff4e2";
    ctx.fill();
  }
}
