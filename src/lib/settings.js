/**
 * Settings defaults and form validation. The backend validates again (it's the
 * one that can tell whether a hotkey registers); this catches the obvious
 * mistakes before a round trip.
 */

export const DEFAULT_SETTINGS = {
  notifyYourTurn: true,
  notifyNeedsInput: false,
  hotkey: "Ctrl+Alt+M",
  staleHours: 8,
  terminalApp: "iTerm",
};

/** raw: values read straight off the form controls. */
export function readSettingsForm(raw) {
  const hotkey = String(raw.hotkey ?? "").trim();
  const terminalApp = String(raw.terminalApp ?? "").trim();
  const staleText = String(raw.staleHours ?? "").trim();
  const staleHours = Number(staleText);
  if (!hotkey) return { ok: false, field: "hotkey", error: "Enter a shortcut, such as Ctrl+Alt+M." };
  if (!staleText || !Number.isInteger(staleHours) || staleHours < 1 || staleHours > 720) {
    return { ok: false, field: "staleHours", error: "Use a whole number of hours between 1 and 720." };
  }
  if (!terminalApp) return { ok: false, field: "terminalApp", error: "Enter the terminal app to bring forward, such as iTerm." };
  return {
    ok: true,
    settings: { notifyYourTurn: Boolean(raw.notifyYourTurn), notifyNeedsInput: Boolean(raw.notifyNeedsInput), hotkey, staleHours, terminalApp },
  };
}

export const sameSettings = (a, b) => Object.keys(DEFAULT_SETTINGS).every((k) => a?.[k] === b?.[k]);
