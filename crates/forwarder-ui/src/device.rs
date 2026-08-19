//! Persistence for the device config, in RON at `$XDG_CONFIG_HOME/deckhand/devcfg.ron`.
//!
//! This is the **same file the main `deckhand` UI owns** — intentionally shared. The forwarder only
//! ever changes `master_rumble` (its one slider), and it does so by loading the whole
//! [`DeviceConfig`], mutating that field, and saving — so the other fields (LED / idle / rumble Hz)
//! are preserved untouched. The in-memory copy, this file, and the daemon stay in lock-step (a
//! `DeviceConfigSet` event re-lands the value; see `apply_event`).

use std::path::PathBuf;

use config::DeviceConfig;

use crate::settings::config_dir;

/// Where the device config lives (`$XDG_CONFIG_HOME/deckhand/devcfg.ron`,
/// `%APPDATA%\deckhand\devcfg.ron`) — shared with the main UI.
fn device_path() -> PathBuf {
    config_dir().join("deckhand").join("devcfg.ron")
}

/// Load from [`device_path`]; returns the default (never an error) when the file is missing or
/// unparseable — a stale device file shouldn't stop the app launching.
pub(crate) fn load() -> DeviceConfig {
    crate::persist::load_or_default(&device_path())
}

/// Write to [`device_path`] as pretty RON, creating the parent directory.
pub(crate) fn save(d: &DeviceConfig) -> std::io::Result<()> {
    crate::persist::save(&device_path(), d)
}
