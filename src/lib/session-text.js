/**
 * Short human-readable strings derived from a Session: the tmux location, the
 * one-line snippet under each list row, and the current-tool label.
 */

/**
 * "3:zsh", or just "1" when tmux auto-named the window after the running
 * binary: for a Claude pane that's its version ("2.1.301"), which is noise.
 */
export function windowText(tmux) {
  const name = tmux.windowName || "";
  return /^\d+(\.\d+)+$/.test(name) || !name ? String(tmux.windowIndex) : `${tmux.windowIndex}:${name}`;
}

export function tmuxLabel(tmux) {
  if (!tmux) return "Outside tmux";
  return `${tmux.session} › ${windowText(tmux)} › ${tmux.paneId}`;
}

/**
 * The tmux location as a list row shows it: only the tmux session, and only
 * when it isn't named after the project (or the list is grouped by tmux
 * session, whose header names it). Window and pane live in the detail
 * crumbs. "" means the row shows nothing.
 */
export function rowLocation(s, { groupedByTmux = false } = {}) {
  const t = s.tmux;
  if (!t) return "Outside tmux";
  const same = Boolean(s.project) && t.session.trim().toLowerCase() === s.project.trim().toLowerCase();
  return groupedByTmux || same ? "" : t.session;
}

export function tmuxCrumbs(tmux) {
  if (!tmux) return [];
  return [
    { label: "Session", value: tmux.session },
    { label: "Window", value: windowText(tmux) },
    { label: "Pane", value: tmux.paneId },
  ];
}

export function toolLabel(tool) {
  if (!tool) return "";
  return tool.summary ? `${tool.name} · ${tool.summary}` : tool.name;
}

/**
 * Claude's replies are Markdown; the one thing worth honouring in a summary is
 * inline code. Splits `text` into plain and code runs. A run never crosses a
 * line break, and a lone or empty pair of backticks stays literal.
 */
export function inlineCode(text) {
  const out = [];
  const re = /`([^`\n]+)`/g;
  let last = 0;
  for (const m of (text || "").matchAll(re)) {
    if (m.index > last) out.push({ code: false, text: text.slice(last, m.index) });
    out.push({ code: true, text: m[1] });
    last = m.index + m[0].length;
  }
  if (last < (text || "").length) out.push({ code: false, text: text.slice(last) });
  return out;
}

const oneLine = (t) => inlineCode(t).map((r) => r.text).join("").replace(/\s+/g, " ").trim();

/**
 * What a row should whisper about the session: the tool it's running, the
 * question it's asking, or the reply it left you. `display` is the derived
 * display state (stale reads like your_turn).
 */
export function snippet(session, display) {
  if (display === "needs_input") {
    if (session.prompt?.question) return { kind: "question", text: oneLine(session.prompt.question) };
    return { kind: "question", text: session.prompt?.kind === "trust" ? "Waiting on a trust prompt" : "Waiting on a menu" };
  }
  if (display === "working" && session.currentTool) return { kind: "tool", text: toolLabel(session.currentTool) };
  if ((display === "your_turn" || display === "stale") && session.lastReply) return { kind: "reply", text: oneLine(session.lastReply) };
  if (session.lastPrompt) return { kind: "prompt", text: oneLine(session.lastPrompt) };
  if (session.lastReply) return { kind: "reply", text: oneLine(session.lastReply) };
  return { kind: "none", text: "No messages yet" };
}

/** "/Users/me/code/shopfront" → "~/code/shopfront". The webview can't ask for $HOME, so match the macOS layout. */
export function homeRelative(path) {
  return (path || "").replace(/^\/Users\/[^/]+(?=\/|$)/, "~");
}
