import test from "node:test";
import assert from "node:assert/strict";
import { fuzzyScore, matchSession, filterSessions } from "../../src/lib/filter.js";

const mk = (o) => ({
  id: o.id, title: o.title || "", project: o.project || "", branch: o.branch ?? null,
  cwd: o.cwd || "", lastPrompt: o.lastPrompt ?? null,
  tmux: o.tmux === undefined ? { session: "shopfront" } : o.tmux,
});

test("fuzzyScore: substring beats subsequence, prefix beats inner substring", () => {
  const prefix = fuzzyScore("bul", "bulk refunds");
  const inner = fuzzyScore("bul", "the bulk");
  const subseq = fuzzyScore("bul", "big ugly lamp");
  assert.ok(prefix > inner, `${prefix} > ${inner}`);
  assert.ok(inner > subseq, `${inner} > ${subseq}`);
  assert.ok(subseq > 0);
});

test("fuzzyScore is case-insensitive and 0 when chars are missing", () => {
  assert.ok(fuzzyScore("SHOP", "shopfront") > 0);
  assert.equal(fuzzyScore("xyz", "shopfront"), 0);
  assert.equal(fuzzyScore("", "anything"), 1);
});

test("matchSession requires every token to match some field", () => {
  const s = mk({ id: "a", title: "Fix bulk refunds", project: "shopfront", branch: "feature/ledger" });
  assert.ok(matchSession(s, "bulk ledger") > 0);
  assert.equal(matchSession(s, "bulk zzz"), 0);
});

test("matchSession searches tmux session, cwd and lastPrompt; tolerates nulls", () => {
  const s = mk({ id: "a", cwd: "/Users/x/code/orders-api", lastPrompt: "rename the kafka consumer", tmux: { session: "Checkout redesign — phase 2" } });
  assert.ok(matchSession(s, "checkout") > 0);
  assert.ok(matchSession(s, "orders-api") > 0);
  assert.ok(matchSession(s, "kafka") > 0);
  assert.ok(matchSession(mk({ id: "b", tmux: null, title: "x" }), "x") > 0);
});

test("title matches outrank lastPrompt matches", () => {
  const t = mk({ id: "t", title: "ledger cleanup" });
  const p = mk({ id: "p", title: "other", lastPrompt: "ledger cleanup please" });
  assert.ok(matchSession(t, "ledger") > matchSession(p, "ledger"));
});

test("filterSessions with an empty query returns everything untouched", () => {
  const list = [mk({ id: "a" }), mk({ id: "b" })];
  assert.equal(filterSessions(list, "  ").length, 2);
});

test("filterSessions drops non-matches", () => {
  const list = [mk({ id: "a", title: "bulk" }), mk({ id: "b", title: "zebra" })];
  assert.deepEqual(filterSessions(list, "bulk").map((s) => s.id), ["a"]);
});

test("fuzzyScore rejects letters scattered across a long string", () => {
  assert.equal(fuzzyScore("rails", "tail production logs for the 500s"), 0);
  assert.ok(fuzzyScore("prl", "partial-refunds-ledger") > 0);
});
