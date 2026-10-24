//! Claude Code's process registry: one `~/.claude/sessions/<pid>.json` per
//! running CLI. It is the authoritative list of sessions and carries the
//! status hook's view (`busy` / `idle` / `waiting`) plus the tmux pane the
//! process was started in. Files outlive crashed processes, so each pid is
//! checked for liveness and, since pids get reused, for being Claude.

use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegStatus {
    Busy,
    Idle,
    Waiting,
    Unknown,
}

impl RegStatus {
    fn parse(s: Option<&str>) -> RegStatus {
        match s {
            Some("busy") => RegStatus::Busy,
            Some("idle") => RegStatus::Idle,
            Some("waiting") => RegStatus::Waiting,
            _ => RegStatus::Unknown,
        }
    }
}

/// The `tmux` field split into its parts: `"-claude:@5.%13"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxRef {
    pub session: String,
    pub window_id: String,
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegEntry {
    pub pid: i64,
    pub session_id: Option<String>,
    pub cwd: String,
    pub tmux: Option<TmuxRef>,
    pub status: RegStatus,
    pub status_updated_at: Option<i64>,
    pub started_at: Option<i64>,
    pub version: Option<String>,
    pub name: Option<String>,
}

/// Parse one registry file's JSON. `None` when it lacks a pid.
pub fn parse_entry(json: &str) -> Option<RegEntry> {
    let v: Value = serde_json::from_str(json).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let n = |k: &str| v.get(k).and_then(Value::as_i64);
    Some(RegEntry {
        pid: n("pid")?,
        session_id: s("sessionId"),
        cwd: s("cwd").unwrap_or_default(),
        tmux: s("tmux").as_deref().and_then(parse_tmux_ref),
        status: RegStatus::parse(v.get("status").and_then(Value::as_str)),
        status_updated_at: n("statusUpdatedAt"),
        started_at: n("startedAt"),
        version: s("version"),
        name: s("name").filter(|n| !n.trim().is_empty()),
    })
}

/// Split `"<session>:@<win>.%<pane>"`. The window/pane half never contains
/// `:`, so split on the last one; session names may hold anything else.
pub fn parse_tmux_ref(s: &str) -> Option<TmuxRef> {
    let (session, rest) = s.rsplit_once(':')?;
    let (win, pane) = rest.split_once('.')?;
    if !win.starts_with('@') || !pane.starts_with('%') {
        return None;
    }
    Some(TmuxRef {
        session: session.to_string(),
        window_id: win.to_string(),
        pane_id: pane.to_string(),
    })
}

/// True unless the pid definitely doesn't exist. EPERM (someone else's
/// process) still means alive.
pub fn pid_alive(pid: i64) -> bool {
    if pid <= 0 || pid > i64::from(i32::MAX) {
        return false;
    }
    // SAFETY: kill with signal 0 only performs the existence/permission check.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The process's short name (`proc_name`), when it can be read. Claude Code
/// runs as a binary named after its version (`2.1.296`), the same string
/// tmux reports as the pane's command.
#[cfg(target_os = "macos")]
fn process_name(pid: i64) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for `buf.len()` bytes and proc_name writes
    // at most that many, returning the length written (0 or less on error).
    let n = unsafe {
        libc::proc_name(
            pid as libc::c_int,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
        )
    };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn process_name(_pid: i64) -> Option<String> {
    None
}

/// Alive, and (when its name is readable) a Claude process. A file whose
/// pid was reused by another program would otherwise be a ghost session.
pub fn is_live_claude(pid: i64) -> bool {
    pid_alive(pid) && process_name(pid).is_none_or(|n| crate::tmux::is_claude_command(&n))
}

/// Every registry entry in `dir` whose pid passes `live`. Missing dir → empty.
pub fn load(dir: &Path, live: fn(i64) -> bool) -> Vec<RegEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .map(|e| e.path())
        // `<pid>.<hash>.key` files sit alongside; only `<pid>.json` matters.
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|text| parse_entry(&text))
        .filter(|e| live(e.pid))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_entry() {
        let e = parse_entry(r#"{"pid":10381,"sessionId":"e18a","cwd":"/Users/s/dotfiles","startedAt":1790944568148,"version":"2.1.287","kind":"interactive","tmux":"dotfiles-local:@8.%22","name":"dotfiles-local-2e","status":"idle","statusUpdatedAt":1790944568016}"#).unwrap();
        assert_eq!(e.pid, 10381);
        assert_eq!(e.session_id.as_deref(), Some("e18a"));
        assert_eq!(e.status, RegStatus::Idle);
        assert_eq!(e.status_updated_at, Some(1790944568016));
        assert_eq!(e.started_at, Some(1790944568148));
        assert_eq!(
            e.tmux,
            Some(TmuxRef {
                session: "dotfiles-local".into(),
                window_id: "@8".into(),
                pane_id: "%22".into()
            })
        );
    }

    #[test]
    fn missing_tmux_and_unknown_status() {
        let e = parse_entry(r#"{"pid":5,"cwd":"/x","status":"compacting"}"#).unwrap();
        assert_eq!(e.tmux, None);
        assert_eq!(e.status, RegStatus::Unknown);
        assert_eq!(e.session_id, None);
        let e = parse_entry(r#"{"pid":5,"cwd":"/x"}"#).unwrap();
        assert_eq!(e.status, RegStatus::Unknown);
        assert!(parse_entry(r#"{"cwd":"/x"}"#).is_none());
        assert!(parse_entry("not json").is_none());
    }

    #[test]
    fn tmux_refs_with_odd_session_names() {
        let r = parse_tmux_ref("-claude:@5.%13").unwrap();
        assert_eq!(
            (r.session.as_str(), r.window_id.as_str(), r.pane_id.as_str()),
            ("-claude", "@5", "%13")
        );
        let r = parse_tmux_ref("Checkout redesign — phase 2:@16.%52").unwrap();
        assert_eq!(r.session, "Checkout redesign — phase 2");
        assert_eq!(r.pane_id, "%52");
        assert_eq!(parse_tmux_ref("garbage"), None);
        assert_eq!(parse_tmux_ref("s:5.13"), None);
    }

    #[test]
    fn liveness() {
        assert!(pid_alive(std::process::id() as i64));
        assert!(pid_alive(1)); // launchd: EPERM, still alive
        assert!(!pid_alive(0));
        assert!(!pid_alive(-3));
    }

    /// A reused pid belongs to some other program; only a process whose
    /// name is Claude's version (or `claude`) keeps its registry entry.
    #[test]
    fn claude_check_reads_the_process_name() {
        assert!(
            !is_live_claude(std::process::id() as i64),
            "the test binary"
        );
        assert!(!is_live_claude(2147480000));
        let dir = std::env::temp_dir().join(format!("mc-proc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("2.1.999");
        std::fs::copy("/bin/sleep", &fake).unwrap();
        let mut child = std::process::Command::new(&fake).arg("5").spawn().unwrap();
        let ok = is_live_claude(i64::from(child.id()));
        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(ok, "a process named like a Claude version");
    }

    #[test]
    fn load_skips_dead_pids_and_key_files() {
        let dir = std::env::temp_dir().join(format!("mc-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.join(format!("{me}.json")),
            format!(r#"{{"pid":{me},"cwd":"/a"}}"#),
        )
        .unwrap();
        std::fs::write(dir.join(format!("{me}.abc.key")), "secret").unwrap();
        // pid_max on macOS is 99999; this pid can't exist.
        std::fs::write(
            dir.join("2147480000.json"),
            r#"{"pid":2147480000,"cwd":"/b"}"#,
        )
        .unwrap();
        let entries = load(&dir, pid_alive);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cwd, "/a");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(load(&dir, pid_alive).is_empty());
    }
}
