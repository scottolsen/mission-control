//! Reads a Claude Code pane capture and decides whether a menu is open: a
//! permission prompt, an AskUserQuestion picker, or the folder-trust dialog.
//! When one is, it parses the question and numbered options so the UI can
//! answer it without the user jumping to the pane.
//!
//! The capture is whatever tmux has on screen, scrollback included, so the
//! parser anchors on the `❯ N.` cursor line nearest the bottom and walks out
//! from it collecting consecutively numbered options. A numbered list in an
//! earlier Claude reply never forms a run around the cursor, and a menu that
//! isn't near the bottom of the screen is history, not a live prompt.
//!
//! A prompt the user submitted (or is still drafting) that is itself a
//! numbered list looks exactly like an AskUserQuestion menu: Claude Code
//! echoes it as `❯ 1. …` at column 0. Live menus replace the input box and
//! carry a key-hint footer, so a run only counts when a footer follows it
//! and no input-box `❯` line does.

use crate::types::{Prompt, PromptKind, PromptOption};
use regex::Regex;
use std::sync::OnceLock;

const LABEL_MAX: usize = 120;
// A live menu is the last thing on screen; only a short footer ("Enter to
// select · …", the status line) may follow it.
const MAX_TRAILING_LINES: usize = 8;
// Lines above the first option searched for the kind markers (the `☐ Color`
// header sits two lines above `❯ 1. Red`).
const KIND_WINDOW: usize = 6;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

fn ansi_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // OSC (hyperlinks, titles) ended by BEL or ST, then CSI, then any other
    // two-byte escape. OSC first: its body may contain `[`.
    re(
        &R,
        r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_]",
    )
}

fn cursor_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^\s*❯\s*(\d+)\.\s")
}

fn option_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^(\s*)(?:❯\s*)?(\d+)\.\s+(.*)$")
}

fn proceed_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"Do you want to [^?]*\?")
}

fn trust_no_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^\s*(?:❯\s*)?No, exit\s*$")
}

// The key hints under every live menu: "Enter to select · ↑/↓ to navigate ·
// Esc to cancel", "Esc to cancel · Tab to amend", "Enter to confirm · …".
fn footer_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"Esc to cancel|Enter to (?:select|confirm)")
}

// The input box's prompt line: a bare `❯` or `❯ ` + draft text. Option lines
// (`❯ 1. …`) are excluded by the caller, which only looks past the run.
fn input_prompt_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^\s*❯(?:\s|$)")
}

fn trust_yes_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^\s*(?:❯\s*)?Yes, I trust this folder\s*$")
}

/// Remove terminal escape sequences and carriage returns from a raw capture.
pub fn strip_ansi(raw: &str) -> String {
    ansi_re().replace_all(raw, "").replace('\r', "")
}

/// Parse a raw (possibly ANSI) pane capture. `Some` means a menu is open.
pub fn detect(raw: &str) -> Option<Prompt> {
    let text = strip_ansi(raw);
    let lines: Vec<&str> = text.lines().collect();
    detect_trust(&lines).or_else(|| detect_menu(&lines))
}

// Number of non-blank lines after index `i`.
fn trailing_lines(lines: &[&str], i: usize) -> usize {
    lines[i + 1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .count()
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '─' | '╌' | '━' | '┄' | '┈'))
}

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

// What follows a live menu's last line: a footer within the trailing lines,
// and no input box (whose presence means the "menu" is an echoed prompt).
fn live_below(lines: &[&str], last: usize) -> bool {
    let below = &lines[last + 1..];
    trailing_lines(lines, last) <= MAX_TRAILING_LINES
        && below.iter().any(|l| footer_re().is_match(l))
        && !below.iter().any(|l| input_prompt_re().is_match(l))
}

// The trust dialog has no numbers, so it is matched by its two fixed lines,
// adjacent and at the bottom of the screen.
fn detect_trust(lines: &[&str]) -> Option<Prompt> {
    let pos = lines.iter().rposition(|l| trust_no_re().is_match(l))?;
    let near = |j: Option<&&str>| j.is_some_and(|l| trust_yes_re().is_match(l));
    let yes_below = near(lines.get(pos + 1));
    let paired = yes_below || pos > 0 && near(lines.get(pos - 1));
    // The cursor may sit on either line, so look below the lower one.
    if !paired || !live_below(lines, if yes_below { pos + 1 } else { pos }) {
        return None;
    }
    Some(Prompt {
        kind: PromptKind::Trust,
        question: None,
        options: Vec::new(),
    })
}

struct Opt {
    line: usize,
    num: u32,
    indent: usize,
    label: String,
}

fn parse_option(lines: &[&str], i: usize) -> Option<Opt> {
    let caps = option_re().captures(lines[i])?;
    Some(Opt {
        line: i,
        num: caps[2].parse().ok()?,
        indent: caps[1].chars().count(),
        label: cap_label(caps[3].trim()),
    })
}

fn cap_label(label: &str) -> String {
    if label.chars().count() <= LABEL_MAX {
        return label.to_string();
    }
    let mut out: String = label.chars().take(LABEL_MAX - 1).collect();
    out.push('…');
    out
}

// Lines that may sit between two options: blanks, rules (AskUserQuestion
// draws one above "Chat about this"), and description lines indented deeper
// than the option itself.
fn skippable(line: &str, option_indent: usize) -> bool {
    line.trim().is_empty() || is_rule(line) || indent(line) > option_indent + 1
}

fn detect_menu(lines: &[&str]) -> Option<Prompt> {
    let cursor = lines.iter().rposition(|l| cursor_re().is_match(l))?;
    let anchor = parse_option(lines, cursor)?;
    let base_indent = anchor.indent;

    // Walk up for N-1, N-2, …
    let mut before = Vec::new();
    let mut want = anchor.num.saturating_sub(1);
    let mut i = cursor;
    while want >= 1 && i > 0 {
        i -= 1;
        match parse_option(lines, i) {
            Some(o) if o.num == want => {
                want -= 1;
                before.push(o);
            }
            Some(_) => break,
            None if skippable(lines[i], base_indent) => {}
            None => break,
        }
    }
    if want != 0 {
        // The run doesn't start at 1: not a menu (e.g. "❯ 3. foo" typed text).
        return None;
    }
    before.reverse();

    // Walk down for N+1, N+2, …
    let mut after = Vec::new();
    let mut want = anchor.num + 1;
    for (j, line) in lines.iter().enumerate().skip(cursor + 1) {
        match parse_option(lines, j) {
            Some(o) if o.num == want => {
                want += 1;
                after.push(o);
            }
            Some(_) => break,
            None if skippable(line, base_indent) => {}
            None => break,
        }
    }

    let mut opts = before;
    opts.push(anchor);
    opts.extend(after);
    if opts.len() < 2 {
        return None;
    }
    let first = opts[0].line;
    let last = opts[opts.len() - 1].line;
    if !live_below(lines, last) {
        return None;
    }

    let question = lines[..first]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty() && !is_rule(l))
        .map(|l| l.trim().to_string());

    let window = &lines[first.saturating_sub(KIND_WINDOW)..=last];
    let kind = if window.iter().any(|l| proceed_re().is_match(l)) {
        PromptKind::Permission
    } else if window.iter().any(|l| l.contains('☐') || l.contains('☒')) {
        PromptKind::Question
    } else {
        PromptKind::Other
    };

    Some(Prompt {
        kind,
        question,
        options: opts
            .into_iter()
            .map(|o| PromptOption {
                key: o.num.to_string(),
                label: o.label,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).unwrap()
    }

    fn keys(p: &Prompt) -> Vec<&str> {
        p.options.iter().map(|o| o.key.as_str()).collect()
    }

    #[test]
    fn strips_csi_osc_and_cr() {
        let raw = "\x1b[1;31mred\x1b[0m \x1b]8;id=x;https://e.com\x1b\\link\x1b]8;;\x1b\\ \x1b]0;t\x07ok\r";
        assert_eq!(strip_ansi(raw), "red link ok");
    }

    #[test]
    fn permission_bash_fixture() {
        let p = detect(&fixture("permission-bash.ansi")).expect("menu");
        assert_eq!(p.kind, PromptKind::Permission);
        assert_eq!(p.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(keys(&p), vec!["1", "2", "3", "4"]);
        assert_eq!(p.options[0].label, "Yes");
        assert_eq!(p.options[3].label, "No");
        // Option 2 carries a long path and must be capped.
        assert_eq!(p.options[1].label.chars().count(), LABEL_MAX);
        assert!(p.options[1].label.ends_with('…'));
    }

    #[test]
    fn ask_question_fixture() {
        let p = detect(&fixture("ask-question.ansi")).expect("menu");
        assert_eq!(p.kind, PromptKind::Question);
        assert_eq!(p.question.as_deref(), Some("Which color do you prefer?"));
        let labels: Vec<&str> = p.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Red", "Blue", "Type something.", "Chat about this"]
        );
        assert_eq!(keys(&p), vec!["1", "2", "3", "4"]);
    }

    #[test]
    fn trust_fixtures() {
        for name in ["trust-menu.ansi", "startup.ansi"] {
            let p = detect(&fixture(name)).expect(name);
            assert_eq!(p.kind, PromptKind::Trust, "{name}");
            assert!(p.options.is_empty(), "{name}");
        }
    }

    #[test]
    fn idle_and_busy_have_no_menu() {
        assert_eq!(detect(&fixture("idle.ansi")), None);
        assert_eq!(detect(&fixture("busy.ansi")), None);
    }

    /// A submitted prompt is echoed as `❯ 1. …` at column 0 with its
    /// continuation lines indented 2: the shape of an AskUserQuestion menu.
    #[test]
    fn echoed_and_drafted_numbered_prompts_are_not_menus() {
        for name in [
            "echo-list-busy.ansi",
            "echo-list-reply.ansi",
            "draft-list.ansi",
        ] {
            assert_eq!(detect(&fixture(name)), None, "{name}");
        }
    }

    #[test]
    fn menu_needs_a_footer_and_no_input_box_below() {
        let menu = "Pick one?\n❯ 1. A\n  2. B\n";
        assert_eq!(detect(menu), None, "no footer");
        assert!(detect(&format!("{menu}\nEnter to select · Esc to cancel")).is_some());
        // A footer from an older menu can't vouch for an input box.
        let boxed = format!("{menu}\n────\n❯ \n────\nEsc to cancel");
        assert_eq!(detect(&boxed), None);
        let trust = " ❯ No, exit\n   Yes, I trust this folder\n";
        assert_eq!(detect(trust), None, "trust dialog without a footer");
        assert!(detect(&format!("{trust}\n Enter to confirm · Esc to cancel")).is_some());
    }

    #[test]
    fn numbered_list_in_scrollback_is_not_a_menu() {
        let text = "⏺ Steps:\n  1. Do this\n  2. Do that\n\n❯ 1. typed by me\n\n────\n  status";
        assert_eq!(detect(text), None);
    }

    #[test]
    fn menu_far_above_the_bottom_is_history() {
        let mut text = String::from("Do you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel\n");
        for i in 0..12 {
            text.push_str(&format!("line {i}\n"));
        }
        assert_eq!(detect(&text), None);
    }

    #[test]
    fn cursor_on_later_option_collects_earlier_ones() {
        let text = "Pick one?\n  1. A\n     desc\n❯ 2. B\n  3. C\nEsc to cancel";
        let p = detect(text).expect("menu");
        assert_eq!(keys(&p), vec!["1", "2", "3"]);
        assert_eq!(p.question.as_deref(), Some("Pick one?"));
        assert_eq!(p.kind, PromptKind::Other);
    }

    #[test]
    fn trust_phrase_in_prose_is_not_a_dialog() {
        let text = "⏺ The dialog says Yes, I trust this folder and ❯ No, exit.\n❯ ";
        assert_eq!(detect(text), None);
    }
}
