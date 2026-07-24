//! Layer 2: the unified, normalized snapshot (PLAN §1.5).

use crate::buttons::{Axis, Buttons, GordonButtons};
use crate::report::{BatteryRaw, GordonReport, RawReport};
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
            RawReport::Neptune(_n) => {
                todo!("Neptune → ControllerState — deferred until the Deck path (PLAN §1.4)")
            }
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
}

impl From<&BatteryRaw> for Battery {
    fn from(raw: &BatteryRaw) -> Self {
        Battery {
            voltage_mv: raw.voltage_mv,
        }
    }
}

/// A unified, normalized controller snapshot (PLAN §1.5).
///
/// Analog inputs are normalized to `f32`; IMU (accel/gyro/orientation) passes
/// through as raw `i16` with documented scale factors. Battery is *not* here —
/// it is device-level state / a [`Report::Battery`] signal.
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
            gyro: g.gyro.clone(),
            orientation: g.orientation.clone(),
        }
    }
}

// --- normalization helpers (divisors provisional, verify on HW — PLAN §1.5/§1.9) ---

fn norm_u8(v: u8) -> f32 {
    v as f32 / 255.0
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
