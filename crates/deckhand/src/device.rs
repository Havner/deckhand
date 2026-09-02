//! Persistence for the UI-owned device config, in RON at
//! `$XDG_CONFIG_HOME/deckhand/devcfg.ron`.
//!
//! The UI keeps a [`DeviceConfig`] in memory as the Device screen's source of truth, always in
//! lock-step with this file and the daemon: an edit writes here **and** ships `SetDeviceConfig`; a
//! daemon `DeviceConfigSet` event writes here **and** replaces the in-memory copy. Chords aren't
//! editable in the UI yet but ride along untouched through both directions.

use std::path::PathBuf;

use config::DeviceConfig;

use crate::settings::config_dir;

/// Where the UI's device config lives: alongside the app settings
/// (`$XDG_CONFIG_HOME/deckhand/devcfg.ron`, `%APPDATA%\deckhand\devcfg.ron`).
fn device_path() -> PathBuf {
    config_dir().join("deckhand").join("devcfg.ron")
}

/// Load from [`device_path`]; returns the default (never an error) when the file is missing or
/// unparseable - a stale device file shouldn't stop the UI launching (mirrors [`AppSettings::load`]).
///
/// [`AppSettings::load`]: crate::settings::AppSettings::load
pub(crate) fn load() -> DeviceConfig {
    crate::persist::load_or_default(&device_path())
}

/// Write to [`device_path`] as pretty RON, creating the parent directory.
pub(crate) fn save(g: &DeviceConfig) -> std::io::Result<()> {
    crate::persist::save(&device_path(), g)
}
