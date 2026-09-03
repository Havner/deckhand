//! Commands (activators) + per-command settings (PLAN 3, Round C).
//!
//! Every button-like node (a standalone [`InputSource`](crate::InputSource) button, a
//! `ButtonPad` member, or a behavior's virtual button) holds a `Vec<Command>` - several
//! activators per node. A [`Command`] *is* an activator: a type + settings + an ordered
//! action combo.

use serde::{Deserialize, Serialize};

use crate::action::Action;

/// One command (activator) on a button-like node.
///
/// `actions` is a combo: the 1st is the "command", the rest are **subcommands** (action-only) for
/// key combos (e.g. `Ctrl` + `C`). All of them are **held together** while the command fires - they
/// are *not* sequenced; the engine's level reconciler emits them as one set per tick, sorted by the
/// output enum's order, so **declared order does not affect output** (modifier combos work only
/// because modifiers occupy the lowest `Key` ordinals). `settings` apply to the whole combo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub activator: Activator,
    /// The combo; >=1 in a well-formed config (validation flags empty).
    pub actions: Vec<Action>,
    #[serde(default)]
    pub settings: CommandSettings,
}

/// The trigger condition for a [`Command`] (Round C). (Cycle / double-and-hold deferred.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activator {
    /// Active while the input is held (press -> down, release -> up). The default.
    ///
    /// `interruptible`: suppress this command when another command on the same node fires
    /// (`Long`/`Double`/...) - so one key on short, another on long, without the short firing.
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
/// `Release`) - the UI shows what applies; validation may warn. (Interruptibility is not here:
/// it's Regular-only, so it's a field of [`Activator::Regular`].)
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandSettings {
    /// Latch the combo on/off per activation instead of hold-to-hold.
    pub toggle: bool,
    /// Re-fire the combo while held (rapid-fire); `None` = off.
    pub turbo: Option<Turbo>,
    /// Tactile/audible feedback on the combo's action edges (PLAN 7).
    pub feedback: Feedback,
}

/// Turbo / rapid-fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turbo {
    /// Interval between re-fires, in milliseconds.
    pub interval_ms: u32,
}

/// Command feedback (PLAN 7): an independent [`Effect`] per action edge, played on the trackpad
/// haptic actuator. Because a click and a tone would fight the same actuator, each edge is a single
/// medium (PLAN 7.1). Fires on the **action's** edges (so `Long` fires after its timeout, `Turbo`
/// repeats it); haptics play on the [`side`](crate::InputSource::side) of the triggering input, audio
/// on both. Each edge is optional (its own UI checkbox); `None`/`None` = no feedback.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Feedback {
    pub on_press: Option<Effect>,
    pub on_release: Option<Effect>,
}

/// One feedback effect: a medium plus its pattern. Doubles/triples are short patterns the reader's
/// sequencer plays (PLAN 7.4); each note carries its own [`Click`]/[`Tone`], so a pattern can mix
/// strengths/pitches (e.g. a rising two-beep, or a morse-like short-long). A [`Click`] has no length
/// (an impulse); a [`Tone`] carries pitch x length, so only audio has short/long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Haptic(Click),
    HapticDouble(Click, Click),
    HapticTriple(Click, Click, Click),
    Audio(Tone),
    AudioDouble(Tone, Tone),
    AudioTriple(Tone, Tone, Tone),
    Chirp(Sweep),
}

/// A haptic click's strength (each click in a pattern sets its own - hence not "the haptic's
/// strength"). The reader maps it to the trackpad-pulse duty / gain per device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Click {
    Weak,
    #[default]
    Medium,
    Strong,
}

/// An audio tone: pitch x length. The reader maps the pitch to a per-device frequency (the tone
/// ceilings differ) and the length to a shared `SHORT_MS`/`LONG_MS` duration (PLAN 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tone {
    ShortLow,
    #[default]
    ShortMedium,
    ShortHigh,
    LongLow,
    LongMedium,
    LongHigh,
}

/// A trackpad-actuator sweep ("chirp") - a rising/falling glide, three shapes. No-op on Gordon (no
/// sweep path). The reader maps each to concrete start/end/duration per device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Sweep {
    #[default]
    Up1,
    Down1,
    Up2,
    Down2,
    Up3,
    Down3,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vocab_out::Key;

    #[test]
    fn ctrl_c_combo_round_trips_ron() {
        // Ctrl (command) + C (subcommand), a long-press with a double-click on press.
        let cmd = Command {
            activator: Activator::Long { hold_ms: 250 },
            actions: vec![Action::Key(Key::LeftCtrl), Action::Key(Key::C)],
            settings: CommandSettings {
                feedback: Feedback {
                    on_press: Some(Effect::HapticDouble(Click::Strong, Click::Weak)),
                    on_release: None,
                },
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
        assert_eq!(d.feedback, Feedback::default());
        assert!(d.feedback.on_press.is_none() && d.feedback.on_release.is_none());
    }
}
