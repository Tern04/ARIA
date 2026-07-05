const ACCENT = "#00d4ff";
const TRACK = "rgba(0, 212, 255, 0.1)";
const START = -Math.PI / 2;       // top of circle
const SWEEP = 2 * Math.PI * 0.75; // 270° arc

/**
 * Draw a circular gauge arc onto a canvas element.
 * @param {HTMLCanvasElement} canvas
 * @param {number} value  0–100
 */
export function drawGauge(canvas, value) {
  const ctx = canvas.getContext("2d");
  const w = canvas.width;
  const h = canvas.height;
  const cx = w / 2;
  const cy = h / 2;
  const r = w * 0.38;
  const lineWidth = w * 0.09;
  const arcStart = START - SWEEP / 2 + Math.PI / 2;
  const filled = SWEEP * Math.max(0, Math.min(100, value)) / 100;

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
    ctx.shadowColor = ACCENT;
    ctx.shadowBlur = 10;
    ctx.stroke();
    ctx.shadowBlur = 0;
  }
}
