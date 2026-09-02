//! Top-level switch/command chords (PLAN 3 Round E / 4) - the above-profile switch layer,
//! evaluated before any profile binding with their buttons consumed. Held by the engine as a
//! third, independent config slot (`Option<Chords>`) beside the Main/Fallback profile roles, and
//! persisted separately from the [`DeviceConfig`](crate::DeviceConfig).

use serde::{Deserialize, Serialize};

/// The engine's top-level chords - one whole config unit (the `Option<Chords>` slot). Sent/stored
/// as a single thing so a client owns it wholesale; a named struct (not a bare `Vec`) so it can
/// grow chord-level settings later without a breaking shape change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chords {
    pub chords: Vec<Chord>,
}

/// A top-level chord: raw controller buttons, **AND-combined** (all held), firing a
/// [`ChordAction`]. Any hardware button (`vocab_hid::Button`) - face buttons / dpad included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord {
    pub buttons: Vec<vocab_hid::Button>,
    pub action: ChordAction,
}

/// What a chord does - each variant carries its own params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChordAction {
    /// Switch main <-> fallback profile (`HoldFallback` = while held; `Toggle` = latch; `SetMain`/
    /// `SetFallback` = latch a specific role on engage).
    SwitchProfile { mode: SwitchMode },
    /// Run a headless external program - the escape hatch for system actions (on-screen
    /// keyboard, audio device, ...) that keeps the engine free of X/Wayland/DE/audio deps.
    /// Hold/Toggle is N/A (fires on activation).
    CommandExecute {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// Switch-chord semantics. `HoldFallback`/`Toggle` are relative to the current role; `SetMain`/
/// `SetFallback` latch a **specific** role on engage (same persistent outcome as `Toggle`, but
/// unconditional - the target is chosen by the mode, not by the current state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchMode {
    /// Force Fallback while held; back to the persistent base on release.
    HoldFallback,
    /// Flip the persistent base (Main <-> Fallback) on each engage edge.
    Toggle,
    /// Latch the persistent base to **Main** on engage (no-op if already Main).
    SetMain,
    /// Latch the persistent base to **Fallback** on engage (no-op if already Fallback).
    SetFallback,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_round_trip_ron() {
        let c = Chords {
            chords: vec![
                Chord {
                    buttons: vec![vocab_hid::Button::Steam, vocab_hid::Button::RGrip],
                    action: ChordAction::SwitchProfile { mode: SwitchMode::Toggle },
                },
                Chord {
                    buttons: vec![vocab_hid::Button::Steam, vocab_hid::Button::LGrip],
                    action: ChordAction::CommandExecute {
                        command: "wvkbd".into(),
                        args: vec!["--toggle".into()],
                    },
                },
            ],
        };
        let s = ron::to_string(&c).unwrap();
        assert_eq!(ron::from_str::<Chords>(&s).unwrap(), c);
    }
}
