// NOTES widget: a short list of things to do, kept on this machine.
//
// Pure functions over a note list so the parts with rules — what a typed line
// means, what order the list comes out in, when a reminder is due — can be
// tested without a DOM. The widget in main.js is the rendering around this.
//
// A note is `{ id, text, done, at, due, pinned }`. `due` is epoch ms or null.

export const MAX_NOTES = 60;
export const MAX_LEN = 140;
/** A reminder counts as "soon" inside this window before it is due. */
export const SOON_MS = 30 * 60_000;

/**
 * What a typed line means:
 *
 *   "!"     leading, pins the note to the top
 *   "@17:00" anywhere, a reminder time — today, or tomorrow if that has passed
 *
 * Both are stripped from the text that gets stored, so the note reads as
 * written rather than keeping the punctuation that configured it.
 */
export function parseInput(raw, now) {
  let text = String(raw ?? "").trim();
  const pinned = text.startsWith("!");
  if (pinned) text = text.slice(1).trim();

  let due = null;
  const m = /(?:^|\s)@(\d{1,2}):(\d{2})(?=\s|$)/.exec(text);
  if (m) {
    const h = Number(m[1]);
    const min = Number(m[2]);
    if (h < 24 && min < 60) {
      const d = new Date(now);
      d.setHours(h, min, 0, 0);
      // A time that has already gone by today means the next one, tomorrow.
      if (d.getTime() <= now) d.setDate(d.getDate() + 1);
      due = d.getTime();
      text = (text.slice(0, m.index) + " " + text.slice(m.index + m[0].length))
        .replace(/\s+/g, " ")
        .trim();
    }
  }
  return { text: text.slice(0, MAX_LEN), due, pinned };
}

/** Parse stored JSON, keeping only well-formed notes. */
export function parseNotes(raw) {
  let data;
  try {
    data = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(data)) return [];
  const out = [];
  const seen = new Set();
  for (const n of data) {
    const text = String(n?.text ?? "").trim().slice(0, MAX_LEN);
    const id = String(n?.id ?? "");
    if (!text || !id || seen.has(id)) continue;
    seen.add(id);
    out.push({
      id,
      text,
      done: !!n.done,
      at: Number.isFinite(n.at) ? n.at : 0,
      due: Number.isFinite(n.due) ? n.due : null,
      pinned: !!n.pinned,
    });
    if (out.length >= MAX_NOTES) break;
  }
  return out;
}

export function addNote(notes, raw, now) {
  const { text, due, pinned } = parseInput(raw, now);
  if (!text) return notes;
  const note = { id: `${now}-${Math.random().toString(36).slice(2, 8)}`, text, done: false, at: now, due, pinned };
  // Oldest done note gives way at the cap; an open one is never dropped.
  const next = [...notes, note];
  if (next.length <= MAX_NOTES) return next;
  const victim = next.find((n) => n.done) ?? next[0];
  return next.filter((n) => n !== victim);
}

export function toggleNote(notes, id, now) {
  return notes.map((n) => (n.id === id ? { ...n, done: !n.done, at: n.done ? n.at : now } : n));
}

export function removeNote(notes, id) {
  return notes.filter((n) => n.id !== id);
}

export function clearDone(notes) {
  return notes.filter((n) => !n.done);
}

/**
 * Display order: what still needs doing first — overdue and soon-due ahead of
 * the rest, pinned above plain, newest first within a group — and everything
 * finished at the bottom. The widget shows the first few, so this order is
 * what decides which ones a small widget gets to show.
 */
export function orderNotes(notes, now) {
  const rank = (n) => {
    if (n.done) return 4;
    if (n.due != null && n.due - now <= SOON_MS) return 0;
    if (n.pinned) return 1;
    return n.due != null ? 2 : 3;
  };
  return [...notes].sort((a, b) => {
    const d = rank(a) - rank(b);
    if (d) return d;
    // Within the due groups, the soonest reminder leads.
    if (a.due != null && b.due != null && a.due !== b.due) return a.due - b.due;
    return b.at - a.at;
  });
}

/** "overdue" | "soon" | "" — what a reminder's colour should say. */
export function noteStatus(note, now) {
  if (note.done || note.due == null) return "";
  if (note.due <= now) return "overdue";
  return note.due - now <= SOON_MS ? "soon" : "";
}

/** Headline for the small size: how much is left, and the next reminder. */
export function notesSummary(notes, now) {
  const open = notes.filter((n) => !n.done);
  const next = orderNotes(open, now).find((n) => n.due != null);
  return { open: open.length, done: notes.length - open.length, next: next ?? null };
}

/** "17:00", or "17:00 tue" when it is not today. */
export function dueLabel(due, now) {
  const d = new Date(due);
  const time = d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  const today = new Date(now);
  const sameDay =
    d.getFullYear() === today.getFullYear() &&
    d.getMonth() === today.getMonth() &&
    d.getDate() === today.getDate();
  return sameDay ? time : `${time} ${d.toLocaleDateString("en-GB", { weekday: "short" }).toLowerCase()}`;
}
