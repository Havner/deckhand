//! Device / engine-level config — Tier B (PLAN §3, Round E).
//!
//! Profile-independent device settings handed to the engine **once** (uncompiled): the master
//! rumble % + pulse frequency and the LED/idle toggles. Applied reader-side (device-local); the
//! top-level chords are a separate slot ([`Chords`](crate::Chords)).

use serde::{Deserialize, Serialize};

/// The device (above-profile) configuration — reader-side device settings, no chords.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    /// LED brightness `0..=100 %` (applied on connect); `None` = leave default.
    pub led_brightness: Option<u8>,
    /// Sleep/idle timeout, seconds (applied on connect); `None` = leave default.
    pub idle_timeout: Option<u16>,
    /// `0..=100 %` — scales **all** haptic output (activator haptics + rumble back-channel). A
    /// master attenuator only; per-profile `strength` (which may exceed 100) does per-game gain.
    pub master_rumble: u8,
    /// Rumble pulse frequency, Hz — the Gordon pulse-train rate (applied reader-side; the Deck's
    /// motor rumble ignores it). Was per-profile (`RumbleSettings.hz`); now device-level.
    pub rumble_hz: u16,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig {
            led_brightness: None,
            idle_timeout: None,
            master_rumble: 100,
            rumble_hz: 60,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_master_rumble_is_full() {
        assert_eq!(DeviceConfig::default().master_rumble, 100);
    }

    #[test]
    fn device_config_round_trips_ron() {
        let d = DeviceConfig {
            led_brightness: Some(50),
            idle_timeout: None,
            master_rumble: 80,
            rumble_hz: 90,
        };
        let s = ron::to_string(&d).unwrap();
        assert_eq!(ron::from_str::<DeviceConfig>(&s).unwrap(), d);
    }
}
