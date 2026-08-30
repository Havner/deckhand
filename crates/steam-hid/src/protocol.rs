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

/// Feature-report command IDs (SDL `FeatureReportMessageIDs`), numeric-sorted, full set. Markers:
/// `(used)` = we send it; **`(no-payload)`** = a fire-and-forget command that carries no payload, so
/// it never needs a payload struct; `(no-payload?)` = the same but unverified; `⚠` = destructive.
///
/// NOTE: SDL's `DigitalIO` (~75) + `AnalogIO` (~25) enums are the mapping-target vocabulary for
/// `SetDigitalMappings`. We bypass on-controller mapping entirely (`ClearDigitalMappings` + map in our
/// engine), so those enums are intentionally NOT mirrored here. See SDL `controller_constants.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
// `TriggerHapticCmd`/`TriggerRumbleCmd` keep SDL's `_CMD` suffix (distinguishes 0xEA from the 0x8F
// `TriggerHapticPulse`), which trips the "variant ends with enum name" lint.
#[allow(clippy::enum_variant_names)]
pub(crate) enum Cmd {
    // --- digital button mappings (lizard-mode gamepad emulation) ---
    SetDigitalMappings = 0x80,
    ClearDigitalMappings = 0x81, // (used) lizard-off (no-payload)
    GetDigitalMappings = 0x82,
    GetAttributesValues = 0x83, // (used) read-only attributes
    GetAttributeLabel = 0x84,
    SetDefaultDigitalMappings = 0x85, // (used) lizard-on (no-payload)
    FactoryReset = 0x86,              // ⚠ wipes config
    // --- settings I/O (see the `setting` table + `ControllerSetting`) ---
    SetSettingsValues = 0x87, // (used)
    ClearSettingsValues = 0x88,
    GetSettingsValues = 0x89, // (used) read setting values
    GetSettingLabel = 0x8A,
    GetSettingsMaxs = 0x8B,
    GetSettingsDefaults = 0x8C,
    SetControllerMode = 0x8D,  // SDL/IP only (not in kernel)
    LoadDefaultSettings = 0x8E, // (used) lizard-on (no-payload)
    // --- haptics (pulse; the `0xEA`/`0xEB` command haptics live in section 3 structs) ---
    TriggerHapticPulse = 0x8F, // (used) → MsgFireHapticPulse
    TurnOffController = 0x9F,   // (used) power off — takes the "off!" magic (NOT no-payload)
    // --- read-only queries ---
    GetDeviceInfo = 0xA1,
    // --- calibration (fire-and-forget triggers; payloads unconfirmed) ---
    CalibrateTrackpads = 0xA7, // (no-payload?)
    Reserved0 = 0xA8,
    SetSerialNumber = 0xA9, // ⚠ writes the unit serial
    GetTrackpadCalibration = 0xAA,
    GetTrackpadFactoryCalibration = 0xAB,
    GetTrackpadRawData = 0xAC,
    // --- dongle / pairing ---
    EnablePairing = 0xAD,
    GetStringAttribute = 0xAE, // (used) serial getter
    RadioEraseRecords = 0xAF,  // ⚠⚠ firmware/radio — DO NOT TOUCH
    RadioWriteRecord = 0xB0,   // ⚠⚠ firmware/radio — DO NOT TOUCH
    SetDongleSetting = 0xB1,
    DongleDisconnectDevice = 0xB2,
    DongleCommitDevice = 0xB3,     // (no-payload) — the empty struct we dropped
    DongleGetWirelessState = 0xB4, // (used) dongle prompt (no-payload)
    CalibrateGyro = 0xB5,          // (no-payload?)
    // --- audio (preset play + custom-audio upload; the whole path is unimplemented, no payload ref) ---
    PlayAudio = 0xB6,
    AudioUpdateStart = 0xB7,
    AudioUpdateData = 0xB8,
    AudioUpdateComplete = 0xB9,
    GetChipId = 0xBA,
    CalibrateJoystick = 0xBF,       // (no-payload?)
    CalibrateAnalogTriggers = 0xC0, // (no-payload?)
    SetAudioMapping = 0xC1,
    CheckGyroFwLoad = 0xC2,
    CalibrateAnalog = 0xC3, // (no-payload?)
    DongleGetConnectedSlots = 0xC4,
    ResetImu = 0xCE, // (no-payload?)
    // --- command haptics (Deck; section-3 structs) ---
    TriggerHapticCmd = 0xEA, // (used) → MsgTriggerHaptic
    TriggerRumbleCmd = 0xEB, // (used) → MsgSimpleRumbleCmd
    // --- unknown opcodes (no reference documents these — purpose TBD) ---
    /// InputPlumber `UnknownDc` — seen in its command enum, purpose unknown. sc-controller lists
    /// `DC` among the Triton v2 pairing opcodes (`ED`/`AD`/`DC`/`E2`) it captured but never decoded.
    UnknownDc = 0xDC,
    /// InputPlumber `UnknownE2` — as above; also in sc-controller's Triton pairing opcode set.
    UnknownE2 = 0xE2,
    /// sc-controller Triton v2 pairing opcode `ED` (captured, undecoded). Not in SDL/kernel/IP.
    UnknownEd = 0xED,
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
    pub(crate) const IMU_MODE: u8 = 48; // (used) gyro/accel mode bits (see `GyroMode`)
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

// --- setting VALUE enums (what gets written into a specific setting) ---

/// Trackpad operating mode — value for `LEFT/RIGHT_TRACKPAD_MODE` (and the `*_SECONDARY_MODE`
/// settings). SDL `TrackpadDPadMode`. Lizard-off writes **`None`** to hand the mapper raw pad data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrackpadDPadMode {
    AbsoluteMouse = 0,
    RelativeMouse = 1,
    DpadFourWayDiscrete = 2,
    DpadFourWayOverlap = 3,
    DpadEightWay = 4,
    RadialMode = 5,
    AbsoluteDpad = 6,
    None = 7, // (used) lizard-off → raw pad
    GestureKeyboard = 8,
}

/// Value for the `LIZARD_MODE` setting (id 9). SDL `LizardModeState_t`. We reach lizard-off via
/// `CLEAR_DIGITAL_MAPPINGS` + pad `None` instead, so this is unused here — **HW-UNTESTED**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LizardModeState {
    Off = 0,
    On = 1,
}

/// Value for `SET_DONGLE_SETTING` (`0xB1`). SDL `DongleSettings`. **HW-UNTESTED.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DongleSetting {
    MouseKeyboardEnabled = 0,
}

/// Priority flags for the `0x8f` pulse's `priority` byte (SDL's 10-byte `MsgFireHapticPulse` variant).
/// We send the kernel 8-byte form with no priority byte, so this is informational. SDL
/// `SettingHapticPulseFlags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HapticPulseFlags {
    Normal = 0,
    HighPriority = 1,
    VeryHighPriority = 2,
    IgnoreUserPrefs = 3,
}

bitflags::bitflags! {
    /// IMU mode bits — the value written to the `IMU_MODE` setting (id 48). SDL `SettingGyroMode`
    /// (a bitmask; we model it as bitflags); matches the C# `GCGyroMode` / kernel gyro-mode bits.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct GyroMode: u16 {
        const STEERING         = 0x01;
        const TILT             = 0x02;
        const SEND_ORIENTATION = 0x04;
        const SEND_RAW_ACCEL   = 0x08;
        const SEND_RAW_GYRO    = 0x10;
    }
}

impl GyroMode {
    /// Raw accel + raw gyro — what `set_gyro(true)` enables.
    pub fn raw_motion() -> Self {
        Self::SEND_RAW_ACCEL | Self::SEND_RAW_GYRO
    }
}

/// Index into a [`SettingValueRange`] triple — SDL `SettingDefaultMinMax`. The reply of
/// `GET_SETTINGS_DEFAULTS` (`0x8C`) / `GET_SETTINGS_MAXS` (`0x8B`) packs default/min/max in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingDefaultMinMax {
    Default = 0,
    Min = 1,
    Max = 2,
    Count = 3,
}

/// Wireless scan intervals (SDL `#define FAST/SLOW_SCAN_INTERVAL`). Dongle/radio scan cadence; exact
/// use unconfirmed. Trace only.
pub(crate) const FAST_SCAN_INTERVAL: u8 = 6;
pub(crate) const SLOW_SCAN_INTERVAL: u8 = 9;

// --- read-only attribute tags (which attribute to query; the response structs live in the INBOUND
//     section, parsing deferred) ---

/// Numeric read-only attribute tags for `GET_ATTRIBUTES_VALUES` (`0x83`). SDL `ControllerAttributes`
/// (its struct element is [`ControllerAttribute`]). `Capabilities` aliases the deprecated
/// `PRODUCT_REVISION` at index 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerAttributes {
    UniqueId = 0,
    ProductId = 1,
    Capabilities = 2, // aka the deprecated PRODUCT_REVISION
    FirmwareVersion = 3, // deprecated
    FirmwareBuildTime = 4,
    RadioFirmwareBuildTime = 5,
    RadioDeviceId0 = 6,
    RadioDeviceId1 = 7,
    DongleFirmwareBuildTime = 8,
    BoardRevision = 9,
    BootloaderBuildTime = 10,
    ConnectionIntervalInUs = 11,
}

impl ControllerAttributes {
    /// Name a raw attribute tag byte (as returned by `GET_ATTRIBUTES_VALUES`); `None` for a tag we
    /// don't have a name for.
    pub fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            0 => Self::UniqueId,
            1 => Self::ProductId,
            2 => Self::Capabilities,
            3 => Self::FirmwareVersion,
            4 => Self::FirmwareBuildTime,
            5 => Self::RadioFirmwareBuildTime,
            6 => Self::RadioDeviceId0,
            7 => Self::RadioDeviceId1,
            8 => Self::DongleFirmwareBuildTime,
            9 => Self::BoardRevision,
            10 => Self::BootloaderBuildTime,
            11 => Self::ConnectionIntervalInUs,
            _ => return None,
        })
    }
}

/// String read-only attribute tags for `GET_STRING_ATTRIBUTE` (`0xAE`). SDL
/// `ControllerStringAttributes`. `UnitSerial` is the real per-unit serial getter (its response is
/// [`MsgGetStringAttribute`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerStringAttributes {
    BoardSerial = 0,
    UnitSerial = 1,
}

// =====================================================================================
// 3. Command payload structs (+ the value-enums they use), command-id ascending. Each `to_bytes()`
//    emits the little-endian wire body; `device::feature` prefixes the [`FeatureReportHeader`].
// =====================================================================================

/// The 2-byte header prefixing every host→controller feature-report command: SDL
/// `FeatureReportHeader` (`{ type, length }`). `device::feature` writes this ahead of a payload's
/// `to_bytes()`. (`type` is a Rust keyword, so the command-id field is named `cmd` here.)
#[derive(Debug, Clone, Copy)]
pub(crate) struct FeatureReportHeader {
    pub cmd: u8,
    pub length: u8,
}

impl FeatureReportHeader {
    pub(crate) fn to_bytes(self) -> [u8; 2] {
        [self.cmd, self.length]
    }
}

/// One `settingNum: u8, settingValue: u16` pair — mirrors SDL `ControllerSetting`. An array of these
/// is the payload for **`SET_SETTINGS_VALUES` (`0x87`)** (what `device.rs` sends), and the same array
/// shape is the *request* for **`GET_SETTINGS_VALUES` (`0x89`)** / **`GET_SETTINGS_MAXS` (`0x8B`)** /
/// **`GET_SETTINGS_DEFAULTS` (`0x8C`)** (SDL's `MsgSetSettingsValues`/`MsgGetSettings*` are all this
/// one array — not distinct types). **HW: SET verified (used).**
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

/// Payload of `SET_CONTROLLER_MODE` (`0x8D`) — SDL `MsgSetControllerMode` (`{ mode }`); selects a
/// controller operating mode. **The `mode` values are undocumented in our sources and we don't send
/// this — HW-UNTESTED**, kept as a trace.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MsgSetControllerMode {
    pub mode: u8,
}

impl MsgSetControllerMode {
    pub(crate) fn to_bytes(self) -> [u8; 1] {
        [self.mode]
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

/// SDL `MsgHapticSetMode` (`{ mode }`). Present in SDL's `FeatureReportMsg` union but **no command id
/// in SDL/kernel references it**, so we can't send it — purpose unclear, **HW-UNTESTED**. Trace only.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MsgHapticSetMode {
    pub mode: u8,
}

impl MsgHapticSetMode {
    pub(crate) fn to_bytes(self) -> [u8; 1] {
        [self.mode]
    }
}

/// Payload of `ENABLE_PAIRING` (`0xAD`) — begin/stop dongle pairing. SDL builds this inline in
/// `SDL_hidapi_steam.c` (`[0xAD, 2, enable, duration_s]`; **no named struct there**), so this form is
/// ours. `enable` = 0/1, `duration_s` = the pairing window in seconds. Flow:
/// `ENABLE_PAIRING(1, secs)` → the controller announces (wireless status) →
/// `DONGLE_COMMIT_DEVICE` (`0xB3`, no payload) accepts it. **HW-UNTESTED.**
#[derive(Debug, Clone, Copy)]
pub(crate) struct MsgEnablePairing {
    pub enable: u8,
    pub duration_s: u8,
}

impl MsgEnablePairing {
    pub(crate) fn to_bytes(self) -> [u8; 2] {
        [self.enable, self.duration_s]
    }
}

/// Preset sound slot for `PLAY_AUDIO` (`0xB6`) — SDL `ControllerAudio`. 0..=6 are Valve's named
/// presets; 7..=14 are undocumented (filler names); `MaxSlot` = 15 (`AUDIO_MAX_SLOT`) bounds the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerAudio {
    Startup = 0,
    Shutdown = 1,
    Pair = 2,
    PairSuccess = 3,
    Identify = 4, // ≈ the "ding"
    LizardMode = 5,
    NormalMode = 6,
    Reserved7 = 7,
    Reserved8 = 8,
    Reserved9 = 9,
    Reserved10 = 10,
    Reserved11 = 11,
    Reserved12 = 12,
    Reserved13 = 13,
    Reserved14 = 14,
    MaxSlot = 15,
}

/// Payload of `PLAY_AUDIO` (`0xB6`) — play a firmware preset sound. **No SDL struct exists** (SDL/
/// kernel/C#/sc-controller define the id + `ControllerAudio` enum, but none SEND it); this single-slot
/// form is ours. **HW-DISPROVEN as a standalone send:** sweeping slots 0..14 on Gordon+Triton was
/// silent — the presets are empty until Steam uploads audio (`0xB7`–`0xB9`+`0xC1`, an undocumented
/// blob). Recorded for completeness; the enum is real.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MsgPlayAudio {
    pub slot: ControllerAudio,
}

impl MsgPlayAudio {
    pub(crate) fn to_bytes(self) -> [u8; 1] {
        [self.slot as u8]
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

    // Haptic OUTPUT reports live below the `triton` module as top-level structs (see
    // `TritonOutReport` + the `MsgHaptic*` structs), matching the section-3 command-struct style.
}

/// Triton haptic **output**-report ids — SDL `ValveTritonOutReportMessageIDs`. Triton drives haptics
/// via **output reports** (interrupt-OUT endpoint, `Device::output`), not feature reports; each id is
/// the first byte of its report. Payloads are the `MsgHaptic*` structs below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum TritonOutReport {
    Rumble = 0x80,
    Pulse = 0x81,
    Command = 0x82,
    LfoTone = 0x83,
    LogSweep = 0x84,
    Script = 0x85,
}

/// Triton `0x80` dual-motor **rumble** — SDL `MsgHapticRumble`. `rumble_type` is HW-confirmed inert
/// (like the Deck's `unRumbleType`) → send 0; `intensity` a finer amplitude lever (SDL sends 0);
/// per-motor `speed` (drive rate) + `gain` (dB). Same levers as the Deck's `0xeb`. **HW-verified
/// (puck).** `to_bytes` prepends the report id.
#[derive(Debug, Clone)]
pub(crate) struct MsgHapticRumble {
    pub rumble_type: u8,
    pub intensity: u16,
    pub left_speed: u16,
    pub left_gain: i8,
    pub right_speed: u16,
    pub right_gain: i8,
}

impl MsgHapticRumble {
    pub(crate) fn to_bytes(&self) -> [u8; 10] {
        let it = self.intensity.to_le_bytes();
        let ls = self.left_speed.to_le_bytes();
        let rs = self.right_speed.to_le_bytes();
        [
            TritonOutReport::Rumble as u8,
            self.rumble_type,
            it[0], it[1],
            ls[0], ls[1], self.left_gain as u8,
            rs[0], rs[1], self.right_gain as u8,
        ]
    }
}

/// Triton `0x81` trackpad **pulse** — SDL `MsgHapticPulse` (Triton's analog of Gordon's `0x8f`).
/// `on_us`/`off_us` = pulse high/low µs, `repeat_count` = pulses. **Unused / HW-UNTESTED** (kept for
/// completeness — a Triton beep could ride this, cf. the audio work).
#[derive(Debug, Clone)]
pub(crate) struct MsgHapticPulse {
    pub side: u8,
    pub on_us: u16,
    pub off_us: u16,
    pub repeat_count: u16,
}

impl MsgHapticPulse {
    pub(crate) fn to_bytes(&self) -> [u8; 8] {
        let on = self.on_us.to_le_bytes();
        let off = self.off_us.to_le_bytes();
        let rc = self.repeat_count.to_le_bytes();
        [TritonOutReport::Pulse as u8, self.side, on[0], on[1], off[0], off[1], rc[0], rc[1]]
    }
}

/// Triton `0x82` haptic **command / click** — SDL `MsgHapticCommand`. `command` is the haptic type
/// (SDL types it a bare `u8`; we send off/weak/strong — possibly the shared `haptic_type_t`, only 3
/// HW-verified). `gain_db` is `i8` in SDL, but HW shows this byte as a subtle **unsigned** amplitude
/// trim (`0`=medium..`255`=strong, sc-controller); the reader sends it unsigned. **HW-verified
/// (puck).**
#[derive(Debug, Clone)]
pub(crate) struct MsgHapticCommand {
    pub side: u8,
    pub command: u8,
    pub gain_db: i8,
}

impl MsgHapticCommand {
    pub(crate) fn to_bytes(&self) -> [u8; 4] {
        [TritonOutReport::Command as u8, self.side, self.command, self.gain_db as u8]
    }
}

/// Triton `0x83` **LFO tone** — SDL `MsgHapticLfoTone`. A firmware-synthesized tone (the promising
/// Triton *audio* path): `frequency` Hz, `duration_ms`, `lfo_freq`/`lfo_depth` modulation.
/// **Unused / HW-UNTESTED.**
#[derive(Debug, Clone)]
pub(crate) struct MsgHapticLfoTone {
    pub side: u8,
    pub gain_db: i8,
    pub frequency: u16,
    pub duration_ms: u16,
    pub lfo_freq: u16,
    pub lfo_depth: u8,
}

impl MsgHapticLfoTone {
    pub(crate) fn to_bytes(&self) -> [u8; 10] {
        let f = self.frequency.to_le_bytes();
        let d = self.duration_ms.to_le_bytes();
        let lf = self.lfo_freq.to_le_bytes();
        [
            TritonOutReport::LfoTone as u8,
            self.side, self.gain_db as u8,
            f[0], f[1],
            d[0], d[1],
            lf[0], lf[1],
            self.lfo_depth,
        ]
    }
}

/// Triton `0x84` log-frequency **sweep** (chirp) — SDL `MsgHapticLogSweep`. **Unused / HW-UNTESTED.**
#[derive(Debug, Clone)]
pub(crate) struct MsgHapticLogSweep {
    pub side: u8,
    pub gain_db: i8,
    pub duration_ms: u16,
    pub start_freq: u16,
    pub end_freq: u16,
}

impl MsgHapticLogSweep {
    pub(crate) fn to_bytes(&self) -> [u8; 9] {
        let d = self.duration_ms.to_le_bytes();
        let s = self.start_freq.to_le_bytes();
        let e = self.end_freq.to_le_bytes();
        [TritonOutReport::LogSweep as u8, self.side, self.gain_db as u8, d[0], d[1], s[0], s[1], e[0], e[1]]
    }
}

/// Triton `0x85` named haptic **script** — SDL `MsgHapticScript`. `script_id` selects a firmware
/// effect; `gain_db` scales. **Unused / HW-UNTESTED.**
#[derive(Debug, Clone)]
pub(crate) struct MsgHapticScript {
    pub side: u8,
    pub script_id: u8,
    pub gain_db: i8,
}

impl MsgHapticScript {
    pub(crate) fn to_bytes(&self) -> [u8; 4] {
        [TritonOutReport::Script as u8, self.side, self.script_id, self.gain_db as u8]
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

/// `GET_ATTRIBUTES_VALUES` (`0x83`) response element — SDL `ControllerAttribute` (`{ tag, value }`).
/// **RESPONSE (read-back)**; `tag` is a [`ControllerAttributes`]. We don't query/parse attributes
/// yet, so this is a layout trace — a `from_bytes` lands in the inbound pass.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ControllerAttribute {
    pub tag: u8,
    pub value: u32,
}

/// `GET_STRING_ATTRIBUTE` (`0xAE`) response — SDL `MsgGetStringAttribute` (`{ tag, value[20] }`).
/// **RESPONSE (read-back)**; `tag` is a [`ControllerStringAttributes`] (e.g. `UnitSerial`). Layout
/// trace; parsing deferred to the inbound pass.
#[derive(Debug, Clone)]
pub(crate) struct MsgGetStringAttribute {
    pub tag: u8,
    pub value: [u8; 20],
}

/// `GET_SETTINGS_DEFAULTS` (`0x8C`) / `GET_SETTINGS_MAXS` (`0x8B`) reply element — SDL
/// `SettingValueRange_t` (`short defaultminmax[3]`, indexed by [`SettingDefaultMinMax`]).
/// **RESPONSE (read-back)**; parsing deferred to the inbound pass. **HW-UNTESTED.**
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingValueRange {
    /// `[default, min, max]` (i16), in `SettingDefaultMinMax` order.
    pub defaultminmax: [i16; 3],
}

/// Wireless dongle event type — SDL `EWirelessEventType`. **INBOUND** (dongle status). We currently
/// decode only connect/disconnect (see the `wireless` module above); the full set incl. `Pair` is a
/// trace, parsing deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WirelessEventType {
    Disconnect = 1,
    Connect = 2,
    Pair = 3,
}

/// Controller status event code — SDL `ControllerStatusEventCodes`. **INBOUND** (status reports),
/// parsing deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerStatusEventCode {
    Normal = 0,
    CriticalBattery = 1,
    GyroInitError = 2,
}

/// Controller status state flags — SDL `ControllerStatusStateFlags`. **INBOUND** (status reports),
/// parsing deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerStatusStateFlag {
    LowBattery = 0,
}

/// IMU scale constants — HW-verified on Gordon. `ControllerState` carries the raw i16 IMU readings,
/// so consumers (the engine's gyro-to-mouse) need these to convert to physical units:
/// `raw / GYRO_RES_PER_DPS` = degrees/second, `raw / ACCEL_RES_PER_G` = g. Re-exported at the crate
/// root so there's a single source of truth for the scale.
pub const ACCEL_RES_PER_G: f32 = 16384.0;
pub const GYRO_RES_PER_DPS: f32 = 16.0;
