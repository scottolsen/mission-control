import test from "node:test";
import assert from "node:assert/strict";
import { keyAction } from "../../src/lib/keys.js";

const ev = (key, mods = {}) => ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods });
const ctx = (o = {}) => ({ focus: "none", modalOpen: false, searchHasText: false, needsInput: false, optionCount: 0, armed: true, ...o });

test("navigation keys move the selection", () => {
  assert.equal(keyAction(ev("ArrowDown"), ctx()), "next");
  assert.equal(keyAction(ev("j"), ctx()), "next");
  assert.equal(keyAction(ev("ArrowUp"), ctx()), "prev");
  assert.equal(keyAction(ev("k"), ctx()), "prev");
});

test("arrows still move the list from the search field, letters do not", () => {
  assert.equal(keyAction(ev("ArrowDown"), ctx({ focus: "search" })), "next");
  assert.equal(keyAction(ev("j"), ctx({ focus: "search" })), null);
});

test("Enter jumps, except inside the composer", () => {
  assert.equal(keyAction(ev("Enter"), ctx()), "jump");
  assert.equal(keyAction(ev("Enter"), ctx({ focus: "search" })), "jump");
  assert.equal(keyAction(ev("Enter"), ctx({ focus: "composer" })), null);
  assert.equal(keyAction(ev("Enter", { metaKey: true }), ctx({ focus: "composer" })), "send");
});

test("single-letter shortcuts only fire with nothing focused", () => {
  assert.equal(keyAction(ev("/"), ctx()), "focusSearch");
  assert.equal(keyAction(ev("g"), ctx()), "toggleGroup");
  assert.equal(keyAction(ev("r"), ctx()), "focusComposer");
  assert.equal(keyAction(ev("g"), ctx({ focus: "composer" })), null);
  assert.equal(keyAction(ev("/"), ctx({ focus: "field" })), null);
});

test("Esc inside the composer leaves the composer before touching the search filter", () => {
  assert.equal(keyAction(ev("Escape"), ctx({ focus: "composer", searchHasText: true })), "blur");
  assert.equal(keyAction(ev("Escape"), ctx({ focus: "search", searchHasText: true })), "clearSearch");
});

test("Esc priority: modal, then search, then blur, then dismiss", () => {
  assert.equal(keyAction(ev("Escape"), ctx({ modalOpen: true, searchHasText: true, needsInput: true })), "closeModal");
  assert.equal(keyAction(ev("Escape"), ctx({ searchHasText: true, needsInput: true })), "clearSearch");
  assert.equal(keyAction(ev("Escape"), ctx({ focus: "composer", needsInput: true })), "blur");
  assert.equal(keyAction(ev("Escape"), ctx({ focus: "search" })), "blur");
  assert.equal(keyAction(ev("Escape"), ctx({ needsInput: true, optionCount: 2 })), "dismiss");
  assert.equal(keyAction(ev("Escape"), ctx()), null);
});

test("digits choose an option only when the prompt has that many", () => {
  const c = ctx({ needsInput: true, optionCount: 3 });
  assert.equal(keyAction(ev("2"), c), "choose:2");
  assert.equal(keyAction(ev("4"), c), null);
  assert.equal(keyAction(ev("2"), ctx({ ...c, focus: "composer" })), null);
  assert.equal(keyAction(ev("1"), ctx()), null);
});

test("Esc does not dismiss a prompt without options (trust dialogs are Jump-only)", () => {
  assert.equal(keyAction(ev("Escape"), ctx({ needsInput: true, optionCount: 0 })), null);
});

test("command shortcuts work from anywhere, but the modal swallows the rest", () => {
  assert.equal(keyAction(ev(",", { metaKey: true }), ctx({ focus: "composer" })), "openSettings");
  assert.equal(keyAction(ev("r", { metaKey: true }), ctx()), "refresh");
  assert.equal(keyAction(ev("w", { metaKey: true }), ctx()), "hide");
  assert.equal(keyAction(ev("j"), ctx({ modalOpen: true })), null);
  assert.equal(keyAction(ev("Enter"), ctx({ modalOpen: true })), null);
});

test("modified letters are left to the system", () => {
  assert.equal(keyAction(ev("g", { ctrlKey: true }), ctx()), null);
  assert.equal(keyAction(ev("c", { metaKey: true }), ctx()), null);
});

test("Enter and Space on a focused button belong to the button", () => {
  assert.equal(keyAction(ev("Enter"), ctx({ focus: "button" })), null);
  assert.equal(keyAction(ev(" "), ctx({ focus: "button" })), null);
  assert.equal(keyAction(ev("j"), ctx({ focus: "button" })), "next");
});

test("Esc with a button focused only blurs it, never dismisses the prompt", () => {
  // Focus stays on Jump, a chip or the gear after a click; Esc out of habit
  // must not send Escape to the live pane.
  assert.equal(keyAction(ev("Escape"), ctx({ focus: "button", needsInput: true, optionCount: 2 })), "blur");
  assert.equal(keyAction(ev("Escape"), ctx({ focus: "button" })), "blur");
});

test("choose and dismiss shortcuts wait until the user picks the session", () => {
  // The selection moved here by itself (the last one left the list).
  const c = ctx({ needsInput: true, optionCount: 3, armed: false });
  assert.equal(keyAction(ev("1"), c), null);
  assert.equal(keyAction(ev("Escape"), c), null);
  assert.equal(keyAction(ev("j"), c), "next", "navigation still works");
  assert.equal(keyAction(ev("1"), ctx({ ...c, armed: true })), "choose:1");
});
