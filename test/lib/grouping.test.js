import test from "node:test";
import assert from "node:assert/strict";
import { groupSessions, flattenGroups } from "../../src/lib/grouping.js";

const H = 3600 * 1000;
const NOW = 1_800_000_000_000;
const s = (id, state, agoMin, tmux = "shopfront") => ({
  id, state, lastActivityAt: NOW - agoMin * 60000,
  tmux: tmux ? { session: tmux, windowIndex: 1, paneIndex: 0 } : null,
});
const opts = (mode) => ({ mode, now: NOW, staleHours: 8 });

test("by status: fixed order, newest first within a group, empty groups omitted", () => {
  const list = [
    s("w-old", "working", 30), s("w-new", "working", 1),
    s("n", "needs_input", 50), s("y", "your_turn", 5),
    s("stale", "idle", 9 * 60), s("i", "idle", 10),
  ];
  const groups = groupSessions(list, opts("status"));
  assert.deepEqual(groups.map((g) => g.key), ["needs_input", "your_turn", "working", "idle", "stale"]);
  assert.deepEqual(groups[2].sessions.map((x) => x.id), ["w-new", "w-old"]);
  assert.equal(groups[0].label, "Needs input");
});

test("by tmux: groups by session name, attention first inside, Outside tmux last", () => {
  const list = [
    s("a", "idle", 1, "shopfront"), s("b", "needs_input", 40, "shopfront"), s("c", "working", 2, "shopfront"),
    s("d", "your_turn", 3, "-claude"), s("e", "idle", 1, null),
  ];
  const groups = groupSessions(list, opts("tmux"));
  assert.deepEqual(groups.map((g) => g.label), ["shopfront", "-claude", "Outside tmux"]);
  assert.deepEqual(groups[0].sessions.map((x) => x.id), ["b", "c", "a"]);
});

test("by tmux: groups needing attention sort ahead, then alphabetical", () => {
  const list = [
    s("a", "idle", 1, "alpha"), s("b", "working", 1, "beta"),
    s("c", "your_turn", 1, "zulu"), s("d", "idle", 1, "Checkout redesign — phase 2"),
  ];
  const labels = groupSessions(list, opts("tmux")).map((g) => g.label);
  assert.deepEqual(labels, ["zulu", "alpha", "beta", "Checkout redesign — phase 2"]);
});

test("groups carry an attention count for headers", () => {
  const g = groupSessions([s("a", "needs_input", 1), s("b", "your_turn", 1), s("c", "idle", 1)], opts("tmux"));
  assert.equal(g[0].attention, 2);
});

test("each grouped session carries its display state", () => {
  const g = groupSessions([s("x", "your_turn", 10 * 60)], opts("status"));
  assert.equal(g[0].key, "stale");
  assert.equal(g[0].sessions[0].display, "stale");
});

test("flattenGroups yields ids in on-screen order", () => {
  const g = groupSessions([s("a", "idle", 1), s("b", "needs_input", 1)], opts("status"));
  assert.deepEqual(flattenGroups(g), ["b", "a"]);
});
