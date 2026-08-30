//! Wire-level protocol — the **single canonical home for all hardware/protocol knowledge**
//! (CLAUDE.md golden rule). Command IDs, settings, and the exact payload struct layouts we send to
//! the hardware live here and nowhere else; `device.rs` only *uses* them.
//!
//! Naming follows the **kernel `hid-steam`** driver (command/setting IDs) and **SDL**
//! (`SDL/.../steam/controller_structs.h`, Valve's own struct names) for the payload structs. Each
//! command struct's doc names the command id it serializes for, the SDL struct it mirrors, and any
//! divergence. Structs serialize via explicit little-endian `to_bytes()` — no `unsafe`, no
//! transmute; byte-identical to a packed struct on any host.
//!
//! **Scope (incremental):** command-authoritative first, fully-authoritative later. The **INBOUND**
//! (input-report parsing) constants at the bottom, and the transport framing in `device.rs`, are
//! slated to migrate here (as input structs / a framing section) in a **later dedicated pass**.

#![allow(dead_code)]

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// =====================================================================================
// 1. General constants — vendor/product ids, report framing.
// =====================================================================================

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
/// Report id prepended to feature-report buffers on Gordon/Neptune.
pub(crate) const REPORT_ID: u8 = 0x00;
/// Report id prepended to Triton feature-report buffers. Triton's command channel rides
/// **feature report `0x01`**, not `0x00` — confirmed in SDL (`DisableSteamTritonLizardMode`
/// sets `buffer[0]=1`) and sc-controller (`wValue 0x0301`, `0x01`-prefixed payload). The
/// command *body* (`[cmd_id, len, payload…]`) is otherwise identical to Gordon/Neptune.
pub(crate) const REPORT_ID_TRITON: u8 = 0x01;

// =====================================================================================
// 2. Command & setting IDs. Kernel `hid-steam` naming (adopted); the full set is Valve's, from SDL
//    `controller_constants.h` (matches the kernel where both define an id). Recorded in full as a
//    knowledge trace — most are unused (`#![allow(dead_code)]`). `(used)` marks what we send today.
// =====================================================================================

/// Feature-report command IDs (SDL `FeatureReportMessageIDs`), numeric-sorted, full set.
pub(crate) mod cmd {
    // --- digital button mappings (lizard-mode gamepad emulation) ---
    pub(crate) const SET_DIGITAL_MAPPINGS: u8 = 0x80;
    pub(crate) const CLEAR_DIGITAL_MAPPINGS: u8 = 0x81; // (used) lizard-off
    pub(crate) const GET_DIGITAL_MAPPINGS: u8 = 0x82;
    pub(crate) const GET_ATTRIBUTES_VALUES: u8 = 0x83;
    pub(crate) const GET_ATTRIBUTE_LABEL: u8 = 0x84;
    pub(crate) const SET_DEFAULT_DIGITAL_MAPPINGS: u8 = 0x85; // (used) lizard-on
    pub(crate) const FACTORY_RESET: u8 = 0x86; // ⚠ wipes config
    // --- settings I/O (see the `setting` table + `ControllerSetting`) ---
    pub(crate) const SET_SETTINGS_VALUES: u8 = 0x87; // (used)
    pub(crate) const CLEAR_SETTINGS_VALUES: u8 = 0x88;
    pub(crate) const GET_SETTINGS_VALUES: u8 = 0x89;
    pub(crate) const GET_SETTING_LABEL: u8 = 0x8A;
    pub(crate) const GET_SETTINGS_MAXS: u8 = 0x8B;
    pub(crate) const GET_SETTINGS_DEFAULTS: u8 = 0x8C;
    pub(crate) const SET_CONTROLLER_MODE: u8 = 0x8D; // SDL/IP only (not in kernel)
    pub(crate) const LOAD_DEFAULT_SETTINGS: u8 = 0x8E; // (used) lizard-on
    // --- haptics (pulse; the `0xEA`/`0xEB` command haptics live in section 3 structs) ---
    pub(crate) const TRIGGER_HAPTIC_PULSE: u8 = 0x8F; // (used) → MsgFireHapticPulse
    pub(crate) const TURN_OFF_CONTROLLER: u8 = 0x9F; // (used) power off ("off!")
    // --- read-only queries ---
    pub(crate) const GET_DEVICE_INFO: u8 = 0xA1;
    // --- calibration ---
    pub(crate) const CALIBRATE_TRACKPADS: u8 = 0xA7;
    pub(crate) const RESERVED_0: u8 = 0xA8;
    pub(crate) const SET_SERIAL_NUMBER: u8 = 0xA9; // ⚠ writes the unit serial
    pub(crate) const GET_TRACKPAD_CALIBRATION: u8 = 0xAA;
    pub(crate) const GET_TRACKPAD_FACTORY_CALIBRATION: u8 = 0xAB;
    pub(crate) const GET_TRACKPAD_RAW_DATA: u8 = 0xAC;
    // --- dongle / pairing ---
    pub(crate) const ENABLE_PAIRING: u8 = 0xAD;
    pub(crate) const GET_STRING_ATTRIBUTE: u8 = 0xAE; // (used) serial getter
    pub(crate) const RADIO_ERASE_RECORDS: u8 = 0xAF; // ⚠⚠ firmware/radio — DO NOT TOUCH
    pub(crate) const RADIO_WRITE_RECORD: u8 = 0xB0; // ⚠⚠ firmware/radio — DO NOT TOUCH
    pub(crate) const SET_DONGLE_SETTING: u8 = 0xB1;
    pub(crate) const DONGLE_DISCONNECT_DEVICE: u8 = 0xB2;
    pub(crate) const DONGLE_COMMIT_DEVICE: u8 = 0xB3;
    pub(crate) const DONGLE_GET_WIRELESS_STATE: u8 = 0xB4; // (used) dongle slot probe
    pub(crate) const CALIBRATE_GYRO: u8 = 0xB5;
    // --- audio (preset play + custom-audio upload; the whole path is unimplemented, no payload ref) ---
    pub(crate) const PLAY_AUDIO: u8 = 0xB6;
    pub(crate) const AUDIO_UPDATE_START: u8 = 0xB7;
    pub(crate) const AUDIO_UPDATE_DATA: u8 = 0xB8;
    pub(crate) const AUDIO_UPDATE_COMPLETE: u8 = 0xB9;
    pub(crate) const GET_CHIPID: u8 = 0xBA;
    pub(crate) const CALIBRATE_JOYSTICK: u8 = 0xBF;
    pub(crate) const CALIBRATE_ANALOG_TRIGGERS: u8 = 0xC0;
    pub(crate) const SET_AUDIO_MAPPING: u8 = 0xC1;
    pub(crate) const CHECK_GYRO_FW_LOAD: u8 = 0xC2;
    pub(crate) const CALIBRATE_ANALOG: u8 = 0xC3;
    pub(crate) const DONGLE_GET_CONNECTED_SLOTS: u8 = 0xC4;
    pub(crate) const RESET_IMU: u8 = 0xCE;
    // --- command haptics (Deck; section-3 structs) ---
    pub(crate) const TRIGGER_HAPTIC_CMD: u8 = 0xEA; // (used) → MsgTriggerHaptic
    pub(crate) const TRIGGER_RUMBLE_CMD: u8 = 0xEB; // (used) → MsgSimpleRumbleCmd
    // --- unknown opcodes (no reference documents these — purpose TBD) ---
    /// InputPlumber `UnknownDc` — seen in its command enum, purpose unknown. sc-controller lists
    /// `DC` among the Triton v2 pairing opcodes (`ED`/`AD`/`DC`/`E2`) it captured but never decoded.
    pub(crate) const UNKNOWN_DC: u8 = 0xDC;
    /// InputPlumber `UnknownE2` — as above; also in sc-controller's Triton pairing opcode set.
    pub(crate) const UNKNOWN_E2: u8 = 0xE2;
    /// sc-controller Triton v2 pairing opcode `ED` (captured, undecoded). Not in SDL/kernel/IP.
    pub(crate) const UNKNOWN_ED: u8 = 0xED;
}

/// Setting ids (SDL `ControllerSettings`; **index == id**, order frozen — "only add, never reorder").
/// Written via `SET_SETTINGS_VALUES` as [`ControllerSetting`] pairs. Full set as a trace; most unused.
pub(crate) mod setting {
    pub(crate) const MOUSE_SENSITIVITY: u8 = 0;
    pub(crate) const MOUSE_ACCELERATION: u8 = 1;
    pub(crate) const TRACKBALL_ROTATION_ANGLE: u8 = 2;
    pub(crate) const HAPTIC_INTENSITY_UNUSED: u8 = 3;
    pub(crate) const LEFT_GAMEPAD_STICK_ENABLED: u8 = 4;
    pub(crate) const RIGHT_GAMEPAD_STICK_ENABLED: u8 = 5;
    pub(crate) const USB_DEBUG_MODE: u8 = 6;
    pub(crate) const LEFT_TRACKPAD_MODE: u8 = 7; // (used) lizard-off → NONE
    pub(crate) const RIGHT_TRACKPAD_MODE: u8 = 8; // (used) lizard-off → NONE
    pub(crate) const LIZARD_MODE: u8 = 9; // InputPlumber mislabels this index as "MousePointerEnabled"
    pub(crate) const DPAD_DEADZONE: u8 = 10;
    pub(crate) const MINIMUM_MOMENTUM_VEL: u8 = 11;
    pub(crate) const MOMENTUM_DECAY_AMOUNT: u8 = 12;
    pub(crate) const TRACKPAD_RELATIVE_MODE_TICKS_PER_PIXEL: u8 = 13;
    pub(crate) const HAPTIC_INCREMENT: u8 = 14;
    pub(crate) const DPAD_ANGLE_SIN: u8 = 15;
    pub(crate) const DPAD_ANGLE_COS: u8 = 16;
    pub(crate) const MOMENTUM_VERTICAL_DIVISOR: u8 = 17;
    pub(crate) const MOMENTUM_MAXIMUM_VELOCITY: u8 = 18;
    pub(crate) const TRACKPAD_Z_ON: u8 = 19;
    pub(crate) const TRACKPAD_Z_OFF: u8 = 20;
    pub(crate) const SENSITIVITY_SCALE_AMOUNT: u8 = 21;
    pub(crate) const LEFT_TRACKPAD_SECONDARY_MODE: u8 = 22;
    pub(crate) const RIGHT_TRACKPAD_SECONDARY_MODE: u8 = 23;
    pub(crate) const SMOOTH_ABSOLUTE_MOUSE: u8 = 24;
    pub(crate) const STEAMBUTTON_POWEROFF_TIME: u8 = 25;
    pub(crate) const UNUSED_1: u8 = 26;
    pub(crate) const TRACKPAD_OUTER_RADIUS: u8 = 27;
    pub(crate) const TRACKPAD_Z_ON_LEFT: u8 = 28;
    pub(crate) const TRACKPAD_Z_OFF_LEFT: u8 = 29;
    pub(crate) const TRACKPAD_OUTER_SPIN_VEL: u8 = 30;
    pub(crate) const TRACKPAD_OUTER_SPIN_RADIUS: u8 = 31;
    pub(crate) const TRACKPAD_OUTER_SPIN_HORIZONTAL_ONLY: u8 = 32;
    pub(crate) const TRACKPAD_RELATIVE_MODE_DEADZONE: u8 = 33;
    pub(crate) const TRACKPAD_RELATIVE_MODE_MAX_VEL: u8 = 34;
    pub(crate) const TRACKPAD_RELATIVE_MODE_INVERT_Y: u8 = 35;
    pub(crate) const TRACKPAD_DOUBLE_TAP_BEEP_ENABLED: u8 = 36;
    pub(crate) const TRACKPAD_DOUBLE_TAP_BEEP_PERIOD: u8 = 37;
    pub(crate) const TRACKPAD_DOUBLE_TAP_BEEP_COUNT: u8 = 38;
    pub(crate) const TRACKPAD_OUTER_RADIUS_RELEASE_ON_TRANSITION: u8 = 39;
    pub(crate) const RADIAL_MODE_ANGLE: u8 = 40;
    pub(crate) const HAPTIC_INTENSITY_MOUSE_MODE: u8 = 41;
    pub(crate) const LEFT_DPAD_REQUIRES_CLICK: u8 = 42;
    pub(crate) const RIGHT_DPAD_REQUIRES_CLICK: u8 = 43;
    pub(crate) const LED_BASELINE_BRIGHTNESS: u8 = 44;
    pub(crate) const LED_USER_BRIGHTNESS: u8 = 45; // (used)
    pub(crate) const ENABLE_RAW_JOYSTICK: u8 = 46;
    pub(crate) const ENABLE_FAST_SCAN: u8 = 47;
    pub(crate) const IMU_MODE: u8 = 48; // (used) gyro/accel mode bits (see command::ImuMode)
    pub(crate) const WIRELESS_PACKET_VERSION: u8 = 49;
    pub(crate) const SLEEP_INACTIVITY_TIMEOUT: u8 = 50; // (used) idle timeout (s)
    pub(crate) const TRACKPAD_NOISE_THRESHOLD: u8 = 51;
    pub(crate) const LEFT_TRACKPAD_CLICK_PRESSURE: u8 = 52;
    pub(crate) const RIGHT_TRACKPAD_CLICK_PRESSURE: u8 = 53;
    pub(crate) const LEFT_BUMPER_CLICK_PRESSURE: u8 = 54;
    pub(crate) const RIGHT_BUMPER_CLICK_PRESSURE: u8 = 55;
    pub(crate) const LEFT_GRIP_CLICK_PRESSURE: u8 = 56;
    pub(crate) const RIGHT_GRIP_CLICK_PRESSURE: u8 = 57;
    pub(crate) const LEFT_GRIP2_CLICK_PRESSURE: u8 = 58; // Deck (4 back buttons)
    pub(crate) const RIGHT_GRIP2_CLICK_PRESSURE: u8 = 59; // Deck
    pub(crate) const PRESSURE_MODE: u8 = 60;
    pub(crate) const CONTROLLER_TEST_MODE: u8 = 61;
    pub(crate) const TRIGGER_MODE: u8 = 62;
    pub(crate) const TRACKPAD_Z_THRESHOLD: u8 = 63;
    pub(crate) const FRAME_RATE: u8 = 64;
    pub(crate) const TRACKPAD_FILT_CTRL: u8 = 65;
    pub(crate) const TRACKPAD_CLIP: u8 = 66;
    pub(crate) const DEBUG_OUTPUT_SELECT: u8 = 67;
    pub(crate) const TRIGGER_THRESHOLD_PERCENT: u8 = 68;
    pub(crate) const TRACKPAD_FREQUENCY_HOPPING: u8 = 69;
    pub(crate) const HAPTICS_ENABLED: u8 = 70;
    pub(crate) const STEAM_WATCHDOG_ENABLE: u8 = 71;
    pub(crate) const TIMP_TOUCH_THRESHOLD_ON: u8 = 72;
    pub(crate) const TIMP_TOUCH_THRESHOLD_OFF: u8 = 73;
    pub(crate) const FREQ_HOPPING: u8 = 74;
    pub(crate) const TEST_CONTROL: u8 = 75;
    pub(crate) const HAPTIC_MASTER_GAIN_DB: u8 = 76;
    pub(crate) const THUMB_TOUCH_THRESH: u8 = 77;
    pub(crate) const DEVICE_POWER_STATUS: u8 = 78;
    pub(crate) const HAPTIC_INTENSITY: u8 = 79;
    pub(crate) const STABILIZER_ENABLED: u8 = 80;
    pub(crate) const TIMP_MODE_MTE: u8 = 81;
    // SETTING_COUNT = 82; SETTING_ALL = 0xFF.
}

/// Trackpad-mode *values* for the `LEFT/RIGHT_TRACKPAD_MODE` settings.
pub(crate) mod trackpad_mode {
    /// Disables the pad for the mapper → raw input (lizard-off).
    pub(crate) const NONE: u8 = 7;
}

/// String-attribute id for the unit serial number (used with `GET_STRING_ATTRIBUTE`).
pub(crate) const ATTRIB_STR_UNIT_SERIAL: u8 = 0x01;

// =====================================================================================
// 3. Command payload structs (+ the value-enums they use), command-id ascending. Each `to_bytes()`
//    emits the little-endian wire body (the `[cmd_id, len]` header is added by `device.rs`).
// =====================================================================================

/// One `settingNum: u8, settingValue: u16` pair — the element of `SET_SETTINGS_VALUES` (`0x87`).
///
/// Mirrors SDL `ControllerSetting`; `MsgSetSettingsValues` is just an array of these. `device.rs`
/// concatenates one per setting written.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ControllerSetting {
    pub setting_num: u8,
    pub value: u16,
}

impl ControllerSetting {
    pub(crate) fn to_bytes(self) -> [u8; 3] {
        let [lo, hi] = self.value.to_le_bytes();
        [self.setting_num, lo, hi]
    }
}

/// Payload of `TRIGGER_HAPTIC_PULSE` (`0x8F`) — a trackpad haptic pulse train (Gordon's only haptic;
/// works on the Deck too). The actuator plays `count` pulses, each `duration` µs on then `interval`
/// µs off, so `duration`/`interval` set the tone and `count` its length. `gain` (dB) is honored on
/// the Deck, **inert on Gordon** (amplitude there = duty cycle).
///
/// **Layout = the kernel's 8-byte `steam_haptic_pulse` form** (HW-verified on Gordon; the kernel is
/// the USB authority). SDL's `MsgFireHapticPulse` is a **10-byte variant** (`dBgain` as `short`/i16
/// plus a trailing `priority` byte) that we deliberately do **not** use. `which_pad`: **0 = right,
/// 1 = left** (the kernel's legacy swap); pad 2 (both) no-ops on HW, so the caller drives the two
/// pads separately.
#[derive(Debug, Clone)]
pub(crate) struct MsgFireHapticPulse {
    pub which_pad: u8,
    pub duration: u16,
    pub interval: u16,
    pub count: u16,
    pub gain: i8,
}

impl MsgFireHapticPulse {
    pub(crate) fn to_bytes(&self) -> [u8; 8] {
        let d = self.duration.to_le_bytes();
        let i = self.interval.to_le_bytes();
        let c = self.count.to_le_bytes();
        [self.which_pad, d[0], d[1], i[0], i[1], c[0], c[1], self.gain as u8]
    }
}

/// Haptic-command type — the `cmd` field of [`MsgTriggerHaptic`] (`0x8f` is a separate pulse). SDL
/// `haptic_type_t`, full set (we currently only send `Tick`/`Click` for the Deck command-click;
/// `Tone`/`LogSweep` — a firmware-synthesized tone/sweep with a real `freq` — are Valve-defined but
/// **sent by no reference and HW-unproven**, kept here for completeness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum HapticType {
    #[default]
    Off = 0,
    Tick = 1,
    Click = 2,
    Tone = 3,
    Rumble = 4,
    Noise = 5,
    Script = 6,
    LogSweep = 7,
}

/// UI-intensity — the `ui_intensity` field of [`MsgTriggerHaptic`]. SDL `haptic_intensity_t`.
/// **HW-observed on the Deck:** `System`..`Medium` (0..2) feel identical, `Long` (3) is noticeably
/// stronger, `Insane` (4) is sometimes stronger / sometimes a different character; values outside
/// 0..=4 do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum HapticIntensity {
    #[default]
    System = 0,
    Short = 1,
    Medium = 2,
    Long = 3,
    Insane = 4,
}

/// Payload of `TRIGGER_HAPTIC_CMD` (`0xEA`) — the Deck's `SET_HAPTIC2`. The full Valve struct,
/// `MsgTriggerHaptic` (SDL `controller_structs.h`, present since 2023-12-19). We send it for
/// the Deck's short trackpad **click** (`cmd = Tick|Click`, `ui_intensity`, `dbgain`); the remaining
/// tone/noise/lfo/sweep fields are left zero (they apply only to `Tone`/`Noise`/`LogSweep`, which we
/// don't send yet).
///
/// **Note:** the C#/InputPlumber "short packet" with a `0x04` + timestamp tail was reverse-engineered
/// before this struct was found — those tail bytes land on `freq`/`dur_ms`/`lfo` and are inert for a
/// click, which is why zeroing them (as here) makes no HW difference. `side`: 0 = left, 1 = right,
/// 2 = both (all HW-verified; note this is the **reverse** of `0x8f`'s `which_pad`).
#[derive(Debug, Clone, Default)]
pub(crate) struct MsgTriggerHaptic {
    pub side: u8,
    pub cmd: HapticType,
    pub ui_intensity: HapticIntensity,
    pub dbgain: i8,
    pub freq: u16,
    pub dur_ms: i16,
    pub noise_intensity: u16,
    pub lfo_freq: u16,
    pub lfo_depth: u8,
    pub rand_tone_gain: u8,
    pub script_id: u8,
    pub lss_start_freq: u16,
    pub lss_end_freq: u16,
}

impl MsgTriggerHaptic {
    pub(crate) fn to_bytes(&self) -> [u8; 19] {
        let freq = self.freq.to_le_bytes();
        let dur = self.dur_ms.to_le_bytes();
        let noise = self.noise_intensity.to_le_bytes();
        let lfo = self.lfo_freq.to_le_bytes();
        let lss_s = self.lss_start_freq.to_le_bytes();
        let lss_e = self.lss_end_freq.to_le_bytes();
        [
            self.side,
            self.cmd as u8,
            self.ui_intensity as u8,
            self.dbgain as u8,
            freq[0], freq[1],
            dur[0], dur[1],
            noise[0], noise[1],
            lfo[0], lfo[1],
            self.lfo_depth,
            self.rand_tone_gain,
            self.script_id,
            lss_s[0], lss_s[1],
            lss_e[0], lss_e[1],
        ]
    }
}

/// Payload of `TRIGGER_RUMBLE_CMD` (`0xEB`) — the Deck's native dual-motor rumble. Mirrors SDL
/// `MsgSimpleRumbleCmd` exactly. `left_speed`/`right_speed` are the per-motor pulse **rate**;
/// `left_gain`/`right_gain` (dB) the amplitude trim; `intensity` a finer, **inverted** amplitude
/// lever (`0` = strongest). `rumble_type` (SDL `unRumbleType`) is HW-confirmed inert → always 0.
/// **Deck-only** (Gordon has no motors). `Motor` mapping is via the caller (left = strong/large
/// motor, right = weak/small).
#[derive(Debug, Clone)]
pub(crate) struct MsgSimpleRumbleCmd {
    pub rumble_type: u8,
    pub intensity: u16,
    pub left_speed: u16,
    pub right_speed: u16,
    pub left_gain: i8,
    pub right_gain: i8,
}

impl MsgSimpleRumbleCmd {
    pub(crate) fn to_bytes(&self) -> [u8; 9] {
        let it = self.intensity.to_le_bytes();
        let l = self.left_speed.to_le_bytes();
        let r = self.right_speed.to_le_bytes();
        [self.rumble_type, it[0], it[1], l[0], l[1], r[0], r[1], self.left_gain as u8, self.right_gain as u8]
    }
}

// =====================================================================================
// 4. Gordon Bluetooth (BLE) transport framing + compact input layout.
// =====================================================================================

/// The kernel `hid-steam` driver is USB-only, so BLE is reverse-engineered from SDL
/// (`SDL_hidapi_steam.c`) and sc-controller (`sc_by_bt`). Everything rides **Report ID 3** on a
/// 20-byte HID report (report id + 1 header byte + 18 payload). Feature *and* input reports longer
/// than 18 bytes are split into segments; the command bytes themselves are identical to USB.
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

// =====================================================================================
// 5. Triton (new Steam Controller) — its haptics ride OUTPUT reports, not feature reports. Full
//    struct/enum modelling of these is a SECOND PASS; for now, the report/output ids only.
// =====================================================================================

/// Reverse-engineered from SDL `SDL_hidapi_steam_triton.c` + `steam/controller_structs.h` (Valve's
/// own struct names) and sc-controller `sc2.py` / `docs/steam-controller-v2-protocol.md`. Triton
/// does **not** use the `0x01`-framed report of Gordon/Neptune: its input and haptic reports carry
/// the **report id in byte 0** (dispatched in `parse_triton`).
pub(crate) mod triton {
    /// Input/status report ids (byte 0 of each read).
    pub(crate) mod report {
        /// Main gamepad state (with on-controller quaternion on older firmware). **HW: the real
        /// puck/dongle (0x1304) streams `0x42` by default** (firmware sends the quaternion body) —
        /// parsed as NoQuat regardless.
        pub(crate) const STATE: u8 = 0x42;
        /// Battery status.
        pub(crate) const BATTERY: u8 = 0x43;
        /// Gamepad state, "NoQuat" body — same leading fields as `STATE`, parsed identically. **HW:
        /// the real controller over Bluetooth (0x1303) streams `0x45`**, whereas puck/wire stream
        /// `0x42`.
        pub(crate) const STATE_NOQUAT: u8 = 0x45;
        /// Wireless connect/disconnect status (dongle), alternate id.
        pub(crate) const WIRELESS_X: u8 = 0x46;
        /// Gamepad state with a trackpad timestamp + 16-bit IMU timestamp ("Ibex" packet).
        /// Not parsed yet — added only if a unit is seen streaming it.
        pub(crate) const STATE_TIMESTAMP: u8 = 0x47;
        /// Wireless connect/disconnect status (dongle).
        pub(crate) const WIRELESS: u8 = 0x79;
    }

    /// Payload byte of a wireless-status report (`WIRELESS`/`WIRELESS_X`).
    pub(crate) mod wireless {
        pub(crate) const DISCONNECT: u8 = 1;
        pub(crate) const CONNECT: u8 = 2;
    }

    /// Haptic **output**-report ids (Triton drives haptics via output reports, not feature reports).
    /// Recorded in full for reference; only a subset is wired up initially. **Second pass:** promote
    /// these to full payload structs + enums (SDL `MsgHapticRumble`/`MsgHapticCommand`/`MsgHapticLfoTone`
    /// /`MsgHapticLogSweep`/`MsgHapticScript`) alongside a Triton haptic-style enum.
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

// =====================================================================================
// INBOUND — input-report parsing constants. NOT part of the command protocol; these (and the input
// report structs, currently in `report.rs`) migrate into this file in a LATER dedicated pass.
// =====================================================================================

/// Input-frame event type, at byte offset 2 of every Gordon/Neptune report.
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

/// IMU scale constants — HW-verified on Gordon. `ControllerState` carries the raw i16 IMU readings,
/// so consumers (the engine's gyro-to-mouse) need these to convert to physical units:
/// `raw / GYRO_RES_PER_DPS` = degrees/second, `raw / ACCEL_RES_PER_G` = g. Re-exported at the crate
/// root so there's a single source of truth for the scale.
pub const ACCEL_RES_PER_G: f32 = 16384.0;
pub const GYRO_RES_PER_DPS: f32 = 16.0;
