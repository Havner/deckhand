//! Commands (activators) + per-command settings (PLAN §3, Round C).
//!
//! Every button-like node (a standalone [`InputSource`](crate::InputSource) button, a
//! `ButtonPad` member, or a behavior's virtual button) holds a `Vec<Command>` — several
//! activators per node. A [`Command`] *is* an activator: a type + settings + an ordered
//! action combo.

use serde::{Deserialize, Serialize};

use crate::action::Action;

/// One command (activator) on a button-like node.
///
/// `actions` is an ordered combo: the 1st is the "command", the rest are **subcommands**
/// (action-only) for key combos — pressed in order, released in reverse (`Ctrl↓ C↓ /
/// C↑ Ctrl↑`), so modifiers wrap the key. `settings` apply to the whole combo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub activator: Activator,
    /// Ordered combo; ≥1 in a well-formed config (validation flags empty).
    pub actions: Vec<Action>,
    #[serde(default)]
    pub settings: CommandSettings,
}

/// The trigger condition for a [`Command`] (Round C). (Cycle / double-and-hold deferred.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activator {
    /// Active while the input is held (press → down, release → up). The default.
    ///
    /// `interruptible`: suppress this command when another command on the same node fires
    /// (`Long`/`Double`/…) — so one key on short, another on long, without the short firing.
    /// If unset, it fires whenever held regardless of siblings. Meaningful only here (a
    /// short-vs-long distinction needs the held Regular), hence a field of the variant.
    Regular { interruptible: bool },
    /// Fires after the input is held for at least `hold_ms`.
    Long { hold_ms: u32 },
    /// Fires on a second press within `window_ms`.
    Double { window_ms: u32 },
    /// Fires once, on the initial press edge.
    Start,
    /// Fires on release.
    Release,
}

/// Per-command settings (Round C). Applicability is activator-dependent (`turbo` is moot on
/// `Release`) — the UI shows what applies; validation may warn. (Interruptibility is not here:
/// it's Regular-only, so it's a field of [`Activator::Regular`].)
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandSettings {
    /// Latch the combo on/off per activation instead of hold-to-hold.
    pub toggle: bool,
    /// Re-fire the combo while held (rapid-fire); `None` = off.
    pub turbo: Option<Turbo>,
    /// Haptic pulse on the combo's action edges.
    pub haptics: Haptics,
}

/// Turbo / rapid-fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turbo {
    /// Interval between re-fires, in milliseconds.
    pub interval_ms: u32,
}

/// Haptic feedback for a command (Round C). Fires on the **action's** edges (so `Long`
/// pulses after its timeout, `Turbo` repeats it), on the [`side`](crate::InputSource::side)
/// of the triggering input — a singular pulse at one of three strengths.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Haptics {
    pub on: HapticEdge,
    pub strength: HapticStrength,
}

/// When a command's haptic fires.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HapticEdge {
    #[default]
    Off,
    OnPress,
    OnRelease,
    Both,
}

/// Haptic pulse strength (mapped to the trackpad-pulse duty at runtime).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HapticStrength {
    Low,
    #[default]
    Medium,
    High,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vocab_out::Key;

    #[test]
    fn ctrl_c_combo_round_trips_ron() {
        // Ctrl (command) + C (subcommand), a long-press that pulses.
        let cmd = Command {
            activator: Activator::Long { hold_ms: 250 },
            actions: vec![Action::Key(Key::LeftCtrl), Action::Key(Key::C)],
            settings: CommandSettings {
                haptics: Haptics { on: HapticEdge::OnPress, strength: HapticStrength::High },
                ..Default::default()
            },
        };
        let s = ron::to_string(&cmd).unwrap();
        assert_eq!(ron::from_str::<Command>(&s).unwrap(), cmd);
    }

    #[test]
    fn defaults_are_all_off() {
        let d = CommandSettings::default();
        assert!(!d.toggle && d.turbo.is_none());
        assert_eq!(d.haptics.on, HapticEdge::Off);
    }
}
