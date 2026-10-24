import test from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_SETTINGS, readSettingsForm, sameSettings } from "../../src/lib/settings.js";

test("defaults match the contract", () => {
  assert.deepEqual(DEFAULT_SETTINGS, {
    notifyYourTurn: true, notifyNeedsInput: false, hotkey: "Ctrl+Alt+M", staleHours: 8, terminalApp: "iTerm",
  });
});

test("readSettingsForm trims and coerces", () => {
  const r = readSettingsForm({ notifyYourTurn: false, notifyNeedsInput: true, hotkey: "  Cmd+Shift+K ", staleHours: "12", terminalApp: " Ghostty " });
  assert.deepEqual(r, { ok: true, settings: { notifyYourTurn: false, notifyNeedsInput: true, hotkey: "Cmd+Shift+K", staleHours: 12, terminalApp: "Ghostty" } });
});

test("readSettingsForm rejects bad values with a field and message", () => {
  assert.deepEqual(readSettingsForm({ ...DEFAULT_SETTINGS, hotkey: " " }), { ok: false, field: "hotkey", error: "Enter a shortcut, such as Ctrl+Alt+M." });
  assert.equal(readSettingsForm({ ...DEFAULT_SETTINGS, staleHours: "0" }).field, "staleHours");
  assert.equal(readSettingsForm({ ...DEFAULT_SETTINGS, staleHours: "abc" }).field, "staleHours");
  assert.equal(readSettingsForm({ ...DEFAULT_SETTINGS, staleHours: "1000" }).field, "staleHours");
  // The backend stores staleHours as an unsigned integer and rejects fractions.
  assert.equal(readSettingsForm({ ...DEFAULT_SETTINGS, staleHours: "1.5" }).field, "staleHours");
  assert.equal(readSettingsForm({ ...DEFAULT_SETTINGS, staleHours: "0.5" }).field, "staleHours");
  assert.equal(readSettingsForm({ ...DEFAULT_SETTINGS, terminalApp: "" }).field, "terminalApp");
});

test("sameSettings compares by value", () => {
  assert.equal(sameSettings(DEFAULT_SETTINGS, { ...DEFAULT_SETTINGS }), true);
  assert.equal(sameSettings(DEFAULT_SETTINGS, { ...DEFAULT_SETTINGS, staleHours: 9 }), false);
});
