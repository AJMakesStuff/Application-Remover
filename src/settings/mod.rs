//! Persistent user settings (JSON in the XDG config directory).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub show_icons: bool,
    pub show_system_by_default: bool,
    pub remember_window_size: bool,
    pub confirm_before_uninstall: bool,
    pub refresh_on_launch: bool,
    pub window_width: i32,
    pub window_height: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            show_icons: true,
            show_system_by_default: false,
            remember_window_size: true,
            confirm_before_uninstall: true,
            refresh_on_launch: true,
            window_width: 1100,
            window_height: 720,
        }
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("linux-application-remover").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| Self::from_json(&t)).unwrap_or_default()
    }

    pub fn from_json(t: &str) -> Option<Self> {
        serde_json::from_str(t).ok()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let s = Settings { theme: Theme::Dark, confirm_before_uninstall: false, ..Default::default() };
        let t = serde_json::to_string(&s).unwrap();
        assert_eq!(Settings::from_json(&t).unwrap(), s);
        // Missing keys fall back to defaults (forward/backward compatibility).
        let partial = Settings::from_json(r#"{"theme":"Light"}"#).unwrap();
        assert_eq!(partial.theme, Theme::Light);
        assert!(partial.confirm_before_uninstall);
        assert!(Settings::from_json("not json").is_none());
    }
}
