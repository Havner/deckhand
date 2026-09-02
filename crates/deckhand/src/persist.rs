//! Small shared RON persistence helpers.
//!
//! The UI keeps several files on disk in RON - the app settings ([`crate::settings`]), the UI-owned
//! device config ([`crate::device`]), and the profile documents ([`crate::profiles`]). They all
//! (de)serialize the same way, so the read/parse/pretty-write boilerplate lives here once. Two load
//! flavours, one save:
//!
//! - [`load_or_default`] - a stale/missing file must never stop the UI launching (settings, device config).
//! - [`load`] - a caller that wants the parse error surfaced, with the path (profiles).

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Load a RON file into `T`, or return `T::default()` when the file is missing or unparseable - a
/// stale config file shouldn't fail to launch.
pub(crate) fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => ron::from_str(&text).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

/// Load + parse a RON file into `T`, mapping any read/parse error to a string prefixed with the path.
pub(crate) fn load<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
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
