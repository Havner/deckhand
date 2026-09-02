//! Profile *management* (as opposed to editing): resolving the profiles directory, listing the
//! `.ron` files in it, and loading/saving a profile document.
//!
//! The directory is either the default (`<config>/deckhand/profiles`) or a user-set custom one (see
//! [`AppSettings::use_custom_profile_dir`]). It's created at startup ([`ensure_dir`]).

use std::path::{Path, PathBuf};

use config::ConfigDoc;

use crate::settings::{AppSettings, config_dir};

/// The default profiles directory: `<config>/deckhand/profiles`.
fn default_dir() -> PathBuf {
    config_dir().join("deckhand").join("profiles")
}

/// The active profiles directory: the custom one when enabled and non-empty, else the default.
pub(crate) fn dir(s: &AppSettings) -> PathBuf {
    if s.use_custom_profile_dir && !s.custom_profile_dir.trim().is_empty() {
        PathBuf::from(s.custom_profile_dir.trim())
    } else {
        default_dir()
    }
}

/// Create the active profiles directory (best-effort; called at startup and after a dir change).
pub(crate) fn ensure_dir(s: &AppSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(s))
}

/// The `.ron` file names (not full paths) in the active profiles directory, sorted. Silent on a
/// missing/unreadable directory (returns empty) - the Profiles combobox just shows nothing.
pub(crate) fn list(s: &AppSettings) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir(s)) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("ron")
                && let Some(name) = p.file_name().and_then(|n| n.to_str())
            {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names
}

/// Load + parse a profile RON from a path (mirrors the daemon-client loader).
pub(crate) fn load(path: &Path) -> Result<ConfigDoc, String> {
    crate::persist::load(path)
}

/// Save a profile document to a path as pretty RON, creating the parent directory.
pub(crate) fn save(path: &Path, doc: &ConfigDoc) -> std::io::Result<()> {
    crate::persist::save(path, doc)
}
