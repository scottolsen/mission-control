/**
 * Which reply affordance a session gets, plus error helpers for the
 * send_reply / send_choice round trip.
 */

/**
 * "compose" — free-text reply; "queue" — free text while Claude works (it
 * queues the message); "choose" — numbered menu buttons; "jump" — the pane
 * needs you in person (trust dialog, menus we couldn't parse); "none" — no
 * pane to talk to.
 */
export function replyMode(session) {
  if (session.tmux === null) return "none";
  switch (session.state) {
    case "your_turn":
    case "idle":
      return "compose";
    case "working":
      return "queue";
    case "needs_input":
      return session.prompt?.options?.length ? "choose" : "jump";
    default:
      return "jump";
  }
}

/**
 * What send_choice compares against the pane before sending: the question
 * and option labels the user was shown, so a key meant for one prompt can't
 * answer the one that replaced it.
 */
export function promptFingerprint(prompt) {
  return { question: prompt?.question ?? null, options: (prompt?.options ?? []).map((o) => o.label) };
}

export function errorText(err) {
  if (typeof err === "string") return err;
  if (err && typeof err.message === "string") return err.message;
  return "Something went wrong.";
}

export const isStateChanged = (err) => errorText(err) === "state_changed";

/** capture_pane's answer while the window is hidden in the tray. */
export const isWindowHidden = (err) => errorText(err) === "window_hidden";

/** xterm needs CR+LF; capture-pane emits bare LF. */
export const toTerminalText = (ansi) => ansi.replace(/\r?\n/g, "\r\n");
