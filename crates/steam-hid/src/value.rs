//! Normalized value vocabulary for the input snapshot (PLAN §1.5).
//!
//! The geometry/analog types that make up a [`crate::ControllerState`]: normalized
//! `Vec2`/`TrackPad`, the raw wire `Vec2i`/`Vec3i`/`Quati`, and `Timestamp`. The
//! structural conversions **from** the packed wire chunks in `protocol` (`WireVec2`
//! etc.) into these raw types live here, next to the target types; the decode layer
//! (`state.rs`) then applies its own *policy* (normalization divisors, gyro
//! handedness) on top.
//!
//! Per project convention these derive `Clone` but **not** `Copy` — clone
//! explicitly where a copy is wanted.

use core::time::Duration;

use crate::protocol::{WireQuat, WireVec2, WireVec3};

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

// Structural conversions from the packed wire chunks (`protocol`) into the raw value types. Plain
// field copies — no policy; the decode layer normalizes on top (see `state.rs`). Kept separate from
// the serde types so the packed `Wire` structs never need `Serialize`/refs into unaligned fields.
impl From<WireVec2> for Vec2i {
    fn from(v: WireVec2) -> Self {
        Vec2i { x: v.x, y: v.y }
    }
}
impl From<WireVec3> for Vec3i {
    fn from(v: WireVec3) -> Self {
        Vec3i { x: v.x, y: v.y, z: v.z }
    }
}
impl From<WireQuat> for Quati {
    fn from(q: WireQuat) -> Self {
        Quati { x: q.x, y: q.y, z: q.z, w: q.w }
    }
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
