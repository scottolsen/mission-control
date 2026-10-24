import test from "node:test";
import assert from "node:assert/strict";
import { isLoaded } from "../../src/lib/snapshot.js";

test("a snapshot from before the first poll (generatedAt 0) is not loaded", () => {
  // Otherwise a cold start flashes "tmux isn't running" and "updated 56 years ago".
  assert.equal(isLoaded({ sessions: [], generatedAt: 0, tmuxRunning: false, error: null }), false);
  assert.equal(isLoaded(null), false);
  assert.equal(isLoaded(undefined), false);
  assert.equal(isLoaded({ sessions: [], generatedAt: 1792152313750, tmuxRunning: false, error: null }), true);
});
