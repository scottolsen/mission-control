//! Thin tmux wrappers. Everything targets panes and windows by id (`%N`,
//! `@N`), never by session name: names like `-claude` parse as flags and
//! `Checkout redesign — phase 2` needs quoting, while ids are always safe.
//!
//! Commands go through a `Runner` so the argument lists and the output
//! parsing are unit-testable without a tmux server; tests never execute
//! anything that would touch the user's real panes.

use regex::Regex;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long any child process (tmux, git, osascript) may run. A wedged tmux
/// server or a git on a hung network mount would otherwise stall the poll,
/// and every send queued behind it, for good.
pub const DEADLINE: Duration = Duration::from_secs(3);

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait Runner: Send + Sync {
    fn run(&self, args: &[String]) -> std::io::Result<Output>;
}

/// Runs the real `tmux` binary.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, args: &[String]) -> std::io::Result<Output> {
        let mut cmd = Command::new("tmux");
        cmd.args(args);
        if needs_utf8_locale(|k| std::env::var_os(k).is_some()) {
            cmd.env("LC_CTYPE", "UTF-8");
        }
        let out = output_with_deadline(&mut cmd, DEADLINE)?;
        Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// An app launched from Finder or the Dock has no locale at all, and tmux in
/// the C locale prints every tab in a `-F` format as `_` and every `❯` as
/// `_`: no pane row would parse and no menu would be found. Only filled in
/// when the user has set none of the variables, so a real locale wins.
pub fn needs_utf8_locale(is_set: impl Fn(&str) -> bool) -> bool {
    !["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|k| is_set(k))
}

/// `Command::output` with a deadline: past it the child is killed and the
/// error is `TimedOut`. Pipes are drained on their own threads so a chatty
/// child can't fill one and stall.
pub fn output_with_deadline(
    cmd: &mut Command,
    deadline: Duration,
) -> std::io::Result<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out after {:.1}s", deadline.as_secs_f32()),
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub session: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_id: String,
    pub pane_index: u32,
    pub pane_pid: i64,
    pub command: String,
    pub width: u32,
    pub height: u32,
    pub attached: bool,
    pub current_path: String,
    pub title: String,
}

// `pane_title` goes last and the line is split with a field limit, so a tab
// inside a title can't shift the fixed columns.
const PANE_FIELDS: &[&str] = &[
    "session_name",
    "window_id",
    "window_index",
    "window_name",
    "pane_id",
    "pane_index",
    "pane_pid",
    "pane_current_command",
    "pane_width",
    "pane_height",
    "session_attached",
    "pane_current_path",
    "pane_title",
];

pub fn list_panes_args() -> Vec<String> {
    let format = PANE_FIELDS
        .iter()
        .map(|f| format!("#{{{f}}}"))
        .collect::<Vec<_>>()
        .join("\t");
    strings(&["list-panes", "-a", "-F", &format])
}

/// The same fields for the panes of one pane's window: a cheap way to read a
/// single pane right before sending to it.
pub fn pane_row_args(pane_id: &str) -> Vec<String> {
    let mut args = list_panes_args();
    args[1] = "-t".into();
    args.insert(2, pane_id.into());
    args
}

/// One pane's row, or `None` when it (or the server) is gone.
pub fn find_pane(r: &dyn Runner, pane_id: &str) -> Result<Option<Pane>, String> {
    let out = r
        .run(&pane_row_args(pane_id))
        .map_err(|e| format!("tmux: {e}"))?;
    if !out.ok {
        return Ok(None);
    }
    Ok(parse_panes(&out.stdout)
        .into_iter()
        .find(|p| p.pane_id == pane_id))
}

pub fn parse_panes(stdout: &str) -> Vec<Pane> {
    stdout.lines().filter_map(parse_pane_line).collect()
}

fn parse_pane_line(line: &str) -> Option<Pane> {
    let f: Vec<&str> = line.splitn(PANE_FIELDS.len(), '\t').collect();
    if f.len() != PANE_FIELDS.len() {
        return None;
    }
    Some(Pane {
        session: f[0].to_string(),
        window_id: f[1].to_string(),
        window_index: f[2].parse().ok()?,
        window_name: f[3].to_string(),
        pane_id: f[4].to_string(),
        pane_index: f[5].parse().ok()?,
        pane_pid: f[6].parse().ok()?,
        command: f[7].to_string(),
        width: f[8].parse().ok()?,
        height: f[9].parse().ok()?,
        attached: f[10].parse::<u32>().is_ok_and(|n| n > 0),
        current_path: f[11].to_string(),
        title: f[12].to_string(),
    })
}

/// Claude Code renames its process to its version, so a Claude pane's
/// `pane_current_command` looks like `2.1.296`.
pub fn is_claude_command(cmd: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\d+\.\d+\.\d+$").expect("static regex"))
        .is_match(cmd)
        || cmd == "claude"
}

// "no server running on /tmp/tmux-504/default" or "error connecting to …":
// tmux simply isn't up, which is a state, not an error.
pub fn is_no_server(stderr: &str) -> bool {
    stderr.contains("no server running") || stderr.contains("error connecting to")
}

/// `Ok(None)` when no tmux server is running.
pub fn list_panes(r: &dyn Runner) -> Result<Option<Vec<Pane>>, String> {
    let out = r
        .run(&list_panes_args())
        .map_err(|e| format!("tmux: {e}"))?;
    if out.ok {
        let panes = parse_panes(&out.stdout);
        // Rows that all fail to parse mean tmux printed something we can't
        // read; showing every session as "outside tmux" would hide that.
        if panes.is_empty() && !out.stdout.trim().is_empty() {
            return Err("tmux list-panes printed rows that didn't parse".into());
        }
        return Ok(Some(panes));
    }
    if is_no_server(&out.stderr) {
        return Ok(None);
    }
    Err(format!("tmux list-panes failed: {}", out.stderr.trim()))
}

pub fn valid_pane_id(id: &str) -> bool {
    id.len() > 1 && id.starts_with('%') && id[1..].bytes().all(|b| b.is_ascii_digit())
}

pub fn valid_window_id(id: &str) -> bool {
    id.len() > 1 && id.starts_with('@') && id[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Plain-text capture of the visible screen, used for menu detection.
pub fn detect_capture_args(pane_id: &str) -> Vec<String> {
    strings(&["capture-pane", "-p", "-J", "-t", pane_id])
}

/// ANSI capture with 200 lines of history, for the UI's pane preview.
pub fn preview_capture_args(pane_id: &str) -> Vec<String> {
    strings(&[
        "capture-pane",
        "-e",
        "-p",
        "-J",
        "-S",
        "-200",
        "-t",
        pane_id,
    ])
}

pub fn capture(r: &dyn Runner, args: &[String]) -> Result<String, String> {
    let out = r.run(args).map_err(|e| format!("tmux: {e}"))?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(format!("tmux capture-pane failed: {}", out.stderr.trim()))
    }
}

pub fn list_clients_args() -> Vec<String> {
    strings(&["list-clients", "-F", "#{client_tty}\t#{client_activity}"])
}

/// The tty of the client with the newest activity: the one the user is
/// looking at.
pub fn pick_client(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|l| {
            let (tty, act) = l.split_once('\t')?;
            Some((tty.trim(), act.trim().parse::<i64>().unwrap_or(0)))
        })
        .filter(|(tty, _)| !tty.is_empty())
        .max_by_key(|(_, act)| *act)
        .map(|(tty, _)| tty.to_string())
}

/// The tmux commands that bring `pane_id` to the front of client `tty`.
pub fn jump_commands(tty: &str, window_id: &str, pane_id: &str) -> Vec<Vec<String>> {
    vec![
        strings(&["switch-client", "-c", tty, "-t", pane_id]),
        strings(&["select-window", "-t", window_id]),
        strings(&["select-pane", "-t", pane_id]),
    ]
}

/// Literal text, typed as-is (`-l`). `--` keeps text starting with `-` from
/// being read as a flag.
pub fn send_literal_args(pane_id: &str, text: &str) -> Vec<String> {
    strings(&["send-keys", "-t", pane_id, "-l", "--", text])
}

/// A single named key (`Enter`, `Escape`, or a digit).
pub fn send_key_args(pane_id: &str, key: &str) -> Vec<String> {
    strings(&["send-keys", "-t", pane_id, key])
}

/// Keys given as hex bytes (`-H 3b` is `;`), for characters tmux would
/// otherwise parse.
pub fn send_hex_args(pane_id: &str, bytes: &[&str]) -> Vec<String> {
    let mut args = strings(&["send-keys", "-t", pane_id, "-H"]);
    args.extend(bytes.iter().map(|b| b.to_string()));
    args
}

/// Window id and geometry of one pane, for jump and capture.
pub fn pane_info_args(pane_id: &str) -> Vec<String> {
    strings(&[
        "display-message",
        "-p",
        "-t",
        pane_id,
        "#{window_id}\t#{pane_width}\t#{pane_height}",
    ])
}

#[derive(Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub window_id: String,
    pub width: u32,
    pub height: u32,
}

pub fn parse_pane_info(stdout: &str) -> Option<PaneInfo> {
    let mut f = stdout.trim_end_matches('\n').split('\t');
    let info = PaneInfo {
        window_id: f.next()?.to_string(),
        width: f.next()?.parse().ok()?,
        height: f.next()?.parse().ok()?,
    };
    valid_window_id(&info.window_id).then_some(info)
}

pub fn pane_info(r: &dyn Runner, pane_id: &str) -> Result<PaneInfo, String> {
    let out = r
        .run(&pane_info_args(pane_id))
        .map_err(|e| format!("tmux: {e}"))?;
    if !out.ok {
        return Err(format!("Pane {pane_id} not found: {}", out.stderr.trim()));
    }
    parse_pane_info(&out.stdout).ok_or_else(|| format!("Pane {pane_id}: unexpected tmux output"))
}

pub fn run_ok(r: &dyn Runner, args: &[String]) -> Result<(), String> {
    let out = r.run(args).map_err(|e| format!("tmux: {e}"))?;
    if out.ok {
        Ok(())
    } else {
        Err(format!("tmux {} failed: {}", args[0], out.stderr.trim()))
    }
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every call and answers from a canned table keyed by the
    /// first argument (the tmux subcommand).
    #[derive(Default)]
    pub struct FakeRunner {
        pub calls: Mutex<Vec<Vec<String>>>,
        pub replies: Vec<(&'static str, bool, String, String)>,
    }

    impl Runner for FakeRunner {
        fn run(&self, args: &[String]) -> std::io::Result<Output> {
            self.calls.lock().unwrap().push(args.to_vec());
            let reply = self.replies.iter().find(|(cmd, ..)| *cmd == args[0]);
            Ok(match reply {
                Some((_, ok, out, err)) => Output {
                    ok: *ok,
                    stdout: out.clone(),
                    stderr: err.clone(),
                },
                None => Output {
                    ok: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            })
        }
    }

    pub const PANES: &str = "-claude\t@5\t1\t2.1.296\t%12\t1\t73717\tvim\t437\t39\t0\t/Users/dev/code\tvim
-claude\t@5\t1\t2.1.296\t%13\t2\t73721\t2.1.296\t219\t37\t0\t/Users/dev/code/.claude\t✳ Claude Code
Checkout redesign — phase 2\t@16\t1\t2.1.294\t%52\t2\t45013\t2.1.294\t364\t64\t1\t/Users/dev/Downloads/Checkout redesign — phase 2\t✳ Checkout Redesign Phase 2
notes\t@2\t1\tzsh\t%7\t2\t21758\tzsh\t219\t39\t0\t/Users/dev/code/notes\tdev@laptop:~/code/notes
weird\t@9\t3\tw\t%90\t0\t1\t2.1.300\t80\t24\t0\t/tmp/bare\ttitle\twith\ttabs";

    #[test]
    fn children_are_killed_at_the_deadline() {
        let t = std::time::Instant::now();
        let mut slow = Command::new("sleep");
        slow.arg("10");
        let err = output_with_deadline(&mut slow, Duration::from_millis(200)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        let mut quick = Command::new("echo");
        quick.arg("hi");
        let out = output_with_deadline(&mut quick, Duration::from_secs(3)).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hi\n");
    }

    #[test]
    fn parses_list_panes_output() {
        let panes = parse_panes(PANES);
        assert_eq!(panes.len(), 5);
        let p = &panes[1];
        assert_eq!(p.session, "-claude");
        assert_eq!(p.window_id, "@5");
        assert_eq!(p.pane_id, "%13");
        assert_eq!(p.pane_pid, 73721);
        assert_eq!(p.command, "2.1.296");
        assert_eq!(p.title, "✳ Claude Code");
        assert_eq!((p.width, p.height), (219, 37));
        assert!(!p.attached);
        let inc = &panes[2];
        assert_eq!(inc.session, "Checkout redesign — phase 2");
        assert!(inc.attached);
        assert_eq!(
            inc.current_path,
            "/Users/dev/Downloads/Checkout redesign — phase 2"
        );
        assert_eq!(panes[4].title, "title\twith\ttabs");
    }

    #[test]
    fn malformed_lines_are_skipped() {
        assert!(parse_panes("only\tthree\tfields\n\n").is_empty());
    }

    #[test]
    fn utf8_locale_only_fills_a_missing_locale() {
        assert!(needs_utf8_locale(|_| false));
        assert!(!needs_utf8_locale(|k| k == "LANG"));
        assert!(!needs_utf8_locale(|k| k == "LC_ALL"));
        assert!(!needs_utf8_locale(|k| k == "LC_CTYPE"));
    }

    #[test]
    fn unparseable_rows_are_an_error() {
        // What tmux prints in the C locale: tabs come out as underscores.
        let r = FakeRunner {
            replies: vec![(
                "list-panes",
                true,
                "big-ring-labs_@5_1_vim_%13_0_5522_vim_437_39_0_/tmp_vim\n".into(),
                String::new(),
            )],
            ..Default::default()
        };
        assert!(list_panes(&r).unwrap_err().contains("didn't parse"));
        let r = FakeRunner::default();
        assert_eq!(list_panes(&r), Ok(Some(vec![])));
    }

    #[test]
    fn format_puts_title_last() {
        let args = list_panes_args();
        assert_eq!(&args[..3], &["list-panes", "-a", "-F"]);
        let one = pane_row_args("%13");
        assert_eq!(&one[..4], &["list-panes", "-t", "%13", "-F"]);
        assert_eq!(one[4], args[3]);
        assert!(args[3].starts_with("#{session_name}\t"));
        assert!(args[3].ends_with("\t#{pane_title}"));
    }

    #[test]
    fn claude_command_detection() {
        assert!(is_claude_command("2.1.296"));
        assert!(is_claude_command("claude"));
        assert!(!is_claude_command("zsh"));
        assert!(!is_claude_command("2.1"));
        assert!(!is_claude_command("v2.1.3"));
    }

    #[test]
    fn no_server_is_not_an_error() {
        let r = FakeRunner {
            replies: vec![(
                "list-panes",
                false,
                String::new(),
                "no server running on /tmp/tmux-504/default\n".into(),
            )],
            ..Default::default()
        };
        assert_eq!(list_panes(&r), Ok(None));
        let r = FakeRunner {
            replies: vec![("list-panes", false, String::new(), "bad format".into())],
            ..Default::default()
        };
        assert!(list_panes(&r).unwrap_err().contains("bad format"));
    }

    #[test]
    fn client_with_newest_activity_wins() {
        let out = "/dev/ttys000\t1792152534\n/dev/ttys004\t1792152999\n/dev/ttys002\t17\n";
        assert_eq!(pick_client(out).as_deref(), Some("/dev/ttys004"));
        assert_eq!(pick_client(""), None);
    }

    #[test]
    fn pane_info_parsing() {
        assert_eq!(
            parse_pane_info("@16\t364\t64\n"),
            Some(PaneInfo {
                window_id: "@16".into(),
                width: 364,
                height: 64
            })
        );
        assert_eq!(parse_pane_info("garbage"), None);
    }

    #[test]
    fn ids_are_validated() {
        assert!(valid_pane_id("%40"));
        assert!(!valid_pane_id("%"));
        assert!(!valid_pane_id("-claude"));
        assert!(!valid_pane_id("%4;kill"));
        assert!(valid_window_id("@12"));
        assert!(!valid_window_id("12"));
    }

    #[test]
    fn command_shapes_target_ids() {
        assert_eq!(
            jump_commands("/dev/ttys000", "@5", "%13"),
            vec![
                strings(&["switch-client", "-c", "/dev/ttys000", "-t", "%13"]),
                strings(&["select-window", "-t", "@5"]),
                strings(&["select-pane", "-t", "%13"]),
            ]
        );
        assert_eq!(
            send_literal_args("%1", "-rf"),
            strings(&["send-keys", "-t", "%1", "-l", "--", "-rf"])
        );
        assert_eq!(
            send_key_args("%1", "Enter"),
            strings(&["send-keys", "-t", "%1", "Enter"])
        );
        assert_eq!(
            preview_capture_args("%1"),
            strings(&["capture-pane", "-e", "-p", "-J", "-S", "-200", "-t", "%1"])
        );
    }
}
