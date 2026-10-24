//! Pulls "what is this session doing" out of a Claude Code transcript
//! (`~/.claude/projects/<dir>/<sessionId>.jsonl`): the latest human prompt,
//! the latest assistant reply, the tool currently in flight, and the AI title.
//!
//! Transcripts grow to many megabytes, so only the tail is read, and the
//! parsed result is cached against the file's (length, mtime) so an
//! unchanged transcript costs one `stat` per poll.

use crate::types::{CurrentTool, LoopInfo};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

pub const TAIL_BYTES: u64 = 512 * 1024;
// A single newer line (a big Write, a tool_result, an image) can outgrow the
// tail; the window grows to this before giving up.
pub const TAIL_CAP: u64 = 8 * 1024 * 1024;
const PROMPT_MAX: usize = 400;
const REPLY_MAX: usize = 600;
const LOOP_LABEL_MAX: usize = 80;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extract {
    pub last_prompt: Option<String>,
    pub last_prompt_at: Option<i64>,
    pub last_reply: Option<String>,
    pub last_reply_at: Option<i64>,
    pub current_tool: Option<CurrentTool>,
    pub last_activity_at: Option<i64>,
    pub ai_title: Option<String>,
    /// The loop this session is running, if the tail shows one still going.
    pub loop_info: Option<LoopInfo>,
    /// True when a human prompt was found in the tail, or must precede it
    /// (see `infer_prompt_before_tail`).
    pub has_prompt: bool,
}

/// Read at most `max` bytes from the end of `path`, and whether the read
/// started mid-file. A window that starts mid-line drops that partial first
/// line; one that starts exactly on a line keeps it.
pub fn read_tail(path: &Path, max: u64) -> std::io::Result<(String, bool)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max);
    // One byte early, to see whether `start` is a line start.
    let from = start.saturating_sub(1);
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity((len - from) as usize);
    file.read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        text = match text.find('\n') {
            Some(i) => text[i + 1..].to_string(),
            None => String::new(),
        };
    }
    Ok((text, start > 0))
}

/// `read_tail`, doubling the window (up to `cap`) while the newest line is
/// too long for it to hold a single complete line.
pub fn read_tail_growing(path: &Path, max: u64, cap: u64) -> std::io::Result<(String, bool)> {
    let mut window = max;
    loop {
        let (text, truncated) = read_tail(path, window)?;
        if !truncated || !text.is_empty() || window >= cap {
            return Ok((text, truncated));
        }
        window = (window * 2).min(cap);
    }
}

/// Apply the contract's transcript rules to JSONL text.
pub fn extract(text: &str) -> Extract {
    let mut out = Extract::default();
    // Open tool_use blocks in order: (id, tool). Closed when a tool_result
    // with the matching tool_use_id appears.
    let mut open: Vec<(String, CurrentTool)> = Vec::new();
    let mut fallback_prompt: Option<String> = None;
    // The newest recurring CronCreate's label: a fire only names its prompt.
    let mut cron_label: Option<String> = None;

    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("user") => {
                bump_activity(&mut out, &entry);
                if is_scheduled(&entry) {
                    on_fire(&mut out, &mut open, &entry, cron_label.as_deref());
                } else {
                    on_user(&mut out, &mut open, &entry);
                }
            }
            Some("assistant") => {
                bump_activity(&mut out, &entry);
                on_assistant(&mut out, &mut open, &entry);
                on_loop_tools(&mut out, &mut cron_label, &entry);
            }
            Some("ai-title") => {
                if let Some(t) = entry.get("aiTitle").and_then(Value::as_str) {
                    if !t.trim().is_empty() {
                        out.ai_title = Some(t.trim().to_string());
                    }
                }
            }
            Some("last-prompt") => {
                if let Some(p) = entry.get("lastPrompt").and_then(Value::as_str) {
                    fallback_prompt = Some(p.to_string());
                }
            }
            _ => {}
        }
    }

    // `last-prompt` carries no timestamp, so it only fills in the text when
    // the human prompt itself scrolled out of the tail.
    if out.last_prompt.is_none() {
        out.last_prompt = fallback_prompt
            .filter(|p| !is_wrapper(p))
            .map(|p| collapse(&p, PROMPT_MAX));
    }
    out.current_tool = open.pop().map(|(_, tool)| tool);
    out
}

fn bump_activity(out: &mut Extract, entry: &Value) {
    if let Some(ts) = timestamp(entry) {
        out.last_activity_at = Some(out.last_activity_at.map_or(ts, |cur| cur.max(ts)));
    }
}

fn timestamp(entry: &Value) -> Option<i64> {
    entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_iso_ms)
}

fn on_user(out: &mut Extract, open: &mut Vec<(String, CurrentTool)>, entry: &Value) {
    let content = &entry["message"]["content"];
    close_tools(open, content);
    // Meta entries and the compaction summary ("This session is being
    // continued…") are written as `user` but aren't the human.
    let flag = |k: &str| entry.get(k).and_then(Value::as_bool) == Some(true);
    if flag("isMeta") || flag("isCompactSummary") || flag("isVisibleInTranscriptOnly") {
        return;
    }
    let Some(text) = human_text(content) else {
        return;
    };
    out.has_prompt = true;
    out.last_prompt = Some(collapse(&text, PROMPT_MAX));
    out.last_prompt_at = timestamp(entry);
}

fn close_tools(open: &mut Vec<(String, CurrentTool)>, content: &Value) {
    if let Some(blocks) = content.as_array() {
        for b in blocks {
            if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                if let Some(id) = b.get("tool_use_id").and_then(Value::as_str) {
                    open.retain(|(open_id, _)| open_id != id);
                }
            }
        }
    }
}

// A turn Claude Code started on a schedule (a `/loop` interval or wakeup)
// rather than from the human.
fn is_scheduled(entry: &Value) -> bool {
    entry.get("turnOrigin").and_then(Value::as_str) == Some("scheduled")
        || entry.get("scheduledTaskId").is_some()
        || entry.get("wakeupSource").is_some()
}

// A scheduled run: the loop is alive as of now. Never a human prompt, even
// when the scheduled prompt is plain text.
fn on_fire(
    out: &mut Extract,
    open: &mut Vec<(String, CurrentTool)>,
    entry: &Value,
    cron_label: Option<&str>,
) {
    let content = &entry["message"]["content"];
    close_tools(open, content);
    // The expanded skill body rides along as a meta entry with the same tags.
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return;
    }
    let label = cron_label
        .map(str::to_string)
        .or_else(|| fired_prompt(content))
        .or_else(|| out.loop_info.as_ref().map(|l| l.label.clone()))
        .unwrap_or_else(|| "Scheduled prompt".to_string());
    out.loop_info = Some(LoopInfo {
        label,
        last_fired_at: timestamp(entry),
    });
}

// What a scheduled turn runs: `/kanban-worker` for a slash command (the
// `<command-name>` wrapper plus any args), else the prompt text itself.
fn fired_prompt(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{name}>"))? + start;
        Some(text[start..end].trim().to_string())
    };
    let prompt = match tag("command-name") {
        Some(cmd) => match tag("command-args").filter(|a| !a.is_empty()) {
            Some(args) => format!("{cmd} {args}"),
            None => cmd,
        },
        None if !is_wrapper(&text) => text,
        None => return None,
    };
    let prompt = collapse(&prompt, LOOP_LABEL_MAX);
    (!prompt.is_empty()).then_some(prompt)
}

// The tools `/loop` drives: a recurring CronCreate (or a ScheduleWakeup)
// starts a loop, CronDelete or `ScheduleWakeup {stop: true}` ends it.
fn on_loop_tools(out: &mut Extract, cron_label: &mut Option<String>, entry: &Value) {
    let Some(blocks) = entry["message"]["content"].as_array() else {
        return;
    };
    for b in blocks {
        if b.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let input = &b["input"];
        let prompt = || {
            input
                .get("prompt")
                .and_then(Value::as_str)
                .map(|p| collapse(p, LOOP_LABEL_MAX))
                .filter(|p| !p.is_empty())
        };
        match b.get("name").and_then(Value::as_str) {
            Some("CronCreate")
                if input.get("recurring").and_then(Value::as_bool) != Some(false) =>
            {
                let label = prompt().unwrap_or_else(|| "Scheduled prompt".to_string());
                let label = match input.get("cron").and_then(Value::as_str) {
                    Some(cron) => format!("{label} · {}", cron_text(cron)),
                    None => label,
                };
                *cron_label = Some(label.clone());
                out.loop_info = Some(LoopInfo {
                    label,
                    last_fired_at: None,
                });
            }
            Some("CronDelete") => {
                *cron_label = None;
                out.loop_info = None;
            }
            Some("ScheduleWakeup") if input.get("stop").and_then(Value::as_bool) == Some(true) => {
                out.loop_info = None;
            }
            Some("ScheduleWakeup") => {
                let last_fired_at = out.loop_info.as_ref().and_then(|l| l.last_fired_at);
                let label = out
                    .loop_info
                    .as_ref()
                    .map(|l| l.label.clone())
                    .or_else(prompt)
                    .unwrap_or_else(|| "Self-paced loop".to_string());
                out.loop_info = Some(LoopInfo {
                    label,
                    last_fired_at,
                });
            }
            _ => {}
        }
    }
}

// "*/5 * * * *" → "every 5m", "0 * * * *" → "hourly"; anything else as written.
fn cron_text(cron: &str) -> String {
    let f: Vec<&str> = cron.split_whitespace().collect();
    match f.as_slice() {
        ["*", "*", "*", "*", "*"] => "every minute".to_string(),
        [m, "*", "*", "*", "*"] if m.starts_with("*/") => format!("every {}m", &m[2..]),
        [m, h, "*", "*", "*"] if h.starts_with("*/") && m.parse::<u32>().is_ok() => {
            format!("every {}h", &h[2..])
        }
        [m, "*", "*", "*", "*"] if m.parse::<u32>().is_ok() => "hourly".to_string(),
        _ => format!("cron {cron}"),
    }
}

// A human prompt is string content, or text blocks with no tool_result.
// Wrappers like `<command-name>` or `<system-reminder>` are not the human.
fn human_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            let mut texts = Vec::new();
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("tool_result") => return None,
                    Some("text") => {
                        let t = b.get("text").and_then(Value::as_str).unwrap_or("");
                        if !is_wrapper(t) {
                            texts.push(t.to_string());
                        }
                    }
                    _ => {}
                }
            }
            texts.join("\n")
        }
        _ => return None,
    };
    if is_wrapper(&text) {
        return None;
    }
    let cleaned = pasted_tag_re().replace_all(&text, " ");
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

// Text that opens with a tag is a CLI wrapper (`<command-name>`,
// `<local-command-stdout>`, `<system-reminder>`, `<bash-input>` …), except a
// prompt that is only pasted content, which is the human's.
fn is_wrapper(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') && !t.starts_with("<pasted_content")
}

fn pasted_tag_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"</?pasted_content[^>]*>").expect("static regex"))
}

fn on_assistant(out: &mut Extract, open: &mut Vec<(String, CurrentTool)>, entry: &Value) {
    let Some(blocks) = entry["message"]["content"].as_array() else {
        return;
    };
    let mut texts = Vec::new();
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    if !t.trim().is_empty() {
                        texts.push(t.trim());
                    }
                }
            }
            Some("tool_use") => {
                let id = b
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = b
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let summary = tool_summary(&name, &b["input"]);
                open.push((id, CurrentTool { name, summary }));
            }
            _ => {}
        }
    }
    if !texts.is_empty() {
        out.last_reply = Some(cap(texts.join("\n\n").trim(), REPLY_MAX));
        out.last_reply_at = timestamp(entry);
    }
}

fn tool_summary(name: &str, input: &Value) -> String {
    let field = |k: &str| {
        input
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let summary = match name {
        "Bash" => field("command"),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let p = field("file_path");
            Path::new(&p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or(p)
        }
        "Grep" | "Glob" => field("pattern"),
        "Agent" | "Task" => field("description"),
        "WebFetch" => field("url"),
        _ => String::new(),
    };
    collapse(&summary, 200)
}

/// Collapse all whitespace runs to single spaces and cap at `max` chars.
pub fn collapse(s: &str, max: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    cap(&joined, max)
}

fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Parse an RFC 3339 timestamp (`2026-10-16T12:05:13.750Z`, or with a
/// `±HH:MM` offset) to Unix milliseconds. Hand-rolled to avoid a date crate
/// for one format.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    let mut ms = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
        let padded = format!("{digits:0<3}");
        ms = padded[..3].parse().ok()?;
        rest = &frac[digits.len()..];
    }
    let offset_min = match rest {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (oh * 60 + om)
        }
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    Some(secs * 1000 + ms)
}

// Howard Hinnant's days_from_civil: proleptic Gregorian date → days since 1970-01-01.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Resolves and caches transcripts. Paths are cached per sessionId only once
/// found: a fresh session has no file yet and must be looked up again.
pub struct TranscriptCache {
    paths: HashMap<String, PathBuf>,
    parsed: HashMap<PathBuf, (u64, Option<SystemTime>, Extract)>,
    pub tail_bytes: u64,
    pub tail_cap: u64,
}

impl Default for TranscriptCache {
    fn default() -> Self {
        TranscriptCache {
            paths: HashMap::new(),
            parsed: HashMap::new(),
            tail_bytes: TAIL_BYTES,
            tail_cap: TAIL_CAP,
        }
    }
}

impl TranscriptCache {
    pub fn get(&mut self, projects_dir: &Path, session_id: &str, cwd: &str) -> Option<Extract> {
        let path = self.resolve(projects_dir, session_id, cwd)?;
        let meta = std::fs::metadata(&path).ok()?;
        let key = (meta.len(), meta.modified().ok());
        if let Some((len, mtime, ex)) = self.parsed.get(&path) {
            if (*len, *mtime) == key {
                return Some(ex.clone());
            }
        }
        let (text, truncated) = read_tail_growing(&path, self.tail_bytes, self.tail_cap).ok()?;
        let previous = self.parsed.get(&path).map(|(.., ex)| ex.clone());
        let ex = match previous {
            // One line bigger than the cap: nothing readable, so the last
            // extract is still the best answer (cached against the new size
            // so it isn't re-read every poll).
            Some(prev) if truncated && text.is_empty() => prev,
            _ => {
                let mut ex = extract(&text);
                infer_prompt_before_tail(&mut ex, truncated);
                ex
            }
        };
        self.parsed.insert(path, (key.0, key.1, ex.clone()));
        Some(ex)
    }

    /// Drop cache entries for sessions no longer alive.
    pub fn retain(&mut self, live: &std::collections::HashSet<String>) {
        self.paths.retain(|sid, _| live.contains(sid));
        let keep: std::collections::HashSet<&PathBuf> = self.paths.values().collect();
        self.parsed.retain(|p, _| keep.contains(p));
    }

    fn resolve(&mut self, projects_dir: &Path, session_id: &str, cwd: &str) -> Option<PathBuf> {
        if let Some(p) = self.paths.get(session_id) {
            if p.exists() {
                return Some(p.clone());
            }
        }
        let file = format!("{session_id}.jsonl");
        // Claude Code names the project dir after the cwd with `/` and `.`
        // replaced by `-`; try that before scanning every project dir.
        let guess = projects_dir.join(encode_cwd(cwd)).join(&file);
        let found = if guess.exists() {
            Some(guess)
        } else {
            std::fs::read_dir(projects_dir)
                .ok()?
                .flatten()
                .map(|e| e.path().join(&file))
                .find(|p| p.exists())
        }?;
        self.paths.insert(session_id.to_string(), found.clone());
        Some(found)
    }
}

/// Long tool-heavy turns push the human prompt out of a 512 KB tail. When
/// the tail is truncated, holds a reply, and no prompt, the prompt is older
/// than everything in the tail, so the reply is newer than it: that is a
/// completed exchange (your turn), not a fresh session.
pub fn infer_prompt_before_tail(ex: &mut Extract, truncated: bool) {
    if truncated && !ex.has_prompt && ex.last_reply_at.is_some() {
        ex.has_prompt = true;
    }
}

pub fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        let path = format!(
            "{}/tests/fixtures/transcript.jsonl",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn fixture_extraction() {
        let ex = extract(&fixture());
        assert!(ex.has_prompt);
        let prompt = ex.last_prompt.as_deref().unwrap();
        assert!(
            prompt.starts_with("Use the AskUserQuestion tool"),
            "{prompt}"
        );
        assert_eq!(ex.last_reply.as_deref(), Some("You picked red."));
        assert!(ex.last_reply_at.unwrap() > ex.last_prompt_at.unwrap());
        assert_eq!(ex.current_tool, None);
        assert_eq!(ex.ai_title.as_deref(), Some("Create hello.txt file"));
        assert_eq!(
            ex.last_activity_at,
            parse_iso_ms("2026-10-16T12:05:51.965Z")
        );
    }

    #[test]
    fn tool_result_entries_are_not_prompts() {
        let jsonl = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"real prompt"}}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}
{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"x"},{"type":"text","text":"not human"}]}}"#;
        let ex = extract(jsonl);
        assert_eq!(ex.last_prompt.as_deref(), Some("real prompt"));
        assert_eq!(ex.last_prompt_at, parse_iso_ms("2026-01-01T00:00:00Z"));
        assert_eq!(ex.current_tool, None);
    }

    #[test]
    fn pending_tool_is_reported() {
        let jsonl = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"text","text":"do  it\n now"}]}}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"a","name":"Read","input":{"file_path":"/x/y/main.rs"}}]}}
{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"a"}]}}
{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"text","text":"Running tests"},{"type":"tool_use","id":"b","name":"Bash","input":{"command":"cargo test\n--quiet"}}]}}"#;
        let ex = extract(jsonl);
        assert_eq!(ex.last_prompt.as_deref(), Some("do it now"));
        assert_eq!(
            ex.current_tool,
            Some(CurrentTool {
                name: "Bash".into(),
                summary: "cargo test --quiet".into()
            })
        );
        assert_eq!(ex.last_reply.as_deref(), Some("Running tests"));
    }

    #[test]
    fn tool_summaries() {
        let v = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        assert_eq!(
            tool_summary("Edit", &v(r#"{"file_path":"/a/b.rs"}"#)),
            "b.rs"
        );
        assert_eq!(
            tool_summary("Grep", &v(r#"{"pattern":"fn main"}"#)),
            "fn main"
        );
        assert_eq!(
            tool_summary("Agent", &v(r#"{"description":"Explore"}"#)),
            "Explore"
        );
        assert_eq!(
            tool_summary("WebFetch", &v(r#"{"url":"https://x"}"#)),
            "https://x"
        );
        assert_eq!(tool_summary("TodoWrite", &v(r#"{"todos":[]}"#)), "");
    }

    #[test]
    fn wrappers_and_meta_are_skipped() {
        let jsonl = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"first"}}
{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":"<command-name>/clear</command-name>"}}
{"type":"user","timestamp":"2026-01-01T00:00:02Z","isMeta":true,"message":{"content":"Caveat: meta"}}
{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"text","text":"<system-reminder>x</system-reminder>"}]}}
{"type":"system","timestamp":"2026-01-01T00:00:09Z"}"#;
        let ex = extract(jsonl);
        assert_eq!(ex.last_prompt.as_deref(), Some("first"));
        // Wrapper entries still count as activity; `system` entries do not.
        assert_eq!(ex.last_activity_at, parse_iso_ms("2026-01-01T00:00:03Z"));
    }

    #[test]
    fn last_prompt_entry_is_a_fallback_only() {
        let jsonl = r#"{"type":"last-prompt","lastPrompt":"scrolled  away"}"#;
        let ex = extract(jsonl);
        assert_eq!(ex.last_prompt.as_deref(), Some("scrolled away"));
        assert!(!ex.has_prompt);
        assert_eq!(ex.last_prompt_at, None);
    }

    #[test]
    fn prompt_and_reply_are_capped() {
        let long = "x".repeat(1000);
        let jsonl = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"{long}\"}}}}\n{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{long}\"}}]}}}}"
        );
        let ex = extract(&jsonl);
        assert_eq!(ex.last_prompt.unwrap().chars().count(), PROMPT_MAX);
        assert_eq!(ex.last_reply.unwrap().chars().count(), REPLY_MAX);
    }

    #[test]
    fn iso_parsing() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(
            parse_iso_ms("2026-10-16T12:05:13.750Z"),
            Some(1_792_152_313_750)
        );
        assert_eq!(parse_iso_ms("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_iso_ms("nope"), None);
    }

    #[test]
    fn tail_drops_partial_first_line() {
        let dir = std::env::temp_dir().join(format!("mc-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, "aaaaaaaaaa\nbbb\nccc\n").unwrap();
        // 19 bytes; the last 10 start inside "aaaaaaaaaa", which is dropped.
        assert_eq!(read_tail(&path, 10).unwrap(), ("bbb\nccc\n".into(), true));
        assert_eq!(read_tail(&path, 6).unwrap(), ("ccc\n".into(), true));
        // Windows that start exactly on a line keep that line.
        assert_eq!(read_tail(&path, 8).unwrap(), ("bbb\nccc\n".into(), true));
        assert_eq!(read_tail(&path, 4).unwrap(), ("ccc\n".into(), true));
        assert_eq!(
            read_tail(&path, 1000).unwrap(),
            ("aaaaaaaaaa\nbbb\nccc\n".into(), false)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cache_finds_transcript_and_does_not_cache_misses() {
        let root = std::env::temp_dir().join(format!("mc-proj-{}", std::process::id()));
        let proj = root.join(encode_cwd("/Users/me/app"));
        std::fs::create_dir_all(&proj).unwrap();
        let mut cache = TranscriptCache::default();
        assert!(cache.get(&root, "s1", "/Users/me/app").is_none());
        std::fs::write(proj.join("s1.jsonl"), fixture()).unwrap();
        let ex = cache
            .get(&root, "s1", "/Users/me/app")
            .expect("found after creation");
        assert_eq!(ex.last_reply.as_deref(), Some("You picked red."));
        // Elsewhere-named dir is found by scanning.
        let other = root.join("odd-name");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("s2.jsonl"), fixture()).unwrap();
        assert!(cache.get(&root, "s2", "/nowhere").is_some());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A 600 KB Write or tool_result as the newest line is longer than the
    /// tail window. The window grows (to a cap) instead of coming back empty,
    /// and past the cap the previous extract is kept.
    #[test]
    fn huge_last_line_does_not_erase_the_extract() {
        let root = std::env::temp_dir().join(format!("mc-huge-{}", std::process::id()));
        let proj = root.join(encode_cwd("/w"));
        std::fs::create_dir_all(&proj).unwrap();
        let path = proj.join("s.jsonl");
        let mut cache = TranscriptCache {
            tail_bytes: 64,
            tail_cap: 4096,
            ..Default::default()
        };
        let big = |n: usize| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t","name":"Write","input":{{"file_path":"/x","content":"{}"}}}}]}}}}"#,
                "x".repeat(n)
            )
        };
        let base = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"write it"}}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"text","text":"Writing."}]}}
"#;
        std::fs::write(&path, format!("{base}{}\n", big(1000))).unwrap();
        let ex = cache.get(&root, "s", "/w").unwrap();
        assert_eq!(ex.last_prompt.as_deref(), Some("write it"));
        assert_eq!(ex.last_reply.as_deref(), Some("Writing."));
        assert_eq!(ex.current_tool.as_ref().unwrap().name, "Write");
        // Bigger than the cap: the extract from before stands.
        std::fs::write(&path, format!("{base}{}\n{}\n", big(1000), big(9000))).unwrap();
        let again = cache.get(&root, "s", "/w").unwrap();
        assert_eq!(again, ex);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn compact_summary_and_pasted_content() {
        let jsonl = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"\n\n<pasted_content id=\"a1\">\nlog line\n</pasted_content>"}}
{"type":"user","timestamp":"2026-01-01T00:00:05Z","isCompactSummary":true,"isVisibleInTranscriptOnly":true,"message":{"content":"This session is being continued from a previous conversation"}}"#;
        let ex = extract(jsonl);
        assert_eq!(ex.last_prompt.as_deref(), Some("log line"));
        assert_eq!(ex.last_prompt_at, parse_iso_ms("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn prompt_before_truncated_tail_is_inferred() {
        let reply_only = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"text","text":"done"}]}}"#;
        let mut ex = extract(reply_only);
        infer_prompt_before_tail(&mut ex, false);
        assert!(!ex.has_prompt, "whole file read: no prompt means none");
        infer_prompt_before_tail(&mut ex, true);
        assert!(ex.has_prompt);
        let mut nothing = Extract::default();
        infer_prompt_before_tail(&mut nothing, true);
        assert!(!nothing.has_prompt);
    }

    #[test]
    fn scheduled_runs_mark_a_loop() {
        // What `/loop 5m /kanban-worker` leaves behind: the CronCreate, then
        // fires tagged turnOrigin "scheduled" with a meta skill body.
        let jsonl = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"<command-message>loop</command-message>\n<command-name>/loop</command-name>\n<command-args>5m /kanban-worker</command-args>"}}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"c1","name":"CronCreate","input":{"cron":"*/5 * * * *","prompt":"/kanban-worker","recurring":true}}]}}
{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"Scheduled recurring job e6801d8f"}]}}
{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"text","text":"Loop started."}]}}"#;
        let ex = extract(jsonl);
        assert_eq!(
            ex.loop_info,
            Some(LoopInfo {
                label: "/kanban-worker · every 5m".into(),
                last_fired_at: None
            })
        );

        let fire = r#"{"type":"user","timestamp":"2026-01-01T00:05:00Z","turnOrigin":"scheduled","scheduledTaskId":"e6801d8f","wakeupSource":"schedule_wakeup","message":{"content":"<command-message>kanban-worker</command-message>\n<command-name>/kanban-worker</command-name>"}}
{"type":"user","timestamp":"2026-01-01T00:05:00Z","isMeta":true,"wakeupSource":"schedule_wakeup","message":{"content":[{"type":"text","text":"Base directory for this skill: /x"}]}}
{"type":"assistant","timestamp":"2026-01-01T00:05:09Z","message":{"content":[{"type":"text","text":"Nothing new."}]}}"#;
        let ex = extract(&format!("{jsonl}\n{fire}"));
        let l = ex.loop_info.unwrap();
        assert_eq!(l.label, "/kanban-worker · every 5m");
        assert_eq!(l.last_fired_at, parse_iso_ms("2026-01-01T00:05:00Z"));
        assert_eq!(ex.last_reply.as_deref(), Some("Nothing new."));

        // CronCreate scrolled out of the tail: the fire alone names it.
        let ex = extract(fire);
        assert_eq!(ex.loop_info.unwrap().label, "/kanban-worker");
        assert!(!ex.has_prompt);
    }

    #[test]
    fn plain_text_fires_are_not_human_prompts() {
        let jsonl = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"real prompt"}}
{"type":"user","timestamp":"2026-01-01T00:05:00Z","turnOrigin":"scheduled","message":{"content":"check   the deploy"}}"#;
        let ex = extract(jsonl);
        assert_eq!(ex.last_prompt.as_deref(), Some("real prompt"));
        assert_eq!(ex.loop_info.unwrap().label, "check the deploy");
    }

    #[test]
    fn deleting_or_stopping_ends_the_loop() {
        let fire = r#"{"type":"user","timestamp":"2026-01-01T00:05:00Z","turnOrigin":"scheduled","message":{"content":"<command-name>/babysit</command-name>"}}"#;
        let delete = r#"{"type":"assistant","timestamp":"2026-01-01T00:06:00Z","message":{"content":[{"type":"tool_use","id":"d1","name":"CronDelete","input":{"id":"e6801d8f"}}]}}"#;
        let stop = r#"{"type":"assistant","timestamp":"2026-01-01T00:06:00Z","message":{"content":[{"type":"tool_use","id":"w1","name":"ScheduleWakeup","input":{"stop":true}}]}}"#;
        assert!(extract(fire).loop_info.is_some());
        assert_eq!(extract(&format!("{fire}\n{delete}")).loop_info, None);
        assert_eq!(extract(&format!("{fire}\n{stop}")).loop_info, None);
        // A one-shot reminder isn't a loop.
        let once = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"c1","name":"CronCreate","input":{"cron":"30 14 * * *","prompt":"ping","recurring":false}}]}}"#;
        assert_eq!(extract(once).loop_info, None);
    }

    #[test]
    fn self_paced_loops_start_at_their_first_wakeup() {
        let wake = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"w1","name":"ScheduleWakeup","input":{"delaySeconds":1200,"prompt":"/babysit-prs","reason":"watching CI"}}]}}"#;
        assert_eq!(
            extract(wake).loop_info,
            Some(LoopInfo {
                label: "/babysit-prs".into(),
                last_fired_at: None
            })
        );
    }

    #[test]
    fn cron_intervals_read_as_words() {
        assert_eq!(cron_text("*/5 * * * *"), "every 5m");
        assert_eq!(cron_text("* * * * *"), "every minute");
        assert_eq!(cron_text("0 */2 * * *"), "every 2h");
        assert_eq!(cron_text("7 * * * *"), "hourly");
        assert_eq!(cron_text("0 9 * * 1-5"), "cron 0 9 * * 1-5");
    }

    #[test]
    fn encode_cwd_matches_claude_dirs() {
        assert_eq!(
            encode_cwd("/Users/dev/code/.claude"),
            "-Users-dev-code--claude"
        );
    }
}
