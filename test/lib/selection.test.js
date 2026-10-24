import test from "node:test";
import assert from "node:assert/strict";
import { moveSelection, keepSelection, autoMoved } from "../../src/lib/selection.js";

test("moveSelection steps and clamps at the ends", () => {
  const ids = ["a", "b", "c"];
  assert.equal(moveSelection(ids, "a", 1), "b");
  assert.equal(moveSelection(ids, "c", 1), "c");
  assert.equal(moveSelection(ids, "a", -1), "a");
  assert.equal(moveSelection(ids, null, 1), "a");
  assert.equal(moveSelection(ids, "gone", -1), "a");
  assert.equal(moveSelection([], "a", 1), null);
});

test("keepSelection sticks to an id that still exists", () => {
  assert.equal(keepSelection(["b", "a"], "a", ["a", "b"]), "a");
});

test("keepSelection picks the neighbour when the selected session disappears", () => {
  // previous order a,b,c; b vanished → move to what was after it (c)
  assert.equal(keepSelection(["a", "c"], "b", ["a", "b", "c"]), "c");
  // last one vanished → the one before
  assert.equal(keepSelection(["a", "b"], "c", ["a", "b", "c"]), "b");
});

test("keepSelection defaults to the first row and handles empty lists", () => {
  assert.equal(keepSelection(["x", "y"], null, []), "x");
  assert.equal(keepSelection([], "x", ["x"]), null);
});

test("autoMoved: the selection changed without the user choosing it", () => {
  assert.equal(autoMoved("a", "b"), true);
  assert.equal(autoMoved("a", null), true);
  assert.equal(autoMoved("a", "a"), false);
  // The first selection at boot is not a move away from anything.
  assert.equal(autoMoved(null, "a"), false);
});
