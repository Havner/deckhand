//! Button and axis taxonomy (PLAN §1.4, §1.5).
//!
//! [`Buttons`] (bitflags) and [`Button`] (one variant per bit) are two views of the
//! same unified superset — the device-independent button vocabulary. The **raw
//! per-device wire bitfields** (`GordonButtons`/`NeptuneButtons`/`TritonButtons`)
//! live in `protocol`, next to the wire structs that carry them; the folds into this
//! unified set ([`map_gordon`]/[`map_neptune`]/[`map_triton`]) live **here**, next to
//! the target bitflags (the decode layer in `state.rs` only *calls* them). One naming
//! scheme throughout (PLAN §1.4): `LB/RB` bumpers, `LT/RT` trigger full-pulls,
//! `LGRIP/RGRIP` (+ `LGRIP2/RGRIP2` on the Deck) back buttons, `View/Menu` the two
//! small top buttons.

use crate::protocol::{GordonButtons, NeptuneButtons, TritonButtons};

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

/// Fold Gordon's per-device button bits into the unified [`Buttons`] superset.
///
/// Serves **both** USB and Bluetooth Gordon (they share [`GordonButtons`]). A plain 1:1 fold: the USB
/// left-click multiplex is already resolved in `state::from_gordon`, and BLE has none, so no
/// touch-gating happens here.
pub(crate) fn map_gordon(g: &GordonButtons) -> Buttons {
    let mut out = Buttons::empty();
    let mut set = |cond: bool, flag: Buttons| {
        if cond {
            out |= flag;
        }
    };
    set(g.contains(GordonButtons::A), Buttons::A);
    set(g.contains(GordonButtons::B), Buttons::B);
    set(g.contains(GordonButtons::X), Buttons::X);
    set(g.contains(GordonButtons::Y), Buttons::Y);
    set(g.contains(GordonButtons::DPAD_UP), Buttons::DPAD_UP);
    set(g.contains(GordonButtons::DPAD_DOWN), Buttons::DPAD_DOWN);
    set(g.contains(GordonButtons::DPAD_LEFT), Buttons::DPAD_LEFT);
    set(g.contains(GordonButtons::DPAD_RIGHT), Buttons::DPAD_RIGHT);
    set(g.contains(GordonButtons::LB), Buttons::LB);
    set(g.contains(GordonButtons::RB), Buttons::RB);
    set(g.contains(GordonButtons::LT), Buttons::LT);
    set(g.contains(GordonButtons::RT), Buttons::RT);
    set(g.contains(GordonButtons::LGRIP), Buttons::LGRIP);
    set(g.contains(GordonButtons::RGRIP), Buttons::RGRIP);
    set(g.contains(GordonButtons::VIEW), Buttons::VIEW);
    set(g.contains(GordonButtons::MENU), Buttons::MENU);
    set(g.contains(GordonButtons::STEAM), Buttons::STEAM);
    // Pad/stick clicks arrive already de-multiplexed (USB in `state::from_gordon`; BLE has no multiplex).
    set(g.contains(GordonButtons::LPAD_PRESS), Buttons::LPAD_PRESS);
    set(g.contains(GordonButtons::RPAD_PRESS), Buttons::RPAD_PRESS);
    set(g.contains(GordonButtons::LPAD_TOUCH), Buttons::LPAD_TOUCH);
    set(g.contains(GordonButtons::RPAD_TOUCH), Buttons::RPAD_TOUCH);
    set(g.contains(GordonButtons::LSTICK_PRESS), Buttons::LSTICK_PRESS);
    out
}

/// Fold Neptune's per-device button bits into the unified [`Buttons`] superset (1:1 — the Deck has
/// dedicated press/touch bits and its raw layout already matches the unified naming).
pub(crate) fn map_neptune(n: &NeptuneButtons) -> Buttons {
    let mut out = Buttons::empty();
    let mut set = |cond: bool, flag: Buttons| {
        if cond {
            out |= flag;
        }
    };
    set(n.contains(NeptuneButtons::A), Buttons::A);
    set(n.contains(NeptuneButtons::B), Buttons::B);
    set(n.contains(NeptuneButtons::X), Buttons::X);
    set(n.contains(NeptuneButtons::Y), Buttons::Y);
    set(n.contains(NeptuneButtons::DPAD_UP), Buttons::DPAD_UP);
    set(n.contains(NeptuneButtons::DPAD_DOWN), Buttons::DPAD_DOWN);
    set(n.contains(NeptuneButtons::DPAD_LEFT), Buttons::DPAD_LEFT);
    set(n.contains(NeptuneButtons::DPAD_RIGHT), Buttons::DPAD_RIGHT);
    set(n.contains(NeptuneButtons::LB), Buttons::LB);
    set(n.contains(NeptuneButtons::RB), Buttons::RB);
    set(n.contains(NeptuneButtons::LT), Buttons::LT);
    set(n.contains(NeptuneButtons::RT), Buttons::RT);
    set(n.contains(NeptuneButtons::LGRIP), Buttons::LGRIP);
    set(n.contains(NeptuneButtons::RGRIP), Buttons::RGRIP);
    set(n.contains(NeptuneButtons::LGRIP2), Buttons::LGRIP2);
    set(n.contains(NeptuneButtons::RGRIP2), Buttons::RGRIP2);
    set(n.contains(NeptuneButtons::VIEW), Buttons::VIEW);
    set(n.contains(NeptuneButtons::MENU), Buttons::MENU);
    set(n.contains(NeptuneButtons::STEAM), Buttons::STEAM);
    set(n.contains(NeptuneButtons::QUICK_ACCESS), Buttons::QUICK_ACCESS);
    set(n.contains(NeptuneButtons::LPAD_PRESS), Buttons::LPAD_PRESS);
    set(n.contains(NeptuneButtons::RPAD_PRESS), Buttons::RPAD_PRESS);
    set(n.contains(NeptuneButtons::LPAD_TOUCH), Buttons::LPAD_TOUCH);
    set(n.contains(NeptuneButtons::RPAD_TOUCH), Buttons::RPAD_TOUCH);
    set(n.contains(NeptuneButtons::LSTICK_PRESS), Buttons::LSTICK_PRESS);
    set(n.contains(NeptuneButtons::RSTICK_PRESS), Buttons::RSTICK_PRESS);
    set(n.contains(NeptuneButtons::LSTICK_TOUCH), Buttons::LSTICK_TOUCH);
    set(n.contains(NeptuneButtons::RSTICK_TOUCH), Buttons::RSTICK_TOUCH);
    out
}

/// Fold Triton's per-device button bits into the unified [`Buttons`] superset (1:1). The two
/// capacitive **grip-touch** sensors fold into the `L/RGRIP_TOUCH` bits (a Triton-only input).
pub(crate) fn map_triton(t: &TritonButtons) -> Buttons {
    let mut out = Buttons::empty();
    let mut set = |cond: bool, flag: Buttons| {
        if cond {
            out |= flag;
        }
    };
    set(t.contains(TritonButtons::A), Buttons::A);
    set(t.contains(TritonButtons::B), Buttons::B);
    set(t.contains(TritonButtons::X), Buttons::X);
    set(t.contains(TritonButtons::Y), Buttons::Y);
    set(t.contains(TritonButtons::DPAD_UP), Buttons::DPAD_UP);
    set(t.contains(TritonButtons::DPAD_DOWN), Buttons::DPAD_DOWN);
    set(t.contains(TritonButtons::DPAD_LEFT), Buttons::DPAD_LEFT);
    set(t.contains(TritonButtons::DPAD_RIGHT), Buttons::DPAD_RIGHT);
    set(t.contains(TritonButtons::LB), Buttons::LB);
    set(t.contains(TritonButtons::RB), Buttons::RB);
    set(t.contains(TritonButtons::LT), Buttons::LT);
    set(t.contains(TritonButtons::RT), Buttons::RT);
    set(t.contains(TritonButtons::LGRIP), Buttons::LGRIP);
    set(t.contains(TritonButtons::RGRIP), Buttons::RGRIP);
    set(t.contains(TritonButtons::LGRIP2), Buttons::LGRIP2);
    set(t.contains(TritonButtons::RGRIP2), Buttons::RGRIP2);
    set(t.contains(TritonButtons::LGRIP_TOUCH), Buttons::LGRIP_TOUCH);
    set(t.contains(TritonButtons::RGRIP_TOUCH), Buttons::RGRIP_TOUCH);
    set(t.contains(TritonButtons::VIEW), Buttons::VIEW);
    set(t.contains(TritonButtons::MENU), Buttons::MENU);
    set(t.contains(TritonButtons::STEAM), Buttons::STEAM);
    set(t.contains(TritonButtons::QUICK_ACCESS), Buttons::QUICK_ACCESS);
    set(t.contains(TritonButtons::LPAD_PRESS), Buttons::LPAD_PRESS);
    set(t.contains(TritonButtons::RPAD_PRESS), Buttons::RPAD_PRESS);
    set(t.contains(TritonButtons::LPAD_TOUCH), Buttons::LPAD_TOUCH);
    set(t.contains(TritonButtons::RPAD_TOUCH), Buttons::RPAD_TOUCH);
    set(t.contains(TritonButtons::LSTICK_PRESS), Buttons::LSTICK_PRESS);
    set(t.contains(TritonButtons::RSTICK_PRESS), Buttons::RSTICK_PRESS);
    set(t.contains(TritonButtons::LSTICK_TOUCH), Buttons::LSTICK_TOUCH);
    set(t.contains(TritonButtons::RSTICK_TOUCH), Buttons::RSTICK_TOUCH);
    out
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
