//! The UI application's own settings — persisted separately from the daemon's config, in RON.
//!
//! These drive the Settings screen (start-the-daemon / load-a-profile-on-start toggles + the two
//! profile paths) and, eventually, the launch behaviour. Currently only read/written; the UI does
//! not act on `start_daemon` yet.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Application settings (see the Settings screen). `#[serde(default)]` so a partial or older file
/// still loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Start the daemon if it isn't running when the UI launches.
    pub start_daemon: bool,
    /// Load the Main profile on start.
    pub load_main: bool,
    /// Path to the Main profile RON (meaningful only when `load_main`).
    pub main_path: String,
    /// Load the Fallback profile on start.
    pub load_fallback: bool,
    /// Path to the Fallback profile RON (meaningful only when `load_fallback`).
    pub fallback_path: String,
    /// The UI theme, stored by **name** (e.g. `Dark`, `Dracula`) — one of iced's built-in themes,
    /// falling back to the default when the name is empty/unknown.
    pub theme: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            start_daemon: false,
            load_main: false,
            main_path: String::new(),
            load_fallback: false,
            fallback_path: String::new(),
            theme: "Dark".to_string(),
        }
    }
}

impl AppSettings {
    /// Load from [`settings_path`]; returns the default (never an error) when the file is missing
    /// or unparseable — a config test shouldn't fail to launch over a stale settings file.
    pub fn load() -> Self {
        let path = settings_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => ron::from_str(&text).unwrap_or_default(),
            Err(_) => AppSettings::default(),
        }
    }

    /// Write to [`settings_path`] as pretty RON, creating the parent directory.
    pub fn save(&self) -> std::io::Result<()> {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        std::fs::write(&path, text)
    }
}

/// Where the UI settings live: `$XDG_CONFIG_HOME/deckhand/settings.ron` (fallback `~/.config/…`) on
/// unix, `%APPDATA%\deckhand\settings.ron` on Windows.
pub fn settings_path() -> PathBuf {
    let dir = config_dir().join("deckhand");
    dir.join("settings.ron")
}

#[cfg(unix)]
fn config_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(x);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(windows)]
fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
