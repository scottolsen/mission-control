//! Tauri commands, exactly as listed in `docs/CONTRACT.md`. Each returns
//! `Result<T, String>`. The logic lives in plain functions that take a
//! `Runner`/`Poller`, so the send and jump sequences are unit-tested against
//! a fake tmux; the `#[tauri::command]` wrappers only fetch app state.
//!
//! Anything that types into a pane re-detects that pane's state first: the
//! UI's view can be up to 2 s old, and a digit sent to a menu that has
//! already closed would land in the prompt as text.

use crate::monitor::{self, lock, Shared};
use crate::settings;
use crate::snapshot::{PaneCheck, Poller};
use crate::tmux::{self, Runner};
use crate::types::{PaneCapture, PromptFingerprint, Session, Settings, Snapshot, State};
use std::process::Command;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State as TauriState};

pub const STATE_CHANGED: &str = "state_changed";
/// `capture_pane` while the window is hidden: the UI pauses its preview.
pub const WINDOW_HIDDEN: &str = "window_hidden";

type AppShared<'a> = TauriState<'a, Arc<Shared>>;

fn check_pane_id(pane_id: &str) -> Result<(), String> {
    if tmux::valid_pane_id(pane_id) {
        Ok(())
    } else {
        Err(format!("Invalid pane id: {pane_id}"))
    }
}

pub const NOT_CLAUDE: &str =
    "This pane isn't running Claude Code any more. Jump to it to see what's there.";

/// What actually gets typed: line breaks and tabs become spaces (a raw
/// newline would submit mid-message, a tab would autocomplete), and other
/// control characters are dropped (ESC would cancel the input).
pub fn sanitize_reply(text: &str) -> String {
    text.replace("\r\n", " ")
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' | '\u{2028}' | '\u{2029}' | '\u{85}' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// States a reply may be typed into. Claude Code queues text sent while it
/// is working.
pub fn validate_reply(text: &str, expected: &str) -> Result<State, String> {
    if sanitize_reply(text).is_empty() {
        return Err("Reply is empty".into());
    }
    match State::parse(expected) {
        Some(s @ (State::YourTurn | State::Idle | State::Working)) => Ok(s),
        _ => Err(format!(
            "Can't send a reply to a session that is {expected}"
        )),
    }
}

pub fn validate_choice(key: &str, expected: &str) -> Result<(), String> {
    let digit = key.len() == 1 && matches!(key.as_bytes()[0], b'1'..=b'9');
    if !digit && key != "Escape" {
        return Err(format!("Invalid choice key: {key}"));
    }
    if State::parse(expected) != Some(State::NeedsInput) {
        return Err(format!("Choices only answer needs_input, not {expected}"));
    }
    Ok(())
}

/// The tmux commands for one reply: the text literally, then Enter as its
/// own key. tmux parses a trailing `;` as a command separator and drops it,
/// so trailing semicolons go separately as hex keys.
pub fn reply_commands(pane_id: &str, text: &str) -> Vec<Vec<String>> {
    let flat = sanitize_reply(text);
    let body = flat.trim_end_matches(';');
    let mut cmds = Vec::new();
    if !body.is_empty() {
        cmds.push(tmux::send_literal_args(pane_id, body));
    }
    let semis = flat.len() - body.len();
    if semis > 0 {
        cmds.push(tmux::send_hex_args(pane_id, &vec!["3b"; semis]));
    }
    cmds.push(tmux::send_key_args(pane_id, "Enter"));
    cmds
}

// The pre-send check misses `your_turn` (no transcript read), and both mean
// "registry idle, no menu" anyway.
fn coarse(s: State) -> State {
    if s == State::YourTurn {
        State::Idle
    } else {
        s
    }
}

/// The pane's session right now, if it is still the one the UI showed.
fn live_session(poller: &Poller, pane_id: &str, session_id: &str) -> Result<Session, String> {
    match poller.check_pane(monitor::now_ms(), pane_id)? {
        PaneCheck::NotClaude => Err(NOT_CLAUDE.into()),
        PaneCheck::Session(s) if s.id == session_id => Ok(*s),
        _ => Err(STATE_CHANGED.into()),
    }
}

/// Re-detect, then type `text` literally and press Enter as a separate key.
pub fn reply(
    poller: &Poller,
    pane_id: &str,
    text: &str,
    expected: &str,
    session_id: &str,
) -> Result<(), String> {
    check_pane_id(pane_id)?;
    let want = validate_reply(text, expected)?;
    let now = live_session(poller, pane_id, session_id)?;
    if coarse(now.state) != coarse(want) {
        return Err(STATE_CHANGED.into());
    }
    let r = poller.runner.as_ref();
    reply_commands(pane_id, text)
        .iter()
        .try_for_each(|args| tmux::run_ok(r, args))
}

/// Re-detect, then send exactly one key, only while the pane shows the very
/// menu the user answered. Never Enter: Claude Code menus act on the digit.
pub fn choose(
    poller: &Poller,
    pane_id: &str,
    key: &str,
    expected: &str,
    session_id: &str,
    seen: &PromptFingerprint,
) -> Result<(), String> {
    check_pane_id(pane_id)?;
    validate_choice(key, expected)?;
    let now = live_session(poller, pane_id, session_id)?;
    let showing = now
        .prompt
        .as_ref()
        .filter(|_| now.state == State::NeedsInput)
        .map(PromptFingerprint::of);
    if seen.options.is_empty() || showing.as_ref() != Some(seen) {
        return Err(STATE_CHANGED.into());
    }
    tmux::run_ok(poller.runner.as_ref(), &tmux::send_key_args(pane_id, key))
}

/// Point the most recently active tmux client at the pane.
pub fn jump_tmux(r: &dyn Runner, pane_id: &str) -> Result<(), String> {
    check_pane_id(pane_id)?;
    let clients = r
        .run(&tmux::list_clients_args())
        .map_err(|e| format!("tmux: {e}"))?;
    let tty = tmux::pick_client(if clients.ok { &clients.stdout } else { "" })
        .ok_or("No tmux client is attached. Open a terminal and attach to tmux first.")?;
    let info = tmux::pane_info(r, pane_id)?;
    for args in tmux::jump_commands(&tty, &info.window_id, pane_id) {
        tmux::run_ok(r, &args)?;
    }
    Ok(())
}

pub fn activate_script(app_name: &str) -> String {
    let escaped = app_name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("tell application \"{escaped}\" to activate")
}

pub fn do_jump(app: &AppHandle, pane_id: &str) -> Result<(), String> {
    let shared = app.state::<Arc<Shared>>();
    jump_tmux(shared.runner.as_ref(), pane_id)?;
    let terminal = lock(&shared.settings).terminal_app.clone();
    if !terminal.trim().is_empty() {
        // The jump itself succeeded; failing to raise the terminal isn't fatal.
        let mut osa = Command::new("osascript");
        osa.args(["-e", &activate_script(terminal.trim())]);
        let _ = tmux::output_with_deadline(&mut osa, tmux::DEADLINE);
    }
    Ok(())
}

/// The preview polls every second; sitting hidden in the tray it shouldn't
/// cost a 200-line capture each time.
pub fn capture_if_visible(
    visible: bool,
    r: &dyn Runner,
    pane_id: &str,
) -> Result<PaneCapture, String> {
    if !visible {
        return Err(WINDOW_HIDDEN.into());
    }
    capture(r, pane_id)
}

pub fn capture(r: &dyn Runner, pane_id: &str) -> Result<PaneCapture, String> {
    check_pane_id(pane_id)?;
    let info = tmux::pane_info(r, pane_id)?;
    let ansi = tmux::capture(r, &tmux::preview_capture_args(pane_id))?;
    Ok(PaneCapture {
        ansi,
        width: info.width,
        height: info.height,
    })
}

#[tauri::command]
pub fn get_snapshot(shared: AppShared<'_>) -> Result<Snapshot, String> {
    Ok(shared.latest())
}

#[tauri::command]
pub async fn refresh(app: AppHandle) -> Result<Snapshot, String> {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || monitor::poll_now(&app, &shared))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn capture_pane(shared: AppShared<'_>, pane_id: String) -> Result<PaneCapture, String> {
    let runner = shared.runner.clone();
    let visible = shared.window_visible();
    blocking(move || capture_if_visible(visible, runner.as_ref(), &pane_id)).await
}

#[tauri::command]
pub async fn jump(app: AppHandle, pane_id: String) -> Result<(), String> {
    blocking(move || do_jump(&app, &pane_id)).await
}

#[tauri::command]
pub async fn send_reply(
    shared: AppShared<'_>,
    pane_id: String,
    text: String,
    expected_state: String,
    expected_session_id: String,
) -> Result<(), String> {
    let shared = shared.inner().clone();
    blocking(move || {
        // Under the core lock so a send never interleaves with a poll; the
        // check itself is a few tmux calls on this one pane.
        let core = lock(&shared.core);
        reply(
            &core.poller,
            &pane_id,
            &text,
            &expected_state,
            &expected_session_id,
        )
    })
    .await
}

#[tauri::command]
pub async fn send_choice(
    shared: AppShared<'_>,
    pane_id: String,
    key: String,
    expected_state: String,
    expected_session_id: String,
    expected_prompt: PromptFingerprint,
) -> Result<(), String> {
    let shared = shared.inner().clone();
    blocking(move || {
        let core = lock(&shared.core);
        choose(
            &core.poller,
            &pane_id,
            &key,
            &expected_state,
            &expected_session_id,
            &expected_prompt,
        )
    })
    .await
}

#[tauri::command]
pub fn get_settings(shared: AppShared<'_>) -> Result<Settings, String> {
    Ok(lock(&shared.settings).clone())
}

/// Register `next`'s hotkey, then save. Registration runs even when the
/// string is unchanged (it may have failed at startup); a failed register
/// puts the previous shortcut back, and so does a failed save, so the live
/// shortcut always matches the stored settings.
pub fn apply_settings(
    prev: &Settings,
    next: Settings,
    mut register: impl FnMut(&str) -> Result<(), String>,
    save: impl FnOnce(&Settings) -> Result<(), String>,
) -> Result<Settings, String> {
    if let Err(e) = register(&next.hotkey) {
        let _ = register(&prev.hotkey);
        return Err(e);
    }
    if let Err(e) = save(&next) {
        let _ = register(&prev.hotkey);
        return Err(e);
    }
    Ok(next)
}

#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    shared: AppShared<'_>,
    settings: Settings,
) -> Result<Settings, String> {
    let prev = lock(&shared.settings).clone();
    let dir = crate::config_dir(&app)?;
    let saved = apply_settings(
        &prev,
        settings,
        |h| crate::register_hotkey(&app, h),
        |s| settings::save(&dir, s),
    )?;
    *lock(&shared.settings) = saved.clone();
    Ok(saved)
}

#[tauri::command]
pub fn hide_window(app: AppHandle) -> Result<(), String> {
    crate::hide_window(&app)
}

// tmux and osascript block; keep them off the IPC thread.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::tests::FakeRunner;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    // A fake world: one registry entry (this test process's pid, so it is
    // "alive") in pane %13, with the given status and screen.
    fn world(tag: &str, status: &str, screen: String) -> (Poller, Arc<FakeRunner>, PathBuf) {
        world_cmd(tag, status, screen, "2.1.296")
    }

    // The same, with pane %13 running `command` (a version string is Claude).
    fn world_cmd(
        tag: &str,
        status: &str,
        screen: String,
        command: &str,
    ) -> (Poller, Arc<FakeRunner>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mc-cmd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.join(format!("{me}.json")),
            format!(r#"{{"pid":{me},"sessionId":"s1","cwd":"/tmp","status":"{status}","tmux":"-claude:@5.%13"}}"#),
        )
        .unwrap();
        let panes = format!(
            "-claude\t@5\t1\tw\t%12\t1\t1\tvim\t80\t37\t1\t/tmp\tvim\n\
             -claude\t@5\t1\tw\t%13\t2\t1\t{command}\t219\t37\t1\t/tmp\t✳ Claude Code\n"
        );
        let runner = Arc::new(FakeRunner {
            replies: vec![
                ("list-panes", true, panes, String::new()),
                ("capture-pane", true, screen, String::new()),
                (
                    "list-clients",
                    true,
                    "/dev/ttys000\t10\n/dev/ttys003\t20\n".into(),
                    String::new(),
                ),
                (
                    "display-message",
                    true,
                    "@5\t219\t37\n".into(),
                    String::new(),
                ),
            ],
            ..Default::default()
        });
        let mut poller = Poller::new(runner.clone());
        poller.sessions_dir = dir.clone();
        poller.projects_dir = dir.join("projects");
        // The registry entry is this test process, which isn't named Claude.
        poller.live_pid = crate::registry::pid_alive;
        (poller, runner, dir)
    }

    fn sends(r: &FakeRunner) -> Vec<Vec<String>> {
        r.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c[0] == "send-keys")
            .cloned()
            .collect()
    }

    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // What the UI saw for the permission-bash menu.
    fn bash_menu() -> PromptFingerprint {
        PromptFingerprint::of(&crate::detect::detect(&fixture("permission-bash.ansi")).unwrap())
    }

    #[test]
    fn reply_sends_literal_then_enter() {
        let (p, r, dir) = world("reply", "idle", fixture("idle.ansi"));
        reply(&p, "%13", "  run the\ntests ", "idle", "s1").unwrap();
        assert_eq!(
            sends(&r),
            vec![
                v(&["send-keys", "-t", "%13", "-l", "--", "run the tests"]),
                v(&["send-keys", "-t", "%13", "Enter"]),
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reply_rejects_when_state_moved() {
        // The UI thought it was idle; a permission menu has opened since.
        let (p, r, dir) = world("moved", "idle", fixture("permission-bash.ansi"));
        assert_eq!(
            reply(&p, "%13", "hi", "idle", "s1"),
            Err(STATE_CHANGED.into())
        );
        assert!(sends(&r).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reply_validation() {
        assert!(validate_reply("   ", "idle").is_err());
        assert!(validate_reply("x", "needs_input").is_err());
        assert!(validate_reply("x", "unknown").is_err());
        assert_eq!(validate_reply("x", "working"), Ok(State::Working));
        assert_eq!(validate_reply("x", "your_turn"), Ok(State::YourTurn));
        let (p, r, dir) = world("empty", "idle", fixture("idle.ansi"));
        assert!(reply(&p, "%13", "", "idle", "s1").is_err());
        assert!(reply(&p, "%13", "\t\x1b\u{7f}", "idle", "s1").is_err());
        assert!(reply(&p, "-claude", "x", "idle", "s1").is_err());
        assert!(sends(&r).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Suspended with Ctrl+Z, the pane runs zsh: text would run as a shell
    /// command. Refused outright (not "state_changed", which reads as "look
    /// again and resend").
    #[test]
    fn sends_refuse_a_pane_not_running_claude() {
        let (p, r, dir) = world_cmd("zsh", "idle", fixture("permission-bash.ansi"), "zsh");
        let err = reply(&p, "%13", "rm -rf build", "idle", "s1").unwrap_err();
        assert!(err.contains("isn't running Claude"), "{err}");
        let err = choose(&p, "%13", "1", "needs_input", "s1", &bash_menu()).unwrap_err();
        assert!(err.contains("isn't running Claude"), "{err}");
        assert!(sends(&r).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sends_reject_another_session_in_the_pane() {
        // After a tmux restart pane ids start over: %13 is someone else now.
        let (p, r, dir) = world("other", "idle", fixture("idle.ansi"));
        assert_eq!(
            reply(&p, "%13", "hi", "idle", "s-old"),
            Err(STATE_CHANGED.into())
        );
        std::fs::remove_dir_all(dir).unwrap();
        let (p, r2, dir) = world("other2", "busy", fixture("permission-bash.ansi"));
        assert_eq!(
            choose(&p, "%13", "1", "needs_input", "s-old", &bash_menu()),
            Err(STATE_CHANGED.into())
        );
        assert!(sends(&r).is_empty() && sends(&r2).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The pre-send check reads the registry and this one pane only: no
    /// `list-panes -a`, no other pane's capture.
    #[test]
    fn pre_send_check_touches_only_the_target_pane() {
        let (p, r, dir) = world("narrow", "idle", fixture("idle.ansi"));
        reply(&p, "%13", "hi", "your_turn", "s1").unwrap();
        let calls = r.calls.lock().unwrap().clone();
        assert!(
            !calls.iter().any(|c| c.contains(&"-a".to_string())),
            "{calls:?}"
        );
        let captured: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "capture-pane").collect();
        assert_eq!(captured, vec![&tmux::detect_capture_args("%13")]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reply_text_is_sanitized() {
        let (p, r, dir) = world("clean", "idle", fixture("idle.ansi"));
        reply(
            &p,
            "%13",
            "a\tb\x1bc\u{7f}d\u{2028}e\u{85}f\x03",
            "idle",
            "s1",
        )
        .unwrap();
        assert_eq!(
            sends(&r)[0],
            v(&["send-keys", "-t", "%13", "-l", "--", "a bcd e f"])
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// tmux reads a trailing `;` as a command separator and drops it, so it
    /// goes as a hex key after the body.
    #[test]
    fn trailing_semicolons_are_sent_as_keys() {
        assert_eq!(
            reply_commands("%1", "add the index;"),
            vec![
                v(&["send-keys", "-t", "%1", "-l", "--", "add the index"]),
                v(&["send-keys", "-t", "%1", "-H", "3b"]),
                v(&["send-keys", "-t", "%1", "Enter"]),
            ]
        );
        assert_eq!(
            reply_commands("%1", "end\\;;"),
            vec![
                v(&["send-keys", "-t", "%1", "-l", "--", "end\\"]),
                v(&["send-keys", "-t", "%1", "-H", "3b", "3b"]),
                v(&["send-keys", "-t", "%1", "Enter"]),
            ]
        );
        assert_eq!(
            reply_commands("%1", ";"),
            vec![
                v(&["send-keys", "-t", "%1", "-H", "3b"]),
                v(&["send-keys", "-t", "%1", "Enter"]),
            ]
        );
        assert_eq!(
            reply_commands("%1", "a;b"),
            vec![
                v(&["send-keys", "-t", "%1", "-l", "--", "a;b"]),
                v(&["send-keys", "-t", "%1", "Enter"]),
            ]
        );
    }

    #[test]
    fn choice_sends_only_the_key() {
        let (p, r, dir) = world("choice", "busy", fixture("permission-bash.ansi"));
        choose(&p, "%13", "1", "needs_input", "s1", &bash_menu()).unwrap();
        choose(&p, "%13", "Escape", "needs_input", "s1", &bash_menu()).unwrap();
        assert_eq!(
            sends(&r),
            vec![
                v(&["send-keys", "-t", "%13", "1"]),
                v(&["send-keys", "-t", "%13", "Escape"])
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The user read "Bash(ls) … 1. Yes"; meanwhile it was answered in tmux
    /// and a different prompt opened. Its "1" must not approve that one.
    #[test]
    fn choice_rejects_a_different_prompt() {
        let (p, r, dir) = world("diff", "busy", fixture("permission-bash.ansi"));
        let mut seen = bash_menu();
        seen.options[1] = "Yes, and don't ask again for ls".into();
        assert_eq!(
            choose(&p, "%13", "1", "needs_input", "s1", &seen),
            Err(STATE_CHANGED.into())
        );
        let mut seen = bash_menu();
        seen.question = Some("Do you want to make this edit?".into());
        assert_eq!(
            choose(&p, "%13", "Escape", "needs_input", "s1", &seen),
            Err(STATE_CHANGED.into())
        );
        assert!(sends(&r).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn choice_rejects_closed_menu_and_bad_keys() {
        let (p, r, dir) = world("closed", "idle", fixture("idle.ansi"));
        let menu = bash_menu();
        assert_eq!(
            choose(&p, "%13", "1", "needs_input", "s1", &menu),
            Err(STATE_CHANGED.into())
        );
        for bad in ["0", "10", "Enter", "a", "", "1\n"] {
            assert!(
                choose(&p, "%13", bad, "needs_input", "s1", &menu).is_err(),
                "{bad:?}"
            );
        }
        assert!(choose(&p, "%13", "1", "idle", "s1", &menu).is_err());
        assert!(sends(&r).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn choice_needs_a_parsed_menu() {
        // Registry says waiting but nothing parses: the UI offered no options.
        let (p, r, dir) = world("waitonly", "waiting", fixture("idle.ansi"));
        assert_eq!(
            choose(&p, "%13", "1", "needs_input", "s1", &bash_menu()),
            Err(STATE_CHANGED.into())
        );
        assert!(sends(&r).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn jump_targets_newest_client_by_id() {
        let (_p, r, dir) = world("jump", "idle", String::new());
        jump_tmux(r.as_ref(), "%13").unwrap();
        let calls = r.calls.lock().unwrap().clone();
        let tail: Vec<Vec<String>> = calls.into_iter().skip(2).collect();
        assert_eq!(
            tail,
            vec![
                v(&["switch-client", "-c", "/dev/ttys003", "-t", "%13"]),
                v(&["select-window", "-t", "@5"]),
                v(&["select-pane", "-t", "%13"]),
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn jump_without_client_errors() {
        let r = FakeRunner {
            replies: vec![("list-clients", true, String::new(), String::new())],
            ..Default::default()
        };
        assert!(jump_tmux(&r, "%13").unwrap_err().contains("No tmux client"));
    }

    #[test]
    fn capture_uses_pane_geometry() {
        let (_p, r, dir) = world("cap", "idle", "\x1b[1mhi\x1b[0m".into());
        let c = capture(r.as_ref(), "%13").unwrap();
        assert_eq!((c.width, c.height), (219, 37));
        assert_eq!(c.ansi, "\x1b[1mhi\x1b[0m");
        let calls = r.calls.lock().unwrap().clone();
        assert!(calls.contains(&tmux::preview_capture_args("%13")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn capture_is_skipped_while_the_window_is_hidden() {
        let r = FakeRunner::default();
        assert_eq!(
            capture_if_visible(false, &r, "%13"),
            Err(WINDOW_HIDDEN.into())
        );
        assert!(r.calls.lock().unwrap().is_empty());
    }

    // A fake hotkey registry: records attempts, fails for `bad`.
    fn registrar<'a>(
        calls: &'a std::cell::RefCell<Vec<String>>,
        bad: &'a str,
    ) -> impl FnMut(&str) -> Result<(), String> + 'a {
        move |h: &str| {
            calls.borrow_mut().push(h.to_string());
            if h == bad {
                Err(format!("Couldn't register hotkey \"{h}\""))
            } else {
                Ok(())
            }
        }
    }

    fn with_hotkey(h: &str) -> Settings {
        Settings {
            hotkey: h.into(),
            ..Default::default()
        }
    }

    /// The stored hotkey may have failed to register at startup (another app
    /// held it); saving the same string must try again, not report success.
    #[test]
    fn saving_settings_always_registers_the_hotkey() {
        let calls = std::cell::RefCell::new(Vec::new());
        let prev = with_hotkey("Ctrl+Alt+M");
        let out = apply_settings(&prev, prev.clone(), registrar(&calls, ""), |_| Ok(()));
        assert_eq!(out, Ok(prev));
        assert_eq!(*calls.borrow(), vec!["Ctrl+Alt+M"]);
    }

    #[test]
    fn a_bad_hotkey_restores_the_previous_one() {
        let calls = std::cell::RefCell::new(Vec::new());
        let saved = std::cell::Cell::new(false);
        let out = apply_settings(
            &with_hotkey("Ctrl+Alt+M"),
            with_hotkey("Hyper+X"),
            registrar(&calls, "Hyper+X"),
            |_| {
                saved.set(true);
                Ok(())
            },
        );
        assert!(out.unwrap_err().contains("hotkey"));
        assert_eq!(*calls.borrow(), vec!["Hyper+X", "Ctrl+Alt+M"]);
        assert!(!saved.get());
    }

    #[test]
    fn a_failed_save_rolls_the_hotkey_back() {
        let calls = std::cell::RefCell::new(Vec::new());
        let out = apply_settings(
            &with_hotkey("Ctrl+Alt+M"),
            with_hotkey("Cmd+Shift+K"),
            registrar(&calls, ""),
            |_| Err("save settings: disk full".into()),
        );
        assert_eq!(out, Err("save settings: disk full".into()));
        assert_eq!(*calls.borrow(), vec!["Cmd+Shift+K", "Ctrl+Alt+M"]);
    }

    #[test]
    fn activate_script_escapes_quotes() {
        assert_eq!(
            activate_script("iTerm"),
            "tell application \"iTerm\" to activate"
        );
        assert_eq!(
            activate_script("a\"b"),
            "tell application \"a\\\"b\" to activate"
        );
    }
}
