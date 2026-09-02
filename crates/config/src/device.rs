//! Device / engine-level config - Tier B (PLAN 3, Round E).
//!
//! Profile-independent device settings handed to the engine **once** (uncompiled): the LED/idle
//! toggles and the per-device rumble shaping (one struct per device, since the hardware differs).
//! Applied reader-side (device-local); the top-level chords are a separate slot
//! ([`Chords`](crate::Chords)).

use serde::{Deserialize, Serialize};

/// A rumble **lever** - how one field of a rumble command is driven from the per-motor strength (the
/// game's FF amplitude, after the profile scaling). Either a constant, or the strength scaled
/// linearly into a `[min, max]` band. `T` is the lever's unit: `u8` percent for the drive/rate fields
/// (Gordon duty, Neptune/Triton speed), `i8` dB for the gain field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lever<T> {
    /// Ignore strength - drive the field at this constant.
    Fixed(T),
    /// Scale the per-motor strength into `[min, max]` (a zero-strength motor stays silent; `min` is
    /// only a floor for *nonzero* strength - see [`Lever::drive`]).
    Scaled { min: T, max: T },
}

impl Lever<u8> {
    /// Resolve a **drive/rate** field (the `u16` full-scale value the device wants) for a per-motor
    /// `strength` (the profile-scaled FF amplitude). Endpoints are percent of full drive. A
    /// **zero-strength motor is always silent** (`0`) regardless of variant - so `Scaled { min }` is a
    /// floor for nonzero strength, never forces an idle motor on. Shared by the Neptune/Triton speed
    /// field and Gordon's pulse duty (both are "percent of full drive").
    pub fn drive(&self, strength: u16) -> u16 {
        if strength == 0 {
            return 0;
        }
        let pct = match self {
            Lever::Fixed(p) => *p as f32,
            Lever::Scaled { min, max } => {
                let f = strength as f32 / u16::MAX as f32; // (0, 1]
                *min as f32 + f * (*max as f32 - *min as f32)
            }
        };
        (pct.clamp(0.0, 100.0) / 100.0 * u16::MAX as f32) as u16
    }
}

impl Lever<i8> {
    /// Resolve the rumble **gain** field (dB) for a per-motor `strength`. `Fixed` is constant;
    /// `Scaled` lerps the strength into `[min, max]` dB. Silence is handled by the drive lever (a zero
    /// motor has drive `0`, so its gain is moot).
    pub fn gain(&self, strength: u16) -> i8 {
        match self {
            Lever::Fixed(g) => *g,
            Lever::Scaled { min, max } => {
                let f = strength as f32 / u16::MAX as f32;
                (*min as f32 + f * (*max as f32 - *min as f32)).round() as i8
            }
        }
    }
}

/// Gordon (original Steam Controller) rumble shaping. Gordon has **no motors** - it rumbles via the
/// `0x8f` pulse-train on its trackpad actuators, so the only levers are the pulse **duty** (encodes
/// felt strength) and the pulse **frequency**. Distinct from [`RumbleTuning`] on purpose: the
/// dual-motor devices have nothing like a pulse frequency, and Gordon has nothing like a gain field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GordonTuning {
    /// The pulse **duty** lever (percent of full drive -> the actuator's usable duty band). The felt
    /// strength control; constant, or scaled into a band.
    pub duty: Lever<u8>,
    /// The pulse **frequency**, Hz - the pulse-train rate.
    pub hz: u16,
}

/// Neptune / Triton (dual-motor) rumble shaping: how the incoming per-motor strength maps onto the
/// two amplitude levers of the dual-motor command (`0xeb` on the Deck, `0x80` on Triton - same param
/// shape). Neptune and Triton each keep their own (identical shape, but different motors, so tuned
/// separately). Gordon uses [`GordonTuning`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RumbleTuning {
    /// The motor **speed/rate** lever (percent of full drive -> the device's `speed` field). The
    /// coarse amplitude control; on its own it maps only weakly to felt strength.
    pub speed: Lever<u8>,
    /// The **gain** lever (dB). The real strength trim, layered on top of `speed`.
    pub gain: Lever<i8>,
}

/// The device (above-profile) configuration - reader-side device settings, no chords. Every device's
/// rumble shaping lives here (the reader picks the bound one); there is no global rumble knob - each
/// device's levers already scale its own strength.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    /// LED brightness `0..=100 %` (applied on connect); `None` = leave default.
    pub led_brightness: Option<u8>,
    /// Sleep/idle timeout, seconds (applied on connect); `None` = leave default.
    pub idle_timeout: Option<u16>,
    /// Gordon pulse-train rumble shaping (duty + frequency).
    pub gordon: GordonTuning,
    /// Neptune (Steam Deck) motor-rumble shaping - how per-motor strength drives speed + gain.
    pub neptune: RumbleTuning,
    /// Triton (new Steam Controller) motor-rumble shaping - separate from Neptune (different motors).
    pub triton: RumbleTuning,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig {
            led_brightness: None,
            idle_timeout: None,
            // Defaults reproduce the prior hard-coded behaviour exactly: strength passes straight
            // through to the drive field (Scaled 0..100% is an identity on the raw u16), the Gordon
            // pulse runs at 60 Hz, and gain is the constant the old `NEPTUNE_*_GAIN` / `TRITON_*_GAIN`
            // reader constants used.
            gordon: GordonTuning { duty: Lever::Scaled { min: 0, max: 100 }, hz: 60 },
            neptune: RumbleTuning { speed: Lever::Scaled { min: 0, max: 100 }, gain: Lever::Fixed(2) },
            triton: RumbleTuning { speed: Lever::Scaled { min: 0, max: 100 }, gain: Lever::Fixed(0) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_config_round_trips_ron() {
        let d = DeviceConfig {
            led_brightness: Some(50),
            idle_timeout: None,
            gordon: GordonTuning { duty: Lever::Fixed(40), hz: 90 },
            neptune: RumbleTuning { speed: Lever::Fixed(70), gain: Lever::Scaled { min: -4, max: 12 } },
            triton: RumbleTuning { speed: Lever::Scaled { min: 10, max: 90 }, gain: Lever::Fixed(3) },
        };
        let s = ron::to_string(&d).unwrap();
        assert_eq!(ron::from_str::<DeviceConfig>(&s).unwrap(), d);
    }

    /// Old `devcfg.ron` (pre-split fields like `master_rumble`/`rumble_hz`, no per-device rumble)
    /// still parses - RON ignores the now-unknown fields and the container `#[serde(default)]` fills
    /// the new ones from `DeviceConfig::default()`.
    #[test]
    fn legacy_devcfg_parses() {
        let ron = "(led_brightness: Some(50), idle_timeout: None, master_rumble: 80, rumble_hz: 90)";
        let d = ron::from_str::<DeviceConfig>(ron).unwrap();
        assert_eq!(d.led_brightness, Some(50));
        assert_eq!(d.gordon, DeviceConfig::default().gordon);
        assert_eq!(d.neptune, DeviceConfig::default().neptune);
    }

    #[test]
    fn drive_lever_zero_strength_is_silent() {
        // A zero motor is silent regardless of variant - `min` is a floor for nonzero strength only.
        assert_eq!(Lever::Scaled { min: 30, max: 90 }.drive(0), 0);
        assert_eq!(Lever::<u8>::Fixed(50).drive(0), 0);
    }

    #[test]
    fn drive_lever_scaled_full_range_is_identity() {
        // Scaled 0..100% passes the raw strength straight through (the migration default).
        let l = Lever::Scaled { min: 0, max: 100 };
        assert_eq!(l.drive(u16::MAX), u16::MAX);
        // 25% strength -> 25% drive -> the same value (within rounding).
        let quarter = u16::MAX / 4;
        assert!((l.drive(quarter) as i32 - quarter as i32).abs() <= 2);
    }

    #[test]
    fn gain_lever_fixed_and_scaled() {
        assert_eq!(Lever::<i8>::Fixed(3).gain(1234), 3);
        let l = Lever::Scaled { min: -8, max: 16 };
        assert_eq!(l.gain(u16::MAX), 16);
        assert_eq!(l.gain(0), -8); // moot (drive silences), but well-defined at the min endpoint
    }
}
