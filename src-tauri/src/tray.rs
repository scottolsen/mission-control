//! Menu-bar presence: a template icon whose title is the number of sessions
//! wanting attention, a left click that toggles the window, and a menu that
//! lists those sessions for a one-click jump.

use crate::types::{Session, Snapshot, State};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

const TRAY_ID: &str = "main";
const MAX_ITEMS: usize = 8;
const LABEL_MAX: usize = 70;
const JUMP_PREFIX: &str = "jump:";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let icon = Image::from_bytes(include_bytes!("../icons/tray@2x.png"))?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Mission Control")
        .menu(&menu(app, &[])?)
        // Left click toggles the window; the menu is on right click.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::toggle_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    if id == "open" {
        crate::show_window(app);
    } else if id == "quit" {
        app.exit(0);
    } else if let Some(pane_id) = id.strip_prefix(JUMP_PREFIX) {
        let app = app.clone();
        let pane_id = pane_id.to_string();
        // Jumping shells out to tmux and osascript; keep it off the main thread.
        std::thread::spawn(move || {
            let _ = crate::commands::do_jump(&app, &pane_id);
        });
    }
}

/// Sessions shown in the tray: needs_input first, then your_turn, newest
/// activity first within each.
pub fn attention(snap: &Snapshot) -> Vec<&Session> {
    let mut list: Vec<&Session> = snap
        .sessions
        .iter()
        .filter(|s| s.state.wants_attention())
        .collect();
    list.sort_by_key(|s| (s.state != State::NeedsInput, -s.last_activity_at));
    list
}

pub fn title_for(count: usize) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

pub fn item_label(s: &Session) -> String {
    let label = format!("{} — {}", s.project, s.title);
    if label.chars().count() <= LABEL_MAX {
        return label;
    }
    let mut out: String = label.chars().take(LABEL_MAX - 1).collect();
    out.push('…');
    out
}

fn menu(app: &AppHandle, sessions: &[&Session]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open Mission Control",
        true,
        None::<&str>,
    )?)?;
    if !sessions.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    for s in sessions.iter().take(MAX_ITEMS) {
        let (id, enabled) = match &s.tmux {
            Some(t) => (format!("{JUMP_PREFIX}{}", t.pane_id), true),
            None => (format!("nojump:{}", s.id), false),
        };
        menu.append(&MenuItem::with_id(
            app,
            id,
            item_label(s),
            enabled,
            None::<&str>,
        )?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?)?;
    Ok(menu)
}

pub fn update(app: &AppHandle, snap: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let list = attention(snap);
    let _ = tray.set_title(title_for(list.len()));
    if let Ok(m) = menu(app, &list) {
        let _ = tray.set_menu(Some(m));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, state: State, at: i64) -> Session {
        let mut sess: Session = serde_json::from_value(serde_json::json!({
            "id": id, "pid": null, "sessionId": null, "cwd": "/p/proj", "project": "proj",
            "branch": null, "tmux": null, "title": "A title", "state": "idle", "prompt": null,
            "lastPrompt": null, "lastPromptAt": null, "lastReply": null, "lastReplyAt": null,
            "currentTool": null, "lastActivityAt": 0, "statusUpdatedAt": null, "startedAt": null,
            "version": null, "jumpable": false
        }))
        .unwrap();
        sess.state = state;
        sess.last_activity_at = at;
        sess
    }

    #[test]
    fn attention_order_and_count() {
        let snap = Snapshot {
            sessions: vec![
                s("a", State::YourTurn, 10),
                s("b", State::Working, 99),
                s("c", State::NeedsInput, 1),
                s("d", State::YourTurn, 50),
                s("e", State::Idle, 70),
            ],
            ..Default::default()
        };
        let ids: Vec<&str> = attention(&snap).iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "d", "a"]);
        assert_eq!(title_for(3).as_deref(), Some("3"));
        assert_eq!(title_for(0), None);
    }

    #[test]
    fn labels_are_capped() {
        let mut long = s("a", State::YourTurn, 0);
        long.title = "x".repeat(200);
        assert_eq!(item_label(&long).chars().count(), LABEL_MAX);
        assert_eq!(item_label(&s("a", State::YourTurn, 0)), "proj — A title");
    }
}
