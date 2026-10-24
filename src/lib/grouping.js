/**
 * Grouping and ordering for the session list.
 *
 * "status" mode buckets by display state in GROUP_ORDER, newest activity
 * first. "tmux" mode buckets by tmux session name; inside a bucket the most
 * urgent state comes first, and buckets with something waiting on you float
 * to the top so the eye lands on work, not alphabet.
 */

import { GROUP_ORDER, STATE_META, displayState, stateRank } from "./states.js";

const OUTSIDE = "Outside tmux";
const newestFirst = (a, b) => b.lastActivityAt - a.lastActivityAt;
const urgentFirst = (a, b) => stateRank(a.display) - stateRank(b.display) || newestFirst(a, b);
const isAttention = (s) => s.display === "needs_input" || s.display === "your_turn";

export function groupSessions(sessions, { mode, now, staleHours }) {
  const rows = sessions.map((s) => ({ ...s, display: displayState(s, now, staleHours) }));
  return mode === "tmux" ? byTmux(rows) : byStatus(rows);
}

function byStatus(rows) {
  return GROUP_ORDER.map((key) => {
    const sessions = rows.filter((r) => r.display === key).sort(newestFirst);
    return { key, label: STATE_META[key].label, sessions, attention: sessions.filter(isAttention).length };
  }).filter((g) => g.sessions.length);
}

function byTmux(rows) {
  const buckets = new Map();
  for (const r of rows) {
    const name = r.tmux ? r.tmux.session : OUTSIDE;
    if (!buckets.has(name)) buckets.set(name, []);
    buckets.get(name).push(r);
  }
  const groups = [...buckets].map(([name, list]) => ({
    key: `tmux:${name}`,
    label: name,
    outside: name === OUTSIDE,
    sessions: list.sort(urgentFirst),
    attention: list.filter(isAttention).length,
    // Sessions are already urgent-first, so the head is the bucket's most urgent state.
    urgency: stateRank(list[0].display),
  }));
  // Buckets that need you come first, most urgent leading; the rest stay
  // alphabetical so their positions don't shuffle as work starts and stops.
  return groups.sort((a, b) =>
    (a.outside - b.outside) ||
    (Boolean(b.attention) - Boolean(a.attention)) ||
    (a.attention && b.attention ? a.urgency - b.urgency : 0) ||
    a.label.localeCompare(b.label, undefined, { sensitivity: "base" }));
}

export const flattenGroups = (groups) => groups.flatMap((g) => g.sessions.map((s) => s.id));
