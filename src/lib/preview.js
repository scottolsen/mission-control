/**
 * Terminal preview scaling. xterm renders at the pane's true cols×rows; the
 * result is shrunk with a CSS transform to fit the detail panel. Wide panes
 * (300+ cols) would shrink past legibility, so the scale stops at a floor and
 * the preview scrolls sideways instead.
 */

export function previewScale({ termWidth, termHeight, boxWidth, minScale = 0.5 }) {
  if (!(termWidth > 0) || !(boxWidth > 0)) return { scale: 1, height: termHeight || 0, overflow: false };
  const fit = Math.min(1, boxWidth / termWidth);
  const scale = Math.max(minScale, fit);
  return { scale, height: Math.round(termHeight * scale), overflow: fit < minScale };
}

import { toTerminalText } from "./reply.js";

// CSI sequences (colors, cursor) and OSC sequences (hyperlinks) render nothing.
const INVISIBLE = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g;

/**
 * capture-pane pads its output to the pane's full height, so a fresh Claude
 * pane is mostly empty rows below the prompt. Dropping them lets the preview
 * size its rows to what's actually on screen and keeps the live bottom of the
 * pane in view.
 */
export function trimCapture(ansi) {
  const rows = ansi.split("\n");
  while (rows.length && !rows[rows.length - 1].replace(INVISIBLE, "").trim()) rows.pop();
  return { text: rows.join("\n"), lines: rows.length };
}

// Reset attributes, home, clear the screen, clear the scrollback.
const CLEAR = "\x1b[0m\x1b[H\x1b[2J\x1b[3J";

/**
 * One frame for xterm: the clear travels in the same write as the content.
 * term.reset() is synchronous but term.write() is queued, so a reset issued
 * while an earlier frame was still queued let that frame land first and the
 * new one append below it — the preview showed the old tail above the header.
 */
export const paintSequence = (ansi) => CLEAR + toTerminalText(ansi);
