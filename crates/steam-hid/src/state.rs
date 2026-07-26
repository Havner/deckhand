//! Layer 2: the unified, normalized snapshot (PLAN §1.5).

use crate::buttons::{Axis, Buttons, GordonButtons, NeptuneButtons};
use crate::report::{BatteryRaw, GordonReport, NeptuneReport, RawReport};
use crate::value::{Quati, Timestamp, TrackPad, Vec2, Vec3i};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A high-level frame: a unified input snapshot, or a lifecycle signal (PLAN §1.5).
///
/// `read`/`poll` return this. `State` is the converted input snapshot; the other
/// variants are the lifecycle frames passed through.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum Report {
    State(ControllerState),
    Connected,
    Disconnected,
    Battery(Battery),
}

impl Report {
    /// Convert a raw wire frame into a high-level [`Report`], stamping input
    /// snapshots with the read `timestamp`.
    pub(crate) fn decode(raw: &RawReport, timestamp: Timestamp) -> Report {
        match raw {
            RawReport::Gordon(g) => Report::State(ControllerState::from_gordon(g, timestamp)),
            RawReport::Neptune(n) => Report::State(ControllerState::from_neptune(n, timestamp)),
            RawReport::Connected => Report::Connected,
            RawReport::Disconnected => Report::Disconnected,
            RawReport::Battery(b) => Report::Battery(Battery::from(b)),
        }
    }
}

/// Battery status (wireless controllers only; PLAN §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Battery {
    pub voltage_mv: u16,
    /// Battery charge, in percent (0..=100).
    pub charge_percent: u8,
}

impl From<&BatteryRaw> for Battery {
    fn from(raw: &BatteryRaw) -> Self {
        Battery {
            voltage_mv: raw.voltage_mv,
            charge_percent: raw.charge_percent,
        }
    }
}

/// A unified, normalized controller snapshot (PLAN §1.5).
///
/// Analog inputs are normalized to `f32`; IMU (accel/gyro/orientation) passes
/// through as raw `i16` with documented scale factors. Battery is *not* here —
/// it is device-level state / a [`Report::Battery`] signal.
///
/// **IMU frame (HW-verified, PLAN §1.9):** right-handed, `X=right, Y=forward
/// (toward the nose), Z=up (out of the face)`.
/// - `accel` — specific force; reads `+1g` along whichever axis points up
///   (`ACCEL_RES_PER_G = 16384`). Passed through raw (already right-handed).
/// - `gyro` — angular velocity, `x`=pitch, `y`=roll, `z`=yaw rate
///   (`GYRO_RES_PER_DPS = 16`), right-hand rule: pitch-up / yaw-left / roll-right
///   are positive. (Gordon's raw `y` is negated during conversion to make the
///   triple right-handed — see `gordon_gyro`.)
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
    /// Read a normalized analog channel (used by `diff`, PLAN §1.5).
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

    /// Convert a Gordon wire frame to a unified snapshot (hybrid normalization).
    fn from_gordon(g: &GordonReport, timestamp: Timestamp) -> Self {
        let b = &g.buttons;
        ControllerState {
            seq: g.seq,
            timestamp,
            buttons: map_gordon_buttons(b),
            left_trigger: norm_u8(g.left_trigger),
            right_trigger: norm_u8(g.right_trigger),
            left_stick: norm_stick(&g.left_stick),
            right_stick: Vec2::default(), // Gordon has no right stick
            left_pad: TrackPad {
                pos: norm_stick(&g.left_pad),
                pressure: 0.0, // Gordon pads report no pressure
                touched: b.contains(GordonButtons::LPAD_TOUCH),
            },
            right_pad: TrackPad {
                pos: norm_stick(&g.right_pad),
                pressure: 0.0,
                touched: b.contains(GordonButtons::RPAD_TOUCH),
            },
            accel: g.accel.clone(),
            gyro: gordon_gyro(&g.gyro),
            orientation: g.orientation.clone(),
        }
    }

    /// Convert a Neptune (Steam Deck) wire frame to a unified snapshot.
    ///
    /// The Deck reports **separate** stick/pad fields (no Gordon multiplex) and
    /// direct press/touch bits, so the fold is 1:1. Triggers are `i16` (`0..=32767`).
    /// IMU passes through **raw** — Neptune axis/sign are unverified (a different
    /// sensor from Gordon; no `gordon_gyro`-style correction until verified, §1.9).
    fn from_neptune(n: &NeptuneReport, timestamp: Timestamp) -> Self {
        let b = &n.buttons;
        ControllerState {
            seq: n.seq,
            timestamp,
            buttons: map_neptune_buttons(b),
            left_trigger: norm_trigger(n.left_trigger),
            right_trigger: norm_trigger(n.right_trigger),
            left_stick: norm_stick(&n.left_stick),
            right_stick: norm_stick(&n.right_stick),
            left_pad: TrackPad {
                pos: norm_stick(&n.left_pad),
                pressure: norm_pressure(n.left_pad_pressure),
                touched: b.contains(NeptuneButtons::LPAD_TOUCH),
            },
            right_pad: TrackPad {
                pos: norm_stick(&n.right_pad),
                pressure: norm_pressure(n.right_pad_pressure),
                touched: b.contains(NeptuneButtons::RPAD_TOUCH),
            },
            accel: n.accel.clone(),
            gyro: n.gyro.clone(),
            orientation: n.orientation.clone(),
        }
    }
}

// --- normalization helpers (divisors provisional, verify on HW — PLAN §1.5/§1.9) ---

/// Normalize Gordon's raw gyro into the unified right-handed IMU frame.
///
/// HW-verified (PLAN §1.9): the raw channels are axis-aligned with the accel
/// frame `X=right, Y=forward, Z=up` — `x`=pitch, `y`=roll, `z`=yaw rate — but the
/// device's `y` (roll) channel is mounted **inverted**, giving a left-handed triple
/// `(ωx, −ωy, ωz)`. Negating `y` yields a proper right-handed angular velocity that
/// shares the accelerometer's frame and obeys the right-hand rule (pitch-up,
/// yaw-left, roll-right all positive). (The C# reference's `gyaw`/`groll` field
/// names are transposed — offsets are right, names lie.)
fn gordon_gyro(raw: &Vec3i) -> Vec3i {
    Vec3i {
        x: raw.x,
        y: raw.y.saturating_neg(),
        z: raw.z,
    }
}

fn norm_u8(v: u8) -> f32 {
    v as f32 / 255.0
}
/// Normalize a Deck trigger (raw `i16`, `0..=32767`) to `0.0..=1.0`.
fn norm_trigger(v: i16) -> f32 {
    (v as f32 / 32767.0).clamp(0.0, 1.0)
}
/// Normalize Deck trackpad pressure (raw `i16`) to `0.0..=1.0`. Full-scale is
/// provisional — **unverified** (PLAN §1.9).
fn norm_pressure(v: i16) -> f32 {
    (v as f32 / 32767.0).clamp(0.0, 1.0)
}
fn norm_axis(v: i16) -> f32 {
    (v as f32 / 32768.0).clamp(-1.0, 1.0)
}
fn norm_stick(v: &crate::value::Vec2i) -> Vec2 {
    Vec2 {
        x: norm_axis(v.x),
        y: norm_axis(v.y),
    }
}

/// Fold Gordon's per-device button bits into the unified [`Buttons`] superset.
fn map_gordon_buttons(g: &GordonButtons) -> Buttons {
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
    set(g.contains(GordonButtons::L1), Buttons::L1);
    set(g.contains(GordonButtons::R1), Buttons::R1);
    set(g.contains(GordonButtons::L2), Buttons::L2);
    set(g.contains(GordonButtons::R2), Buttons::R2);
    set(g.contains(GordonButtons::L4), Buttons::L4);
    set(g.contains(GordonButtons::R4), Buttons::R4);
    set(g.contains(GordonButtons::MENU), Buttons::MENU);
    set(g.contains(GordonButtons::OPTIONS), Buttons::OPTIONS);
    set(g.contains(GordonButtons::STEAM), Buttons::STEAM);
    // Left multiplex (PLAN §1.4/§1.9): the left click bit is shared — it also sets
    // on a left-stick click. Disambiguate on left touch: it's a pad press only when
    // the pad is actually touched; a stick click surfaces as LSTICK_PRESS alone.
    set(
        g.contains(GordonButtons::LPAD_PRESS) && g.contains(GordonButtons::LPAD_TOUCH),
        Buttons::LPAD_PRESS,
    );
    set(g.contains(GordonButtons::RPAD_PRESS), Buttons::RPAD_PRESS);
    set(g.contains(GordonButtons::LPAD_TOUCH), Buttons::LPAD_TOUCH);
    set(g.contains(GordonButtons::RPAD_TOUCH), Buttons::RPAD_TOUCH);
    set(
        g.contains(GordonButtons::LSTICK_PRESS),
        Buttons::LSTICK_PRESS,
    );
    out
}

/// Fold Neptune's per-device button bits into the unified [`Buttons`] superset.
///
/// 1:1 — the Deck has dedicated press/touch bits (no Gordon left-multiplex), and
/// its raw bit layout already matches the unified naming.
fn map_neptune_buttons(n: &NeptuneButtons) -> Buttons {
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
    set(n.contains(NeptuneButtons::L1), Buttons::L1);
    set(n.contains(NeptuneButtons::R1), Buttons::R1);
    set(n.contains(NeptuneButtons::L2), Buttons::L2);
    set(n.contains(NeptuneButtons::R2), Buttons::R2);
    set(n.contains(NeptuneButtons::L4), Buttons::L4);
    set(n.contains(NeptuneButtons::R4), Buttons::R4);
    set(n.contains(NeptuneButtons::L5), Buttons::L5);
    set(n.contains(NeptuneButtons::R5), Buttons::R5);
    set(n.contains(NeptuneButtons::MENU), Buttons::MENU);
    set(n.contains(NeptuneButtons::OPTIONS), Buttons::OPTIONS);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_stick_click_is_not_a_pad_press() {
        // Stick click shares the LPAD_PRESS bit but has no LPAD_TOUCH.
        let g = GordonReport {
            buttons: GordonButtons::LPAD_PRESS | GordonButtons::LSTICK_PRESS,
            ..Default::default()
        };
        let s = ControllerState::from_gordon(&g, Timestamp::default());
        assert!(s.buttons.contains(Buttons::LSTICK_PRESS));
        assert!(!s.buttons.contains(Buttons::LPAD_PRESS));
    }

    #[test]
    fn touched_pad_click_is_a_pad_press() {
        let g = GordonReport {
            buttons: GordonButtons::LPAD_PRESS | GordonButtons::LPAD_TOUCH,
            ..Default::default()
        };
        let s = ControllerState::from_gordon(&g, Timestamp::default());
        assert!(s.buttons.contains(Buttons::LPAD_PRESS));
    }
}
