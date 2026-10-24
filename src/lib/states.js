/**
 * State metadata and the derived "display state".
 *
 * The backend reports one of the contract states. The UI adds "stale": a
 * your_turn or idle session nobody has touched for `staleHours`. Stale is for
 * display only — anything sent back to the backend (expectedState) must use
 * the session's real `state`, never this derived value.
 */

export const STATE_META = {
  needs_input: { label: "Needs input", short: "Needs you", icon: "alert", tone: "urgent" },
  your_turn: { label: "Your turn", short: "Your turn", icon: "reply", tone: "attention" },
  working: { label: "Working", short: "Working", icon: "spinner", tone: "calm" },
  idle: { label: "Idle", short: "Idle", icon: "pause", tone: "quiet" },
  stale: { label: "Stale", short: "Stale", icon: "moon", tone: "dim" },
  unknown: { label: "Unknown", short: "Unknown", icon: "question", tone: "dim" },
};

export const GROUP_ORDER = ["needs_input", "your_turn", "working", "idle", "stale", "unknown"];

export const stateRank = (display) => {
  const i = GROUP_ORDER.indexOf(display);
  return i < 0 ? GROUP_ORDER.length : i;
};

export function isStale(session, now, staleHours) {
  if (!(staleHours > 0)) return false;
  if (session.state !== "your_turn" && session.state !== "idle") return false;
  return now - session.lastActivityAt > staleHours * 3_600_000;
}

export function displayState(session, now, staleHours) {
  if (!(session.state in STATE_META) || session.state === "stale") return "unknown";
  return isStale(session, now, staleHours) ? "stale" : session.state;
}

/**
 * Top-bar filter chips. "Needs you" folds both attention states together.
 * "Looping" cuts across states: it matches sessions running a /loop, whatever
 * they're doing right now.
 */
export const CHIPS = [
  { key: "all", label: "All", states: null },
  { key: "attention", label: "Needs you", states: ["needs_input", "your_turn"] },
  { key: "working", label: "Working", states: ["working"] },
  // Not "Idle": it also covers the Stale group, and a chip reading "Idle 22"
  // over a list headed "Stale 22" reads as a contradiction.
  { key: "idle", label: "Inactive", states: ["idle", "stale", "unknown"] },
  { key: "loop", label: "Looping", states: null, loop: true },
];

/** `row` is a session with its `display` state. */
export function matchesChip(chipKey, row) {
  const chip = CHIPS.find((c) => c.key === chipKey);
  if (chip?.loop) return Boolean(row.loop);
  return !chip || !chip.states || chip.states.includes(row.display);
}

/**
 * Counts per chip, plus the states inside the two folded chips separately:
 * the attention split drives the badge dot, idle/stale the Inactive tooltip.
 */
export function chipCounts(rows) {
  const counts = { needs_input: 0, your_turn: 0, idleOnly: 0, stale: 0 };
  for (const c of CHIPS) counts[c.key] = 0;
  for (const row of rows) {
    const d = row.display;
    for (const c of CHIPS) if (matchesChip(c.key, row)) counts[c.key] += 1;
    if (d === "needs_input" || d === "your_turn" || d === "stale") counts[d] += 1;
    if (d === "idle") counts.idleOnly += 1;
  }
  return counts;
}
