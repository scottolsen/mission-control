import test from "node:test";
import assert from "node:assert/strict";
import { relativeTime, absoluteTime } from "../../src/lib/time.js";

const S = 1000, M = 60 * S, H = 60 * M, D = 24 * H;
// Local-time constructor so the expectations don't depend on the TZ the tests run in.
const NOW = new Date(2026, 9, 16, 9, 30, 0).getTime();

test("relativeTime says 'now' under a minute, including small future skew", () => {
  assert.equal(relativeTime(NOW - 20 * S, NOW), "now");
  assert.equal(relativeTime(NOW + 5 * S, NOW), "now");
});

test("relativeTime uses minutes, then hours, then days", () => {
  assert.equal(relativeTime(NOW - 2 * M, NOW), "2m");
  assert.equal(relativeTime(NOW - 59 * M, NOW), "59m");
  assert.equal(relativeTime(NOW - 3 * H, NOW), "3h");
  assert.equal(relativeTime(NOW - 23 * H, NOW), "23h");
  assert.equal(relativeTime(NOW - 2 * D, NOW), "2d");
  assert.equal(relativeTime(NOW - 6 * D, NOW), "6d");
});

test("relativeTime falls back to a short date after a week", () => {
  assert.equal(relativeTime(new Date(2026, 9, 2, 12).getTime(), NOW), "Oct 2");
  assert.equal(relativeTime(new Date(2025, 11, 30).getTime(), NOW), "Dec 30, 2025");
});

test("relativeTime returns an em dash for missing timestamps", () => {
  assert.equal(relativeTime(null, NOW), "—");
  assert.equal(relativeTime(undefined, NOW), "—");
});

test("absoluteTime spells out weekday, date and 12h clock", () => {
  assert.equal(absoluteTime(new Date(2026, 9, 16, 7, 5, 2).getTime()), "Fri, Oct 16, 2026 at 7:05:02 AM");
  assert.equal(absoluteTime(new Date(2026, 9, 16, 0, 0, 9).getTime()), "Fri, Oct 16, 2026 at 12:00:09 AM");
  assert.equal(absoluteTime(new Date(2026, 9, 16, 13, 40, 0).getTime()), "Fri, Oct 16, 2026 at 1:40:00 PM");
  assert.equal(absoluteTime(null), "");
});

import { clockTime } from "../../src/lib/time.js";

test("clockTime shows just the time today, the date otherwise", () => {
  assert.equal(clockTime(new Date(2026, 9, 16, 7, 5).getTime(), NOW), "7:05 AM");
  assert.equal(clockTime(new Date(2026, 9, 2, 15, 40).getTime(), NOW), "Oct 2, 3:40 PM");
  assert.equal(clockTime(new Date(2025, 0, 9, 9, 0).getTime(), NOW), "Jan 9, 2025, 9:00 AM");
  assert.equal(clockTime(null, NOW), "");
});
