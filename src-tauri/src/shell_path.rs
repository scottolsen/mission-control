//! Adopt the user's login-shell PATH at startup. A bundled app launched from
//! Finder or the Dock inherits launchd's bare PATH (`/usr/bin:/bin:…`), which
//! misses Homebrew — so `overmind`, `tmux`, and every Procfile tool resolve in
//! `tauri dev` but not in the release build. Setting PATH once, before any
//! thread or child process starts, fixes every spawn at once.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const START: &str = "__MC_PATH_START__";
const END: &str = "__MC_PATH_END__";
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

// Common install dirs appended when the shell probe fails or omits them, so a
// broken or slow profile still leaves Homebrew reachable.
const FALLBACK_DIRS: &[&str] = &["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"];

// Merge the login shell's PATH into this process's environment. Must run
// before any other thread starts: set_var is not thread-safe.
pub(crate) fn inherit_login_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let login = login_shell_path().unwrap_or_default();
    std::env::set_var("PATH", merge_paths(&login, &current));
}

// Ask an interactive login shell for its PATH, so both profile and rc files
// (where mise/Homebrew hooks usually live) are sourced. Markers fence the value
// off from anything the profile prints.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let script = format!("printf '%s%s%s' '{START}' \"$PATH\" '{END}'");
    let mut cmd = Command::new(shell);
    cmd.args(["-l", "-i", "-c", &script]);
    probe(cmd, SHELL_TIMEOUT)
}

// Run `cmd` and return the marked PATH it prints, waiting at most `timeout`.
// Stdout is read on a helper thread because the shell exiting doesn't end the
// read: a background job started by a profile (ssh-agent, gpg-agent, async
// prompt helpers) inherits the pipe and holds it open. So the reader returns as
// soon as the end marker arrives, and on timeout the whole process group is
// killed. A reader stuck behind a daemon that escaped the group is abandoned;
// it never touches the environment, so the caller's set_var stays safe.
fn probe(mut cmd: Command, timeout: Duration) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read_until_marker(stdout));
    });

    let path = rx.recv_timeout(timeout).ok().flatten();
    if path.is_none() {
        // The group id is the child's pid (process_group(0) above).
        unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };
    }
    // Reap in the background so startup never waits on the shell's exit.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    path
}

// Read until the end marker shows up (or EOF), not until the pipe closes.
fn read_until_marker(mut reader: impl Read) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return parse_marked_path(&String::from_utf8_lossy(&buf)),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(path) = parse_marked_path(&String::from_utf8_lossy(&buf)) {
            return Some(path);
        }
    }
}

fn parse_marked_path(output: &str) -> Option<String> {
    let start = output.find(START)? + START.len();
    let len = output[start..].find(END)?;
    let path = output[start..start + len].trim();
    (!path.is_empty()).then(|| path.to_string())
}

// Login-shell entries first, then the inherited ones, then fallbacks — first
// occurrence wins and empty entries are dropped.
fn merge_paths(login: &str, current: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    login
        .split(':')
        .chain(current.split(':'))
        .chain(FALLBACK_DIRS.iter().copied())
        .filter(|dir| !dir.is_empty() && seen.insert(*dir))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn probe_returns_once_marked_even_if_a_background_job_holds_stdout() {
        let started = Instant::now();
        let path = probe(
            sh(&format!("sleep 30 & printf '%s' '{START}/x{END}'")),
            Duration::from_secs(3),
        );
        assert_eq!(path.as_deref(), Some("/x"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn probe_gives_up_at_the_timeout_when_no_marker_arrives() {
        let started = Instant::now();
        assert_eq!(probe(sh("sleep 30"), Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn marked_path_ignores_profile_noise_around_it() {
        let output = format!("Welcome!\n{START}/opt/homebrew/bin:/usr/bin{END}\nbye");
        assert_eq!(
            parse_marked_path(&output).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn marked_path_is_none_without_both_markers() {
        assert_eq!(parse_marked_path("zsh: command not found"), None);
        assert_eq!(parse_marked_path(&format!("{START}/usr/bin")), None);
    }

    #[test]
    fn marked_path_is_none_when_empty() {
        assert_eq!(parse_marked_path(&format!("{START}{END}")), None);
    }

    #[test]
    fn merge_puts_login_entries_first_without_duplicates() {
        let merged = merge_paths("/opt/homebrew/bin:/usr/bin", "/usr/bin:/bin");
        assert!(merged.starts_with("/opt/homebrew/bin:/usr/bin:/bin:"));
        assert_eq!(merged.matches("/usr/bin:").count(), 1);
    }

    #[test]
    fn merge_adds_fallbacks_when_login_probe_failed() {
        let merged = merge_paths("", "/usr/bin:/bin");
        assert!(merged.starts_with("/usr/bin:/bin:"));
        assert!(merged.contains("/opt/homebrew/bin"));
        assert!(!merged.contains("::"));
    }
}
