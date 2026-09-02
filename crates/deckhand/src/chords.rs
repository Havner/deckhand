//! Persistence for the UI-owned chords, in RON at `$XDG_CONFIG_HOME/deckhand/chords.ron`.
//!
//! The UI keeps a [`Chords`] in memory as the Chords screen's source of truth, always in lock-step
//! with this file and the daemon: an edit writes here **and** ships `SetChords`; a daemon
//! `ChordsSet` event writes here **and** replaces the in-memory copy.

use std::path::PathBuf;

use config::Chords;

use crate::settings::config_dir;

/// Where the UI's chords live: alongside the app settings
/// (`$XDG_CONFIG_HOME/deckhand/chords.ron`, `%APPDATA%\deckhand\chords.ron`).
fn chords_path() -> PathBuf {
    config_dir().join("deckhand").join("chords.ron")
}

/// Load from [`chords_path`]; returns the default (empty, never an error) when the file is missing
/// or unparseable - a stale chords file shouldn't stop the UI launching (mirrors [`AppSettings::load`]).
///
/// [`AppSettings::load`]: crate::settings::AppSettings::load
pub(crate) fn load() -> Chords {
    crate::persist::load_or_default(&chords_path())
}

/// Write to [`chords_path`] as pretty RON, creating the parent directory.
pub(crate) fn save(c: &Chords) -> std::io::Result<()> {
    crate::persist::save(&chords_path(), c)
}

/// The value to ship to the daemon: `None` when there are no chords, else the chords themselves. The
/// UI has no separate "cleared" state - an empty list *is* "no chords", so it clears the daemon's
/// chords (`None`) rather than shipping an empty set.
pub(crate) fn to_push(chords: &Chords) -> Option<Chords> {
    (!chords.chords.is_empty()).then(|| chords.clone())
}
