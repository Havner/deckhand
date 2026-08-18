//! Device / engine-level config — Tier B (PLAN §3, Round E).
//!
//! Profile-independent settings handed to the engine **once** (uncompiled): the master
//! rumble % + pulse frequency, device toggles, and the top-level chords. The engine evaluates
//! chords before any profile binding and consumes their buttons (§4).

use serde::{Deserialize, Serialize};

/// The device (above-profile) configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    /// LED brightness `0..=100 %` (applied on connect); `None` = leave default.
    pub led_brightness: Option<u8>,
    /// Sleep/idle timeout, seconds (applied on connect); `None` = leave default.
    pub idle_timeout: Option<u16>,
    /// `0..=100 %` — scales **all** haptic output (activator haptics + rumble back-channel). A
    /// global attenuator only; per-profile `strength` (which may exceed 100) does per-game gain.
    pub master_rumble: u8,
    /// Rumble pulse frequency, Hz — the Gordon pulse-train rate (applied reader-side; the Deck's
    /// motor rumble ignores it). Was per-profile (`RumbleSettings.hz`); now global.
    pub rumble_hz: u16,
    /// Top-level switch/command chords.
    pub chords: Vec<Chord>,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig {
            led_brightness: None,
            idle_timeout: None,
            master_rumble: 100,
            rumble_hz: 60,
            chords: vec![],
        }
    }
}

/// A top-level chord: raw controller buttons, **AND-combined** (all held), firing a
/// [`ChordAction`]. Any hardware button (`vocab_hid::Button`) — face buttons / dpad included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord {
    pub buttons: Vec<vocab_hid::Button>,
    pub action: ChordAction,
}

/// What a chord does — each variant carries its own params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChordAction {
    /// Switch main ↔ fallback profile (`HoldFallback` = while held; `Toggle` = latch; `SetMain`/
    /// `SetFallback` = latch a specific role on engage).
    SwitchProfile { mode: SwitchMode },
    /// Run a headless external program — the escape hatch for system actions (on-screen
    /// keyboard, audio device, …) that keeps the engine free of X/Wayland/DE/audio deps.
    /// Hold/Toggle is N/A (fires on activation).
    CommandExecute {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// Switch-chord semantics. `HoldFallback`/`Toggle` are relative to the current role; `SetMain`/
/// `SetFallback` latch a **specific** role on engage (same persistent outcome as `Toggle`, but
/// unconditional — the target is chosen by the mode, not by the current state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchMode {
    /// Force Fallback while held; back to the persistent base on release.
    HoldFallback,
    /// Flip the persistent base (Main ↔ Fallback) on each engage edge.
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
    fn default_master_rumble_is_full() {
        assert_eq!(DeviceConfig::default().master_rumble, 100);
    }

    #[test]
    fn device_config_round_trips_ron() {
        let d = DeviceConfig {
            led_brightness: Some(50),
            idle_timeout: None,
            master_rumble: 80,
            rumble_hz: 90,
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
        let s = ron::to_string(&d).unwrap();
        assert_eq!(ron::from_str::<DeviceConfig>(&s).unwrap(), d);
    }
}
