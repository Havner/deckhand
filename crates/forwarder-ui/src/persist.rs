//! Small shared RON persistence helpers (copied from the `deckhand` UI — the forwarder is a
//! deliberate duplicate of the parts it reuses, per the prototype's scope).
//!
//! The forwarder keeps two files on disk in RON — its own [`crate::settings`] and the shared
//! device config ([`crate::device`], the very same `devcfg.ron` the main UI owns). Both load with
//! [`load_or_default`] so a stale/missing file never stops the app launching.

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Load a RON file into `T`, or return `T::default()` when the file is missing or unparseable — a
/// stale config file shouldn't fail to launch.
pub(crate) fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => ron::from_str(&text).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

/// Write `value` to `path` as pretty RON, creating the parent directory.
pub(crate) fn save<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
        .map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}
