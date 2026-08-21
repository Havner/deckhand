//! The forwarder UI's own settings — RON, at `$XDG_CONFIG_HOME/deckhand/forwarder.ron`.
//!
//! A deliberately tiny analogue of the main UI's `settings.ron`: this app has no configurable
//! policy (launch-daemon / restore-input / manual-start are all fixed), so the file holds only what
//! the app *remembers* between runs — the theme, the window size, and the last input/output. The
//! field order below is the on-disk order (matches the design note).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted forwarder settings. `#[serde(default)]` so a partial or older file still loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// The iced theme, stored by **name** (default `Dark`). Not exposed in the UI, but honored from
    /// the file so it can be changed by hand — falls back to Dark for an empty/unknown name.
    pub(crate) theme: String,
    /// Last *non-maximized* window size, restored when the window opens; saved on resize/quit. While
    /// maximized we keep the pre-maximize size here (not the maximized extent) so restoring maximized
    /// and then un-maximizing lands back on the previous floating size.
    pub(crate) window_width: u32,
    pub(crate) window_height: u32,
    /// Whether the window was maximized when last saved; restored on open.
    pub(crate) window_maximized: bool,
    /// The last input spec set from the UI (re-staged on connect).
    pub(crate) last_input: String,
    /// The last content of the output (`ip:port`) text field — restored into the field on launch,
    /// but only *applied* to the daemon when Start is pressed. Named `_network` to mirror the main
    /// UI's `last_output_network` (the forwarder's output is always a network target).
    pub(crate) last_output_network: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "Dark".to_string(),
            // Large by default: the 2× UI scale halves the logical space, and the Deck's own display
            // is 1280×800 — so open near full-screen so the scaled content fits without scrolling.
            window_width: 1280,
            window_height: 800,
            window_maximized: false,
            last_input: String::new(),
            last_output_network: String::new(),
        }
    }
}

impl Settings {
    /// Load from [`settings_path`]; returns the default (never an error) when the file is missing or
    /// unparseable — a stale settings file shouldn't stop the app launching.
    pub(crate) fn load() -> Self {
        crate::persist::load_or_default(&settings_path())
    }

    /// Write to [`settings_path`] as pretty RON, creating the parent directory.
    pub(crate) fn save(&self) -> std::io::Result<()> {
        crate::persist::save(&settings_path(), self)
    }
}

/// Where the forwarder settings live: `$XDG_CONFIG_HOME/deckhand/forwarder.ron` (fallback
/// `~/.config/…`) on unix, `%APPDATA%\deckhand\forwarder.ron` on Windows.
fn settings_path() -> PathBuf {
    config_dir().join("deckhand").join("forwarder.ron")
}

#[cfg(unix)]
pub(crate) fn config_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(x);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(windows)]
pub(crate) fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
