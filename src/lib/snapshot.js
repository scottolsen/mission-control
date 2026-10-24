/**
 * Snapshot helpers.
 */

/**
 * get_snapshot answers before the monitor's first poll with an empty
 * snapshot stamped generatedAt 0. That is "still loading", not "tmux isn't
 * running" or "no sessions".
 */
export function isLoaded(snap) {
  return Boolean(snap) && snap.generatedAt > 0;
}
