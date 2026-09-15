import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MAX_NOTES,
  addNote,
  clearDone,
  dueLabel,
  noteStatus,
  notesSummary,
  orderNotes,
  parseInput,
  parseNotes,
  removeNote,
  toggleNote,
} from "../src/lib/notes.js";

// Wednesday 16 Sep 2026, 09:00 local.
const NOW = new Date(2026, 8, 16, 9, 0).getTime();
const at = (h, m, day = 16) => new Date(2026, 8, day, h, m).getTime();

test("a plain line is just text", () => {
  assert.deepEqual(parseInput("  buy milk  ", NOW), { text: "buy milk", due: null, pinned: false });
});

test("a leading ! pins the note and is stripped", () => {
  const n = parseInput("! hand in KIV/PT", NOW);
  assert.equal(n.pinned, true);
  assert.equal(n.text, "hand in KIV/PT");
});

test("@HH:MM sets a reminder and leaves the text clean", () => {
  const n = parseInput("call the office @14:30 about the form", NOW);
  assert.equal(n.text, "call the office about the form");
  assert.equal(n.due, at(14, 30));
});

test("a reminder time that has passed today means tomorrow", () => {
  assert.equal(parseInput("standup @08:00", NOW).due, at(8, 0, 17));
});

test("something that only looks like a time is left alone", () => {
  for (const line of ["email bob@10x.com", "meeting @25:00", "call @9:5"]) {
    const n = parseInput(line, NOW);
    assert.equal(n.due, null, line);
    assert.equal(n.text, line);
  }
});

test("malformed storage parses to no notes rather than throwing", () => {
  assert.deepEqual(parseNotes("not json"), []);
  assert.deepEqual(parseNotes(null), []);
  assert.deepEqual(parseNotes('{"a":1}'), []);
});

test("stored notes are cleaned field by field", () => {
  const raw = JSON.stringify([
    { id: "a", text: "keep", done: 1, at: 5, due: 9, pinned: 1, evil: "x" },
    { id: "b", text: "   " }, // empty text
    { id: "a", text: "duplicate id" },
    { text: "no id" },
  ]);
  const notes = parseNotes(raw);
  assert.deepEqual(notes, [{ id: "a", text: "keep", done: true, at: 5, due: 9, pinned: true }]);
});

test("adding, toggling and removing", () => {
  let notes = addNote([], "read the assignment", NOW);
  assert.equal(notes.length, 1);
  assert.equal(notes[0].done, false);
  notes = toggleNote(notes, notes[0].id, NOW);
  assert.equal(notes[0].done, true);
  notes = toggleNote(notes, notes[0].id, NOW);
  assert.equal(notes[0].done, false);
  assert.deepEqual(addNote(notes, "   ", NOW), notes); // nothing to add
  assert.deepEqual(removeNote(notes, notes[0].id), []);
});

test("at the cap a done note gives way, never an open one", () => {
  let notes = [];
  for (let i = 0; i < MAX_NOTES; i++) notes = addNote(notes, `note ${i}`, NOW + i);
  notes = toggleNote(notes, notes[10].id, NOW); // mark one done
  const doneId = notes[10].id;
  notes = addNote(notes, "one more", NOW + 1000);
  assert.equal(notes.length, MAX_NOTES);
  assert.ok(!notes.some((n) => n.id === doneId));
  assert.ok(notes.some((n) => n.text === "one more"));
});

test("clearing done notes keeps the open ones", () => {
  let notes = addNote(addNote([], "a", NOW), "b", NOW);
  notes = toggleNote(notes, notes[0].id, NOW);
  assert.deepEqual(clearDone(notes).map((n) => n.text), ["b"]);
});

test("order puts due-soon first, then pinned, then the rest, done last", () => {
  const note = (text, extra) => ({ id: text, text, done: false, at: NOW, due: null, pinned: false, ...extra });
  const notes = [
    note("plain"),
    note("done", { done: true }),
    note("pinned", { pinned: true }),
    note("later", { due: at(20, 0) }),
    note("soon", { due: at(9, 20) }),
    note("overdue", { due: at(8, 0) }),
  ];
  assert.deepEqual(
    orderNotes(notes, NOW).map((n) => n.text),
    ["overdue", "soon", "pinned", "later", "plain", "done"],
  );
});

test("a reminder's status turns soon, then overdue", () => {
  const n = { done: false, due: at(9, 20) };
  assert.equal(noteStatus(n, NOW), "soon");
  assert.equal(noteStatus(n, at(9, 30)), "overdue");
  assert.equal(noteStatus({ done: false, due: at(23, 0) }, NOW), "");
  assert.equal(noteStatus({ done: true, due: at(8, 0) }, NOW), ""); // finished
  assert.equal(noteStatus({ done: false, due: null }, NOW), "");
});

test("the summary counts what is left and names the next reminder", () => {
  let notes = addNote(addNote([], "a @10:00", NOW), "b", NOW);
  notes = addNote(notes, "c", NOW);
  notes = toggleNote(notes, notes[1].id, NOW);
  const s = notesSummary(notes, NOW);
  assert.deepEqual([s.open, s.done], [2, 1]);
  assert.equal(s.next.text, "a");
});

test("due labels name the day only when it is not today", () => {
  assert.equal(dueLabel(at(14, 30), NOW), "14:30");
  assert.equal(dueLabel(at(8, 0, 17), NOW), "08:00 thu");
});
