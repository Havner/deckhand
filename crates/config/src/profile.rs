//! The profile document — Tier A (PLAN §3, rounds A–E).
//!
//! A [`ConfigDoc`] is **one profile** (one per file). It holds action sets (full-controller
//! modes, one active at a time), each with layers (stackable overlays) and per-source
//! bindings, plus the per-profile rumble feel. It is **device-independent** — no `Shape`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::binding::SourceBinding;
use crate::input::InputSource;
use crate::settings::Curve;

/// One profile: a complete mapping. The **first** action set is active on load;
/// layers start inactive and are applied by `HoldLayer`/`AddLayer`/`RemoveLayer` actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDoc {
    /// On-disk format version (versioning/migration deferred to stable — PLAN §3).
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub name: String,
    /// At least one; the first is the initially-active set.
    pub action_sets: Vec<ActionSet>,
    #[serde(default)]
    pub rumble: RumbleSettings,
}

/// A full-controller mode; exactly one active at a time (`ChangeActionSet` swaps it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSet {
    pub name: String,
    /// Base bindings for this set (keyed by logical input; deterministic order).
    #[serde(default)]
    pub bindings: BTreeMap<InputSource, SourceBinding>,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

/// A partial overlay on top of the active action set — stackable, applied/removed by
/// actions. Only the inputs it names are overridden; the rest fall through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    #[serde(default)]
    pub bindings: BTreeMap<InputSource, SourceBinding>,
}

/// Per-profile rumble feel (the back-channel: game rumble → Gordon trackpad haptics). The
/// profile tunes the texture; `GlobalConfig::master_rumble` scales it (Round E).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RumbleSettings {
    /// Pulse frequency, Hz.
    pub hz: u16,
    /// Strength, percent (before the global master %).
    pub strength: u8,
    /// Strength → drive response curve (the non-linear Xbox-strength map is a future tweak).
    pub curve: Curve,
}

impl Default for RumbleSettings {
    fn default() -> Self {
        RumbleSettings { hz: 60, strength: 100, curve: Curve::Linear }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::command::{Activator, Command};
    use vocab::GamepadButton;

    #[test]
    fn config_doc_round_trips_ron() {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            InputSource::LeftBumper,
            SourceBinding::Button {
                commands: vec![Command {
                    activator: Activator::Regular,
                    actions: vec![Action::GamepadButton(GamepadButton::LeftBumper)],
                    settings: Default::default(),
                }],
            },
        );
        bindings.insert(
            InputSource::RightPad,
            SourceBinding::AsMouse { settings: Default::default() },
        );

        let doc = ConfigDoc {
            version: 0,
            name: "Default".into(),
            action_sets: vec![ActionSet { name: "Game".into(), bindings, layers: vec![] }],
            rumble: RumbleSettings::default(),
        };

        let s = ron::to_string(&doc).unwrap();
        assert_eq!(ron::from_str::<ConfigDoc>(&s).unwrap(), doc);
    }

    #[test]
    fn rumble_defaults() {
        let r = RumbleSettings::default();
        assert_eq!((r.hz, r.strength), (60, 100));
        assert_eq!(r.curve, Curve::Linear);
    }
}
