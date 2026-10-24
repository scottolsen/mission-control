//! Mission Control: one board for every Claude Code session running across
//! the user's tmux sessions. `run` wires the app shell: login PATH, the
//! monitor thread, the tray, the global hotkey, and close-to-hide.

mod commands;
mod detect;
mod monitor;
mod notify;
mod registry;
mod settings;
mod shell_path;
mod snapshot;
mod tmux;
mod transcript;
mod tray;
mod types;

use monitor::Shared;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub fn run() {
    // Before any thread or child process: a Dock-launched app has launchd's
    // bare PATH and would find neither tmux nor terminal-notifier.
    shell_path::inherit_login_path();

    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // The handler fires on press and on release.
                    if event.state == ShortcutState::Pressed {
                        toggle_window(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            let settings = settings::load(&config_dir(&handle)?);
            let poller = snapshot::Poller::new(Arc::new(tmux::SystemRunner));
            let shared = Arc::new(Shared::new(poller, settings.clone()));
            app.manage(shared.clone());

            tray::build(&handle)?;
            if let Err(e) = register_hotkey(&handle, &settings.hotkey) {
                // A bad stored hotkey shouldn't stop the app; the UI can fix it.
                eprintln!("mission-control: {e}");
            }
            monitor::start(handle, shared);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing hides; quitting is the tray's "Quit" or ⌘Q.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.hide().is_ok() {
                    set_visible(window.app_handle(), false);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh,
            commands::capture_pane,
            commands::jump,
            commands::send_reply,
            commands::send_choice,
            commands::get_settings,
            commands::set_settings,
            commands::hide_window,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        // Clicking the Dock icon with the window hidden brings it back.
        if let RunEvent::Reopen { .. } = event {
            show_window(app);
        }
    });
}

pub(crate) fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

pub(crate) fn parse_hotkey(hotkey: &str) -> Result<Shortcut, String> {
    hotkey
        .parse::<Shortcut>()
        .map_err(|e| format!("Invalid hotkey \"{hotkey}\": {e}"))
}

/// Replace the registered global shortcut. Parses first, so a bad string
/// leaves the current one in place.
pub(crate) fn register_hotkey(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    let shortcut = parse_hotkey(hotkey)?;
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;
    gs.register(shortcut)
        .map_err(|e| format!("Couldn't register hotkey \"{hotkey}\": {e}"))
}

/// Tauri has no shown/hidden window event, so every show and hide goes
/// through here: it records the state (capture_pane checks it) and tells the
/// UI (`visibility` event), which pauses the pane preview while hidden.
pub(crate) fn set_visible(app: &AppHandle, visible: bool) {
    if let Some(shared) = app.try_state::<Arc<Shared>>() {
        shared.set_window_visible(visible);
    }
    let _ = app.emit("visibility", visible);
}

pub(crate) fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        set_visible(app, true);
    }
}

pub(crate) fn hide_window(app: &AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        w.hide().map_err(|e| e.to_string())?;
        set_visible(app, false);
    }
    Ok(())
}

/// Show and focus, or hide when already visible and focused.
pub(crate) fn toggle_window(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let visible = w.is_visible().unwrap_or(false);
    let focused = w.is_focused().unwrap_or(false);
    if visible && focused {
        let _ = hide_window(app);
    } else {
        show_window(app);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_hotkey;

    #[test]
    fn hotkeys_parse_or_error() {
        assert!(parse_hotkey("Ctrl+Alt+M").is_ok());
        assert!(parse_hotkey("CommandOrControl+Shift+K").is_ok());
        assert!(parse_hotkey("Ctrl+Alt+").is_err());
        assert!(parse_hotkey("Hyper+Nope+Banana").is_err());
        assert!(parse_hotkey("").is_err());
    }
}
