//! Button and axis taxonomy (PLAN §1.4, §1.5).
//!
//! [`Buttons`] (bitflags) and [`Button`] (one variant per bit) are two views of the
//! same unified superset — the device-independent button vocabulary. The **raw
//! per-device wire bitfields** (`GordonButtons`/`NeptuneButtons`/`TritonButtons`)
//! live in `protocol`, next to the reports that carry them; `state.rs` folds each
//! into this unified set. One naming scheme throughout (PLAN §1.4): `LB/RB` bumpers,
//! `LT/RT` trigger full-pulls, `LGRIP/RGRIP` (+ `LGRIP2/RGRIP2` on the Deck) back
//! buttons, `View/Menu` the two small top buttons.

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
        const LGRIP_TOUCH  = 1 << 28; // capacitive left handle/grip touch (Triton)
        const RGRIP_TOUCH  = 1 << 29; // capacitive right handle/grip touch (Triton)
    }
}

// The unified [`Button`] enum lives in `vocab-hid` (the shared input vocabulary) so `config` can
// name hardware buttons in chords/gaters without depending on `steam-hid`. Re-exported here so this
// crate's own consumers keep using `steam_hid::Button`. The `Button` ↔ [`Buttons`] mapping stays
// here, next to the bitflags.
pub use vocab_hid::Button;

/// The [`Buttons`] flag corresponding to a unified [`Button`]. (A free fn rather than a method: the
/// enum is foreign to this crate now, and the mapping belongs with the bitflags anyway.)
pub fn button_flag(b: &Button) -> Buttons {
    match b {
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
        Button::LGripTouch => Buttons::LGRIP_TOUCH,
        Button::RGripTouch => Buttons::RGRIP_TOUCH,
    }
}

/// Normalized analog channels (PLAN §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
