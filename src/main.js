/**
 * Mission Control — UI glue.
 *
 * Everything decidable without a DOM lives in ./lib (and is unit-tested); this
 * file wires those decisions to elements. The backend pushes a Snapshot every
 * time something changes, so the list is reconciled by session id rather than
 * rebuilt: rows keep their nodes, scroll position and focus across updates,
 * and only text that actually changed is written.
 */

import { relativeTime, absoluteTime, clockTime } from "./lib/time.js";
import { STATE_META, CHIPS, chipCounts, matchesChip, displayState } from "./lib/states.js";
import { filterSessions } from "./lib/filter.js";
import { groupSessions, flattenGroups } from "./lib/grouping.js";
import { snippet, tmuxLabel, tmuxCrumbs, homeRelative, inlineCode } from "./lib/session-text.js";
import { keyAction } from "./lib/keys.js";
import { moveSelection, keepSelection, autoMoved } from "./lib/selection.js";
import { DEFAULT_SETTINGS, readSettingsForm } from "./lib/settings.js";
import { replyMode, isStateChanged, isWindowHidden, errorText, promptFingerprint } from "./lib/reply.js";
import { isLoaded } from "./lib/snapshot.js";
import { previewScale, trimCapture, paintSequence } from "./lib/preview.js";
import { ICONS } from "./lib/icons.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const el = {
  chips: $("chips"),
  search: $("search"),
  groupStatus: $("group-status"),
  groupTmux: $("group-tmux"),
  settingsBtn: $("btn-settings"),
  banner: $("banner"),
  bannerText: $("banner-text"),
  bannerRetry: $("banner-retry"),
  listPane: $("list-pane"),
  list: $("list"),
  listEmpty: $("list-empty"),
  split: $("split"),
  detail: $("detail"),
  detailEmpty: $("detail-empty"),
  detailBody: $("detail-body"),
  dState: $("d-state"),
  dTitle: $("d-title"),
  dCrumbs: $("d-crumbs"),
  dMeta: $("d-meta"),
  dAction: $("d-action"),
  dPrompt: $("d-prompt"),
  dPromptAt: $("d-prompt-at"),
  dReply: $("d-reply"),
  dReplyAt: $("d-reply-at"),
  convo: $("d-convo"),
  jump: $("btn-jump"),
  preview: $("preview"),
  previewLive: $("preview-live"),
  previewDims: $("preview-dims"),
  previewFit: $("preview-fit"),
  previewViewport: $("preview-viewport"),
  previewStage: $("preview-stage"),
  previewNote: $("preview-note"),
  refresh: $("btn-refresh"),
  syncText: $("sync-text"),
  settings: $("settings"),
  settingsForm: $("settings-form"),
  settingsCancel: $("settings-cancel"),
  settingsError: $("settings-error"),
  sNotifyYourTurn: $("s-notify-your-turn"),
  sNotifyNeedsInput: $("s-notify-needs-input"),
  sHotkey: $("s-hotkey"),
  sStale: $("s-stale"),
  sTerminal: $("s-terminal"),
  toast: $("toast"),
};

// Per-viewer conveniences only; the page must work when storage throws.
const store = {
  get(key, fallback) {
    try { return localStorage.getItem(`mc:${key}`) ?? fallback; } catch { return fallback; }
  },
  set(key, value) {
    try { localStorage.setItem(`mc:${key}`, value); } catch { /* private mode */ }
  },
};

const state = {
  snapshot: null,
  settings: { ...DEFAULT_SETTINGS },
  chip: "all",
  query: "",
  mode: store.get("group", "status") === "tmux" ? "tmux" : "status",
  selectedId: null,
  order: [],
  byId: new Map(),
  drafts: new Map(),     // session id → unsent composer text
  notices: new Map(),    // session id → { text, kind }
  sending: false,
  // Off when the selection moved by itself; digits and Esc then don't answer
  // the newly selected session's prompt until the user selects it.
  armed: true,
  // The backend's `visibility` event: false while hidden in the tray.
  windowVisible: true,
  fit: store.get("fit", "0") === "1",
};

/* ------------------------------------------------------------ helpers */

function setText(node, text) {
  if (node.textContent !== text) node.textContent = text;
}

function setAttr(node, name, value) {
  if (value == null) node.removeAttribute(name);
  else if (node.getAttribute(name) !== String(value)) node.setAttribute(name, value);
}

function setIcon(node, name) {
  if (node.dataset.iconName === name) return;
  node.dataset.iconName = name;
  node.innerHTML = ICONS[name] || "";
}

function make(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function iconSpan(name, className = "") {
  const span = make("span", className);
  span.dataset.icon = name;
  setIcon(span, name);
  return span;
}

/** A <time> that the 30 s ticker keeps current. */
function stampTime(node, ms, { withClock = false } = {}) {
  if (ms == null) {
    delete node.dataset.ts;
    setText(node, "");
    node.removeAttribute("title");
    return;
  }
  node.dataset.ts = String(ms);
  node.dataset.clock = withClock ? "1" : "";
  paintTime(node, Date.now());
  node.setAttribute("datetime", new Date(ms).toISOString());
}

function paintTime(node, now) {
  const ms = Number(node.dataset.ts);
  const rel = relativeTime(ms, now);
  const text = node.dataset.clock ? `${rel === "now" ? "just now" : `${rel} ago`} · ${clockTime(ms, now)}` : rel;
  setText(node, text);
  setAttr(node, "title", absoluteTime(ms));
}

// Static icons declared in the HTML.
for (const node of document.querySelectorAll("[data-icon]")) setIcon(node, node.dataset.icon);

/* --------------------------------------------------------------- model */

const sessions = () => state.snapshot?.sessions ?? [];
const now = () => Date.now();

function visibleGroups() {
  const opts = { mode: state.mode, now: now(), staleHours: state.settings.staleHours };
  const searched = filterSessions(sessions(), state.query);
  return groupSessions(searched, opts)
    .map((g) => ({ ...g, sessions: g.sessions.filter((s) => matchesChip(state.chip, s)) }))
    .filter((g) => g.sessions.length);
}

function selected() {
  return state.selectedId ? state.byId.get(state.selectedId) ?? null : null;
}

/* -------------------------------------------------------------- render */

function render({ reveal = false } = {}) {
  renderBanner();
  renderChips();
  const groups = visibleGroups();
  const ids = flattenGroups(groups);
  state.byId = new Map(groups.flatMap((g) => g.sessions.map((s) => [s.id, s])));
  const next = keepSelection(ids, state.selectedId, state.order);
  if (autoMoved(state.selectedId, next)) state.armed = false;
  state.selectedId = next;
  state.order = ids;
  renderList(groups);
  renderListEmpty(groups);
  renderDetail(selected());
  renderSync();
  if (reveal) revealSelected();
}

function renderBanner() {
  const err = state.snapshot?.error;
  el.banner.hidden = !err;
  if (err) setText(el.bannerText, err);
}

const chipEls = new Map();
function renderChips() {
  const rows = sessions().map((s) => ({ ...s, display: displayState(s, now(), state.settings.staleHours) }));
  const counts = chipCounts(rows);
  for (const chip of CHIPS) {
    let node = chipEls.get(chip.key);
    if (!node) {
      node = make("button", "chip");
      node.type = "button";
      node.dataset.chip = chip.key;
      node.append(make("span", "chip-dot"), make("span", "chip-label", chip.label), make("span", "chip-count"));
      node.addEventListener("click", () => setChip(state.chip === chip.key && chip.key !== "all" ? "all" : chip.key));
      el.chips.append(node);
      chipEls.set(chip.key, node);
    }
    const n = counts[chip.key];
    setText(node.querySelector(".chip-count"), String(n));
    node.classList.toggle("zero", n === 0);
    setAttr(node, "aria-pressed", String(state.chip === chip.key));
    if (chip.key === "attention") {
      node.classList.toggle("split", counts.needs_input > 0 && counts.your_turn > 0);
      node.classList.toggle("turn-only", counts.needs_input === 0 && counts.your_turn > 0);
      setAttr(node, "title", `${counts.needs_input} need input · ${counts.your_turn} your turn`);
    } else if (chip.key === "idle") {
      setAttr(node, "title", `${counts.idleOnly} idle · ${counts.stale} stale`);
    } else if (chip.key === "loop") {
      setAttr(node, "title", "Show sessions running a /loop");
    } else if (chip.key === "remote") {
      setAttr(node, "title", "Show sessions connected to Remote Control");
    } else {
      setAttr(node, "title", `Show ${chip.label.toLowerCase()} sessions`);
    }
  }
}

function setChip(key) {
  state.chip = key;
  render({ reveal: true });
}

/* ---- list (keyed reconcile) */

const rowEls = new Map();
const headEls = new Map();

function renderList(groups) {
  const nodes = [];
  const seenRows = new Set();
  const seenHeads = new Set();
  for (const g of groups) {
    let head = headEls.get(g.key);
    if (!head) {
      head = createHead();
      headEls.set(g.key, head);
    }
    updateHead(head, g);
    seenHeads.add(g.key);
    nodes.push(head);
    for (const s of g.sessions) {
      let row = rowEls.get(s.id);
      if (!row) {
        row = createRow(s.id);
        rowEls.set(s.id, row);
      }
      updateRow(row, s);
      seenRows.add(s.id);
      nodes.push(row);
    }
  }
  for (const [id, node] of rowEls) if (!seenRows.has(id)) { node.remove(); rowEls.delete(id); }
  for (const [key, node] of headEls) if (!seenHeads.has(key)) { node.remove(); headEls.delete(key); }

  // Move only nodes that are out of place, so unchanged rows are never touched.
  let ref = el.list.firstChild;
  for (const node of nodes) {
    if (node === ref) ref = ref.nextSibling;
    else el.list.insertBefore(node, ref);
  }
}

function createHead() {
  const head = make("div", "group-head");
  head.setAttribute("role", "presentation");
  const icon = make("span", "g-icon");
  head.append(icon, make("span", "g-label"), make("span", "g-count"), make("span", "g-attn"), make("span", "g-rule"));
  return head;
}

const STATE_COLOR = {
  needs_input: "var(--needs-ink)", your_turn: "var(--turn-ink)", working: "var(--work-ink)",
  idle: "var(--idle-ink)", stale: "var(--stale-ink)", unknown: "var(--stale-ink)",
};

function updateHead(head, g) {
  const byStatus = state.mode === "status";
  head.classList.toggle("by-status", byStatus);
  const icon = head.querySelector(".g-icon");
  if (byStatus) {
    setIcon(icon, STATE_META[g.key].icon);
    head.style.setProperty("--g-c", STATE_COLOR[g.key]);
  } else {
    setIcon(icon, g.outside ? "terminal" : "layers");
    head.style.removeProperty("--g-c");
  }
  setText(head.querySelector(".g-label"), g.label);
  setText(head.querySelector(".g-count"), String(g.sessions.length));
  const attn = head.querySelector(".g-attn");
  const showAttn = !byStatus && g.attention > 0;
  attn.hidden = !showAttn;
  setText(attn, showAttn ? `${g.attention} need${g.attention === 1 ? "s" : ""} you` : "");
  // Coral only when a menu is actually blocking; a reply owed reads amber.
  attn.classList.toggle("turn", showAttn && !g.sessions.some((s) => s.display === "needs_input"));
}

function createRow(id) {
  const row = make("div", "row");
  row.dataset.id = id;
  row.setAttribute("role", "option");
  const icon = make("span", "row-icon");
  const title = make("span", "row-title");
  const time = make("time", "row-time");
  const meta = make("span", "row-meta");
  const project = make("span", "row-project");
  const branch = make("span", "row-branch");
  branch.append(iconSpan("branch"), make("span"));
  const loc = make("span", "row-loc");
  const loop = make("span", "row-loop");
  loop.append(iconSpan("loop"), make("span", null, "loop"));
  const remote = make("span", "row-remote");
  remote.append(iconSpan("remote"), make("span", null, "remote"));
  meta.append(loop, remote, project, branch, loc);
  const snip = make("span", "row-snippet");
  snip.append(make("span", "s-kind"), make("span", "s-text"));
  row.append(icon, title, time, meta, snip);
  return row;
}

const SNIPPET_ICON = { tool: "tool", question: "alert", reply: "spark", prompt: "user", none: "" };

function updateRow(row, s) {
  setAttr(row, "data-state", s.display);
  setAttr(row, "data-loop", s.loop ? "" : null);
  const loop = row.querySelector(".row-loop");
  loop.hidden = !s.loop;
  setAttr(loop, "title", s.loop ? `Looping: ${s.loop.label}` : null);
  const remote = row.querySelector(".row-remote");
  remote.hidden = !s.remoteSessionId;
  setAttr(remote, "title", s.remoteSessionId ? "Connected to Remote Control" : null);
  setAttr(row, "aria-selected", String(s.id === state.selectedId));
  const meta = STATE_META[s.display];
  const icon = row.querySelector(".row-icon");
  setIcon(icon, meta.icon);
  setAttr(icon, "title", meta.label);
  setAttr(icon, "aria-label", meta.label);

  const title = s.title || s.project || "Untitled session";
  setText(row.querySelector(".row-title"), title);
  setAttr(row.querySelector(".row-title"), "title", title);
  const time = row.querySelector(".row-time");
  if (time.dataset.ts !== String(s.lastActivityAt)) stampTime(time, s.lastActivityAt);

  setText(row.querySelector(".row-project"), s.project);
  const branch = row.querySelector(".row-branch");
  branch.hidden = !s.branch;
  setText(branch.lastChild, s.branch || "");
  setAttr(branch, "title", s.branch);
  const loc = tmuxLabel(s.tmux);
  // Grouped by tmux session, the header already names it.
  const shortLoc = state.mode === "tmux" && s.tmux ? loc.slice(s.tmux.session.length + 3) : loc;
  setText(row.querySelector(".row-loc"), shortLoc);
  setAttr(row.querySelector(".row-loc"), "title", loc);

  const snip = snippet(s, s.display);
  setAttr(row, "data-snippet", snip.kind);
  const kind = row.querySelector(".s-kind");
  if (SNIPPET_ICON[snip.kind]) setIcon(kind, SNIPPET_ICON[snip.kind]);
  kind.hidden = !SNIPPET_ICON[snip.kind];
  setText(row.querySelector(".s-text"), snip.text);
  setAttr(row.querySelector(".s-text"), "title", snip.text);
  setAttr(row, "aria-label", `${title}, ${meta.label}${s.loop ? ", looping" : ""}${s.remoteSessionId ? ", remote control" : ""}, ${loc}`);
}

function revealSelected() {
  const row = state.selectedId && rowEls.get(state.selectedId);
  if (row) row.scrollIntoView({ block: "nearest" });
}

function renderListEmpty(groups) {
  const snap = state.snapshot;
  el.listEmpty.replaceChildren();
  let show = true;
  if (!snap) {
    el.list.replaceChildren(...[0, 1, 2, 3].map(() => make("div", "ghost")));
    rowEls.clear();
    headEls.clear();
    show = false;
  } else if (el.list.querySelector(".ghost")) {
    for (const g of el.list.querySelectorAll(".ghost")) g.remove();
  }

  if (!snap) {
    // ghosts only
  } else if (!snap.sessions.length && !snap.tmuxRunning) {
    emptyState(el.listEmpty, "plug", "tmux isn't running",
      "Mission Control watches Claude Code sessions in tmux. Start tmux and run claude in a pane; it shows up here within a couple of seconds.");
  } else if (!snap.sessions.length) {
    emptyState(el.listEmpty, "terminal", "No Claude sessions running",
      "Run claude in any tmux pane and it appears here, with its state and last message.");
  } else if (!groups.length && state.query.trim()) {
    const box = emptyState(el.listEmpty, "search", `No sessions match “${state.query.trim()}”`,
      "Search covers titles, projects, branches, tmux sessions, directories and your last prompt.");
    const clear = make("button", "btn", "Clear search");
    clear.type = "button";
    clear.addEventListener("click", clearSearch);
    box.append(clear);
  } else if (!groups.length) {
    const copy = {
      attention: ["Nothing needs you", "No session is waiting on a reply or a menu choice."],
      working: ["Nothing is working", "No session is running a turn right now."],
      idle: ["No inactive sessions", "Every session is working or waiting on you."],
      loop: ["No loops running", "Start one in any session with /loop, e.g. /loop 5m /babysit-prs."],
      remote: ["No remote sessions", "Run /rc in a session to drive it from claude.ai or the Claude app."],
    }[state.chip] || ["No sessions", ""];
    const box = emptyState(el.listEmpty, "check", copy[0], copy[1]);
    const all = make("button", "btn", "Show all sessions");
    all.type = "button";
    all.addEventListener("click", () => setChip("all"));
    box.append(all);
  } else {
    show = false;
  }
  el.listEmpty.hidden = !show;
  el.list.hidden = show;
  // With nothing to list at all, the explanation gets the whole window rather
  // than a narrow column beside an empty detail panel.
  document.body.classList.toggle("no-sessions", Boolean(snap) && !snap.sessions.length);
}

function emptyState(container, icon, heading, body) {
  const iconBox = make("div", "e-icon");
  iconBox.innerHTML = ICONS[icon];
  container.append(iconBox, make("h2", null, heading));
  if (body) container.append(make("p", null, body));
  return container;
}

function renderSync() {
  const snap = state.snapshot;
  if (!snap) return setText(el.syncText, "Connecting…");
  const n = snap.sessions.length;
  const rel = relativeTime(snap.generatedAt, now());
  setText(el.syncText, `${n} session${n === 1 ? "" : "s"} · updated ${rel === "now" ? "just now" : `${rel} ago`}`);
  setAttr(el.refresh, "title", `Last poll ${absoluteTime(snap.generatedAt)} — refresh now (⌘R)`);
}

/* -------------------------------------------------------------- detail */

let detailFor = null;     // id the detail panel currently shows
let actionKey = null;     // signature of the built action region
let renderedState = null; // contract state the action region was built for
let renderedPrompt = null; // the prompt its option buttons were built from

function renderDetail(s) {
  if (!s) {
    detailFor = null;
    actionKey = null;
    el.detailBody.hidden = true;
    el.detailEmpty.hidden = false;
    el.detailEmpty.replaceChildren();
    // When the list is empty it already explains why; the detail stays quiet.
    if (state.snapshot && state.order.length) {
      emptyState(el.detailEmpty, "status", "No session selected", "Pick a session on the left, or use ↑ and ↓.");
    } else if (!state.snapshot) {
      emptyState(el.detailEmpty, "refresh", "Looking for Claude sessions…", "Reading tmux and Claude Code's session registry.");
    }
    stopPreview();
    return;
  }
  const switched = detailFor !== s.id;
  if (switched) {
    detailFor = s.id;
    el.detail.scrollTop = 0;
    resetPreview(s);
  }
  el.detailEmpty.hidden = true;
  el.detailBody.hidden = false;

  const meta = STATE_META[s.display];
  const pill = el.dState;
  setAttr(pill, "data-state", s.display);
  if (pill.dataset.key !== s.display + s.state) {
    pill.dataset.key = s.display + s.state;
    pill.replaceChildren(iconSpan(meta.icon), make("span", null, meta.label));
    if (s.display === "stale") pill.append(make("span", "pill-sub", `· ${STATE_META[s.state].label.toLowerCase()}`));
  }

  setText(el.dTitle, s.title || s.project || "Untitled session");
  renderCrumbs(s);
  renderMeta(s);
  el.jump.disabled = !s.jumpable;
  setAttr(el.jump, "title", s.jumpable ? "Switch tmux to this pane (Enter)" : "This session isn't running in tmux");

  renderAction(s);
  renderBubble(el.dPrompt, el.dPromptAt, s.lastPrompt, s.lastPromptAt, "No prompt yet");
  renderBubble(el.dReply, el.dReplyAt, s.lastReply, s.lastReplyAt, "No reply yet");
  // Two "nothing yet" cards say less than no cards.
  el.convo.hidden = !s.lastPrompt && !s.lastReply;
}

function renderCrumbs(s) {
  const key = JSON.stringify(s.tmux);
  if (el.dCrumbs.dataset.key === key) return;
  el.dCrumbs.dataset.key = key;
  const items = tmuxCrumbs(s.tmux).map((c) => {
    const li = make("li");
    li.append(make("span", "c-label", c.label), make("span", "c-val", c.value));
    return li;
  });
  if (!items.length) items.push(make("li", "c-outside", "Outside tmux — no pane to jump to"));
  el.dCrumbs.replaceChildren(...items);
}

function renderMeta(s) {
  const items = [
    { label: "Directory", value: homeRelative(s.cwd), title: s.cwd, mono: true, wide: true },
    { label: "Branch", value: s.branch, mono: true, none: "No branch", wide: true },
    { label: "PID", value: s.pid != null ? String(s.pid) : null, mono: true, none: "—" },
    { label: "Version", value: s.version, mono: true, none: "—" },
    { label: "Started", ts: s.startedAt },
    { label: "Last activity", ts: s.lastActivityAt },
  ];
  if (s.loop) items.push({ label: "Loop", loop: s.loop });
  if (s.remoteSessionId) items.push({ label: "Remote Control", value: `Connected · ${s.remoteSessionId}`, mono: true, full: true, title: "Bridge session id" });
  const key = JSON.stringify(items);
  if (el.dMeta.dataset.key === key) return;
  el.dMeta.dataset.key = key;
  el.dMeta.replaceChildren(...items.map((it) => {
    const wrap = make("div", it.loop ? "full loop" : it.full ? "full remote" : it.wide ? "wide" : "");
    const dd = make("dd", it.mono ? "mono" : "");
    if (it.loop) {
      const sub = make("span", "sub", it.loop.lastFiredAt == null ? " · no run yet" : " · last run ");
      if (it.loop.lastFiredAt != null) {
        const time = make("time");
        stampTime(time, it.loop.lastFiredAt, { withClock: true });
        sub.append(time);
      }
      dd.append(make("span", "mono", it.loop.label), sub);
      dd.title = it.loop.label;
    } else if (it.ts !== undefined) {
      const time = make("time");
      if (it.ts == null) { dd.classList.add("none"); dd.textContent = "—"; } else { stampTime(time, it.ts, { withClock: true }); dd.append(time); }
    } else if (it.value) {
      dd.textContent = it.value;
      dd.title = it.title || it.value;
    } else {
      dd.classList.add("none");
      dd.textContent = it.none;
    }
    wrap.append(make("dt", null, it.label), dd);
    return wrap;
  }));
}

function renderBubble(textEl, timeEl, text, at, none) {
  const value = text || none;
  if (textEl.dataset.text !== value) {
    textEl.dataset.text = value;
    textEl.replaceChildren(...(text ? inlineCode(text) : [{ code: false, text: none }])
      .map((r) => (r.code ? make("code", null, r.text) : document.createTextNode(r.text))));
  }
  textEl.classList.toggle("none", !text);
  if (timeEl.dataset.ts !== String(at)) stampTime(timeEl, at, { withClock: true });
}

/* ---- action region: options, composer, or just Jump */

function renderAction(s) {
  const mode = replyMode(s);
  const notice = state.notices.get(s.id);
  const key = [s.id, mode, s.state, JSON.stringify(s.prompt), notice?.text, state.sending, mode === "choose" && state.armed].join("|");
  renderedState = s.state;
  renderedPrompt = s.prompt;
  if (key === actionKey) {
    updateToolLine(s);
    return;
  }
  // Rebuilding would drop the caret; carry focus over if the composer had it.
  const composer = el.dAction.querySelector("textarea");
  const hadFocus = composer && document.activeElement === composer;
  const caret = composer ? [composer.selectionStart, composer.selectionEnd] : null;
  actionKey = key;
  el.dAction.dataset.mode = mode;
  el.dAction.dataset.state = s.display;
  el.dAction.replaceChildren();

  if (mode === "none") return;
  if (mode === "choose") buildChoices(s);
  else if (mode === "jump") buildJumpOnly(s);
  else buildComposer(s, mode);

  if (notice) el.dAction.insertBefore(noticeEl(notice), el.dAction.children[1] || null);

  if (hadFocus) {
    const next = el.dAction.querySelector("textarea");
    if (next) {
      next.focus();
      if (caret) next.setSelectionRange(...caret);
    }
  }
}

function actionHead(icon, text, kind) {
  const head = make("div", "a-head");
  head.append(iconSpan(icon), make("span", null, text));
  if (kind) head.append(make("span", "a-kind", kind));
  return head;
}

const PROMPT_KIND = { permission: "Permission", question: "Question", trust: "Trust prompt", other: "Menu" };

function buildChoices(s) {
  const p = s.prompt;
  el.dAction.append(actionHead("alert", "Waiting on you", PROMPT_KIND[p.kind]));
  if (p.question) el.dAction.append(make("p", "a-question", p.question));
  const list = make("div", "options");
  for (const opt of p.options) {
    const btn = make("button", "option");
    btn.type = "button";
    btn.dataset.key = opt.key;
    btn.disabled = state.sending;
    btn.append(make("kbd", null, opt.key), make("span", "o-label", opt.label));
    btn.addEventListener("click", () => sendChoice(opt.key, opt.label));
    list.append(btn);
  }
  el.dAction.append(list);
  const foot = make("div", "a-foot");
  const dismiss = make("button", "btn", "Dismiss");
  dismiss.type = "button";
  dismiss.disabled = state.sending;
  dismiss.append(make("kbd", null, "esc"));
  dismiss.addEventListener("click", () => sendChoice("Escape", null));
  const keys = `1–${p.options.length}`;
  foot.append(make("span", "a-hint", state.armed
    ? `Press ${keys} to choose. Sent straight to the pane, no Enter.`
    : `The selection moved here on its own, so ${keys} and Esc are off. Click this session or press ↑/↓ to turn them on.`), make("span", "spacer"), dismiss);
  el.dAction.append(foot);
}

function buildJumpOnly(s) {
  const trust = s.prompt?.kind === "trust";
  el.dAction.append(actionHead("alert", "Waiting on you", s.prompt ? PROMPT_KIND[s.prompt.kind] : null));
  if (s.prompt?.question) el.dAction.append(make("p", "a-question", s.prompt.question));
  el.dAction.append(make("p", "a-text", trust
    ? "Claude Code is asking whether to trust this folder. Answer it in the pane: jump there with Enter."
    : s.state === "needs_input"
      ? "This menu can't be answered from here. Jump to the pane to respond."
      : "This session's state is unknown. Jump to the pane to see it."));
}

function buildComposer(s, mode) {
  const queue = mode === "queue";
  el.dAction.append(actionHead(queue ? "spinner" : "reply", queue ? "Claude is working" : "Reply", null));
  if (queue) {
    const line = make("div", "tool-line");
    line.append(iconSpan("tool"), make("span", "t-name"), make("span", "t-sum"));
    el.dAction.append(line);
    updateToolLine(s);
  }
  const box = make("div", "composer");
  const ta = make("textarea", "composer-input");
  ta.rows = 3;
  ta.spellcheck = true;
  ta.placeholder = queue ? "Queue a message for when this turn ends…" : `Message ${s.project || "Claude"}…`;
  ta.setAttribute("aria-label", queue ? "Queue message" : "Reply");
  ta.value = state.drafts.get(s.id) || "";
  ta.disabled = state.sending;
  ta.addEventListener("input", () => {
    if (ta.value) state.drafts.set(s.id, ta.value); else state.drafts.delete(s.id);
    autoGrow(ta);
    sendBtn.disabled = !ta.value.trim();
  });
  const bar = make("div", "composer-bar");
  const sendBtn = make("button", "btn btn-primary");
  sendBtn.type = "button";
  sendBtn.append(iconSpan("send"), make("span", null, queue ? "Queue message" : "Send"), make("kbd", null, "⌘⏎"));
  sendBtn.disabled = state.sending || !ta.value.trim();
  sendBtn.addEventListener("click", sendReply);
  // The backend flattens line breaks (a raw newline would submit early), so say so.
  bar.append(make("span", "a-hint", queue
    ? "Claude Code queues it for when the turn ends. Line breaks are sent as spaces."
    : "Typed into the pane, then Enter. Line breaks are sent as spaces."), sendBtn);
  box.append(ta, bar);
  el.dAction.append(box);
  requestAnimationFrame(() => autoGrow(ta));
}

function updateToolLine(s) {
  const line = el.dAction.querySelector(".tool-line");
  if (!line) return;
  const tool = s.currentTool;
  setText(line.querySelector(".t-name"), tool ? tool.name : "Thinking");
  setText(line.querySelector(".t-sum"), tool ? tool.summary : "No tool running");
  setAttr(line.querySelector(".t-sum"), "title", tool?.summary || null);
}

function autoGrow(ta) {
  ta.style.height = "auto";
  ta.style.height = `${Math.min(220, Math.max(64, ta.scrollHeight))}px`;
}

function noticeEl({ text, kind }) {
  const box = make("div", `notice${kind === "error" ? " error" : ""}`);
  box.setAttribute("role", "alert");
  box.append(iconSpan("alert"), make("span", null, text));
  return box;
}

/* ------------------------------------------------------------- actions */

async function jump() {
  const s = selected();
  if (!s) return;
  if (!s.jumpable || !s.tmux) return toast("This session isn't running in tmux, so there's no pane to jump to.", "error");
  try {
    await invoke("jump", { paneId: s.tmux.paneId });
    toast(`Jumped to ${tmuxLabel(s.tmux)}`);
  } catch (err) {
    toast(`Couldn't jump: ${errorText(err)}`, "error");
  }
}

async function sendReply() {
  const s = selected();
  const ta = el.dAction.querySelector("textarea");
  if (!s?.tmux || !ta || state.sending) return;
  const text = ta.value.trim();
  if (!text) return;
  const expectedState = renderedState;
  const queued = expectedState === "working";
  await withSending(s, async () => {
    await invoke("send_reply", { paneId: s.tmux.paneId, text, expectedState, expectedSessionId: s.id });
    state.drafts.delete(s.id);
    toast(queued ? `Queued for ${s.project}` : `Sent to ${s.project}`);
  });
}

async function sendChoice(key, label) {
  const s = selected();
  if (!s?.tmux || state.sending) return;
  const expectedState = renderedState;
  // The backend sends only while the pane shows this exact prompt.
  const expectedPrompt = promptFingerprint(renderedPrompt);
  await withSending(s, async () => {
    await invoke("send_choice", { paneId: s.tmux.paneId, key, expectedState, expectedSessionId: s.id, expectedPrompt });
    toast(key === "Escape" ? `Dismissed the prompt in ${s.project}` : `Chose “${label}” in ${s.project}`);
  });
}

/**
 * Shared send path. A "state_changed" rejection means the pane moved on since
 * we rendered it (the user answered in tmux, Claude finished): never retry —
 * say so, keep the draft, and refresh so they can look again.
 */
async function withSending(s, fn) {
  const fromComposer = document.activeElement?.classList?.contains("composer-input");
  state.sending = true;
  state.notices.delete(s.id);
  renderDetail(selected());
  try {
    await fn();
  } catch (err) {
    if (isStateChanged(err)) {
      state.notices.set(s.id, { text: "This session changed — review before sending.", kind: "warn" });
    } else {
      state.notices.set(s.id, { text: `Couldn't send: ${errorText(err)}`, kind: "error" });
    }
    refresh();
  } finally {
    state.sending = false;
    actionKey = null;
    render();
    // Sending disabled the textarea (dropping focus); hand it back for the next message.
    if (fromComposer && selected()?.id === s.id) el.dAction.querySelector("textarea")?.focus();
  }
}

async function refresh() {
  el.refresh.classList.add("busy");
  try {
    applySnapshot(await invoke("refresh"));
  } catch (err) {
    toast(`Couldn't refresh: ${errorText(err)}`, "error");
  } finally {
    setTimeout(() => el.refresh.classList.remove("busy"), 300);
  }
}

function applySnapshot(snap) {
  // Before the first poll the backend answers generatedAt 0: keep the loading ghosts.
  if (!isLoaded(snap)) return;
  state.snapshot = snap;
  // A send's notice is about a state that no longer exists once the session
  // settles somewhere new; keep it only while its session is still listed.
  for (const id of state.notices.keys()) if (!snap.sessions.some((s) => s.id === id)) state.notices.delete(id);
  render();
}

function select(id, { reveal = true } = {}) {
  if (!id) return;
  // Any deliberate selection, even of the row already selected, turns the
  // choose shortcuts back on.
  if (!state.armed) {
    state.armed = true;
    if (id === state.selectedId) renderDetail(selected());
  }
  if (id === state.selectedId) return;
  state.selectedId = id;
  for (const [rid, row] of rowEls) setAttr(row, "aria-selected", String(rid === id));
  renderDetail(selected());
  if (reveal) revealSelected();
}

function setMode(mode) {
  if (state.mode === mode) return;
  state.mode = mode;
  store.set("group", mode);
  el.groupStatus.setAttribute("aria-checked", String(mode === "status"));
  el.groupTmux.setAttribute("aria-checked", String(mode === "tmux"));
  // Headers are mode-specific; let the reconcile build fresh ones.
  for (const head of headEls.values()) head.remove();
  headEls.clear();
  render({ reveal: true });
}

function clearSearch() {
  el.search.value = "";
  state.query = "";
  render({ reveal: true });
}

/* ------------------------------------------------------------- preview */

let term = null;
let captureTimer = null;
let captureBusy = false;
let lastCapture = null;
let previewFor = null;

function ensureTerm() {
  if (term) return term;
  const css = getComputedStyle(document.documentElement);
  // DOM renderer on purpose: the preview is CSS-scaled, and transformed DOM
  // text stays crisp where a scaled WebGL canvas would blur.
  term = new Terminal({
    fontFamily: 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace',
    fontSize: 12,
    lineHeight: 1.1,
    cursorBlink: false,
    cursorInactiveStyle: "none",
    disableStdin: true,
    scrollback: 500,
    allowProposedApi: false,
    theme: {
      background: css.getPropertyValue("--term-bg").trim() || "#0a0c10",
      foreground: css.getPropertyValue("--term-fg").trim() || "#d9dee8",
      cursor: "transparent",
      selectionBackground: "rgba(143,156,255,0.35)",
    },
  });
  term.open(el.previewStage);
  new ResizeObserver(() => scalePreview()).observe(el.previewViewport);
  return term;
}

function resetPreview(s) {
  previewFor = s.id;
  lastCapture = null;
  const hasPane = Boolean(s.tmux);
  el.previewViewport.hidden = !hasPane;
  el.previewLive.hidden = !hasPane;
  el.previewFit.hidden = !hasPane;
  setText(el.previewDims, hasPane ? `${s.tmux.width}×${s.tmux.height}` : "");
  el.previewNote.hidden = hasPane;
  if (!hasPane) {
    setText(el.previewNote, "This session runs outside tmux, so there's no pane to preview.");
    return;
  }
  if (term) term.reset();
  el.previewStage.style.opacity = "0";
  startPreview();
}

function startPreview() {
  if (!captureTimer) captureTimer = setInterval(captureTick, 1000);
  captureTick();
}

function stopPreview() {
  clearInterval(captureTimer);
  captureTimer = null;
}

async function captureTick() {
  const s = selected();
  const visible = document.visibilityState === "visible" && state.windowVisible;
  el.previewLive.classList.toggle("paused", !visible);
  setText(el.previewLive, visible ? "Live" : "Paused");
  if (!s?.tmux || !visible || captureBusy) return;
  captureBusy = true;
  const id = s.id;
  try {
    const cap = await invoke("capture_pane", { paneId: s.tmux.paneId });
    if (previewFor === id) paintCapture(cap);
  } catch (err) {
    // Hidden between the check above and the capture: just paused.
    if (isWindowHidden(err)) return;
    if (previewFor === id) {
      el.previewNote.hidden = false;
      setText(el.previewNote, `Couldn't capture the pane: ${errorText(err)}`);
    }
  } finally {
    captureBusy = false;
  }
}

function paintCapture(cap) {
  const t = ensureTerm();
  el.previewNote.hidden = true;
  const { text, lines } = trimCapture(cap.ansi || "");
  const cols = Math.max(20, cap.width || 80);
  // Rows track the visible content (capped at the pane's height) so a short
  // pane isn't a tall black box; the newest output stays in view.
  const rows = Math.max(6, Math.min(cap.height || 24, lines + 1));
  setText(el.previewDims, `${cap.width}×${cap.height}`);
  const sameSize = t.cols === cols && t.rows === rows;
  if (sameSize && text === lastCapture) return;
  if (!sameSize) t.resize(cols, rows);
  lastCapture = text;
  t.write(paintSequence(text), () => {
    t.scrollToBottom();
    scalePreview();
    el.previewStage.style.opacity = "1";
  });
}

function scalePreview() {
  if (!term?.element) return;
  const screen = term.element.querySelector(".xterm-screen");
  if (!screen) return;
  const vp = el.previewViewport;
  const pad = parseFloat(getComputedStyle(vp).paddingLeft) * 2;
  const r = previewScale({
    termWidth: screen.offsetWidth,
    termHeight: screen.offsetHeight,
    boxWidth: vp.clientWidth - pad,
    // 0.8 keeps the 12px terminal font near 10px; wider panes scroll sideways.
    minScale: state.fit ? 0.05 : 0.8,
  });
  term.element.style.transform = `scale(${r.scale})`;
  el.previewStage.style.width = `${Math.ceil(screen.offsetWidth * r.scale)}px`;
  el.previewStage.style.height = `${r.height}px`;
}

function toggleFit() {
  state.fit = !state.fit;
  store.set("fit", state.fit ? "1" : "0");
  el.previewFit.setAttribute("aria-pressed", String(state.fit));
  setIcon(el.previewFit.firstElementChild, state.fit ? "shrink" : "expand");
  const label = state.fit ? "Show the pane at a readable size" : "Fit the whole pane width";
  el.previewFit.title = label;
  el.previewFit.setAttribute("aria-label", label);
  scalePreview();
}

document.addEventListener("visibilitychange", captureTick);
// WKWebView may keep reporting "visible" when the window is hidden to the
// tray, so the backend says so itself.
listen("visibility", (e) => {
  state.windowVisible = e.payload !== false;
  captureTick();
});

/* ------------------------------------------------------------ settings */

let settingsReturnFocus = null;

function openSettings() {
  if (!el.settings.hidden) return;
  settingsReturnFocus = document.activeElement;
  const s = state.settings;
  el.sNotifyYourTurn.checked = s.notifyYourTurn;
  el.sNotifyNeedsInput.checked = s.notifyNeedsInput;
  el.sHotkey.value = s.hotkey;
  el.sStale.value = String(s.staleHours);
  el.sTerminal.value = s.terminalApp;
  clearSettingsErrors();
  el.settings.hidden = false;
  el.sNotifyYourTurn.focus();
}

function closeSettings() {
  el.settings.hidden = true;
  if (settingsReturnFocus?.isConnected) settingsReturnFocus.focus();
}

function clearSettingsErrors() {
  el.settingsError.hidden = true;
  for (const p of el.settingsForm.querySelectorAll(".field-error")) p.hidden = true;
  for (const i of el.settingsForm.querySelectorAll("[aria-invalid]")) {
    i.removeAttribute("aria-invalid");
    i.removeAttribute("aria-describedby");
  }
}

const FIELD_INPUT = { hotkey: "sHotkey", staleHours: "sStale", terminalApp: "sTerminal" };

function showFieldError(field, message) {
  const p = el.settingsForm.querySelector(`[data-error-for="${field}"]`);
  const input = el[FIELD_INPUT[field]];
  p.textContent = message;
  p.hidden = false;
  input.setAttribute("aria-invalid", "true");
  input.setAttribute("aria-describedby", p.id);
  input.focus();
}

async function saveSettings(e) {
  e.preventDefault();
  clearSettingsErrors();
  const r = readSettingsForm({
    notifyYourTurn: el.sNotifyYourTurn.checked,
    notifyNeedsInput: el.sNotifyNeedsInput.checked,
    hotkey: el.sHotkey.value,
    staleHours: el.sStale.value,
    terminalApp: el.sTerminal.value,
  });
  if (!r.ok) return showFieldError(r.field, r.error);
  try {
    state.settings = await invoke("set_settings", { settings: r.settings });
    closeSettings();
    toast("Settings saved");
    render();
  } catch (err) {
    const msg = errorText(err);
    // The backend is the judge of whether a shortcut can register.
    if (/hotkey|shortcut|accelerator/i.test(msg)) showFieldError("hotkey", msg);
    else {
      el.settingsError.textContent = `Couldn't save settings: ${msg}`;
      el.settingsError.hidden = false;
    }
  }
}

/* ---------------------------------------------------------------- toast */

let toastTimer = null;
function toast(text, kind = "ok") {
  el.toast.className = `toast${kind === "error" ? " error" : ""}`;
  el.toast.replaceChildren(iconSpan(kind === "error" ? "alert" : "check"), make("span", "t-text", text));
  el.toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.toast.hidden = true; }, kind === "error" ? 5000 : 2600);
}

/* ------------------------------------------------------------- keyboard */

function focusContext() {
  const a = document.activeElement;
  if (!a || a === document.body) return "none";
  if (a === el.search) return "search";
  if (a.classList?.contains("composer-input")) return "composer";
  if (a.matches?.("input, textarea, select, [contenteditable]")) return "field";
  if (a.matches?.("button")) return "button";
  return "none";
}

document.addEventListener("keydown", (e) => {
  if (e.isComposing) return;
  const s = selected();
  const action = keyAction(e, {
    focus: focusContext(),
    modalOpen: !el.settings.hidden,
    searchHasText: Boolean(el.search.value),
    needsInput: s?.state === "needs_input" && replyMode(s) === "choose",
    armed: state.armed,
    optionCount: s?.prompt?.options?.length ?? 0,
  });
  if (!action) return;
  e.preventDefault();
  if (action.startsWith("choose:")) {
    const key = action.slice(7);
    const opt = s.prompt.options.find((o) => o.key === key);
    return sendChoice(key, opt?.label ?? key);
  }
  switch (action) {
    case "next": return select(moveSelection(state.order, state.selectedId, 1));
    case "prev": return select(moveSelection(state.order, state.selectedId, -1));
    case "jump": return jump();
    case "focusSearch": el.search.focus(); el.search.select(); return;
    case "clearSearch": return clearSearch();
    case "blur": document.activeElement.blur(); return;
    case "toggleGroup": return setMode(state.mode === "status" ? "tmux" : "status");
    case "focusComposer": {
      const ta = el.dAction.querySelector("textarea");
      if (ta) ta.focus();
      else if (s) toast(s.state === "needs_input" ? "Choose an option, or jump to the pane." : "This session can't take a reply here.", "error");
      return;
    }
    case "send": return sendReply();
    case "dismiss": return sendChoice("Escape", null);
    case "openSettings": return openSettings();
    case "closeModal": return closeSettings();
    case "refresh": return refresh();
    case "hide": return invoke("hide_window").catch(() => {});
  }
});

/* --------------------------------------------------------------- events */

el.list.addEventListener("click", (e) => {
  const row = e.target.closest(".row");
  if (row) select(row.dataset.id, { reveal: false });
});
el.list.addEventListener("dblclick", (e) => {
  const row = e.target.closest(".row");
  if (row) { select(row.dataset.id, { reveal: false }); jump(); }
});

el.search.addEventListener("input", () => {
  state.query = el.search.value;
  render({ reveal: true });
});

el.groupStatus.addEventListener("click", () => setMode("status"));
el.groupTmux.addEventListener("click", () => setMode("tmux"));
el.settingsBtn.addEventListener("click", openSettings);
el.settingsCancel.addEventListener("click", closeSettings);
el.settingsForm.addEventListener("submit", saveSettings);
el.settings.addEventListener("mousedown", (e) => { if (e.target === el.settings) closeSettings(); });
el.jump.addEventListener("click", jump);
el.refresh.addEventListener("click", refresh);
el.bannerRetry.addEventListener("click", refresh);
el.previewFit.addEventListener("click", toggleFit);
el.groupStatus.setAttribute("aria-checked", String(state.mode === "status"));
el.groupTmux.setAttribute("aria-checked", String(state.mode === "tmux"));
el.previewFit.setAttribute("aria-pressed", String(state.fit));
if (state.fit) setIcon(el.previewFit.firstElementChild, "shrink");

/* ---- resizable split */

const DEFAULT_LIST_W = 392;
function setListWidth(px) {
  const w = Math.round(Math.min(Math.max(px, 300), window.innerWidth * 0.6));
  document.documentElement.style.setProperty("--list-w", `${w}px`);
  return w;
}
setListWidth(Number(store.get("listW", DEFAULT_LIST_W)) || DEFAULT_LIST_W);

el.split.addEventListener("pointerdown", (e) => {
  e.preventDefault();
  el.split.setPointerCapture(e.pointerId);
  el.split.classList.add("dragging");
  const startX = e.clientX;
  const startW = el.listPane.getBoundingClientRect().width;
  const move = (ev) => setListWidth(startW + ev.clientX - startX);
  const up = (ev) => {
    el.split.classList.remove("dragging");
    el.split.removeEventListener("pointermove", move);
    el.split.removeEventListener("pointerup", up);
    store.set("listW", String(setListWidth(startW + ev.clientX - startX)));
  };
  el.split.addEventListener("pointermove", move);
  el.split.addEventListener("pointerup", up);
});
el.split.addEventListener("dblclick", () => store.set("listW", String(setListWidth(DEFAULT_LIST_W))));

/* ---- relative times: re-render every 30 s even without a new snapshot */

setInterval(() => {
  const t = now();
  for (const node of document.querySelectorAll("[data-ts]")) paintTime(node, t);
  renderSync();
  // Staleness is time-based too: a session can cross staleHours between polls.
  if (state.snapshot) render();
}, 30_000);

/* ----------------------------------------------------------------- boot */

render();

(async () => {
  try {
    state.settings = { ...DEFAULT_SETTINGS, ...(await invoke("get_settings")) };
  } catch {
    /* defaults are fine until the backend answers */
  }
  try {
    // A cold start can beat the first poll; ask again until it has landed
    // (the snapshot event usually arrives first).
    while (!state.snapshot) {
      applySnapshot(await invoke("get_snapshot"));
      if (!state.snapshot) await new Promise((r) => setTimeout(r, 500));
    }
    revealSelected();
  } catch (err) {
    applySnapshot({ sessions: [], generatedAt: Date.now(), tmuxRunning: false, error: `Couldn't load sessions: ${errorText(err)}` });
  }
})();

listen("snapshot", (e) => applySnapshot(e.payload));
