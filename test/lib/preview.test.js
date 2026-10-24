import test from "node:test";
import assert from "node:assert/strict";
import { previewScale } from "../../src/lib/preview.js";

test("previewScale fits the terminal to the available width", () => {
  assert.deepEqual(previewScale({ termWidth: 800, termHeight: 400, boxWidth: 400 }), { scale: 0.5, height: 200, overflow: false });
});

test("previewScale never enlarges", () => {
  assert.equal(previewScale({ termWidth: 300, termHeight: 100, boxWidth: 600 }).scale, 1);
});

test("previewScale clamps to a readable floor and reports overflow", () => {
  const r = previewScale({ termWidth: 3000, termHeight: 600, boxWidth: 600, minScale: 0.5 });
  assert.deepEqual(r, { scale: 0.5, height: 300, overflow: true });
});

test("previewScale handles a zero-size box", () => {
  assert.equal(previewScale({ termWidth: 0, termHeight: 0, boxWidth: 0 }).scale, 1);
});

import { trimCapture } from "../../src/lib/preview.js";

test("trimCapture drops trailing blank rows, including ones holding only escapes", () => {
  const r = trimCapture("one\n\x1b[38;5;1mtwo\x1b[0m\n   \n\x1b[0m\n\n");
  assert.equal(r.text, "one\n\x1b[38;5;1mtwo\x1b[0m");
  assert.equal(r.lines, 2);
});

test("trimCapture keeps blank rows between content", () => {
  assert.deepEqual(trimCapture("a\n\nb\n"), { text: "a\n\nb", lines: 3 });
});

test("trimCapture treats OSC hyperlinks as invisible", () => {
  assert.equal(trimCapture("a\n\x1b]8;;\x1b\\\n").lines, 1);
});

test("trimCapture of an empty capture is zero lines", () => {
  assert.deepEqual(trimCapture(""), { text: "", lines: 0 });
});

import { paintSequence } from "../../src/lib/preview.js";

test("paintSequence clears screen and scrollback in-band, before the frame", () => {
  // The clear must ride in the same write stream as the frame: a separate,
  // synchronous term.reset() can land before a still-queued earlier write.
  const out = paintSequence("one\ntwo");
  assert.ok(out.startsWith("\x1b[0m\x1b[H\x1b[2J\x1b[3J"));
  assert.ok(out.endsWith("one\r\ntwo"));
});
