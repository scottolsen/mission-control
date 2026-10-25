//! Wire types shared with the web UI. Every struct here serializes to the
//! camelCase shapes in `docs/CONTRACT.md`; change the contract first, then
//! these, then the UI. Timestamps are Unix milliseconds.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    NeedsInput,
    YourTurn,
    Working,
    Idle,
    Unknown,
}

impl State {
    // The contract's string form, used when the UI passes `expectedState`.
    pub fn parse(s: &str) -> Option<State> {
        match s {
            "needs_input" => Some(State::NeedsInput),
            "your_turn" => Some(State::YourTurn),
            "working" => Some(State::Working),
            "idle" => Some(State::Idle),
            "unknown" => Some(State::Unknown),
            _ => None,
        }
    }

    // Sessions that want the user: counted in the tray title and menu.
    pub fn wants_attention(self) -> bool {
        matches!(self, State::NeedsInput | State::YourTurn)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TmuxLocation {
    pub session: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_id: String,
    pub pane_index: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Permission,
    Question,
    Trust,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOption {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    pub kind: PromptKind,
    pub question: Option<String>,
    pub options: Vec<PromptOption>,
}

/// What the UI showed of a menu when the user chose: its question and option
/// labels. A choice is only sent while the pane still shows exactly this, so
/// a "1" meant for one prompt can't approve the next one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptFingerprint {
    pub question: Option<String>,
    pub options: Vec<String>,
}

impl PromptFingerprint {
    pub fn of(p: &Prompt) -> PromptFingerprint {
        PromptFingerprint {
            question: p.question.clone(),
            options: p.options.iter().map(|o| o.label.clone()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentTool {
    pub name: String,
    pub summary: String,
}

/// A `/loop` (or any recurring scheduled prompt) running in a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopInfo {
    /// What it runs, plus the interval when known: `/kanban-worker · every 5m`.
    pub label: String,
    /// When the last scheduled run started; `None` until the first one fires.
    pub last_fired_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub pid: Option<i64>,
    pub session_id: Option<String>,
    pub cwd: String,
    pub project: String,
    pub branch: Option<String>,
    pub tmux: Option<TmuxLocation>,
    pub title: String,
    pub state: State,
    pub prompt: Option<Prompt>,
    pub last_prompt: Option<String>,
    pub last_prompt_at: Option<i64>,
    pub last_reply: Option<String>,
    pub last_reply_at: Option<i64>,
    pub current_tool: Option<CurrentTool>,
    #[serde(rename = "loop")]
    pub loop_info: Option<LoopInfo>,
    /// The Remote Control session id while connected (registry `bridgeSessionId`).
    pub remote_session_id: Option<String>,
    pub last_activity_at: i64,
    pub status_updated_at: Option<i64>,
    pub started_at: Option<i64>,
    pub version: Option<String>,
    pub jumpable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub sessions: Vec<Session>,
    pub generated_at: i64,
    pub tmux_running: bool,
    pub error: Option<String>,
    /// The installed Claude Code (what a new session would run).
    pub claude_version: Option<String>,
}

impl Snapshot {
    // Content equality for change detection: `generatedAt` ticks every poll
    // and must not, by itself, trigger an emit.
    pub fn same_content(&self, other: &Snapshot) -> bool {
        self.sessions == other.sessions
            && self.tmux_running == other.tmux_running
            && self.error == other.error
            && self.claude_version == other.claude_version
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub notify_your_turn: bool,
    pub notify_needs_input: bool,
    pub hotkey: String,
    pub stale_hours: u32,
    pub terminal_app: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            notify_your_turn: true,
            // The user's Notification hook already alerts on permission prompts.
            notify_needs_input: false,
            hotkey: "Ctrl+Alt+M".to_string(),
            stale_hours: 8,
            terminal_app: "iTerm".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneCapture {
    pub ansi: String,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_serializes_snake_case() {
        let json = serde_json::to_string(&State::NeedsInput).unwrap();
        assert_eq!(json, "\"needs_input\"");
        assert_eq!(State::parse("your_turn"), Some(State::YourTurn));
        assert_eq!(State::parse("bogus"), None);
    }

    #[test]
    fn snapshot_keys_are_camel_case() {
        let snap = Snapshot {
            tmux_running: true,
            ..Default::default()
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert!(v.get("generatedAt").is_some());
        assert!(v.get("tmuxRunning").is_some());
        assert!(v.get("error").is_some());
    }

    #[test]
    fn same_content_ignores_generated_at() {
        let a = Snapshot {
            generated_at: 1,
            tmux_running: true,
            ..Default::default()
        };
        let mut b = a.clone();
        b.generated_at = 99;
        assert!(a.same_content(&b));
        b.error = Some("boom".into());
        assert!(!a.same_content(&b));
    }

    /// The UI's fixture is the UI's idea of the wire format. Round-tripping it
    /// through these types catches drift on either side: a renamed, missing or
    /// extra key comes back different (or fails to parse).
    #[test]
    fn ui_fixture_round_trips() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test/fixtures/snapshot.json"
        );
        let original: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let snap: Snapshot = serde_json::from_value(original.clone()).unwrap();
        assert!(!snap.sessions.is_empty());
        assert_eq!(serde_json::to_value(&snap).unwrap(), original);
    }

    /// What the settings sheet sends (`readSettingsForm` output) must parse.
    #[test]
    fn ui_settings_payload_parses() {
        let ui = r#"{"notifyYourTurn":false,"notifyNeedsInput":true,"hotkey":"Cmd+Shift+K","staleHours":12,"terminalApp":"Ghostty"}"#;
        let s: Settings = serde_json::from_str(ui).unwrap();
        assert_eq!(s.stale_hours, 12);
        assert_eq!(s.terminal_app, "Ghostty");
        // Fractions don't fit a u32; the form rejects them before sending.
        assert!(serde_json::from_str::<Settings>(r#"{"staleHours":1.5}"#).is_err());
    }
}
