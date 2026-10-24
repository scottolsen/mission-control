# Backend ⇄ Frontend contract

The Rust backend (Tauri 2, `src-tauri/`) and the no-bundler web UI (`src/`) talk only through the
commands and the event below. Both sides must match this file exactly. JSON keys are camelCase
(`#[serde(rename_all = "camelCase")]`). Timestamps are Unix **milliseconds**.

## Data sources (backend only)

| Source | What it gives |
|---|---|
| `~/.claude/sessions/<pid>.json` | Every live Claude Code process: `pid`, `sessionId`, `cwd`, `tmux` (`"<session>:@<win>.%<pane>"`, absent outside tmux), `status` (`busy` / `idle` / `waiting`, treat anything else as unknown), `statusUpdatedAt`, `startedAt`, `version`, `name`, `kind`, `bridgeSessionId` (present while the session is connected to Remote Control, `/rc`). Skip files whose `pid` is dead (`kill(pid, 0)` fails with ESRCH), or whose process name (`proc_name`) is readable and isn't Claude's (`^\d+\.\d+\.\d+$` or `claude`; a reused pid). Ignore `*.key` files. |
| `tmux list-panes -a -F …` (tab separated) | `session_name`, `window_id`, `window_index`, `window_name`, `pane_id`, `pane_index`, `pane_pid`, `pane_current_command`, `pane_width`, `pane_height`, `session_attached`, `pane_current_path` (cwd for a pane with no registry entry), `pane_title` (last, so a tab in a title can't shift columns). A Claude pane's `pane_current_command` is its version (`^\d+\.\d+\.\d+$`). Join to the registry by `pane_id` (from the `tmux` field), falling back to `pane_pid == pid`, **only onto panes whose command is Claude's**: a suspended Claude (Ctrl+Z) or one out in `$EDITOR` leaves its entry pointing at a zsh/vim pane, which is then not joined (the session shows with `tmux: null`, not jumpable or replyable). When several entries claim one pane, the newest `startedAt` gets it; the others show without a pane. A Claude pane with no registry entry still appears (state from pane capture alone). Only Claude panes are captured. |
| `tmux capture-pane -p -J -t %N` | Detects an open menu and parses its options (see Detection). |
| `~/.claude/projects/*/<sessionId>.jsonl` | Transcript. Read only the **tail** (last ~512 KB, seek; drop the first line only when the window starts mid-line). When the newest line alone is longer than the window, double it (up to 8 MB); past that, keep the previous extract. Cache the resolved path per sessionId. |
| `git -C <cwd> branch --show-current` | Branch, cached per cwd for 30 s, looked up on a background thread (never under the poll lock); a new cwd's branch appears a poll later. |

Every child process (tmux, git, osascript) runs with a 3 s deadline and is killed past it; a tmux timeout surfaces as `Snapshot.error`. tmux runs with `LC_CTYPE=UTF-8` when none of `LC_ALL`, `LC_CTYPE`, `LANG` is set (a Dock-launched app has no locale, and tmux in the C locale prints the format's tabs and `❯` as `_`). A `list-panes` that succeeds with output but parses to no rows is a `Snapshot.error`, not "no panes".

### Transcript rules
- Only `type` in `user`, `assistant`, `last-prompt`, `ai-title` matter. Everything else is skipped. `last-prompt.lastPrompt` only fills `lastPrompt` (with `lastPromptAt: null`) when no human prompt is in the tail.
- A **human prompt** is a `user` entry whose `message.content` is a string, or an array containing a `text` block and no `tool_result` block. Skip strings that start with `<` (command/system wrappers such as `<command-name>`, `<local-command-stdout>`, `<system-reminder>`) — except `<pasted_content …>`, which is the human's paste (tags stripped for display) — and entries with `isMeta`, `isCompactSummary` or `isVisibleInTranscriptOnly` true (the "This session is being continued…" compaction summary).
- A **reply** is an `assistant` entry with a `text` block; join its text blocks.
- **Current tool** = the latest `assistant` `tool_use` block whose `id` has no later `tool_result` (`tool_use_id`). Summary: `Bash` → `input.command`, `Read`/`Edit`/`Write` → `input.file_path` basename, `Grep`/`Glob` → `input.pattern`, `Agent`/`Task` → `input.description`, `WebFetch` → `input.url`, else empty.
- **Truncated tail**: when the tail started mid-file, holds a reply and no human prompt, the prompt is older than the whole tail, so the reply counts as newer than it (`your_turn` when idle). Long tool-heavy turns push the prompt past 512 KB routinely.
- **Scheduled turns**: a `user` entry with `turnOrigin: "scheduled"`, a `scheduledTaskId` or a `wakeupSource` was started by a `/loop` interval or wakeup, not the human. It is never a human prompt (even as plain text) and closes tool_results as usual.
- **Loop**: walking the tail in order, a recurring `CronCreate` tool_use (`recurring` not `false`) or a `ScheduleWakeup` without `stop: true` starts a loop (`lastFiredAt: null`); a scheduled turn (not `isMeta`: the expanded skill body is a meta companion) sets `lastFiredAt` to its timestamp; a `CronDelete` or `ScheduleWakeup {stop: true}` clears it. `label` is the CronCreate `prompt` plus its interval (`*/5 * * * *` → `every 5m`, `0 * * * *` → `hourly`, `0 */2 * * *` → `every 2h`, else `cron <expr>`); when the CronCreate has scrolled out of the tail, the scheduled turn's own prompt (`<command-name>` + `<command-args>`, or its text), ≤ 80 chars. A one-shot reminder's fire looks the same as a loop's and counts as one.
- `lastActivityAt` = newest `timestamp` among user/assistant entries, else `statusUpdatedAt`.
- Fixture: `src-tauri/tests/fixtures/transcript.jsonl`.

### Title
Pane title with its leading glyph (`✳`, braille spinner, etc.) and whitespace stripped. If that leaves `""` or `"Claude Code"`, use the latest `ai-title.aiTitle` found in the tail; else the registry `name`; else the project name.

### Detection → `state`
Evaluated in this order:
1. `needs_input`: registry `status == "waiting"` **or** the pane capture contains a live menu (if `waiting` but no menu parses, `prompt` is `{kind: "other", question: null, options: []}`): a numbered run anchored on a line matching `^\s*❯\s*\d+\.\s` — note AskUserQuestion menus start at column 0 — or the trust dialog `❯ No, exit` / `Yes, I trust this folder`, **followed by a key-hint footer** (`Esc to cancel`, `Enter to select` or `Enter to confirm`) and **no input-box line** (`^\s*❯(\s|$)`) below it. A prompt the user submitted or is drafting that is itself a numbered list (`❯ 1. fix the build` / `  2. add tests`) has the same shape, but it sits above (or inside) the input box and has no footer, so it is not a menu. Do **not** gate this on `busy`/`idle`.
2. `working`: registry `status == "busy"`.
3. `your_turn`: registry `status == "idle"` and the transcript has at least one human prompt and the latest reply is newer than the latest human prompt.
4. `idle`: everything else that's known (fresh session, no exchange yet, or idle with the human prompt last, e.g. interrupted).
5. `unknown`: no registry entry and no menu in the pane (a bare Claude pane we can see but can't read).

Fixtures (`src-tauri/tests/fixtures/*.ansi`, raw `capture-pane -e`): `permission-bash.ansi` (numbered, indented `❯ 1. Yes`), `ask-question.ansi` (`❯ 1. Red` at column 0), `trust-menu.ansi` (unnumbered), `idle.ansi` and `busy.ansi` (must **not** detect a menu; note the idle input line `❯ Try "…"`), `echo-list-busy.ansi`, `echo-list-reply.ansi` and `draft-list.ansi` (a numbered-list prompt echoed while busy, echoed after a short reply, and still in the input box: must **not** detect a menu). Strip ANSI before matching.

### Prompt parsing (`needsInput` only)
- A menu is anchored on the bottom-most `❯ N.` line and extends through consecutively numbered options up and down (blank lines, `─`/`╌` rules and deeper-indented description lines may sit between). It must start at 1, have ≥ 2 options, and be followed by ≤ 8 non-blank lines including the footer and no input-box line; otherwise it's scrollback or an echoed prompt, not a live menu. The trust dialog likewise needs its two lines adjacent and near the bottom, with the footer and no input box below.
- `options`: every numbered line `N. label` in the menu block (the `❯` cursor may precede one). Labels are trimmed and capped at 120 chars (119 + `…`). Indented description lines under an option are ignored.
- `question`: the nearest non-empty line above the first option that isn't a box-drawing rule (`─`, `╌`): e.g. `Do you want to proceed?` or `Which color do you prefer?`.
- `kind`: `permission` if the block contains `Do you want to proceed?` (or similar `Do you want to …?`), `question` if a `☐`/`☒` header is present, `trust` for the trust dialog, else `other`.
- The trust dialog has no numbers: its options are `{key: "Up…"}`. Don't build options for it; `options` = `[]` and the UI shows only Jump.

## Types

```ts
type State = "needs_input" | "your_turn" | "working" | "idle" | "unknown";

interface Session {
  id: string;              // sessionId, or "pane:%N" when there's no registry entry ("pid:N" for a registry entry with neither sessionId nor pane)
  pid: number | null;
  sessionId: string | null;
  cwd: string;
  project: string;         // basename of cwd
  branch: string | null;
  tmux: {
    session: string;       // tmux session name (display only — never target by name)
    windowId: string;      // "@12"
    windowIndex: number;
    windowName: string;
    paneId: string;        // "%40"
    paneIndex: number;
    width: number;
    height: number;
  } | null;
  title: string;
  state: State;
  prompt: {
    kind: "permission" | "question" | "trust" | "other";
    question: string | null;
    options: { key: string; label: string }[];   // key is the digit to send, "1", "2", …
  } | null;                // only when state == "needs_input"
  lastPrompt: string | null;   // human prompt text, collapsed whitespace, ≤ 400 chars
  lastPromptAt: number | null;
  lastReply: string | null;    // assistant text, ≤ 600 chars
  lastReplyAt: number | null;
  currentTool: { name: string; summary: string } | null;   // only meaningful when working
  loop: {                  // a /loop (recurring scheduled prompt) still running, from the transcript
    label: string;         // "/kanban-worker · every 5m"
    lastFiredAt: number | null;   // null until the first scheduled run
  } | null;
  remoteSessionId: string | null;   // registry bridgeSessionId: connected to Remote Control
  lastActivityAt: number;
  statusUpdatedAt: number | null;
  startedAt: number | null;
  version: string | null;
  jumpable: boolean;       // tmux != null
}

interface Snapshot {
  sessions: Session[];     // unsorted; the UI sorts/groups
  generatedAt: number;     // 0 = the monitor hasn't polled yet: the UI shows loading, not "tmux isn't running"
  tmuxRunning: boolean;
  error: string | null;    // human-readable; the UI shows it in a banner
}

interface Settings {
  notifyYourTurn: boolean;     // default true  — working → your_turn
  notifyNeedsInput: boolean;   // default false — your Notification hook already alerts on permission prompts
  hotkey: string;              // default "Ctrl+Alt+M" (tauri global-shortcut syntax)
  staleHours: number;          // default 8, whole hours 1–720 (u32 in Rust) — your_turn/idle older than this render as "stale"
  terminalApp: string;         // default "iTerm" — app activated after a jump
}

interface PaneCapture { ansi: string; width: number; height: number; }

interface PromptFingerprint {  // what the UI showed when the user chose
  question: string | null;     // Session.prompt.question
  options: string[];           // Session.prompt.options[].label, in order
}
```

## Commands (`window.__TAURI__.core.invoke`)

| Command | Args | Returns | Notes |
|---|---|---|---|
| `get_snapshot` | — | `Snapshot` | Latest snapshot from the monitor thread (polls even with the window hidden). Before the first poll: empty with `generatedAt: 0`. |
| `refresh` | — | `Snapshot` | Polls immediately. |
| `capture_pane` | `{ paneId }` | `PaneCapture` | `capture-pane -e -p -J -S -200 -t <paneId>`; width/height from the pane. Rejects with `"window_hidden"` (no tmux call) while the window is hidden; the UI treats that as paused. |
| `jump` | `{ paneId }` | `null` | Pick the tmux client with the newest `client_activity` (`list-clients -F '#{client_tty}\t#{client_activity}'`), then `switch-client -c <tty> -t <paneId>`, `select-window -t <windowId>`, `select-pane -t <paneId>`, then `osascript -e 'tell application "<terminalApp>" to activate'`. Error string if no client is attached. |
| `send_reply` | `{ paneId, text, expectedState, expectedSessionId }` | `null` | Pre-send check first (below); rejects with `"state_changed"` unless the pane still holds session `expectedSessionId` in `expectedState` (`your_turn` and `idle` match each other: the check reads no transcript). Only allowed for `your_turn`, `idle`, `working` (Claude Code queues it). Text is cleaned: line breaks (CR/LF, U+2028/2029/0085) and tabs become spaces, other control characters (C0, DEL, C1) are dropped, then trimmed; empty after that is rejected. Then `send-keys -t <paneId> -l -- <text>` (`--` so text starting with `-` isn't a flag) and a separate `send-keys -t <paneId> Enter`. Trailing `;` characters are cut from the literal text (tmux reads a trailing `;` as a command separator) and sent as `send-keys -t <paneId> -H 3b …` before the Enter. A reply has no prompt fingerprint. |
| `send_choice` | `{ paneId, key, expectedState, expectedSessionId, expectedPrompt }` | `null` | `key` is a digit (`^[1-9]$`) or `"Escape"`; `expectedPrompt` is a `PromptFingerprint`. Pre-send check first; rejects with `"state_changed"` unless the pane holds session `expectedSessionId`, is `needs_input`, and its freshly parsed prompt has exactly that question and option labels (non-empty). Sends just that key — never Enter. |

**Pre-send check** (both sends): reads the registry, that pane's row (`list-panes -t <paneId> -F …`) and its capture, nothing else (no other panes, no transcripts, no git), to keep the gap between check and send short. A pane whose command isn't Claude's is refused with an error saying so (not `"state_changed"`); a pane that no longer exists is `"state_changed"`.
| `get_settings` | — | `Settings` | Stored as JSON in the app config dir. |
| `set_settings` | `{ settings }` | `Settings` | Re-registers the hotkey every time (even unchanged: it may have failed at startup), then persists. If registering fails, the previous hotkey is registered again and the error returned; if saving fails, the previous hotkey is restored too. |
| `hide_window` | — | `null` | |

All commands return `Result<T, String>`. Every tmux call targets ids (`%N`, `@N`), never names. Session names include `-claude` (leading dash) and `Checkout redesign — phase 2` (spaces, em dash).

## Event

`snapshot` (`window.__TAURI__.event.listen("snapshot", e => e.payload)`): payload `Snapshot`, emitted after every poll (every 2 s) when the content changed (ignore `generatedAt` when comparing).

`visibility`: payload `boolean`, emitted whenever the backend shows (`true`) or hides (`false`) the main window (hotkey, tray, close-to-hide, `hide_window`, Dock reopen). Tauri has no window shown/hidden event and the web view may keep reporting `visible`, so the UI pauses its pane preview on this.

## App shell behavior (backend)

- **Monitor thread** polls every 2 s, caches the snapshot, emits the event, updates the tray, fires notifications on transitions (by `id`): `working → your_turn` (if `notifyYourTurn`) and `* → needs_input` (if `notifyNeedsInput`). Notifications shell out to `/opt/homebrew/bin/terminal-notifier` (fall back to `terminal-notifier` on PATH, skip silently if missing): `-title "Mission Control" -subtitle "<project> · <tmux session>" -message "<title>: Your turn" -group mission-control-<id> -activate com.scottolsen.mission-control` (message `"<title>: Needs input"` for the needs_input notification; subtitle is just `<project>` when not in tmux). No notifications on the first poll. A poll that carries an `error` (tmux timed out, a caught panic) neither notifies nor updates the transition baseline, and an empty first poll isn't a baseline. Values starting with `-` get a zero-width space in front (terminal-notifier would read them as flags).
- **Tray** (menu bar): icon plus `set_title` showing the count of `needs_input` + `your_turn` (empty when 0). Left click toggles the window. Menu: "Open Mission Control", separator (only when sessions follow), the attention sessions (up to 8, needs_input first then your_turn, newest first; `project — title` capped at 70 chars; click = jump; non-jumpable ones disabled), separator, "Quit". Right click opens the menu.
- **Window close** hides instead of quitting (`CloseRequested` → `prevent_close` + `hide`, then `visibility: false`). Quit only from the tray menu or ⌘Q.
- **Global hotkey** (`Settings.hotkey`) shows and focuses the window, or hides it if it's already focused.
- `shell_path::inherit_login_path()` runs first in `run()` (a Dock-launched app has no Homebrew on PATH).
