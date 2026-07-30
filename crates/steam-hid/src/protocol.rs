//! Wire-level protocol constants (PLAN §1.4).
//!
//! Kernel `hid-steam` naming is adopted for command IDs and settings. Only the
//! low, known subset is defined here; the rest is recorded in PLAN §1.4 for later.
//! Everything here is **provisional until verified on hardware** (PLAN §1.9).

#![allow(dead_code)]

/// Valve USB vendor id.
pub(crate) const VALVE_VID: u16 = 0x28DE;

/// Original Steam Controller, wired (Gordon).
pub(crate) const PID_GORDON_WIRED: u16 = 0x1102;
/// Original Steam Controller, wireless dongle (Gordon).
pub(crate) const PID_GORDON_DONGLE: u16 = 0x1142;
/// Steam Deck built-in controls (Neptune).
pub(crate) const PID_NEPTUNE: u16 = 0x1205;

/// All 64-byte HID reports; feature reports are framed with a report-ID-0 byte.
pub(crate) const REPORT_LEN: usize = 64;
/// Report id prepended to feature-report buffers (PLAN §1.4).
pub(crate) const REPORT_ID: u8 = 0x00;

/// Input-frame event type, at byte offset 2 of every report.
pub(crate) mod event_type {
    pub(crate) const INPUT_DATA: u8 = 0x01;
    pub(crate) const CONNECT: u8 = 0x03;
    pub(crate) const BATTERY: u8 = 0x04;
    pub(crate) const DECK_INPUT_DATA: u8 = 0x09;
}

/// Payload byte (offset 4) of a `CONNECT` (0x03) frame.
pub(crate) mod wireless {
    pub(crate) const DISCONNECTED: u8 = 0x01;
    pub(crate) const CONNECTED: u8 = 0x02;
}

/// Feature-report command IDs (kernel naming; subset used initially).
pub(crate) mod cmd {
    pub(crate) const CLEAR_DIGITAL_MAPPINGS: u8 = 0x81;
    pub(crate) const SET_DEFAULT_DIGITAL_MAPPINGS: u8 = 0x85;
    pub(crate) const SET_SETTINGS_VALUES: u8 = 0x87;
    pub(crate) const LOAD_DEFAULT_SETTINGS: u8 = 0x8E;
    pub(crate) const TRIGGER_HAPTIC_PULSE: u8 = 0x8F;
    pub(crate) const TURN_OFF_CONTROLLER: u8 = 0x9F;
    pub(crate) const GET_STRING_ATTRIBUTE: u8 = 0xAE;
    pub(crate) const DONGLE_GET_WIRELESS_STATE: u8 = 0xB4;
    pub(crate) const TRIGGER_HAPTIC_CMD: u8 = 0xEA;
    pub(crate) const TRIGGER_RUMBLE_CMD: u8 = 0xEB;
}

/// Setting ids (kernel naming; index == id). Subset used initially (PLAN §1.4).
pub(crate) mod setting {
    pub(crate) const LEFT_TRACKPAD_MODE: u8 = 7;
    pub(crate) const RIGHT_TRACKPAD_MODE: u8 = 8;
    pub(crate) const LED_USER_BRIGHTNESS: u8 = 45;
    pub(crate) const IMU_MODE: u8 = 48;
    pub(crate) const SLEEP_INACTIVITY_TIMEOUT: u8 = 50;
    pub(crate) const LEFT_TRACKPAD_CLICK_PRESSURE: u8 = 52;
    pub(crate) const RIGHT_TRACKPAD_CLICK_PRESSURE: u8 = 53;
    pub(crate) const STEAM_WATCHDOG_ENABLE: u8 = 71;
}

/// Trackpad modes for `LEFT/RIGHT_TRACKPAD_MODE`.
pub(crate) mod trackpad_mode {
    /// Disables the pad for the mapper → raw input (lizard-off).
    pub(crate) const NONE: u8 = 7;
}

/// IMU scale constants — HW-verified on Gordon (PLAN §1.9). `ControllerState` carries the raw
/// i16 IMU readings, so consumers (the engine's gyro-to-mouse) need these to convert to physical
/// units: `raw / GYRO_RES_PER_DPS` = degrees/second, `raw / ACCEL_RES_PER_G` = g. Re-exported at
/// the crate root so there's a single source of truth for the scale.
pub const ACCEL_RES_PER_G: f32 = 16384.0;
pub const GYRO_RES_PER_DPS: f32 = 16.0;

/// String-attribute id for the unit serial number (used with `GET_STRING_ATTRIBUTE`).
pub(crate) const ATTRIB_STR_UNIT_SERIAL: u8 = 0x01;
