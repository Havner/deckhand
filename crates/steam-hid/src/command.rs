//! Command parameter types (PLAN §1.4, §1.5).

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

bitflags::bitflags! {
    /// IMU (gyro/accel) mode bits for the `IMU_MODE` setting (PLAN §1.4).
    ///
    /// Matches the C# `GCGyroMode` / kernel `GYRO_MODE` bit values exactly.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct ImuMode: u16 {
        const STEERING         = 0x01;
        const TILT             = 0x02;
        const SEND_ORIENTATION = 0x04;
        const SEND_RAW_ACCEL    = 0x08;
        const SEND_RAW_GYRO     = 0x10;
    }
}

impl ImuMode {
    /// The raw accel + raw gyro combo, i.e. what `set_gyro(true)` enables.
    pub fn raw_motion() -> Self {
        Self::SEND_RAW_ACCEL | Self::SEND_RAW_GYRO
    }
}

/// Which haptic actuator to drive.
///
/// Note the kernel swaps left/right for legacy reasons on the pulse path; this
/// enum is the *logical* side and the mapping is handled internally (PLAN §1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum Motor {
    Left,
    Right,
}

/// Parameters for a haptic pulse / rumble.
///
/// The exact packet layout is **unverified** — several candidates exist and the
/// device responds to more than one (PLAN §1.4, §1.9). Treat these fields as the
/// `TRIGGER_HAPTIC_PULSE` (`0x8f`) parameters for now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Rumble {
    pub amplitude: u16,
    pub period: u16,
    pub count: u16,
}
