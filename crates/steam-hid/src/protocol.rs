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
/// Original Steam Controller, Bluetooth (Gordon over BLE).
pub(crate) const PID_GORDON_BLE: u16 = 0x1106;
/// Steam Deck built-in controls (Neptune).
pub(crate) const PID_NEPTUNE: u16 = 0x1205;

/// New Steam Controller (2026; SDL codename "Triton"), wired over USB-C.
pub(crate) const PID_TRITON_WIRED: u16 = 0x1302;
/// New Steam Controller, Bluetooth LE.
pub(crate) const PID_TRITON_BLE: u16 = 0x1303;
/// New Steam Controller wireless dongle ("Controller Puck" / SDL "Proteus"). Enumerates a set of
/// per-slot HID interfaces like the original Gordon dongle — **interfaces 2..=6 on the observed
/// unit** (SDL documents 2..5; real hardware exposes one more). Unlike the Gordon dongle it reports
/// a real serial. (SDL also lists a second dongle variant, "Nereid" `0x1305`, which we don't add
/// until one is seen in the wild.)
pub(crate) const PID_TRITON_PUCK: u16 = 0x1304;

/// All 64-byte HID reports; feature reports are framed with a report-ID-0 byte.
pub(crate) const REPORT_LEN: usize = 64;
/// Report id prepended to feature-report buffers on Gordon/Neptune (PLAN §1.4).
pub(crate) const REPORT_ID: u8 = 0x00;
/// Report id prepended to Triton feature-report buffers. Triton's command channel rides
/// **feature report `0x01`**, not `0x00` — confirmed in SDL (`DisableSteamTritonLizardMode`
/// sets `buffer[0]=1`) and sc-controller (`wValue 0x0301`, `0x01`-prefixed payload). The
/// command *body* (`[cmd_id, len, payload…]`) is otherwise identical to Gordon/Neptune.
pub(crate) const REPORT_ID_TRITON: u8 = 0x01;

/// Triton (new Steam Controller) wire constants (PLAN §1.4).
///
/// Reverse-engineered from SDL `SDL_hidapi_steam_triton.c` + `steam/controller_structs.h`
/// (Valve's own struct names) and sc-controller `sc2.py` / `docs/steam-controller-v2-protocol.md`.
/// Triton does **not** use the `0x01`-framed `ValveInReport_t` of Gordon/Neptune: its input and
/// haptic reports carry the **report id in byte 0** (dispatched in `parse_triton`).
pub(crate) mod triton {
    /// Input/status report ids (byte 0 of each read).
    pub(crate) mod report {
        /// Main gamepad state (with on-controller quaternion on older firmware). **HW: the real
        /// puck/dongle (0x1304) streams `0x42` by default** (observed 2026-08-26 on unit
        /// FXB9…, firmware sends the quaternion body) — parsed as NoQuat regardless.
        pub(crate) const STATE: u8 = 0x42;
        /// Battery status.
        pub(crate) const BATTERY: u8 = 0x43;
        /// Gamepad state, "NoQuat" body — same leading fields as `STATE`, parsed identically (the
        /// quaternion is simply absent). **HW: the real controller over Bluetooth (0x1303) streams
        /// `0x45`** (observed 2026-08-26), whereas the puck/wire stream `0x42`.
        pub(crate) const STATE_NOQUAT: u8 = 0x45;
        /// Wireless connect/disconnect status (dongle), alternate id.
        pub(crate) const WIRELESS_X: u8 = 0x46;
        /// Gamepad state with a trackpad timestamp + 16-bit IMU timestamp ("Ibex" packet).
        /// Not parsed yet — added only if a unit is seen streaming it (PLAN §1.9).
        pub(crate) const STATE_TIMESTAMP: u8 = 0x47;
        /// Wireless connect/disconnect status (dongle).
        pub(crate) const WIRELESS: u8 = 0x79;
    }

    /// Payload byte of a wireless-status report (`WIRELESS`/`WIRELESS_X`).
    pub(crate) mod wireless {
        pub(crate) const DISCONNECT: u8 = 1;
        pub(crate) const CONNECT: u8 = 2;
    }

    /// Haptic **output**-report ids (Triton drives haptics via output reports, not feature
    /// reports). Recorded in full for reference; only a subset is wired up initially.
    #[allow(dead_code)]
    pub(crate) mod haptic {
        /// Dual-motor continuous rumble (`{type, intensity, left{speed,gain}, right{speed,gain}}`).
        pub(crate) const RUMBLE: u8 = 0x80;
        /// Trackpad haptic pulse (`{side, on_us, off_us, repeat_count}`).
        pub(crate) const PULSE: u8 = 0x81;
        /// Haptic command / click (`{side, command, gain_db}`).
        pub(crate) const COMMAND: u8 = 0x82;
        /// LFO tone (`{side, gain_db, frequency, duration_ms, lfo_freq, lfo_depth}`).
        pub(crate) const LFO_TONE: u8 = 0x83;
        /// Log-frequency sweep (`{side, gain_db, duration_ms, start_freq, end_freq}`).
        pub(crate) const LOG_SWEEP: u8 = 0x84;
        /// Named haptic script (`{side, script_id, gain_db}`).
        pub(crate) const SCRIPT: u8 = 0x85;
    }
}

/// Bluetooth (BLE) transport framing + compact input layout (PLAN §1.4).
///
/// The kernel `hid-steam` driver is USB-only, so BLE is reverse-engineered from
/// SDL (`SDL_hidapi_steam.c`) and sc-controller (`sc_by_bt`) — see PLAN §1.4/§1.9.
/// Everything rides **Report ID 3** on a 20-byte HID report (report id + 1 header
/// byte + 18 payload). Feature *and* input reports longer than 18 bytes are split
/// into segments; the command bytes themselves are identical to USB.
pub(crate) mod ble {
    /// Report id prefixing every BLE feature/input report.
    pub(crate) const REPORT_ID: u8 = 0x03;
    /// Total segment size on the wire: report id + header + 18 payload.
    pub(crate) const SEGMENT_SIZE: usize = 20;
    /// Data bytes carried per segment.
    pub(crate) const SEGMENT_PAYLOAD: usize = 18;
    /// Max segments per packet (segment number is 3 bits).
    pub(crate) const MAX_SEGMENTS: usize = 8;
    /// Segment-header bit: this segment carries data.
    pub(crate) const SEG_DATA_FLAG: u8 = 0x80;
    /// Segment-header bit: last segment of the packet.
    pub(crate) const SEG_LAST_FLAG: u8 = 0x40;
    /// Segment-header mask: segment number (low 3 bits).
    pub(crate) const SEG_NUM_MASK: u8 = 0x07;

    /// Reassembled input payload: `byte0` low nibble = report type, high nibble +
    /// `byte1` = the chunk mask; chunk data follows from `byte2`.
    pub(crate) mod report_type {
        /// An input state report (chunks present per the mask).
        pub(crate) const STATE: u8 = 4;
        /// A status report (battery/idle) — not decoded as input yet.
        pub(crate) const STATUS: u8 = 5;
    }

    /// Chunk-present bits (SDL `k_EBLE*Chunk`), in ascending order = wire order.
    /// Each present chunk contributes a fixed number of payload bytes.
    pub(crate) mod chunk {
        pub(crate) const BUTTON1: u16 = 0x0010; // 3B buttons (low)
        pub(crate) const TRIGGERS: u16 = 0x0020; // 2B L/R triggers
        pub(crate) const BUTTON3: u16 = 0x0040; // 3B buttons (high; unused on SC)
        pub(crate) const LSTICK: u16 = 0x0080; // 4B stick x,y
        pub(crate) const LPAD: u16 = 0x0100; // 4B lpad x,y
        pub(crate) const RPAD: u16 = 0x0200; // 4B rpad x,y
        pub(crate) const ACCEL: u16 = 0x0400; // 6B accel x,y,z
        pub(crate) const GYRO: u16 = 0x0800; // 6B gyro x,y,z
        pub(crate) const QUAT: u16 = 0x1000; // 8B quat w,x,y,z (only if SEND_ORIENTATION)
    }
}

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
