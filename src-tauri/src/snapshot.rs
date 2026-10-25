//! Builds a `Snapshot`: joins the process registry, tmux's pane list, pane
//! captures, and transcripts into one `Session` per Claude Code process, then
//! decides each one's title and state.
//!
//! `assemble` is pure (all inputs gathered up front) so the join, title, and
//! state rules are table-testable. `Poller` does the I/O around it and owns
//! the caches that keep a 2 s poll cheap.

use crate::detect;
use crate::monitor::lock;
use crate::registry::{self, RegEntry, RegStatus};
use crate::tmux::{self, Pane, Runner};
use crate::transcript::{Extract, TranscriptCache};
use crate::types::{CurrentTool, Prompt, PromptKind, Session, Snapshot, State, TmuxLocation};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BRANCH_TTL: Duration = Duration::from_secs(30);

/// Everything `assemble` needs, already fetched.
#[derive(Default)]
pub struct Inputs {
    pub registry: Vec<RegEntry>,
    /// `None` when no tmux server is running.
    pub panes: Option<Vec<Pane>>,
    /// Plain-text captures keyed by pane id (Claude panes only).
    pub captures: HashMap<String, String>,
    /// Transcript extracts keyed by sessionId.
    pub extracts: HashMap<String, Extract>,
    /// Branch per cwd.
    pub branches: HashMap<String, Option<String>>,
}

/// One registry entry or bare Claude pane, before titles and states.
struct Joined<'a> {
    reg: Option<&'a RegEntry>,
    pane: Option<&'a Pane>,
}

fn join(inputs: &Inputs) -> Vec<Joined<'_>> {
    let panes: &[Pane] = inputs.panes.as_deref().unwrap_or(&[]);
    // Only a pane running Claude can belong to a registry entry. Suspended
    // with Ctrl+Z or out in $EDITOR, the pane runs zsh or vim; a stale file
    // with a reused pid points anywhere. Typing into either would be wrong.
    let claude: Vec<&Pane> = panes
        .iter()
        .filter(|p| tmux::is_claude_command(&p.command))
        .collect();
    let by_id: HashMap<&str, &Pane> = claude.iter().map(|p| (p.pane_id.as_str(), *p)).collect();
    let mut claimed: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();

    // Newest first, so when two entries claim one pane (a suspended Claude
    // and the one started after it) the newer one gets it.
    let mut registry: Vec<&RegEntry> = inputs.registry.iter().collect();
    registry.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    for reg in registry {
        let pane = reg
            .tmux
            .as_ref()
            .and_then(|t| by_id.get(t.pane_id.as_str()).copied())
            .or_else(|| claude.iter().copied().find(|p| p.pane_pid == reg.pid))
            .filter(|p| !claimed.contains(p.pane_id.as_str()));
        if let Some(p) = pane {
            claimed.insert(p.pane_id.as_str());
        }
        out.push(Joined {
            reg: Some(reg),
            pane,
        });
    }
    // Claude panes the registry doesn't know about (older CLI, or a file
    // that hasn't been written yet) still show up.
    for p in claude {
        if !claimed.contains(p.pane_id.as_str()) {
            out.push(Joined {
                reg: None,
                pane: Some(p),
            });
        }
    }
    out
}

/// Pane ids worth capturing: Claude panes only. Shell and editor panes are
/// never captured, even when a registry entry points at one.
pub fn panes_to_capture(panes: &[Pane]) -> Vec<String> {
    panes
        .iter()
        .filter(|p| tmux::is_claude_command(&p.command))
        .map(|p| p.pane_id.clone())
        .collect()
}

pub fn assemble(inputs: &Inputs, now: i64) -> Snapshot {
    let mut sessions: Vec<Session> = join(inputs)
        .into_iter()
        .map(|j| build_session(inputs, j))
        .collect();
    // Stable order so content comparison between polls is meaningful.
    sessions.sort_by(|a, b| a.id.cmp(&b.id));
    Snapshot {
        sessions,
        generated_at: now,
        tmux_running: inputs.panes.is_some(),
        error: None,
        claude_version: None,
    }
}

fn build_session(inputs: &Inputs, j: Joined<'_>) -> Session {
    let reg = j.reg;
    let pane = j.pane;
    let cwd = reg
        .map(|r| r.cwd.clone())
        .or_else(|| pane.map(|p| p.current_path.clone()))
        .unwrap_or_default();
    let project = project_name(&cwd);
    let session_id = reg.and_then(|r| r.session_id.clone());
    let extract = session_id.as_ref().and_then(|sid| inputs.extracts.get(sid));
    let capture = pane.and_then(|p| inputs.captures.get(&p.pane_id));
    let menu = capture.and_then(|c| detect::detect(c));
    let state = classify(reg.map(|r| r.status), menu.is_some(), extract);
    let prompt = (state == State::NeedsInput).then(|| {
        // Registry says waiting but the menu didn't parse (scrolled, or a
        // dialog we don't know): still a prompt, just with no options.
        menu.unwrap_or(Prompt {
            kind: PromptKind::Other,
            question: None,
            options: Vec::new(),
        })
    });

    let id = match (&session_id, pane) {
        (Some(sid), _) => sid.clone(),
        (None, Some(p)) => format!("pane:{}", p.pane_id),
        (None, None) => format!("pid:{}", reg.map_or(0, |r| r.pid)),
    };

    let title = choose_title(
        pane.map(|p| p.title.as_str()),
        extract.and_then(|e| e.ai_title.as_deref()),
        reg.and_then(|r| r.name.as_deref()),
        &project,
    );

    let status_updated_at = reg.and_then(|r| r.status_updated_at);
    let current_tool: Option<CurrentTool> = if state == State::Working {
        extract.and_then(|e| e.current_tool.clone())
    } else {
        None
    };

    Session {
        id,
        pid: reg.map(|r| r.pid),
        session_id,
        branch: inputs.branches.get(&cwd).cloned().flatten(),
        cwd,
        project,
        tmux: pane.map(location),
        jumpable: pane.is_some(),
        title,
        state,
        prompt,
        last_prompt: extract.and_then(|e| e.last_prompt.clone()),
        last_prompt_at: extract.and_then(|e| e.last_prompt_at),
        last_reply: extract.and_then(|e| e.last_reply.clone()),
        last_reply_at: extract.and_then(|e| e.last_reply_at),
        current_tool,
        loop_info: extract.and_then(|e| e.loop_info.clone()),
        remote_session_id: reg.and_then(|r| r.bridge_session_id.clone()),
        last_activity_at: extract
            .and_then(|e| e.last_activity_at)
            .or(status_updated_at)
            .unwrap_or(0),
        status_updated_at,
        started_at: reg.and_then(|r| r.started_at),
        version: reg.and_then(|r| r.version.clone()).or_else(|| {
            pane.map(|p| p.command.clone())
                .filter(|c| tmux::is_claude_command(c) && c != "claude")
        }),
    }
}

fn location(p: &Pane) -> TmuxLocation {
    TmuxLocation {
        session: p.session.clone(),
        window_id: p.window_id.clone(),
        window_index: p.window_index,
        window_name: p.window_name.clone(),
        pane_id: p.pane_id.clone(),
        pane_index: p.pane_index,
        width: p.width,
        height: p.height,
    }
}

pub fn project_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_string())
}

/// The contract's state table, in order.
pub fn classify(status: Option<RegStatus>, menu: bool, extract: Option<&Extract>) -> State {
    if status == Some(RegStatus::Waiting) || menu {
        return State::NeedsInput;
    }
    match status {
        Some(RegStatus::Busy) => State::Working,
        Some(RegStatus::Idle) if extract.is_some_and(reply_after_prompt) => State::YourTurn,
        Some(_) => State::Idle,
        // A bare Claude pane with no registry entry and no menu: we can see
        // it but can't tell what it's doing.
        None => State::Unknown,
    }
}

fn reply_after_prompt(e: &Extract) -> bool {
    if !e.has_prompt {
        return false;
    }
    match (e.last_reply_at, e.last_prompt_at) {
        (Some(reply), Some(prompt)) => reply > prompt,
        (Some(_), None) => true,
        _ => false,
    }
}

// Claude Code prefixes its pane title with a status glyph: `✳` when idle, a
// spinner (`✢ ✶ ✻ ✽ ·` or braille) while working.
fn is_title_glyph(c: char) -> bool {
    matches!(c, '✳' | '✢' | '✶' | '✻' | '✽' | '·' | '*') || ('\u{2800}'..='\u{28FF}').contains(&c)
}

pub fn strip_title(raw: &str) -> String {
    raw.trim_start_matches(|c: char| c.is_whitespace() || is_title_glyph(c))
        .trim()
        .to_string()
}

pub fn choose_title(
    pane_title: Option<&str>,
    ai_title: Option<&str>,
    name: Option<&str>,
    project: &str,
) -> String {
    if let Some(t) = pane_title.map(strip_title) {
        if !t.is_empty() && t != "Claude Code" {
            return t;
        }
    }
    [ai_title, name]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| project.to_string())
}

type BranchLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type BranchEntries = Arc<Mutex<HashMap<String, (Instant, Option<String>)>>>;

/// `git branch --show-current`, cached per cwd for 30 s. Lookups run on
/// their own threads and `get` answers from the cache meanwhile, so git
/// never runs under the poll lock; a new cwd shows its branch a poll later.
pub struct BranchCache {
    entries: BranchEntries,
    pending: Arc<Mutex<HashSet<String>>>,
    lookup: BranchLookup,
}

impl Default for BranchCache {
    fn default() -> Self {
        BranchCache::with_lookup(Arc::new(git_branch))
    }
}

impl BranchCache {
    pub fn with_lookup(lookup: BranchLookup) -> BranchCache {
        BranchCache {
            entries: Arc::default(),
            pending: Arc::default(),
            lookup,
        }
    }

    pub fn get(&mut self, cwd: &str) -> Option<String> {
        let cached = lock(&self.entries).get(cwd).cloned();
        let fresh = cached
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < BRANCH_TTL);
        if !fresh && lock(&self.pending).insert(cwd.to_string()) {
            let (entries, pending) = (self.entries.clone(), self.pending.clone());
            let (lookup, cwd) = (self.lookup.clone(), cwd.to_string());
            std::thread::spawn(move || {
                let branch = lookup(&cwd);
                lock(&entries).insert(cwd.clone(), (Instant::now(), branch));
                lock(&pending).remove(&cwd);
            });
        }
        cached.and_then(|(_, branch)| branch)
    }
}

fn git_branch(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let mut git = Command::new("git");
    git.args(["-C", cwd, "branch", "--show-current"]);
    let out = tmux::output_with_deadline(&mut git, tmux::DEADLINE).ok()?;
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !b.is_empty()).then_some(b)
}

// Each capture is a separate tmux exec (~5–10 ms); a few threads keep a
// 25-session poll well under the 2 s interval.
const CAPTURE_THREADS: usize = 6;

fn capture_all(runner: &dyn Runner, ids: Vec<String>) -> HashMap<String, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let chunk = ids.len().div_ceil(CAPTURE_THREADS);
    std::thread::scope(|scope| {
        let handles: Vec<_> = ids
            .chunks(chunk)
            .map(|group| {
                scope.spawn(move || {
                    group
                        .iter()
                        .filter_map(|id| {
                            let text =
                                tmux::capture(runner, &tmux::detect_capture_args(id)).ok()?;
                            Some((id.clone(), text))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .flatten()
            .collect()
    })
}

/// Gathers inputs and runs `assemble`. Holds the caches between polls.
pub struct Poller {
    pub runner: Arc<dyn Runner>,
    pub sessions_dir: PathBuf,
    pub projects_dir: PathBuf,
    /// Where `installed_version` looks for the Claude Code install.
    pub home: PathBuf,
    /// Which registry pids count as live sessions (tests accept any live pid).
    pub live_pid: fn(i64) -> bool,
    transcripts: TranscriptCache,
    branches: BranchCache,
}

impl Poller {
    pub fn new(runner: Arc<dyn Runner>) -> Poller {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        Poller {
            runner,
            sessions_dir: home.join(".claude/sessions"),
            projects_dir: home.join(".claude/projects"),
            home,
            live_pid: registry::is_live_claude,
            transcripts: TranscriptCache::default(),
            branches: BranchCache::default(),
        }
    }

    /// One full poll. tmux trouble becomes `error`; the registry still yields
    /// (non-jumpable) sessions.
    pub fn poll(&mut self, now: i64) -> Snapshot {
        let registry = registry::load(&self.sessions_dir, self.live_pid);
        let (panes, error) = match tmux::list_panes(self.runner.as_ref()) {
            Ok(p) => (p, None),
            Err(e) => (None, Some(e)),
        };

        let captures = match &panes {
            Some(panes) => capture_all(self.runner.as_ref(), panes_to_capture(panes)),
            None => HashMap::new(),
        };

        let mut extracts = HashMap::new();
        let mut branches = HashMap::new();
        for r in &registry {
            if let Some(sid) = &r.session_id {
                if let Some(ex) = self.transcripts.get(&self.projects_dir, sid, &r.cwd) {
                    extracts.insert(sid.clone(), ex);
                }
            }
            if !branches.contains_key(&r.cwd) {
                branches.insert(r.cwd.clone(), self.branches.get(&r.cwd));
            }
        }
        let live = registry
            .iter()
            .filter_map(|r| r.session_id.clone())
            .collect();
        self.transcripts.retain(&live);

        let inputs = Inputs {
            registry,
            panes,
            captures,
            extracts,
            branches,
        };
        let mut snap = assemble(&inputs, now);
        snap.error = error;
        snap.claude_version = registry::installed_version(&self.home);
        snap
    }

    /// The pre-send check for one pane: the registry, that pane's row and its
    /// screen, nothing else, to keep the gap between check and send short.
    /// Without transcripts `your_turn` reads as `idle`.
    pub fn check_pane(&self, now: i64, pane_id: &str) -> Result<PaneCheck, String> {
        let r = self.runner.as_ref();
        let Some(pane) = tmux::find_pane(r, pane_id)? else {
            return Ok(PaneCheck::Gone);
        };
        if !tmux::is_claude_command(&pane.command) {
            return Ok(PaneCheck::NotClaude);
        }
        let Ok(screen) = tmux::capture(r, &tmux::detect_capture_args(pane_id)) else {
            return Ok(PaneCheck::Gone);
        };
        let inputs = Inputs {
            registry: registry::load(&self.sessions_dir, self.live_pid),
            panes: Some(vec![pane]),
            captures: HashMap::from([(pane_id.to_string(), screen)]),
            ..Default::default()
        };
        Ok(assemble(&inputs, now)
            .sessions
            .into_iter()
            .find(|s| s.tmux.as_ref().is_some_and(|t| t.pane_id == pane_id))
            .map_or(PaneCheck::Gone, |s| PaneCheck::Session(Box::new(s))))
    }
}

/// What `check_pane` found.
pub enum PaneCheck {
    /// The pane, or the tmux server, no longer exists.
    Gone,
    /// The pane exists but runs something else (a shell, an editor).
    NotClaude,
    Session(Box<Session>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::TmuxRef;
    use crate::tmux::tests::{FakeRunner, PANES};

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn reg(pid: i64, sid: &str, pane: Option<&str>, status: RegStatus) -> RegEntry {
        RegEntry {
            pid,
            session_id: Some(sid.into()),
            cwd: format!("/Users/me/{sid}-proj"),
            tmux: pane.map(|p| TmuxRef {
                session: "x".into(),
                window_id: "@5".into(),
                pane_id: p.into(),
            }),
            status,
            status_updated_at: Some(1000),
            started_at: Some(500),
            version: Some("2.1.296".into()),
            name: Some(format!("{sid}-name")),
            bridge_session_id: None,
        }
    }

    fn ex(prompt_at: Option<i64>, reply_at: Option<i64>) -> Extract {
        Extract {
            has_prompt: prompt_at.is_some(),
            last_prompt: prompt_at.map(|_| "p".into()),
            last_prompt_at: prompt_at,
            last_reply_at: reply_at,
            last_reply: reply_at.map(|_| "r".into()),
            ..Default::default()
        }
    }

    #[test]
    fn classification_table() {
        use RegStatus::*;
        let answered = ex(Some(1), Some(2));
        let interrupted = ex(Some(3), Some(2));
        let fresh = Extract::default();
        let cases: Vec<(Option<RegStatus>, bool, Option<&Extract>, State)> = vec![
            (Some(Waiting), false, None, State::NeedsInput),
            (Some(Busy), true, None, State::NeedsInput), // never gated on busy
            (Some(Idle), true, Some(&answered), State::NeedsInput),
            (None, true, None, State::NeedsInput),
            (Some(Busy), false, Some(&answered), State::Working),
            (Some(Idle), false, Some(&answered), State::YourTurn),
            (Some(Idle), false, Some(&interrupted), State::Idle),
            (Some(Idle), false, Some(&fresh), State::Idle),
            (Some(Idle), false, None, State::Idle),
            (Some(Unknown), false, Some(&answered), State::Idle),
            (None, false, None, State::Unknown),
        ];
        for (status, menu, e, want) in cases {
            assert_eq!(classify(status, menu, e), want, "{status:?} menu={menu}");
        }
    }

    /// git on a hung mount must not hold up a poll: the cache answers from
    /// what it has and looks the branch up in the background.
    #[test]
    fn branch_lookup_never_blocks_the_poll() {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        let mut cache = BranchCache::with_lookup(Arc::new(move |_cwd: &str| {
            let _ = gate.lock().unwrap().recv();
            Some("main".to_string())
        }));
        let t = Instant::now();
        assert_eq!(cache.get("/repo"), None, "first ask: not known yet");
        assert_eq!(cache.get("/repo"), None, "still in flight, not asked twice");
        assert!(t.elapsed() < Duration::from_millis(500));
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while cache.get("/repo").is_none() {
            assert!(Instant::now() < deadline, "lookup never landed");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(cache.get("/repo").as_deref(), Some("main"));
    }

    #[test]
    fn title_stripping() {
        assert_eq!(
            strip_title("✳ Checkout Redesign Phase 2"),
            "Checkout Redesign Phase 2"
        );
        assert_eq!(strip_title("⠂ Fix the build"), "Fix the build");
        assert_eq!(strip_title("⣾  Spinner"), "Spinner");
        assert_eq!(strip_title("✽ Working"), "Working");
        assert_eq!(
            choose_title(Some("✳ Claude Code"), Some("AI title"), Some("n"), "proj"),
            "AI title"
        );
        assert_eq!(
            choose_title(Some("✳ "), None, Some("reg-name"), "proj"),
            "reg-name"
        );
        assert_eq!(
            choose_title(Some("Claude Code"), None, None, "proj"),
            "proj"
        );
        assert_eq!(choose_title(None, Some("  "), None, "proj"), "proj");
        assert_eq!(
            choose_title(Some("✳ Real"), Some("AI"), None, "proj"),
            "Real"
        );
    }

    #[test]
    fn joins_registry_and_panes() {
        let panes = tmux::parse_panes(PANES);
        let mut inputs = Inputs {
            registry: vec![
                reg(73721, "a", Some("%13"), RegStatus::Idle),
                // Pane no longer exists: kept, but not jumpable.
                reg(2, "b", Some("%999"), RegStatus::Busy),
                // No tmux field: matched by pane_pid.
                reg(45013, "c", None, RegStatus::Waiting),
            ],
            panes: Some(panes),
            ..Default::default()
        };
        inputs
            .captures
            .insert("%52".into(), fixture("permission-bash.ansi"));
        inputs.extracts.insert("a".into(), ex(Some(1), Some(2)));
        inputs
            .branches
            .insert("/Users/me/a-proj".into(), Some("main".into()));
        let snap = assemble(&inputs, 42);
        assert!(snap.tmux_running);
        let ids: Vec<&str> = snap.sessions.iter().map(|s| s.id.as_str()).collect();
        // %90 is a Claude pane with no registry entry.
        assert_eq!(ids, vec!["a", "b", "c", "pane:%90"]);

        let a = &snap.sessions[0];
        assert_eq!(a.state, State::YourTurn);
        assert_eq!(a.tmux.as_ref().unwrap().session, "-claude");
        assert_eq!(a.title, "a-name"); // "✳ Claude Code" falls through
        assert_eq!(a.project, "a-proj");
        assert_eq!(a.branch.as_deref(), Some("main"));
        assert_eq!(a.last_activity_at, 1000); // no transcript timestamps
        assert!(a.jumpable && a.prompt.is_none());

        let b = &snap.sessions[1];
        assert!(!b.jumpable && b.tmux.is_none());
        assert_eq!(b.state, State::Working);

        let c = &snap.sessions[2];
        assert_eq!(
            c.tmux.as_ref().unwrap().session,
            "Checkout redesign — phase 2"
        );
        assert_eq!(c.title, "Checkout Redesign Phase 2");
        let p = c.prompt.as_ref().unwrap();
        assert_eq!(p.kind, PromptKind::Permission);
        assert_eq!(p.options.len(), 4);

        let bare = &snap.sessions[3];
        assert_eq!(bare.state, State::Unknown);
        assert_eq!(bare.pid, None);
        assert_eq!(bare.version.as_deref(), Some("2.1.300"));
        assert_eq!(bare.project, "bare");
    }

    /// Claude suspended with Ctrl+Z (or out in $EDITOR) leaves its registry
    /// entry pointing at a pane that now runs zsh. Typing into it would run
    /// shell commands, so the entry must not own that pane.
    #[test]
    fn registry_entry_on_a_shell_pane_is_not_joined() {
        let panes = tmux::parse_panes(PANES);
        let mut inputs = Inputs {
            registry: vec![
                reg(21758, "z", Some("%7"), RegStatus::Idle),
                // pane_pid fallback onto the same zsh pane.
                reg(21758, "y", None, RegStatus::Idle),
            ],
            panes: Some(panes.clone()),
            ..Default::default()
        };
        inputs
            .captures
            .insert("%7".into(), fixture("permission-bash.ansi"));
        inputs.extracts.insert("z".into(), ex(Some(1), Some(2)));
        let snap = assemble(&inputs, 0);
        for id in ["y", "z"] {
            let s = snap.sessions.iter().find(|s| s.id == id).unwrap();
            assert!(s.tmux.is_none() && !s.jumpable, "{id}");
            assert!(
                s.prompt.is_none(),
                "{id}: the shell pane's screen is ignored"
            );
        }
        // And the shell pane isn't captured on its behalf.
        assert!(!panes_to_capture(&panes).contains(&"%7".to_string()));
    }

    #[test]
    fn newest_claim_on_a_pane_wins() {
        let panes = tmux::parse_panes(PANES);
        let mut old = reg(73721, "old", Some("%13"), RegStatus::Waiting);
        old.started_at = Some(100);
        let mut new = reg(73722, "new", Some("%13"), RegStatus::Idle);
        new.started_at = Some(900);
        let inputs = Inputs {
            registry: vec![old, new],
            panes: Some(panes),
            ..Default::default()
        };
        let snap = assemble(&inputs, 0);
        let get = |id: &str| snap.sessions.iter().find(|s| s.id == id).unwrap();
        assert_eq!(get("new").tmux.as_ref().unwrap().pane_id, "%13");
        assert!(get("old").tmux.is_none() && !get("old").jumpable);
    }

    #[test]
    fn waiting_without_parsed_menu_still_has_a_prompt() {
        let inputs = Inputs {
            registry: vec![reg(1, "w", None, RegStatus::Waiting)],
            ..Default::default()
        };
        let snap = assemble(&inputs, 0);
        assert!(!snap.tmux_running);
        let s = &snap.sessions[0];
        assert_eq!(s.state, State::NeedsInput);
        assert_eq!(s.prompt.as_ref().unwrap().kind, PromptKind::Other);
        assert!(!s.jumpable);
    }

    #[test]
    fn only_claude_panes_are_captured() {
        let panes = tmux::parse_panes(PANES);
        let ids = panes_to_capture(&panes);
        // By command only: %12 (vim) and %7 (zsh) are skipped.
        assert_eq!(ids, vec!["%13", "%52", "%90"]);
    }

    #[test]
    fn poller_without_tmux_uses_registry_only() {
        let dir = std::env::temp_dir().join(format!("mc-poll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.join(format!("{me}.json")),
            format!(
                r#"{{"pid":{me},"sessionId":"s","cwd":"/tmp","status":"idle","tmux":"t:@1.%1"}}"#
            ),
        )
        .unwrap();
        let runner = Arc::new(FakeRunner {
            replies: vec![(
                "list-panes",
                false,
                String::new(),
                "no server running on /tmp/x".into(),
            )],
            ..Default::default()
        });
        let mut poller = Poller::new(runner.clone());
        poller.sessions_dir = dir.clone();
        poller.projects_dir = dir.join("none");
        poller.live_pid = registry::pid_alive;
        let snap = poller.poll(7);
        assert!(!snap.tmux_running);
        assert_eq!(snap.error, None);
        assert_eq!(snap.sessions.len(), 1);
        assert!(!snap.sessions[0].jumpable);
        // One list-panes call, nothing captured.
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Prints the real snapshot. Read-only: registry files, list-panes,
    /// capture-pane, transcripts, git. Run with `cargo test live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_snapshot_dump() {
        let mut poller = Poller::new(Arc::new(tmux::SystemRunner));
        let t = Instant::now();
        let snap = poller.poll(0);
        let first = t.elapsed();
        let t = Instant::now();
        let _ = poller.poll(0);
        eprintln!("poll: first {first:?}, cached {:?}", t.elapsed());
        println!("{}", serde_json::to_string_pretty(&snap).unwrap());
        for s in &snap.sessions {
            eprintln!(
                "{:<28} {:<34} {:<12} {}",
                s.project,
                s.tmux
                    .as_ref()
                    .map_or("-".into(), |t| format!("{} {}", t.session, t.pane_id)),
                format!("{:?}", s.state),
                s.title
            );
        }
        eprintln!(
            "{} sessions, tmuxRunning={}, error={:?}",
            snap.sessions.len(),
            snap.tmux_running,
            snap.error
        );
    }
}
