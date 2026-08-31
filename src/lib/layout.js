// Board geometry for the customizable widget grid. Pure functions over a
// layout object, extracted from main.js so the placement rules can be tested
// (`npm test`) without a DOM or localStorage.
//
// A layout maps widget id -> { c, r, size, hidden } with 1-based grid
// coordinates. `widgets` maps widget id -> { sizes: { s|m|l: [w, h] }, ... }.

export const GRID_COLS = 12;
export const GRID_ROWS = 6;

/** Cell rect of a placed widget, from its current size preset. */
export function rectOf(id, layout, widgets) {
  const p = layout[id];
  const [w, h] = widgets[id].sizes[p.size];
  return { c: p.c, r: p.r, w, h };
}

export function overlaps(a, b) {
  return a.c < b.c + b.w && b.c < a.c + a.w && a.r < b.r + b.h && b.r < a.r + a.h;
}

/** True when `rect` is inside the grid and collides with nothing visible. */
export function fits(rect, ignoreId, layout, widgets) {
  if (rect.c < 1 || rect.r < 1) return false;
  if (rect.c + rect.w > GRID_COLS + 1 || rect.r + rect.h > GRID_ROWS + 1) return false;
  return Object.keys(widgets).every(
    (id) => id === ignoreId || layout[id].hidden || !overlaps(rect, rectOf(id, layout, widgets)),
  );
}

/** First free top-left cell for a w×h widget, scanning rows then columns. */
export function findFreeSpot(w, h, ignoreId, layout, widgets) {
  for (let r = 1; r <= GRID_ROWS - h + 1; r++) {
    for (let c = 1; c <= GRID_COLS - w + 1; c++) {
      if (fits({ c, r, w, h }, ignoreId, layout, widgets)) return [c, r];
    }
  }
  return null;
}

/**
 * Saved rects go stale when a widget's size presets change between versions:
 * clamp each one back into the grid, relocate on collision, fall back to a
 * smaller size, and hide (tray-recoverable) as a last resort — so the board is
 * never rendered with overlaps.
 *
 * Mutates `layout` in place; returns true if anything moved (i.e. the caller
 * should persist it).
 */
export function normalizeLayout(layout, widgets) {
  let changed = false;
  for (const id of Object.keys(widgets)) {
    const p = layout[id];
    if (p.hidden) continue;
    let placed = false;
    for (const size of [...new Set([p.size, "m", "s"])]) {
      const dims = widgets[id].sizes[size];
      if (!dims) continue;
      const [w, h] = dims;
      const c = Math.min(Math.max(p.c, 1), GRID_COLS - w + 1);
      const r = Math.min(Math.max(p.r, 1), GRID_ROWS - h + 1);
      const spot = fits({ c, r, w, h }, id, layout, widgets)
        ? [c, r]
        : findFreeSpot(w, h, id, layout, widgets);
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
  return changed;
}
