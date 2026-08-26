//! The UI application's own settings — persisted separately from the daemon's config, in RON.
//!
//! These drive the Settings screen (start-the-daemon / load-a-profile-on-start toggles + the two
//! profile paths) and, eventually, the launch behaviour. Currently only read/written; the UI does
//! not act on `start_daemon` yet.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which controller's inputs the profile editor shows. `Auto` follows the bound device's shape
/// (falling back to `All` when nothing is bound); `Gordon`/`Neptune` force a shape; `All` shows the
/// full device-independent superset. Inputs the shape doesn't report are dropped at the
/// [`InputSource`](config::InputSource) level (not greyed); a group whose inputs all drop out shows
/// no header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum ShowInputs {
    #[default]
    Auto,
    All,
    Gordon,
    Neptune,
    Triton,
}

impl ShowInputs {
    /// All variants in Settings-screen (pick-list) order.
    pub(crate) const ALL: [ShowInputs; 5] =
        [ShowInputs::Auto, ShowInputs::All, ShowInputs::Gordon, ShowInputs::Neptune, ShowInputs::Triton];

    /// The pick-list label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ShowInputs::Auto => "Auto",
            ShowInputs::All => "All",
            ShowInputs::Gordon => "Gordon",
            ShowInputs::Neptune => "Neptune",
            ShowInputs::Triton => "Triton",
        }
    }
}

/// Application settings (see the Settings screen). `#[serde(default)]` so a partial or older file
/// still loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    // Field order mirrors the Settings screen (UI section first, then Daemon), so the serialized
    // file reads top-to-bottom the same as the UI.
    /// The UI theme, stored by **name** (e.g. `Dark`, `Dracula`) — one of iced's built-in themes,
    /// falling back to the default when the name is empty/unknown.
    pub(crate) theme: String,
    /// Which controller's inputs the profile editor shows (Auto follows the bound device).
    pub(crate) show_inputs: ShowInputs,
    /// Show a system-tray icon. Master switch for the two options below.
    pub(crate) use_tray: bool,
    /// Close-to-tray: a window close request hides the window instead of quitting (needs `use_tray`).
    pub(crate) close_to_tray: bool,
    /// Start with the window hidden in the tray (needs `use_tray`; takes effect next launch). Not a
    /// minimize — the window is created hidden, not minimized to the taskbar.
    pub(crate) start_hidden: bool,
    /// Use a custom profiles directory instead of the default (`<config>/deckhand/profiles`).
    pub(crate) use_custom_profile_dir: bool,
    /// The custom profiles directory (meaningful only when `use_custom_profile_dir`).
    pub(crate) custom_profile_dir: String,
    /// Launch the daemon if it isn't running when the UI tries to connect.
    pub(crate) start_daemon: bool,
    /// Load the Main profile on connect.
    pub(crate) load_main: bool,
    /// Path to the Main profile RON (meaningful only when `load_main`).
    pub(crate) main_path: String,
    /// Load the Fallback profile on connect.
    pub(crate) load_fallback: bool,
    /// Path to the Fallback profile RON (meaningful only when `load_fallback`).
    pub(crate) fallback_path: String,
    /// Re-stage the last-used input/output (`last_input`/`last_output`) on connect.
    pub(crate) restore_io: bool,
    /// Start the engine (acquire hardware + run the mapping loop) on connect.
    pub(crate) start_engine: bool,

    // --- Not shown in the UI (persisted at the end). ---
    /// Last *non-maximized* window size, saved on hide/quit and restored when the window (re)opens.
    /// While maximized we deliberately keep the pre-maximize size here (not the maximized extent) so
    /// restoring maximized and then un-maximizing lands back on the previous floating size.
    pub(crate) window_width: u32,
    pub(crate) window_height: u32,
    /// Whether the window was maximized when last saved; restored on (re)open.
    pub(crate) window_maximized: bool,
    /// The last input/output spec set from the UI (restored on connect when `restore_io`).
    pub(crate) last_input: String,
    pub(crate) last_output: String,
    /// The last host:port typed into the network input/output popup (prefills it next time).
    pub(crate) last_input_network: String,
    pub(crate) last_output_network: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            theme: "Dark".to_string(),
            use_tray: false,
            close_to_tray: false,
            start_hidden: false,
            show_inputs: ShowInputs::Auto,
            use_custom_profile_dir: false,
            custom_profile_dir: String::new(),
            start_daemon: true,
            load_main: false,
            main_path: String::new(),
            load_fallback: false,
            fallback_path: String::new(),
            restore_io: true,
            start_engine: true,
            window_width: 1200,
            window_height: 800,
            window_maximized: false,
            last_input: String::new(),
            last_output: String::new(),
            last_input_network: String::new(),
            last_output_network: String::new(),
        }
    }
}

impl AppSettings {
    /// Load from [`settings_path`]; returns the default (never an error) when the file is missing
    /// or unparseable — a config test shouldn't fail to launch over a stale settings file.
    pub(crate) fn load() -> Self {
        crate::persist::load_or_default(&settings_path())
    }

    /// Write to [`settings_path`] as pretty RON, creating the parent directory.
    pub(crate) fn save(&self) -> std::io::Result<()> {
        crate::persist::save(&settings_path(), self)
    }
}

/// Where the UI settings live: `$XDG_CONFIG_HOME/deckhand/settings.ron` (fallback `~/.config/…`) on
/// unix, `%APPDATA%\deckhand\settings.ron` on Windows.
fn settings_path() -> PathBuf {
    let dir = config_dir().join("deckhand");
    dir.join("settings.ron")
}

/// The deckhand config directory: `<config>/deckhand` (holds `settings.ron` and the default
/// `profiles/` dir). The profiles-directory Browse dialog starts here.
pub(crate) fn deckhand_dir() -> PathBuf {
    config_dir().join("deckhand")
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
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
