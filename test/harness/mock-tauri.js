/**
 * Mock Tauri bridge for visual QA of the UI in a plain browser.
 *
 * Serve the repo root and open the harness:
 *
 *   python3 -m http.server 8765        # from the repo root
 *   open http://localhost:8765/test/harness/
 *
 * It defines window.__TAURI__ (core.invoke for every command in
 * docs/CONTRACT.md, event.listen) backed by test/fixtures/snapshot.json, with
 * every timestamp shifted so the fixture reads as "just now". Pane captures
 * are the real backend fixtures in src-tauri/tests/fixtures/*.ansi. Every 2 s
 * it mutates the working sessions and emits "snapshot", the way the monitor
 * thread does.
 *
 * Query params:
 *   ?state=empty    no sessions, tmux running
 *   ?state=notmux   no sessions, tmux not running
 *   ?state=error    fixture plus a Snapshot.error banner
 *   ?state=loading  get_snapshot never resolves
 *   ?reply=state_changed   send_reply / send_choice reject with "state_changed"
 *   ?hidden=1       capture_pane answers "window_hidden" (window in the tray)
 *   ?jump=fail      jump rejects as if no tmux client were attached
 *
 * Guarded so `node --test` (which globs test/**) can load it harmlessly.
 */
if (typeof window !== "undefined") {
  (() => {
    const params = new URLSearchParams(location.search);
    const scenario = params.get("state");
    const listeners = new Map();
    const delay = (ms) => new Promise((r) => setTimeout(r, ms));
    const clone = (v) => JSON.parse(JSON.stringify(v));

    let settings = { notifyYourTurn: true, notifyNeedsInput: false, hotkey: "Ctrl+Alt+M", staleHours: 8, terminalApp: "iTerm" };
    let snapshot = null;
    const ansiCache = new Map();

    const fixture = fetch(new URL("../fixtures/snapshot.json", document.baseURI)).then((r) => r.json()).then((snap) => {
      // Shift every *At timestamp by the same amount so relative ages hold.
      const shift = Date.now() - snap.generatedAt;
      const bump = (o) => {
        for (const k of Object.keys(o)) if (/At$/.test(k) && typeof o[k] === "number") o[k] += shift;
      };
      bump(snap);
      snap.sessions.forEach(bump);
      if (scenario === "empty") snap.sessions = [];
      if (scenario === "notmux") { snap.sessions = []; snap.tmuxRunning = false; }
      if (scenario === "error") snap.error = "tmux list-panes failed: server exited unexpectedly. Showing the last good snapshot.";
      snapshot = snap;
      return snap;
    });

    function emit(event, payload) {
      for (const cb of listeners.get(event) || []) cb({ event, payload: clone(payload) });
    }

    const find = (paneId) => snapshot.sessions.find((s) => s.tmux?.paneId === paneId);

    function ansiFor(s) {
      if (s.state === "needs_input") return s.prompt?.kind === "question" ? "ask-question" : s.prompt?.kind === "trust" ? "trust-menu" : "permission-bash";
      if (s.state === "working") return "busy";
      return "idle";
    }

    async function loadAnsi(name) {
      if (!ansiCache.has(name)) {
        const url = new URL(`../../src-tauri/tests/fixtures/${name}.ansi`, document.baseURI);
        ansiCache.set(name, fetch(url).then((r) => r.text()));
      }
      return ansiCache.get(name);
    }

    const TOOLS = [
      { name: "Agent", summary: "Audit deprecated config for Rails 8" },
      { name: "Read", summary: "application.rb" },
      { name: "Bash", summary: "bin/rails app:update" },
      { name: "Edit", summary: "new_framework_defaults_8_0.rb" },
    ];
    let tick = 0;

    // The monitor thread: every 2 s, working sessions make progress.
    setInterval(() => {
      if (!snapshot || scenario === "loading") return;
      tick += 1;
      const t = Date.now();
      for (const s of snapshot.sessions) {
        if (s.state !== "working") continue;
        s.lastActivityAt = t - 1000 * (1 + (s.pid % 7));
        if (s.title === "Rails 8 upgrade") s.currentTool = TOOLS[tick % TOOLS.length];
      }
      snapshot.generatedAt = t;
      emit("snapshot", snapshot);
    }, 2000);

    function changed() {
      snapshot.generatedAt = Date.now();
      setTimeout(() => emit("snapshot", snapshot), 500);
    }

    // The backend's pre-send check: same session in the pane, same state
    // (your_turn and idle read alike, as the backend can't tell them apart
    // without the transcript), and for choices the very prompt shown.
    const coarse = (st) => (st === "your_turn" ? "idle" : st);
    function stateGuard(s, expectedState, expectedSessionId) {
      if (params.get("reply") === "state_changed") throw "state_changed";
      if (!s) throw "no such pane";
      if (s.id !== expectedSessionId) throw "state_changed";
      if (coarse(s.state) !== coarse(expectedState)) throw "state_changed";
    }

    function samePrompt(prompt, seen) {
      const labels = (prompt?.options ?? []).map((o) => o.label);
      return Boolean(seen) && labels.length > 0 && (prompt.question ?? null) === seen.question
        && JSON.stringify(labels) === JSON.stringify(seen.options);
    }

    const HOTKEY = /^((Cmd|Command|Ctrl|Control|Alt|Option|Shift|Super|CmdOrCtrl|CommandOrControl)\+)+([A-Z0-9]|F\d{1,2}|Space|Enter|Tab|Escape)$/i;

    const commands = {
      async get_snapshot() {
        if (scenario === "loading") return new Promise(() => {});
        await delay(250);
        return fixture;
      },
      async refresh() {
        await fixture;
        await delay(200);
        snapshot.generatedAt = Date.now();
        return snapshot;
      },
      async capture_pane({ paneId }) {
        await fixture;
        if (params.get("hidden") === "1") throw "window_hidden";
        const s = find(paneId);
        if (!s) throw `can't find pane: ${paneId}`;
        // The fixtures were captured from a 364-column pane; report that size
        // so the box-drawing rules don't wrap.
        return { ansi: await loadAnsi(ansiFor(s)), width: 364, height: 64 };
      },
      async jump({ paneId }) {
        await delay(120);
        if (params.get("jump") === "fail") throw "no tmux client is attached";
        if (!find(paneId)) throw `can't find pane: ${paneId}`;
        return null;
      },
      async send_reply({ paneId, text, expectedState, expectedSessionId }) {
        await delay(260);
        const s = find(paneId);
        stateGuard(s, expectedState, expectedSessionId);
        if (!["your_turn", "idle", "working"].includes(s.state)) throw "state_changed";
        if (!text.trim()) throw "empty reply";
        const t = Date.now();
        if (s.state !== "working") {
          s.lastPrompt = text.replace(/\s+/g, " ").slice(0, 400);
          s.lastPromptAt = t;
          s.state = "working";
          s.currentTool = { name: "Read", summary: "README.md" };
          s.lastActivityAt = t;
        }
        changed();
        return null;
      },
      async send_choice({ paneId, key, expectedState, expectedSessionId, expectedPrompt }) {
        await delay(200);
        const s = find(paneId);
        stateGuard(s, expectedState, expectedSessionId);
        if (!samePrompt(s.prompt, expectedPrompt)) throw "state_changed";
        if (!/^([1-9]|Escape)$/.test(key)) throw `invalid key: ${key}`;
        s.state = key === "Escape" ? "idle" : "working";
        s.prompt = null;
        s.lastActivityAt = Date.now();
        changed();
        return null;
      },
      async get_settings() { return clone(settings); },
      async set_settings({ settings: next }) {
        await delay(150);
        if (!HOTKEY.test(next.hotkey)) throw `Couldn't register the hotkey "${next.hotkey}": unrecognised shortcut`;
        settings = clone(next);
        return clone(settings);
      },
      async hide_window() { return null; },
    };

    window.__TAURI__ = {
      core: {
        async invoke(cmd, args = {}) {
          const fn = commands[cmd];
          if (!fn) throw `unknown command: ${cmd}`;
          const result = await fn(args);
          return clone(result);
        },
      },
      event: {
        async listen(event, cb) {
          if (!listeners.has(event)) listeners.set(event, new Set());
          listeners.get(event).add(cb);
          return () => listeners.get(event).delete(cb);
        },
      },
    };
    // __MOCK__.emit("visibility", false) plays the window hiding to the tray.
    window.__MOCK__ = { emit, get snapshot() { return snapshot; } };
  })();
}
