//! The mapper-facing input vocabulary: the unified [`ControllerState`] snapshot the mapper reads,
//! and everything it is made of - the [`Buttons`] bitfield (with [`button_flag`], its bridge to the
//! [`Button`](crate::Button) enum), the [`Axis`] channels, and the geometry/value types. `steam-hid`
//! decodes each device's wire frame into a `ControllerState`; the mapper consumes it. The atomic
//! `Button` enum that `config` alone needs lives in `core`.
//!
//! Value types derive `Clone` but **not** `Copy` - clone explicitly where a copy is wanted.

use std::time::Duration;

use crate::core::Button;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// --- IMU scale constants ---

/// IMU accel resolution: raw LSB per 1 g. `accel` reads `+ACCEL_RES_PER_G` on whichever axis
/// points up. The interpretation key for [`ControllerState::accel`]'s raw `i16` fields.
pub const ACCEL_RES_PER_G: f32 = 16384.0;
/// IMU gyro resolution: raw LSB per degree/second. The interpretation key for
/// [`ControllerState::gyro`]'s raw `i16` fields.
pub const GYRO_RES_PER_DPS: f32 = 16.0;

// --- buttons ---

bitflags::bitflags! {
    /// Unified button superset across all supported devices.
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

/// The [`Buttons`] flag corresponding to a unified [`Button`]. (A free fn rather than a method: it
/// bridges the enum view ([`Button`], in `core`) and the bitfield view ([`Buttons`]).)
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

/// Normalized analog channels.
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
    /// Every analog axis - for iterating diffs.
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

// --- values ---

/// A normalized 2D vector (sticks/pads in [`ControllerState`]), components in `-1.0..=1.0`.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// A raw wire 2D vector (i16 components, as decoded from a report).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Vec2i {
    pub x: i16,
    pub y: i16,
}

/// A raw wire 3D vector (i16) - accel / gyro.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Vec3i {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

/// A raw wire quaternion (i16) - device orientation.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Quati {
    pub x: i16,
    pub y: i16,
    pub z: i16,
    pub w: i16,
}

/// A normalized trackpad sample: position in `-1.0..=1.0`, pressure in `0.0..=1.0`.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TrackPad {
    pub pos: Vec2,
    pub pressure: f32,
    pub touched: bool,
}

/// Monotonic capture time of a frame, as elapsed since the device's stream start.
///
/// Relative (not wall-clock) so it is serializable for traces and can drive the engine's injected
/// clock on replay.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Timestamp(pub Duration);

/// A unified, normalized controller snapshot - the mapper's per-frame input.
///
/// Analog inputs are normalized to `f32`; IMU (accel/gyro/orientation) passes through as raw `i16`
/// with documented scale factors ([`ACCEL_RES_PER_G`] / [`GYRO_RES_PER_DPS`]). Battery is *not*
/// here - it is a device-level signal in `steam-hid`.
///
/// **IMU frame (HW-verified):** right-handed, `X=right, Y=forward (toward the nose), Z=up (out of
/// the face)`.
/// - `accel` - specific force; reads `+1g` along whichever axis points up ([`ACCEL_RES_PER_G`]).
/// - `gyro` - angular velocity, `x`=pitch, `y`=roll, `z`=yaw rate ([`GYRO_RES_PER_DPS`]),
///   right-hand rule: pitch-up / yaw-left / roll-right are positive.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ControllerState {
    pub seq: u32,
    pub timestamp: Timestamp,
    pub buttons: Buttons,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_stick: Vec2,
    pub right_stick: Vec2,
    pub left_pad: TrackPad,
    pub right_pad: TrackPad,
    pub accel: Vec3i,
    pub gyro: Vec3i,
    pub orientation: Quati,
}

impl ControllerState {
    /// Read a normalized analog channel (used by diffing).
    pub fn axis(&self, axis: Axis) -> f32 {
        match axis {
            Axis::LeftStickX => self.left_stick.x,
            Axis::LeftStickY => self.left_stick.y,
            Axis::RightStickX => self.right_stick.x,
            Axis::RightStickY => self.right_stick.y,
            Axis::LeftPadX => self.left_pad.pos.x,
            Axis::LeftPadY => self.left_pad.pos.y,
            Axis::RightPadX => self.right_pad.pos.x,
            Axis::RightPadY => self.right_pad.pos.y,
            Axis::LeftTrigger => self.left_trigger,
            Axis::RightTrigger => self.right_trigger,
            Axis::LeftPadPressure => self.left_pad.pressure,
            Axis::RightPadPressure => self.right_pad.pressure,
        }
    }
}
