//! Global / engine-level config — Tier B (PLAN §3, Round E).
//!
//! Profile-independent settings handed to the engine **once** (uncompiled): the master
//! rumble %, device toggles, and the top-level chords. The engine evaluates chords before
//! any profile binding and consumes their buttons (§4).

use serde::{Deserialize, Serialize};

use crate::input::InputSource;

/// The global (above-profile) configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// Which profile slot the engine boots into. **Read once at `start()`** — changing it via a
    /// live `set_globals` has no effect (by then the role is driven by the chords).
    pub start_profile: StartProfile,
    /// `0..=100 %` — scales **all** haptic output (activator haptics + rumble back-channel). A
    /// global attenuator only; per-profile `strength` (which may exceed 100) does per-game gain.
    pub master_rumble: u8,
    /// LED brightness `0..=100 %` (applied on connect); `None` = leave default.
    pub led_brightness: Option<u8>,
    /// Sleep/idle timeout, seconds (applied on connect); `None` = leave default.
    pub idle_timeout: Option<u16>,
    /// Top-level switch/command chords.
    pub chords: Vec<GlobalChord>,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        GlobalConfig {
            start_profile: StartProfile::default(),
            master_rumble: 100,
            led_brightness: None,
            idle_timeout: None,
            chords: vec![],
        }
    }
}

/// Which of the engine's two profile slots the engine boots into (the **main** profile, or the
/// **fallback**/desktop one). A start-in-`Fallback` boot persists until a `SwitchFallback` chord
/// changes it — so pair it with a `Toggle` chord to switch to `Main` when ready.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StartProfile {
    #[default]
    Main,
    Fallback,
}

/// A top-level chord: physical hardware-bit buttons, **AND-combined** (all held), firing a
/// [`GlobalAction`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalChord {
    pub buttons: Vec<InputSource>,
    pub action: GlobalAction,
}

/// What a global chord does — each variant carries its own params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlobalAction {
    /// Switch main ↔ fallback profile (`Hold` = while held; `Toggle` = latch).
    SwitchFallback { mode: SwitchMode },
    /// Run a headless external program — the escape hatch for system actions (on-screen
    /// keyboard, audio device, …) that keeps the engine free of X/Wayland/DE/audio deps.
    /// Hold/Toggle is N/A (fires on activation).
    CommandExecute {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// Switch-chord semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchMode {
    Hold,
    Toggle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_master_rumble_is_full() {
        assert_eq!(GlobalConfig::default().master_rumble, 100);
    }

    #[test]
    fn global_config_round_trips_ron() {
        let g = GlobalConfig {
            start_profile: StartProfile::Fallback,
            master_rumble: 80,
            led_brightness: Some(50),
            idle_timeout: None,
            chords: vec![
                GlobalChord {
                    buttons: vec![InputSource::Steam, InputSource::RightGrip],
                    action: GlobalAction::SwitchFallback { mode: SwitchMode::Toggle },
                },
                GlobalChord {
                    buttons: vec![InputSource::Steam, InputSource::LeftGrip],
                    action: GlobalAction::CommandExecute {
                        command: "wvkbd".into(),
                        args: vec!["--toggle".into()],
                    },
                },
            ],
        };
        let s = ron::to_string(&g).unwrap();
        assert_eq!(ron::from_str::<GlobalConfig>(&s).unwrap(), g);
    }
}
