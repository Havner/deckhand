//! Persistence for the UI-owned global (engine) config, in RON at
//! `$XDG_CONFIG_HOME/deckhand/globals.ron`.
//!
//! The UI keeps a [`GlobalConfig`] in memory as the Globals screen's source of truth, always in
//! lock-step with this file and the daemon: an edit writes here **and** ships `SetGlobals`; a
//! daemon `GlobalConfigSet` event writes here **and** replaces the in-memory copy. Chords aren't
//! editable in the UI yet but ride along untouched through both directions.

use std::path::PathBuf;

use config::GlobalConfig;

use crate::settings::config_dir;

/// Where the UI's global config lives: alongside the app settings
/// (`$XDG_CONFIG_HOME/deckhand/globals.ron`, `%APPDATA%\deckhand\globals.ron`).
pub fn globals_path() -> PathBuf {
    config_dir().join("deckhand").join("globals.ron")
}

/// Load from [`globals_path`]; returns the default (never an error) when the file is missing or
/// unparseable — a stale globals file shouldn't stop the UI launching (mirrors [`AppSettings::load`]).
///
/// [`AppSettings::load`]: crate::settings::AppSettings::load
pub fn load() -> GlobalConfig {
    match std::fs::read_to_string(globals_path()) {
        Ok(text) => ron::from_str(&text).unwrap_or_default(),
        Err(_) => GlobalConfig::default(),
    }
}

/// Write to [`globals_path`] as pretty RON, creating the parent directory.
pub fn save(g: &GlobalConfig) -> std::io::Result<()> {
    let path = globals_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = ron::ser::to_string_pretty(g, ron::ser::PrettyConfig::default())
        .map_err(std::io::Error::other)?;
    std::fs::write(&path, text)
}
