//! Small value types shared across reports and state (PLAN §1.5).
//!
//! Per project convention these derive `Clone` but **not** `Copy` — clone
//! explicitly where a copy is wanted.

use core::time::Duration;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A normalized 2D vector (sticks/pads in `ControllerState`), components in `-1.0..=1.0`.
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

/// A raw wire 3D vector (i16) — accel / gyro.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Vec3i {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

/// A raw wire quaternion (i16) — device orientation.
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
/// Relative (not wall-clock) so it is serializable for traces and can drive the
/// engine's injected clock on replay (PLAN §1.8, §4).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Timestamp(pub Duration);
