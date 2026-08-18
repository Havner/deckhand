//! `vocab-hid` — the hardware-independent **input vocabulary**: the unified set of controller
//! button bits, shared by `config` (which names them in chords/gaters) and `steam-hid` (which
//! produces them from the per-device wire bitfields). The input counterpart of `vocab-out`
//! (outputs): `vocab-out` pairs with `virt-out`, `vocab-hid` pairs with `steam-hid`.
//!
//! Deliberately **dependency-free** (just the enum + optional serde) so `config` can name hardware
//! buttons without pulling in `steam-hid` (and its `hidapi`/HID platform code). The bit ↔ enum
//! mapping (`Button` ↔ `steam-hid::Buttons`) lives in `steam-hid`, next to the bitflags.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A single unified controller button — one variant per hardware button bit, across all supported
/// devices (a button a given device lacks is simply never pressed). Shape-independent superset.
///
/// `LB/RB/LT/RT` are the same short abbreviations `steam-hid`'s `Buttons` bitflags use (hence the
/// `upper_case_acronyms` allow); the grips use `LGrip`/`LGrip2`. Display labels are the consumer's
/// concern (the UI owns them) — this stays presentation-free, like `vocab-out`.
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
}

impl Button {
    /// Every button, in bit order — for iterating diffs / UI listings.
    pub const ALL: [Button; 28] = [
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
    ];
}
