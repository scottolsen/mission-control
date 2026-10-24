/**
 * Search: a small fuzzy matcher over the fields people remember a session by.
 *
 * Each whitespace-separated token must match at least one field, so "shopfront
 * ledger" narrows to sessions that mention both. A token scores highest as a
 * prefix of a word, then as a substring, then as a scattered subsequence, and
 * the field's weight breaks ties (a title hit beats a hit buried in a prompt).
 */

const FIELDS = [
  [(s) => s.title, 4],
  [(s) => s.project, 3],
  [(s) => s.tmux?.session, 3],
  [(s) => s.branch, 2],
  [(s) => s.cwd, 1],
  [(s) => s.lastPrompt, 1],
];

/** 0 = no match; higher is better. An empty needle matches everything with 1. */
export function fuzzyScore(needle, haystack) {
  const n = needle.toLowerCase();
  if (!n) return 1;
  const h = (haystack || "").toLowerCase();
  if (!h) return 0;
  const at = h.indexOf(n);
  if (at === 0) return 100;
  if (at > 0) return /[\s/_\-.:›]/.test(h[at - 1]) ? 80 : 60;
  // Subsequence: reward tight clusters over letters spread across the string.
  let hi = 0, gaps = 0, last = -1;
  for (const ch of n) {
    const found = h.indexOf(ch, hi);
    if (found < 0) return 0;
    if (last >= 0) gaps += found - last - 1;
    last = found;
    hi = found + 1;
  }
  // Letters strewn across a whole sentence aren't what anyone typed for.
  if (gaps > n.length * 3) return 0;
  return Math.max(1, 40 - gaps);
}

export function matchSession(session, query) {
  const tokens = query.trim().split(/\s+/).filter(Boolean);
  let total = 0;
  for (const t of tokens) {
    let best = 0;
    for (const [get, weight] of FIELDS) best = Math.max(best, fuzzyScore(t, get(session)) * weight);
    if (!best) return 0;
    total += best;
  }
  return tokens.length ? total : 1;
}

export function filterSessions(sessions, query) {
  if (!query.trim()) return sessions;
  return sessions.filter((s) => matchSession(s, query) > 0);
}
