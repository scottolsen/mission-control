//! Decides which state transitions deserve a macOS notification and posts
//! them through terminal-notifier. Deciding is pure (previous states in, list
//! of notifications out) so the rules are testable; posting is fire-and-forget.

use crate::types::{Session, Settings, State};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

const BUNDLE_ID: &str = "com.scottolsen.mission-control";
const HOMEBREW_NOTIFIER: &str = "/opt/homebrew/bin/terminal-notifier";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: String,
    pub subtitle: String,
    pub message: String,
}

/// Notifications for sessions whose state changed since `prev`. `prev` is
/// `None` on the first poll, which never notifies: everything would "change".
pub fn decide(
    prev: Option<&HashMap<String, State>>,
    sessions: &[Session],
    settings: &Settings,
) -> Vec<Notice> {
    let Some(prev) = prev else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|s| {
            let before = prev.get(&s.id).copied();
            let suffix = match (before, s.state) {
                (Some(State::Working), State::YourTurn) if settings.notify_your_turn => "Your turn",
                (b, State::NeedsInput)
                    if b != Some(State::NeedsInput) && settings.notify_needs_input =>
                {
                    "Needs input"
                }
                _ => return None,
            };
            Some(Notice {
                id: s.id.clone(),
                subtitle: subtitle(s),
                message: format!("{}: {suffix}", s.title),
            })
        })
        .collect()
}

fn subtitle(s: &Session) -> String {
    match &s.tmux {
        Some(t) => format!("{} · {}", s.project, t.session),
        None => s.project.clone(),
    }
}

pub fn states(sessions: &[Session]) -> HashMap<String, State> {
    sessions.iter().map(|s| (s.id.clone(), s.state)).collect()
}

// terminal-notifier reads its arguments NSUserDefaults-style, where a value
// starting with `-` is taken for the next flag. A zero-width space in front
// keeps it a value and doesn't show.
fn value(s: &str) -> String {
    if s.starts_with('-') {
        format!("\u{200B}{s}")
    } else {
        s.to_string()
    }
}

pub fn notifier_args(n: &Notice) -> Vec<String> {
    [
        "-title",
        "Mission Control",
        "-subtitle",
        &value(&n.subtitle),
        "-message",
        &value(&n.message),
        "-group",
        &format!("mission-control-{}", n.id),
        "-activate",
        BUNDLE_ID,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Post without waiting. Missing terminal-notifier is silently skipped.
pub fn post(n: &Notice) {
    let program = if Path::new(HOMEBREW_NOTIFIER).exists() {
        HOMEBREW_NOTIFIER
    } else {
        "terminal-notifier"
    };
    let spawned = Command::new(program)
        .args(notifier_args(n))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        // Reap in the background so it doesn't linger as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::types::TmuxLocation;

    pub fn session(id: &str, state: State) -> Session {
        Session {
            id: id.into(),
            pid: Some(1),
            session_id: Some(id.into()),
            cwd: "/x/shopfront".into(),
            project: "shopfront".into(),
            branch: None,
            tmux: Some(TmuxLocation {
                session: "-claude".into(),
                window_id: "@1".into(),
                window_index: 1,
                window_name: "w".into(),
                pane_id: "%1".into(),
                pane_index: 1,
                width: 80,
                height: 24,
            }),
            title: "Fix tests".into(),
            state,
            prompt: None,
            last_prompt: None,
            last_prompt_at: None,
            last_reply: None,
            last_reply_at: None,
            current_tool: None,
            last_activity_at: 0,
            status_updated_at: None,
            started_at: None,
            version: None,
            jumpable: true,
        }
    }

    fn prev(pairs: &[(&str, State)]) -> HashMap<String, State> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn first_poll_never_notifies() {
        let s = vec![
            session("a", State::YourTurn),
            session("b", State::NeedsInput),
        ];
        let all = Settings {
            notify_needs_input: true,
            ..Default::default()
        };
        assert!(decide(None, &s, &all).is_empty());
    }

    #[test]
    fn working_to_your_turn() {
        let p = prev(&[
            ("a", State::Working),
            ("b", State::Idle),
            ("c", State::YourTurn),
        ]);
        let s = vec![
            session("a", State::YourTurn),
            session("b", State::YourTurn),
            session("c", State::YourTurn),
        ];
        let n = decide(Some(&p), &s, &Settings::default());
        assert_eq!(
            n,
            vec![Notice {
                id: "a".into(),
                subtitle: "shopfront · -claude".into(),
                message: "Fix tests: Your turn".into()
            }]
        );
        let off = Settings {
            notify_your_turn: false,
            ..Default::default()
        };
        assert!(decide(Some(&p), &s, &off).is_empty());
    }

    #[test]
    fn needs_input_respects_setting() {
        let p = prev(&[("a", State::Working), ("b", State::NeedsInput)]);
        // "new" appears after the first poll directly in needs_input.
        let s = vec![
            session("a", State::NeedsInput),
            session("b", State::NeedsInput),
            session("new", State::NeedsInput),
        ];
        assert!(decide(Some(&p), &s, &Settings::default()).is_empty());
        let on = Settings {
            notify_needs_input: true,
            ..Default::default()
        };
        let ids: Vec<String> = decide(Some(&p), &s, &on)
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(ids, vec!["a", "new"]);
    }

    #[test]
    fn new_session_in_your_turn_is_not_a_transition() {
        let p = prev(&[]);
        assert!(decide(
            Some(&p),
            &[session("a", State::YourTurn)],
            &Settings::default()
        )
        .is_empty());
    }

    /// terminal-notifier parses its arguments NSUserDefaults-style, so a
    /// value starting with `-` (a session titled "-claude") reads as a flag.
    #[test]
    fn values_starting_with_a_dash_are_escaped() {
        let n = Notice {
            id: "abc".into(),
            subtitle: "-proj · -claude".into(),
            message: "-claude: Your turn".into(),
        };
        let args = notifier_args(&n);
        assert_eq!(args[3], "\u{200B}-proj · -claude");
        assert_eq!(args[5], "\u{200B}-claude: Your turn");
        assert_eq!(args[7], "mission-control-abc");
    }

    #[test]
    fn notifier_command_shape() {
        let n = Notice {
            id: "abc".into(),
            subtitle: "shopfront · -claude".into(),
            message: "T: Your turn".into(),
        };
        assert_eq!(
            notifier_args(&n),
            vec![
                "-title",
                "Mission Control",
                "-subtitle",
                "shopfront · -claude",
                "-message",
                "T: Your turn",
                "-group",
                "mission-control-abc",
                "-activate",
                "com.scottolsen.mission-control"
            ]
        );
    }
}
