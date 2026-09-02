//! The config-facing core of the input vocabulary: the [`Button`] enum. This is the part `config`
//! needs (to name hardware buttons in chords/gaters) - it pulls in nothing else. The mapper-facing
//! rest (the `Buttons` bitfield, value types, and the `ControllerState` snapshot) lives in `state`.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A single unified controller button - one variant per hardware button bit, across all supported
/// devices (a button a given device lacks is simply never pressed). Shape-independent superset.
///
/// `LB/RB/LT/RT` are the same short abbreviations `Buttons` uses (hence the `upper_case_acronyms`
/// allow); the grips use `LGrip`/`LGrip2`. Display labels are the consumer's concern (the UI owns
/// them) - this stays presentation-free, like `vocab-out`.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Button {
    A,
    B,
    X,
    Y,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    LB,
    RB,
    LT,
    RT,
    LGrip,
    RGrip,
    LGrip2,
    RGrip2,
    View,
    Menu,
    Steam,
    QuickAccess,
    LPadPress,
    RPadPress,
    LPadTouch,
    RPadTouch,
    LStickPress,
    RStickPress,
    LStickTouch,
    RStickTouch,
    LGripTouch,  // capacitive left handle/grip touch (Triton)
    RGripTouch,  // capacitive right handle/grip touch (Triton)
}

impl Button {
    /// Every button, in bit order - for iterating diffs / UI listings.
    pub const ALL: [Button; 30] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::DpadUp,
        Button::DpadDown,
        Button::DpadLeft,
        Button::DpadRight,
        Button::LB,
        Button::RB,
        Button::LT,
        Button::RT,
        Button::LGrip,
        Button::RGrip,
        Button::LGrip2,
        Button::RGrip2,
        Button::View,
        Button::Menu,
        Button::Steam,
        Button::QuickAccess,
        Button::LPadPress,
        Button::RPadPress,
        Button::LPadTouch,
        Button::RPadTouch,
        Button::LStickPress,
        Button::RStickPress,
        Button::LStickTouch,
        Button::RStickTouch,
        Button::LGripTouch,
        Button::RGripTouch,
    ];
}
