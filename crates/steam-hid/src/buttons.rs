//! Button and axis taxonomy (PLAN §1.4, §1.5).
//!
//! [`Buttons`] (bitflags) and [`Button`] (one variant per bit) are two views of
//! the same unified superset. [`GordonButtons`] / [`NeptuneButtons`] are the raw
//! per-device wire bitfields, folded into the unified set during conversion —
//! Gordon over **USB and Bluetooth share [`GordonButtons`]** (same bit layout).
//! They share one naming scheme (PLAN §1.4): `LB/RB` bumpers, `LT/RT` trigger
//! full-pulls, `LGRIP/RGRIP` (+ `LGRIP2/RGRIP2` on the Deck) back buttons,
//! `View/Menu` the two small top buttons.

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
        const LB           = 1 << 8;  // left bumper
        const RB           = 1 << 9;  // right bumper
        const LT           = 1 << 10; // left trigger full-pull
        const RT           = 1 << 11; // right trigger full-pull
        const LGRIP        = 1 << 12; // left back grip
        const RGRIP        = 1 << 13; // right back grip
        const LGRIP2       = 1 << 14; // left back grip 2 (Deck)
        const RGRIP2       = 1 << 15; // right back grip 2 (Deck)
        const VIEW         = 1 << 16;
        const MENU         = 1 << 17;
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
    ///
    /// Shared by **USB and Bluetooth** Gordon — identical bit layout; over BLE the
    /// same bits arrive in the compact input's button chunk. `dpad` (bits 8..11) is
    /// firmware-synthesized from left-pad directional clicks on both transports.
    /// `LPAD_AND_JOY` and the shared left-click bit are USB-wire multiplex artifacts
    /// resolved in `parse_gordon` (never set over BLE).
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct GordonButtons: u32 {
        // buttons0
        const RT           = 1 << 0; // right trigger full-pull
        const LT           = 1 << 1; // left trigger full-pull
        const RB           = 1 << 2;
        const LB           = 1 << 3;
        const Y            = 1 << 4;
        const B            = 1 << 5;
        const X            = 1 << 6;
        const A            = 1 << 7;
        // buttons1
        const DPAD_UP      = 1 << 8;
        const DPAD_RIGHT   = 1 << 9;
        const DPAD_LEFT    = 1 << 10;
        const DPAD_DOWN    = 1 << 11;
        const VIEW         = 1 << 12; // BTN_SELECT — Valve "View" (kernel "menu left")
        const STEAM        = 1 << 13;
        const MENU         = 1 << 14; // BTN_START — Valve "Menu" (kernel "menu right")
        const LGRIP        = 1 << 15;
        // buttons2
        const RGRIP        = 1 << 16;
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
        const RT           = 1 << 0; // right trigger full-pull
        const LT           = 1 << 1; // left trigger full-pull
        const RB           = 1 << 2;
        const LB           = 1 << 3;
        const Y            = 1 << 4;
        const B            = 1 << 5;
        const X            = 1 << 6;
        const A            = 1 << 7;
        // buttons1
        const DPAD_UP      = 1 << 8;
        const DPAD_RIGHT   = 1 << 9;
        const DPAD_LEFT    = 1 << 10;
        const DPAD_DOWN    = 1 << 11;
        const VIEW         = 1 << 12;
        const STEAM        = 1 << 13;
        const MENU         = 1 << 14;
        const LGRIP2       = 1 << 15;
        // buttons2
        const RGRIP2       = 1 << 16;
        const LPAD_PRESS   = 1 << 17;
        const RPAD_PRESS   = 1 << 18;
        const LPAD_TOUCH   = 1 << 19;
        const RPAD_TOUCH   = 1 << 20;
        const LSTICK_PRESS = 1 << 22;
        // buttons3
        const RSTICK_PRESS = 1 << 26; // bit 2 of byte 3
        // buttons5 (byte 0x0D → bits 40..)
        const LGRIP        = 1 << 41;
        const RGRIP        = 1 << 42;
        const LSTICK_TOUCH = 1 << 46;
        const RSTICK_TOUCH = 1 << 47;
        // buttons6 (byte 0x0E → bits 48..)
        const QUICK_ACCESS = 1 << 50;
    }
}

/// A single unified button (one variant per [`Buttons`] bit).
///
/// `LB/RB/LT/RT` are deliberately the same short abbreviations as the [`Buttons`]
/// bitflags (hence the `upper_case_acronyms` allow); the grips use `LGrip`/`LGrip2`.
#[allow(clippy::upper_case_acronyms)]
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
            Button::LB => Buttons::LB,
            Button::RB => Buttons::RB,
            Button::LT => Buttons::LT,
            Button::RT => Buttons::RT,
            Button::LGrip => Buttons::LGRIP,
            Button::RGrip => Buttons::RGRIP,
            Button::LGrip2 => Buttons::LGRIP2,
            Button::RGrip2 => Buttons::RGRIP2,
            Button::View => Buttons::VIEW,
            Button::Menu => Buttons::MENU,
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
