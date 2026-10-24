import test from "node:test";
import assert from "node:assert/strict";
import { replyMode, isStateChanged, isWindowHidden, errorText, toTerminalText, promptFingerprint } from "../../src/lib/reply.js";

test("replyMode: composer for your_turn/idle, queue while working", () => {
  assert.equal(replyMode({ state: "your_turn" }), "compose");
  assert.equal(replyMode({ state: "idle" }), "compose");
  assert.equal(replyMode({ state: "working" }), "queue");
});

test("replyMode: options for menus with options, jump-only otherwise", () => {
  assert.equal(replyMode({ state: "needs_input", prompt: { kind: "permission", options: [{ key: "1", label: "Yes" }] } }), "choose");
  assert.equal(replyMode({ state: "needs_input", prompt: { kind: "trust", options: [] } }), "jump");
  assert.equal(replyMode({ state: "needs_input", prompt: null }), "jump");
  assert.equal(replyMode({ state: "unknown" }), "jump");
});

test("replyMode: nothing to send to without a pane", () => {
  assert.equal(replyMode({ state: "your_turn", tmux: null }), "none");
});

test("isStateChanged recognises the backend rejection however it arrives", () => {
  assert.equal(isStateChanged("state_changed"), true);
  assert.equal(isStateChanged(new Error("state_changed")), true);
  assert.equal(isStateChanged("tmux: no such pane"), false);
});

test("errorText unwraps strings and Errors", () => {
  assert.equal(errorText("boom"), "boom");
  assert.equal(errorText(new Error("bang")), "bang");
  assert.equal(errorText({ message: "obj" }), "obj");
  assert.equal(errorText(undefined), "Something went wrong.");
});

test("toTerminalText converts bare newlines to CRLF without doubling", () => {
  assert.equal(toTerminalText("a\nb\r\nc"), "a\r\nb\r\nc");
  assert.equal(toTerminalText(""), "");
});

test("promptFingerprint is the question and option labels the backend compares", () => {
  const prompt = { kind: "permission", question: "Do you want to proceed?", options: [{ key: "1", label: "Yes" }, { key: "2", label: "No" }] };
  assert.deepEqual(promptFingerprint(prompt), { question: "Do you want to proceed?", options: ["Yes", "No"] });
  assert.deepEqual(promptFingerprint({ kind: "other", question: null, options: [] }), { question: null, options: [] });
  assert.deepEqual(promptFingerprint(null), { question: null, options: [] });
});

test("isWindowHidden recognises capture_pane's paused answer", () => {
  assert.equal(isWindowHidden("window_hidden"), true);
  assert.equal(isWindowHidden("state_changed"), false);
});
