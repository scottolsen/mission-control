import test from "node:test";
import assert from "node:assert/strict";
import { snippet, tmuxLabel, tmuxCrumbs, toolLabel, homeRelative, windowText, inlineCode } from "../../src/lib/session-text.js";

const tmux = { session: "shopfront", windowId: "@3", windowIndex: 3, windowName: "zsh", paneId: "%40", paneIndex: 1, width: 182, height: 31 };

test("tmuxLabel joins session, window and pane", () => {
  assert.equal(tmuxLabel(tmux), "shopfront › 3:zsh › %40");
  assert.equal(tmuxLabel(null), "Outside tmux");
});

test("tmuxCrumbs returns labelled parts", () => {
  assert.deepEqual(tmuxCrumbs(tmux), [
    { label: "Session", value: "shopfront" },
    { label: "Window", value: "3:zsh" },
    { label: "Pane", value: "%40" },
  ]);
  assert.deepEqual(tmuxCrumbs(null), []);
});

test("toolLabel combines name and summary", () => {
  assert.equal(toolLabel({ name: "Bash", summary: "bundle exec rspec" }), "Bash · bundle exec rspec");
  assert.equal(toolLabel({ name: "Agent", summary: "" }), "Agent");
  assert.equal(toolLabel(null), "");
});

test("snippet: working shows current tool, falls back to the prompt", () => {
  assert.deepEqual(snippet({ state: "working", currentTool: { name: "Read", summary: "a.rb" }, lastPrompt: "p" }, "working"),
    { kind: "tool", text: "Read · a.rb" });
  assert.deepEqual(snippet({ state: "working", currentTool: null, lastPrompt: "do it" }, "working"),
    { kind: "prompt", text: "do it" });
});

test("snippet: needs_input shows the question, else a generic line", () => {
  assert.deepEqual(snippet({ state: "needs_input", prompt: { kind: "permission", question: "Do you want to proceed?", options: [] } }, "needs_input"),
    { kind: "question", text: "Do you want to proceed?" });
  assert.deepEqual(snippet({ state: "needs_input", prompt: { kind: "trust", question: null, options: [] } }, "needs_input"),
    { kind: "question", text: "Waiting on a trust prompt" });
  assert.deepEqual(snippet({ state: "needs_input", prompt: null }, "needs_input"),
    { kind: "question", text: "Waiting on a menu" });
});

test("snippet: your_turn and stale show the last reply, idle the prompt", () => {
  assert.deepEqual(snippet({ state: "your_turn", lastReply: "Done.\nAll green", lastPrompt: "x" }, "your_turn"),
    { kind: "reply", text: "Done. All green" });
  assert.deepEqual(snippet({ state: "your_turn", lastReply: "old", lastPrompt: "x" }, "stale"),
    { kind: "reply", text: "old" });
  assert.deepEqual(snippet({ state: "idle", lastReply: null, lastPrompt: null }, "idle"),
    { kind: "none", text: "No messages yet" });
});

test("homeRelative abbreviates the home directory", () => {
  assert.equal(homeRelative("/Users/dev/code/shopfront"), "~/code/shopfront");
  assert.equal(homeRelative("/opt/x"), "/opt/x");
  assert.equal(homeRelative("/Users/dev"), "~");
});

test("windowText drops a window name that is only a version number", () => {
  // tmux auto-names a Claude window after the running binary, i.e. its version.
  assert.equal(windowText({ windowIndex: 1, windowName: "2.1.301" }), "1");
  assert.equal(windowText({ windowIndex: 3, windowName: "zsh" }), "3:zsh");
  assert.equal(windowText({ windowIndex: 2, windowName: "v2" }), "2:v2");
  assert.equal(tmuxLabel({ session: "shopfront", windowIndex: 1, windowName: "2.1.301", paneId: "%76" }), "shopfront › 1 › %76");
  assert.deepEqual(tmuxCrumbs({ session: "shopfront", windowIndex: 1, windowName: "2.1.301", paneId: "%76" })[1], { label: "Window", value: "1" });
});

test("inlineCode splits backtick spans out of plain text", () => {
  assert.deepEqual(inlineCode("Found it. `NextNumber` reads `a b`."), [
    { code: false, text: "Found it. " },
    { code: true, text: "NextNumber" },
    { code: false, text: " reads " },
    { code: true, text: "a b" },
    { code: false, text: "." },
  ]);
});

test("inlineCode leaves unmatched and empty backticks as text", () => {
  assert.deepEqual(inlineCode("a ` b"), [{ code: false, text: "a ` b" }]);
  assert.deepEqual(inlineCode("``"), [{ code: false, text: "``" }]);
  assert.deepEqual(inlineCode(""), []);
});

test("inlineCode does not span a code run across lines", () => {
  assert.deepEqual(inlineCode("a `b\nc` d"), [{ code: false, text: "a `b\nc` d" }]);
});

test("snippet strips inline-code backticks from the one-line text", () => {
  const r = snippet({ state: "your_turn", lastReply: "Found it. `NextNumber` reads" }, "your_turn");
  assert.equal(r.text, "Found it. NextNumber reads");
});
