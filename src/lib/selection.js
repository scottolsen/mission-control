/**
 * Selection that survives snapshot updates. Sessions reorder as their states
 * change, so the selection follows the session id, not the row index.
 */

export function moveSelection(ids, current, delta) {
  if (!ids.length) return null;
  const i = ids.indexOf(current);
  if (i < 0) return ids[0];
  return ids[Math.min(ids.length - 1, Math.max(0, i + delta))];
}

/**
 * Keep `current` if it's still listed. If it vanished (the Claude process
 * exited), land on its old neighbour — the next row, else the previous one —
 * so the cursor stays where the user was looking.
 */
export function keepSelection(ids, current, previousIds) {
  if (!ids.length) return null;
  if (current && ids.includes(current)) return current;
  const at = previousIds.indexOf(current);
  if (at >= 0) {
    for (const id of previousIds.slice(at + 1)) if (ids.includes(id)) return id;
    for (const id of previousIds.slice(0, at).reverse()) if (ids.includes(id)) return id;
  }
  return ids[0];
}

/**
 * True when a re-render moved the selection off `prev` without the user
 * choosing (its session left the list, a filter hid it). The new session's
 * prompt hasn't been looked at, so its keyboard shortcuts stay off until the
 * user selects it. The first selection at boot isn't a move.
 */
export function autoMoved(prev, next) {
  return prev != null && next !== prev;
}
