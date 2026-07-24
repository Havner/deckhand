//! Button and axis taxonomy (PLAN §1.4, §1.5).
//!
//! [`Buttons`] (bitflags) and [`Button`] (one variant per bit) are two views of
//! the same unified superset. [`GordonButtons`] / [`NeptuneButtons`] are the raw
//! per-device wire bitfields, folded into the unified set during conversion.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

bitflags::bitflags! {
    /// Unified button superset across all supported devices (PLAN §1.4).
    ///
    /// Buttons a given device lacks are simply never set.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct Buttons: u64 {
        const A            = 1 << 0;
        const B            = 1 << 1;
        const X            = 1 << 2;
        const Y            = 1 << 3;
        const DPAD_UP      = 1 << 4;
        const DPAD_DOWN    = 1 << 5;
        const DPAD_LEFT    = 1 << 6;
        const DPAD_RIGHT   = 1 << 7;
        const L1           = 1 << 8;
        const R1           = 1 << 9;
        const L2           = 1 << 10;
        const R2           = 1 << 11;
        const L4           = 1 << 12;
        const R4           = 1 << 13;
        const L5           = 1 << 14;
        const R5           = 1 << 15;
        const MENU         = 1 << 16;
        const OPTIONS      = 1 << 17;
        const STEAM        = 1 << 18;
        const QUICK_ACCESS = 1 << 19;
        const LPAD_PRESS   = 1 << 20;
        const RPAD_PRESS   = 1 << 21;
        const LPAD_TOUCH   = 1 << 22;
        const RPAD_TOUCH   = 1 << 23;
        const LSTICK_PRESS = 1 << 24;
        const RSTICK_PRESS = 1 << 25;
        const LSTICK_TOUCH = 1 << 26;
        const RSTICK_TOUCH = 1 << 27;
    }
}

bitflags::bitflags! {
    /// Raw Gordon (original Steam Controller) button bits, packed as
    /// `buttons0 | buttons1 << 8 | buttons2 << 16` (PLAN §1.4).
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct GordonButtons: u32 {
        // buttons0
        const R2           = 1 << 0;
        const L2           = 1 << 1;
        const R1           = 1 << 2;
        const L1           = 1 << 3;
        const Y            = 1 << 4;
        const B            = 1 << 5;
        const X            = 1 << 6;
        const A            = 1 << 7;
        // buttons1
        const DPAD_UP      = 1 << 8;
        const DPAD_RIGHT   = 1 << 9;
        const DPAD_LEFT    = 1 << 10;
        const DPAD_DOWN    = 1 << 11;
        const MENU         = 1 << 12; // "prev" / SELECT
        const STEAM        = 1 << 13;
        const OPTIONS      = 1 << 14; // "next" / START
        const L4           = 1 << 15;
        // buttons2
        const R4           = 1 << 16;
        const LPAD_PRESS   = 1 << 17;
        const RPAD_PRESS   = 1 << 18;
        const LPAD_TOUCH   = 1 << 19;
        const RPAD_TOUCH   = 1 << 20;
        const LSTICK_PRESS = 1 << 22;
        const LPAD_AND_JOY = 1 << 23;
    }
}

bitflags::bitflags! {
    /// Raw Neptune (Steam Deck) button bits, packed from `buttons0..6`
    /// (bytes 0x08..0x0E, PLAN §1.4). Byte N occupies bits `8*N..`.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct NeptuneButtons: u64 {
        // buttons0
        const R2           = 1 << 0;
        const L2           = 1 << 1;
        const R1           = 1 << 2;
        const L1           = 1 << 3;
        const Y            = 1 << 4;
        const B            = 1 << 5;
        const X            = 1 << 6;
        const A            = 1 << 7;
        // buttons1
        const DPAD_UP      = 1 << 8;
        const DPAD_RIGHT   = 1 << 9;
        const DPAD_LEFT    = 1 << 10;
        const DPAD_DOWN    = 1 << 11;
        const MENU         = 1 << 12;
        const STEAM        = 1 << 13;
        const OPTIONS      = 1 << 14;
        const L5           = 1 << 15;
        // buttons2
        const R5           = 1 << 16;
        const LPAD_PRESS   = 1 << 17;
        const RPAD_PRESS   = 1 << 18;
        const LPAD_TOUCH   = 1 << 19;
        const RPAD_TOUCH   = 1 << 20;
        const LSTICK_PRESS = 1 << 22;
        // buttons3
        const RSTICK_PRESS = 1 << 26; // bit 2 of byte 3
        // buttons5 (byte 0x0D → bits 40..)
        const L4           = 1 << 41;
        const R4           = 1 << 42;
        const LSTICK_TOUCH = 1 << 46;
        const RSTICK_TOUCH = 1 << 47;
        // buttons6 (byte 0x0E → bits 48..)
        const QUICK_ACCESS = 1 << 50;
    }
}

/// A single unified button (one variant per [`Buttons`] bit).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum Button {
    A,
    B,
    X,
    Y,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    L1,
    R1,
    L2,
    R2,
    L4,
    R4,
    L5,
    R5,
    Menu,
    Options,
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
    /// The [`Buttons`] flag corresponding to this button.
    pub fn flag(&self) -> Buttons {
        match self {
            Button::A => Buttons::A,
            Button::B => Buttons::B,
            Button::X => Buttons::X,
            Button::Y => Buttons::Y,
            Button::DpadUp => Buttons::DPAD_UP,
            Button::DpadDown => Buttons::DPAD_DOWN,
            Button::DpadLeft => Buttons::DPAD_LEFT,
            Button::DpadRight => Buttons::DPAD_RIGHT,
            Button::L1 => Buttons::L1,
            Button::R1 => Buttons::R1,
            Button::L2 => Buttons::L2,
            Button::R2 => Buttons::R2,
            Button::L4 => Buttons::L4,
            Button::R4 => Buttons::R4,
            Button::L5 => Buttons::L5,
            Button::R5 => Buttons::R5,
            Button::Menu => Buttons::MENU,
            Button::Options => Buttons::OPTIONS,
            Button::Steam => Buttons::STEAM,
            Button::QuickAccess => Buttons::QUICK_ACCESS,
            Button::LPadPress => Buttons::LPAD_PRESS,
            Button::RPadPress => Buttons::RPAD_PRESS,
            Button::LPadTouch => Buttons::LPAD_TOUCH,
            Button::RPadTouch => Buttons::RPAD_TOUCH,
            Button::LStickPress => Buttons::LSTICK_PRESS,
            Button::RStickPress => Buttons::RSTICK_PRESS,
            Button::LStickTouch => Buttons::LSTICK_TOUCH,
            Button::RStickTouch => Buttons::RSTICK_TOUCH,
        }
    }

    /// Every button, in bit order — for iterating diffs.
    pub const ALL: [Button; 28] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::DpadUp,
        Button::DpadDown,
        Button::DpadLeft,
        Button::DpadRight,
        Button::L1,
        Button::R1,
        Button::L2,
        Button::R2,
        Button::L4,
        Button::R4,
        Button::L5,
        Button::R5,
        Button::Menu,
        Button::Options,
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

/// Normalized analog channels (PLAN §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum Axis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftPadX,
    LeftPadY,
    RightPadX,
    RightPadY,
    LeftTrigger,
    RightTrigger,
    LeftPadPressure,
    RightPadPressure,
}

impl Axis {
    /// Every analog axis — for iterating diffs.
    pub const ALL: [Axis; 12] = [
        Axis::LeftStickX,
        Axis::LeftStickY,
        Axis::RightStickX,
        Axis::RightStickY,
        Axis::LeftPadX,
        Axis::LeftPadY,
        Axis::RightPadX,
        Axis::RightPadY,
        Axis::LeftTrigger,
        Axis::RightTrigger,
        Axis::LeftPadPressure,
        Axis::RightPadPressure,
    ];
}
