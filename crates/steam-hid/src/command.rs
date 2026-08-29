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
///
/// `Both` drives both actuators. On the paths that carry a native side byte —
/// the Deck's `0xEA` [`Device::haptic_cmd`](crate::Device::haptic_cmd) and Triton's
/// `0x82` [`Device::haptic_command_triton`](crate::Device::haptic_command_triton) —
/// it maps to the firmware's `BOTH` (side = 2), **HW-verified**. The Gordon pulse
/// path (`0x8F`) does **not** honor `BOTH` (it no-ops on HW), so
/// [`Device::haptic_pulse`](crate::Device::haptic_pulse) realizes it by firing the
/// two pads separately.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Motor {
    Left,
    Right,
    Both,
}

/// Style for the Deck's `0xEA` `SET_HAPTIC2` haptic
/// ([`Device::haptic_cmd`](crate::Device::haptic_cmd)) — matches C# `NCHapticStyle`. A short, finely-tuned
/// trackpad "click"; `Disabled` is off, and `Weak` is weaker than `Strong` at the same `gain`.
///
/// InputPlumber calls this same byte `cmd_type` with the names `Off`/`Tick`/`Click`
/// (0/1/2) — the same field, different labels.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum HapticStyle {
    Disabled = 0, // InputPlumber: Off
    Weak = 1,     // InputPlumber: Tick
    Strong = 2,   // InputPlumber: Click
}

/// Intensity for the Deck's `0xEA` `SET_HAPTIC2` haptic — a second lever beside
/// [`HapticStyle`], only exposed in **InputPlumber's** `PackedHapticReport` (C# hard-codes this byte
/// to 0). Names are InputPlumber's. **HW-observed on the Deck:** `Default`..`Medium` (0..2) feel
/// identical, `Long` (3) is noticeably stronger, `Insane` (4) is sometimes stronger / sometimes a
/// different character; values outside 0..=4 do nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum HapticIntensity {
    Default = 0,
    Short = 1,
    Medium = 2,
    Long = 3,
    Insane = 4,
}

/// Parameters for a `TRIGGER_HAPTIC_PULSE` (`0x8f`) trackpad haptic pulse
/// ([`Device::haptic_pulse`](crate::Device::haptic_pulse)).
///
/// **Verified on Gordon** (PLAN §1.9): the actuator plays `count` pulses, each
/// `duration` µs on then `interval` µs off — so `duration`/`interval` set the tone
/// and `count` its length. `gain` (dB, −24..+6) is honored on the Deck per the kernel
/// but **ignored on Gordon** (no audible difference across the range), which makes the
/// C# 7-byte and kernel 8-byte packet variants functionally identical on Gordon.
///
/// This is a *pulse* on the trackpad actuator — Gordon's only haptic, and usable on the
/// Deck too. The Deck's native continuous dual-motor rumble is a different command
/// ([`Device::rumble_cmd`](crate::Device::rumble_cmd), `0xeb`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HapticPulse {
    pub duration: u16,
    pub interval: u16,
    pub count: u16,
    pub gain: i8,
}
