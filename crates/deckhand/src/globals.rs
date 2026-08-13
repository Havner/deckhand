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
fn globals_path() -> PathBuf {
    config_dir().join("deckhand").join("globals.ron")
}

/// Load from [`globals_path`]; returns the default (never an error) when the file is missing or
/// unparseable — a stale globals file shouldn't stop the UI launching (mirrors [`AppSettings::load`]).
///
/// [`AppSettings::load`]: crate::settings::AppSettings::load
pub(crate) fn load() -> GlobalConfig {
    crate::persist::load_or_default(&globals_path())
}

/// Write to [`globals_path`] as pretty RON, creating the parent directory.
pub(crate) fn save(g: &GlobalConfig) -> std::io::Result<()> {
    crate::persist::save(&globals_path(), g)
}
