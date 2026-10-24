//! The background poll loop. Every 2 s it builds a snapshot, and when the
//! content changed it caches it, emits `snapshot` to the UI, refreshes the
//! tray, and posts notifications for transitions. It runs whether or not the
//! window is visible, so the tray and notifications stay current.
//!
//! Nothing here may take the app down: a panic inside a poll is caught and
//! surfaces as `Snapshot.error`, and poisoned locks are recovered.

use crate::notify::{self, Notice};
use crate::snapshot::Poller;
use crate::tmux::Runner;
use crate::types::{Settings, Snapshot, State};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

pub const INTERVAL: Duration = Duration::from_secs(2);

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Remembers the previous poll to detect changes and transitions.
#[derive(Default)]
pub struct Observer {
    last: Option<Snapshot>,
    prev_states: Option<HashMap<String, State>>,
}

pub struct Observation {
    pub changed: bool,
    pub notices: Vec<Notice>,
}

impl Observer {
    pub fn observe(&mut self, snap: &Snapshot, settings: &Settings) -> Observation {
        let changed = self
            .last
            .as_ref()
            .is_none_or(|last| !last.same_content(snap));
        self.last = Some(snap.clone());
        // A failed poll (tmux timed out, a caught panic) has registry-only
        // or stale sessions: it neither notifies nor moves the baseline.
        if snap.error.is_some() {
            return Observation {
                changed,
                notices: Vec::new(),
            };
        }
        let notices = notify::decide(self.prev_states.as_ref(), &snap.sessions, settings);
        // An empty first poll isn't a baseline: the first real one is, so it
        // doesn't fire for every session it finds.
        if self.prev_states.is_some() || !snap.sessions.is_empty() {
            self.prev_states = Some(notify::states(&snap.sessions));
        }
        Observation { changed, notices }
    }
}

/// Poller and observer under one lock, so the monitor thread and `refresh`
/// can't interleave a poll with another poll's change detection.
pub struct Core {
    pub poller: Poller,
    pub observer: Observer,
}

pub struct Shared {
    /// For commands (jump, send, capture) that don't need the poll lock.
    pub runner: Arc<dyn Runner>,
    pub core: Mutex<Core>,
    /// `generatedAt == 0` until the first poll lands (the UI shows loading).
    pub latest: Mutex<Snapshot>,
    pub settings: Mutex<Settings>,
    /// Whether the main window is shown. The pane preview isn't captured
    /// while it's hidden in the tray.
    visible: AtomicBool,
}

impl Shared {
    pub fn new(poller: Poller, settings: Settings) -> Shared {
        Shared {
            runner: poller.runner.clone(),
            core: Mutex::new(Core {
                poller,
                observer: Observer::default(),
            }),
            latest: Mutex::new(Snapshot::default()),
            settings: Mutex::new(settings),
            // tauri.conf.json opens the window visible.
            visible: AtomicBool::new(true),
        }
    }

    pub fn window_visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    pub fn set_window_visible(&self, visible: bool) {
        self.visible.store(visible, Ordering::Relaxed);
    }

    pub fn latest(&self) -> Snapshot {
        lock(&self.latest).clone()
    }
}

/// Poll once and publish. Returns the new snapshot.
pub fn poll_now(app: &AppHandle, shared: &Shared) -> Snapshot {
    let settings = lock(&shared.settings).clone();
    let mut core = lock(&shared.core);
    let now = now_ms();
    let polled = std::panic::catch_unwind(AssertUnwindSafe(|| core.poller.poll(now)));
    let snap = match polled {
        Ok(s) => s,
        Err(panic) => {
            let why = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown".into());
            let mut s = shared.latest();
            s.generated_at = now;
            s.error = Some(format!("Internal error while polling: {why}"));
            s
        }
    };
    let obs = core.observer.observe(&snap, &settings);
    drop(core);

    *lock(&shared.latest) = snap.clone();
    if obs.changed {
        let _ = app.emit("snapshot", &snap);
        crate::tray::update(app, &snap);
    }
    for n in &obs.notices {
        notify::post(n);
    }
    snap
}

pub fn start(app: AppHandle, shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("mission-control-monitor".into())
        .spawn(move || loop {
            // poll_now already catches panics from polling; this guards the
            // emit/tray path too so the loop itself never dies.
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| poll_now(&app, &shared)));
            std::thread::sleep(INTERVAL);
        })
        .expect("spawn monitor thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(at: i64, err: Option<&str>) -> Snapshot {
        Snapshot {
            generated_at: at,
            tmux_running: true,
            error: err.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn change_detection_ignores_generated_at() {
        let mut o = Observer::default();
        let s = Settings::default();
        assert!(
            o.observe(&snap(1, None), &s).changed,
            "first poll always publishes"
        );
        assert!(!o.observe(&snap(2, None), &s).changed);
        assert!(o.observe(&snap(3, Some("tmux broke")), &s).changed);
        assert!(!o.observe(&snap(4, Some("tmux broke")), &s).changed);
    }

    fn with(sessions: Vec<crate::types::Session>, err: Option<&str>) -> Snapshot {
        Snapshot {
            sessions,
            ..snap(1, err)
        }
    }

    /// A failed poll (tmux didn't answer) has registry-only sessions: menus
    /// vanish and come back. It must neither notify nor move the baseline.
    #[test]
    fn errored_polls_do_not_touch_the_baseline() {
        use crate::notify::tests::session;
        let on = Settings {
            notify_needs_input: true,
            ..Default::default()
        };
        let mut o = Observer::default();
        o.observe(&with(vec![session("a", State::NeedsInput)], None), &on);
        let err = with(vec![session("a", State::Idle)], Some("tmux: timed out"));
        assert!(o.observe(&err, &on).notices.is_empty());
        let back = with(vec![session("a", State::NeedsInput)], None);
        assert!(
            o.observe(&back, &on).notices.is_empty(),
            "spurious Needs input"
        );
    }

    #[test]
    fn empty_first_poll_is_not_a_baseline() {
        use crate::notify::tests::session;
        let on = Settings {
            notify_needs_input: true,
            ..Default::default()
        };
        let mut o = Observer::default();
        o.observe(&with(vec![], None), &on);
        let first = with(vec![session("a", State::NeedsInput)], None);
        assert!(o.observe(&first, &on).notices.is_empty());
        let next = with(vec![session("a", State::Working)], None);
        o.observe(&next, &on);
        let again = with(vec![session("a", State::NeedsInput)], None);
        assert_eq!(
            o.observe(&again, &on).notices.len(),
            1,
            "real transitions still fire"
        );
    }

    /// Before the first poll lands, get_snapshot answers with generatedAt 0,
    /// which the UI reads as "still loading", not "tmux isn't running".
    #[test]
    fn cold_start_snapshot_is_marked_unloaded() {
        let shared = Shared::new(
            Poller::new(Arc::new(crate::tmux::tests::FakeRunner::default())),
            Settings::default(),
        );
        assert_eq!(shared.latest().generated_at, 0);
        assert!(shared.window_visible());
    }

    #[test]
    fn poisoned_lock_is_recovered() {
        let m = Arc::new(Mutex::new(5));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.is_poisoned());
        assert_eq!(*lock(&m), 5);
    }
}
