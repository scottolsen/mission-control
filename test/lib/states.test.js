import test from "node:test";
import assert from "node:assert/strict";
import { STATE_META, GROUP_ORDER, displayState, isStale, matchesChip, chipCounts, CHIPS } from "../../src/lib/states.js";

const H = 3600 * 1000;
const NOW = 1_800_000_000_000;
const s = (state, agoH = 0) => ({ id: state + agoH, state, lastActivityAt: NOW - agoH * H });

test("every contract state plus stale has label, icon and tone", () => {
  for (const k of ["needs_input", "your_turn", "working", "idle", "stale", "unknown"]) {
    const m = STATE_META[k];
    assert.ok(m.label && m.icon && m.tone, k);
  }
});

test("group order puts attention first and stale/unknown last", () => {
  assert.deepEqual(GROUP_ORDER, ["needs_input", "your_turn", "working", "idle", "stale", "unknown"]);
});

test("isStale only applies to your_turn and idle older than staleHours", () => {
  assert.equal(isStale(s("your_turn", 9), NOW, 8), true);
  assert.equal(isStale(s("idle", 9), NOW, 8), true);
  assert.equal(isStale(s("idle", 7), NOW, 8), false);
  assert.equal(isStale(s("working", 30), NOW, 8), false);
  assert.equal(isStale(s("needs_input", 30), NOW, 8), false);
});

test("displayState derives stale but never alters the contract state", () => {
  const old = s("your_turn", 10);
  assert.equal(displayState(old, NOW, 8), "stale");
  assert.equal(old.state, "your_turn");
  assert.equal(displayState(s("working"), NOW, 8), "working");
  assert.equal(displayState({ state: "bogus" }, NOW, 8), "unknown");
});

test("staleHours of 0 or less disables staleness", () => {
  assert.equal(isStale(s("idle", 1000), NOW, 0), false);
});

test("chips: Needs you = needs_input + your_turn, Inactive = idle + stale + unknown", () => {
  assert.deepEqual(CHIPS.map((c) => c.key), ["all", "attention", "working", "idle"]);
  assert.equal(matchesChip("attention", "needs_input"), true);
  assert.equal(matchesChip("attention", "your_turn"), true);
  assert.equal(matchesChip("attention", "stale"), false);
  assert.equal(matchesChip("idle", "stale"), true);
  assert.equal(matchesChip("idle", "unknown"), true);
  assert.equal(matchesChip("working", "working"), true);
  assert.equal(matchesChip("all", "unknown"), true);
});

test("the inactive chip isn't labelled Idle, so it can't contradict the Idle and Stale groups it covers", () => {
  const chip = CHIPS.find((c) => c.key === "idle");
  assert.equal(chip.label, "Inactive");
});

test("chipCounts tallies display states into each chip", () => {
  const counts = chipCounts(["needs_input", "your_turn", "your_turn", "working", "idle", "stale"]);
  assert.deepEqual(counts, { all: 6, attention: 3, working: 1, idle: 2, needs_input: 1, your_turn: 2, idleOnly: 1, stale: 1 });
});
