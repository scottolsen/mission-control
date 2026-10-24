//! User settings, stored as `settings.json` in the app config dir. Missing or
//! unreadable files fall back to defaults, and missing keys take their
//! default individually, so adding a setting never breaks an older file.

use crate::types::Settings;
use std::path::Path;

pub const FILE_NAME: &str = "settings.json";

pub fn load(dir: &Path) -> Settings {
    std::fs::read_to_string(dir.join(FILE_NAME))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, settings: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    // Write-then-rename so a crash mid-write can't leave a truncated file.
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    std::fs::write(&tmp, json).map_err(|e| format!("write settings: {e}"))?;
    std::fs::rename(&tmp, dir.join(FILE_NAME)).map_err(|e| format!("save settings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mc-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert!(s.notify_your_turn);
        assert!(!s.notify_needs_input);
        assert_eq!(s.hotkey, "Ctrl+Alt+M");
        assert_eq!(s.stale_hours, 8);
        assert_eq!(s.terminal_app, "iTerm");
        assert_eq!(load(&temp_dir("missing")), s);
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("rt");
        let s = Settings {
            notify_your_turn: false,
            notify_needs_input: true,
            hotkey: "Cmd+Shift+K".into(),
            stale_hours: 3,
            terminal_app: "Ghostty".into(),
        };
        save(&dir, &s).unwrap();
        assert_eq!(load(&dir), s);
        let raw = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(raw.contains("\"notifyYourTurn\""));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn partial_and_corrupt_files() {
        let dir = temp_dir("partial");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), r#"{"staleHours": 2}"#).unwrap();
        let s = load(&dir);
        assert_eq!(s.stale_hours, 2);
        assert_eq!(s.hotkey, "Ctrl+Alt+M");
        std::fs::write(dir.join(FILE_NAME), "{nope").unwrap();
        assert_eq!(load(&dir), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
