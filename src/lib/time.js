/**
 * Time formatting for the session list and detail panel.
 *
 * Month and weekday names are spelled out here rather than taken from Intl so
 * the output is identical in the webview and under node:test, whatever locale
 * either one happens to run in.
 */

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MIN = 60_000, HOUR = 60 * MIN, DAY = 24 * HOUR;

/** Compact age: "now", "2m", "3h", "4d", then "Oct 2" (or "Dec 30, 2025" across years). */
export function relativeTime(ms, now = Date.now()) {
  if (ms == null) return "—";
  // Clock skew between the backend's poll and this tick can put a timestamp
  // slightly in the future; that still reads as "now".
  const age = Math.max(0, now - ms);
  if (age < MIN) return "now";
  if (age < HOUR) return `${Math.floor(age / MIN)}m`;
  if (age < DAY) return `${Math.floor(age / HOUR)}h`;
  if (age < 7 * DAY) return `${Math.floor(age / DAY)}d`;
  const d = new Date(ms);
  const short = `${MONTHS[d.getMonth()]} ${d.getDate()}`;
  return d.getFullYear() === new Date(now).getFullYear() ? short : `${short}, ${d.getFullYear()}`;
}

const clock12 = (d) => `${d.getHours() % 12 || 12}:${String(d.getMinutes()).padStart(2, "0")} ${d.getHours() < 12 ? "AM" : "PM"}`;

/** Short wall-clock stamp beside a relative age: "7:05 AM" today, "Oct 2, 3:40 PM" otherwise. */
export function clockTime(ms, now = Date.now()) {
  if (ms == null) return "";
  const d = new Date(ms), n = new Date(now);
  if (d.toDateString() === n.toDateString()) return clock12(d);
  const day = `${MONTHS[d.getMonth()]} ${d.getDate()}`;
  return d.getFullYear() === n.getFullYear() ? `${day}, ${clock12(d)}` : `${day}, ${d.getFullYear()}, ${clock12(d)}`;
}

/** Full local timestamp for tooltips: "Fri, Oct 16, 2026 at 7:05:02 AM". */
export function absoluteTime(ms) {
  if (ms == null) return "";
  const d = new Date(ms);
  const h = d.getHours();
  const pad = (n) => String(n).padStart(2, "0");
  const clock = `${h % 12 || 12}:${pad(d.getMinutes())}:${pad(d.getSeconds())} ${h < 12 ? "AM" : "PM"}`;
  return `${DAYS[d.getDay()]}, ${MONTHS[d.getMonth()]} ${d.getDate()}, ${d.getFullYear()} at ${clock}`;
}
