//! `ui-test-common` — toolkit-independent helpers shared by the `ui-test-*` GUI bake-off crates.
//!
//! The four toolkit examples (iced / egui / gtk / qt) each own their widget layer; everything
//! that is *not* widgets lives here so it isn't written four times:
//!
//! - [`AppSettings`] — the application's own settings (separate from the daemon), RON-persisted.
//! - [`Client`] — a thin blocking command client to the daemon over `ipc` (reconnects on a
//!   dropped socket).
//! - [`run_event_loop`] — a resilient blocking subscribe loop each toolkit adapts to its own
//!   event pump (iced subscription, egui repaint, glib idle, Qt signal).
//! - [`Category`] — the left-sidebar navigation model.

mod daemon;
mod nav;
mod settings;

pub use daemon::{Client, DaemonUpdate, run_event_loop};
pub use nav::Category;
pub use settings::AppSettings;

/// Preset input selections offered before the daemon's live `list-devices` is appended (the same
/// grammar the daemon parses for `SetInput`).
pub const INPUT_PRESETS: &[&str] = &["auto", "dongle", "wired", "bt"];

/// Preset output selections (the test only offers the local sink; `host:port` is typed, not listed).
pub const OUTPUT_PRESETS: &[&str] = &["local"];
