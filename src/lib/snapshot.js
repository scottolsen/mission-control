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

/**
 * A session running an older Claude Code than the one installed (it picks
 * up the new version only when restarted). Unknown versions aren't old.
 */
export function isOutdated(version, installed) {
  const key = (v) => (/^\d+\.\d+\.\d+$/.test(v ?? "") ? v.split(".").map(Number) : null);
  const a = key(version);
  const b = key(installed);
  if (!a || !b) return false;
  const i = a.findIndex((n, j) => n !== b[j]);
  return i !== -1 && a[i] < b[i];
}
