//! Wire-level protocol - the **single canonical home for all hardware/protocol knowledge**
//! (CLAUDE.md golden rule). Command IDs, settings, and the exact payload struct layouts we send to
//! the hardware live here and nowhere else; `device.rs` only *uses* them.
//!
//! Naming follows the **kernel `hid-steam`** driver (command/setting IDs) and **SDL**
//! (`SDL/.../steam/controller_structs.h`, Valve's own struct names) for the payload structs. Each
//! command struct's doc names the command id it serializes for, the SDL struct it mirrors, and any
//! divergence. Wire structs are `#[repr(C, packed)]` so their in-memory bytes **are** the wire
//! layout, cast to/from bytes via the [`Wire`] trait (contained `unsafe`, size-guarded, LE-only -
//! see its docs). SDL/kernel are C, so the structs literally *are* the hardware.
//!
//! **Scope:** outbound commands (3), inbound GET responses (4), Triton haptic output (5), and the
//! inbound report packets + shared wire chunks (6) all live here. The transport *framing* (segment
//! reassembly, feature/output report I/O) stays in `device.rs`, which only *uses* these types.

#![allow(dead_code)]

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// =====================================================================================
// 0. Wire - the byte-cast serialization primitive shared by every payload/response/report struct
//    (3-6). The `unsafe` lives here, once, guarded by a `size_of` check.
// =====================================================================================

/// A `#[repr(C, packed)]` struct whose in-memory bytes are **identical to the wire layout**, so it
/// casts to/from bytes directly (SDL/kernel are C, so the structs *are* the hardware).
///
/// **LE hosts only:** the cast interprets multi-byte fields in *native* endianness; the wire is
/// little-endian, so this is correct only where native == LE (x86, ARM-LE - everything we run). A
/// big-endian host would byte-swap every `u16`/`i16`. Fields must be **plain integers/arrays** (no
/// enums/refs - any bit pattern must be valid), and every impl must be `#[repr(C, packed)]` with a
/// `const _: () = assert!(size_of == wire_len)` guard. Packed structs can only derive `Clone, Copy`
/// (derived `Debug`/`PartialEq` take references to unaligned fields) and their fields are read **by
/// value** (never `&field`).
pub(crate) trait Wire: Copy {
    /// The struct's bytes - the wire form (borrows `self`).
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: `Self` is `#[repr(C, packed)]` (align 1, no padding), so its `size_of` bytes are
        // exactly the wire layout; the slice borrows `self`.
        unsafe {
            core::slice::from_raw_parts((self as *const Self).cast::<u8>(), core::mem::size_of::<Self>())
        }
    }

    /// Parse the wire form from a byte slice (`None` if too short; trailing bytes ignored).
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < core::mem::size_of::<Self>() {
            return None;
        }
        // SAFETY: length checked above; `read_unaligned` tolerates the packed (align-1) source; all
        // fields are plain integers/arrays so every bit pattern is a valid `Self`.
        Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<Self>()) })
    }
}

// =====================================================================================
// 1. General constants - vendor/product ids, report framing.
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
/// per-slot HID interfaces like the original Gordon dongle - **interfaces 2..=6 on the observed
/// unit** (SDL documents 2..5; real hardware exposes one more). Unlike the Gordon dongle it reports
/// a real serial. (SDL also lists a second dongle variant, "Nereid" `0x1305`, which we don't add
/// until one is seen in the wild.)
pub(crate) const PID_TRITON_PUCK: u16 = 0x1304;

// =====================================================================================
// 2. Command & setting IDs. Kernel `hid-steam` naming (adopted); the full set is Valve's, from SDL
//    `controller_constants.h` (matches the kernel where both define an id). Recorded in full as a
//    knowledge trace - most are unused (`#![allow(dead_code)]`). `(used)` marks what we send today.
// =====================================================================================

/// Feature-report command IDs (SDL `FeatureReportMessageIDs`), numeric-sorted, full set. Markers:
/// `(used)` = we send it; **`(no-payload)`** = a fire-and-forget command that carries no payload, so
/// it never needs a payload struct; `(no-payload?)` = the same but unverified; `(!)` = destructive.
///
/// NOTE: SDL's `DigitalIO` (~75) + `AnalogIO` (~25) enums are the mapping-target vocabulary for
/// `SetDigitalMappings`. We bypass on-controller mapping entirely (`ClearDigitalMappings` + map in our
/// engine), so those enums are intentionally NOT mirrored here. See SDL `controller_constants.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum MsgId {
    // --- digital button mappings (lizard-mode gamepad emulation) ---
    SetDigitalMappings = 0x80,
    ClearDigitalMappings = 0x81, // (used) lizard-off (no-payload)
    GetDigitalMappings = 0x82,
    GetAttributesValues = 0x83, // (used) read-only attributes
    GetAttributeLabel = 0x84,
    SetDefaultDigitalMappings = 0x85, // (used) lizard-on (no-payload)
    FactoryReset = 0x86,              // (!) wipes config
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
    TriggerHapticPulse = 0x8F, // (used) -> MsgFireHapticPulse
    TurnOffController = 0x9F,   // (used) power off - takes the "off!" magic (NOT no-payload)
    // --- read-only queries ---
    GetDeviceInfo = 0xA1,
    // --- calibration (fire-and-forget triggers; payloads unconfirmed) ---
    CalibrateTrackpads = 0xA7, // (no-payload?)
    Reserved0 = 0xA8,
    SetSerialNumber = 0xA9, // (!) writes the unit serial
    GetTrackpadCalibration = 0xAA,
    GetTrackpadFactoryCalibration = 0xAB,
    GetTrackpadRawData = 0xAC,
    // --- dongle / pairing ---
    EnablePairing = 0xAD,
    GetStringAttribute = 0xAE, // (used) serial getter
    RadioEraseRecords = 0xAF,  // (!!) firmware/radio - DO NOT TOUCH
    RadioWriteRecord = 0xB0,   // (!!) firmware/radio - DO NOT TOUCH
    SetDongleSetting = 0xB1,
    DongleDisconnectDevice = 0xB2,
    DongleCommitDevice = 0xB3,     // (no-payload) - the empty struct we dropped
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
    TriggerHapticCmd = 0xEA, // (used) -> MsgTriggerHaptic
    TriggerRumbleCmd = 0xEB, // (used) -> MsgSimpleRumbleCmd
    // --- unknown opcodes (no reference documents these - purpose TBD) ---
    /// InputPlumber `UnknownDc` - seen in its command enum, purpose unknown. sc-controller lists
    /// `DC` among the Triton v2 pairing opcodes (`ED`/`AD`/`DC`/`E2`) it captured but never decoded.
    UnknownDc = 0xDC,
    /// InputPlumber `UnknownE2` - as above; also in sc-controller's Triton pairing opcode set.
    UnknownE2 = 0xE2,
    /// sc-controller Triton v2 pairing opcode `ED` (captured, undecoded). Not in SDL/kernel/IP.
    UnknownEd = 0xED,
}

/// Setting ids (SDL `ControllerSettings`; **index == id**, order frozen - "only add, never reorder").
/// Written via `SET_SETTINGS_VALUES` as [`ControllerSetting`] pairs. Full set as a trace; most unused.
pub(crate) mod setting {
    pub(crate) const MOUSE_SENSITIVITY: u8 = 0;
    pub(crate) const MOUSE_ACCELERATION: u8 = 1;
    pub(crate) const TRACKBALL_ROTATION_ANGLE: u8 = 2;
    pub(crate) const HAPTIC_INTENSITY_UNUSED: u8 = 3;
    pub(crate) const LEFT_GAMEPAD_STICK_ENABLED: u8 = 4;
    pub(crate) const RIGHT_GAMEPAD_STICK_ENABLED: u8 = 5;
    pub(crate) const USB_DEBUG_MODE: u8 = 6;
    pub(crate) const LEFT_TRACKPAD_MODE: u8 = 7; // (used) lizard-off -> NONE
    pub(crate) const RIGHT_TRACKPAD_MODE: u8 = 8; // (used) lizard-off -> NONE
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

/// Trackpad operating mode - value for `LEFT/RIGHT_TRACKPAD_MODE` (and the `*_SECONDARY_MODE`
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
    None = 7, // (used) lizard-off -> raw pad
    GestureKeyboard = 8,
}

/// Value for the `LIZARD_MODE` setting (id 9). SDL `LizardModeState_t`. We reach lizard-off via
/// `CLEAR_DIGITAL_MAPPINGS` + pad `None` instead, so this is unused here - **HW-UNTESTED**.
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
    /// IMU mode bits - the value written to the `IMU_MODE` setting (id 48). SDL `SettingGyroMode`
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
    /// Raw accel + raw gyro - what `set_gyro(true)` enables.
    pub fn raw_motion() -> Self {
        Self::SEND_RAW_ACCEL | Self::SEND_RAW_GYRO
    }
}

/// Index into a [`SettingValueRange`] triple - SDL `SettingDefaultMinMax`. The reply of
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

// --- read-only attribute tags (which attribute to query; the response structs live in 4,
//     parsing deferred) ---

/// Numeric read-only attribute tags for `GET_ATTRIBUTES_VALUES` (`0x83`). SDL `ControllerAttributes`
/// (`ATTRIB_*`, "only add, never reorder"); **const, not an enum** - `0x83` returns the *full set* and
/// each reply element ([`ControllerAttribute`]) echoes its tag, so these are received back /
/// dispatched-on (the const-vs-enum rule). `CAPABILITIES` intentionally aliases the deprecated
/// `PRODUCT_REVISION` at index 2 (const lets us keep both names, unlike the old enum).
pub(crate) mod attribute {
    pub(crate) const UNIQUE_ID: u8 = 0;
    pub(crate) const PRODUCT_ID: u8 = 1;
    pub(crate) const PRODUCT_REVISION: u8 = 2; // deprecated
    pub(crate) const CAPABILITIES: u8 = 2; // intentional alias of PRODUCT_REVISION
    pub(crate) const FIRMWARE_VERSION: u8 = 3; // deprecated
    pub(crate) const FIRMWARE_BUILD_TIME: u8 = 4;
    pub(crate) const RADIO_FIRMWARE_BUILD_TIME: u8 = 5;
    pub(crate) const RADIO_DEVICE_ID0: u8 = 6;
    pub(crate) const RADIO_DEVICE_ID1: u8 = 7;
    pub(crate) const DONGLE_FIRMWARE_BUILD_TIME: u8 = 8;
    pub(crate) const BOARD_REVISION: u8 = 9;
    pub(crate) const BOOTLOADER_BUILD_TIME: u8 = 10;
    pub(crate) const CONNECTION_INTERVAL_IN_US: u8 = 11;
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
// 3. Command payload structs (+ the value-enums they use), command-id ascending. Each is a [`Wire`]
//    struct (`as_bytes()` = the LE wire body); `device::feature` prefixes the [`FeatureReportHeader`].
// =====================================================================================

/// The 2-byte header prefixing every host->controller feature-report command: SDL
/// `FeatureReportHeader` (`{ type, length }`). `device::feature` writes this ahead of a payload's
/// `as_bytes()`. (`type` is a Rust keyword, so the command-id field is named `cmd` here.)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct FeatureReportHeader {
    pub cmd: u8,
    pub length: u8,
}
impl Wire for FeatureReportHeader {}
const _: () = assert!(core::mem::size_of::<FeatureReportHeader>() == 2);

/// One `settingNum: u8, settingValue: u16` pair - mirrors SDL `ControllerSetting`. An array of these
/// is the payload for **`SET_SETTINGS_VALUES` (`0x87`)** (what `device.rs` sends), and the same array
/// shape is the *request* for **`GET_SETTINGS_VALUES` (`0x89`)** / **`GET_SETTINGS_MAXS` (`0x8B`)** /
/// **`GET_SETTINGS_DEFAULTS` (`0x8C`)** (SDL's `MsgSetSettingsValues`/`MsgGetSettings*` are all this
/// one array - not distinct types). **HW: SET verified (used).**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct ControllerSetting {
    pub setting_num: u8,
    pub value: u16,
}
impl Wire for ControllerSetting {}
const _: () = assert!(core::mem::size_of::<ControllerSetting>() == 3);

/// Payload of `SET_CONTROLLER_MODE` (`0x8D`) - SDL `MsgSetControllerMode` (`{ mode }`); selects a
/// controller operating mode. **The `mode` values are undocumented in our sources and we don't send
/// this - HW-UNTESTED**, kept as a trace.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgSetControllerMode {
    pub mode: u8,
}
impl Wire for MsgSetControllerMode {}
const _: () = assert!(core::mem::size_of::<MsgSetControllerMode>() == 1);

/// Payload of `TRIGGER_HAPTIC_PULSE` (`0x8F`) - a trackpad haptic pulse train (Gordon's only haptic;
/// works on the Deck too). The actuator plays `count` pulses, each `duration` us on then `interval`
/// us off, so `duration`/`interval` set the tone and `count` its length. `gain` (dB) is honored on
/// the Deck, **inert on Gordon** (amplitude there = duty cycle).
///
/// **Layout = the kernel's 8-byte `steam_haptic_pulse` form** (HW-verified on Gordon; the kernel is
/// the USB authority). SDL's `MsgFireHapticPulse` is a **10-byte variant** (`dBgain` as `short`/i16
/// plus a trailing `priority` byte) that we deliberately do **not** use. `which_pad`: **0 = right,
/// 1 = left** (the kernel's legacy swap); pad 2 (both) no-ops on HW, so the caller drives the two
/// pads separately.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgFireHapticPulse {
    pub which_pad: u8,
    pub duration: u16,
    pub interval: u16,
    pub count: u16,
    pub gain: i8,
}
impl Wire for MsgFireHapticPulse {}
const _: () = assert!(core::mem::size_of::<MsgFireHapticPulse>() == 8);

/// SDL `MsgHapticSetMode` (`{ mode }`). Present in SDL's `FeatureReportMsg` union but **no command id
/// in SDL/kernel references it**, so we can't send it - purpose unclear, **HW-UNTESTED**. Trace only.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticSetMode {
    pub mode: u8,
}
impl Wire for MsgHapticSetMode {}
const _: () = assert!(core::mem::size_of::<MsgHapticSetMode>() == 1);

/// Payload of `ENABLE_PAIRING` (`0xAD`) - begin/stop dongle pairing. SDL builds this inline in
/// `SDL_hidapi_steam.c` (`[0xAD, 2, enable, duration_s]`; **no named struct there**), so this form is
/// ours. `enable` = 0/1, `duration_s` = the pairing window in seconds. Flow:
/// `ENABLE_PAIRING(1, secs)` -> the controller announces (wireless status) ->
/// `DONGLE_COMMIT_DEVICE` (`0xB3`, no payload) accepts it. **HW-UNTESTED.**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgEnablePairing {
    pub enable: u8,
    pub duration_s: u8,
}
impl Wire for MsgEnablePairing {}
const _: () = assert!(core::mem::size_of::<MsgEnablePairing>() == 2);

/// Preset sound slot for `PLAY_AUDIO` (`0xB6`) - SDL `ControllerAudio`. 0..=6 are Valve's named
/// presets; 7..=14 are undocumented (filler names); `MaxSlot` = 15 (`AUDIO_MAX_SLOT`) bounds the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerAudio {
    Startup = 0,
    Shutdown = 1,
    Pair = 2,
    PairSuccess = 3,
    Identify = 4, // ~ the "ding"
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

/// Payload of `PLAY_AUDIO` (`0xB6`) - play a firmware preset sound. **No SDL struct exists** (SDL/
/// kernel/C#/sc-controller define the id + `ControllerAudio` enum, but none SEND it); this single-slot
/// form is ours. **HW-DISPROVEN as a standalone send:** sweeping slots 0..14 on Gordon+Triton was
/// silent - the presets are empty until Steam uploads audio (`0xB7`-`0xB9`+`0xC1`, an undocumented
/// blob). Recorded for completeness; the enum is real.
///
/// `slot` is a raw `u8` (a [`Wire`] field must be a plain integer) holding a [`ControllerAudio`]
/// discriminant - the caller converts (`slot: ControllerAudio::Identify as u8`).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgPlayAudio {
    pub slot: u8, // ControllerAudio
}
impl Wire for MsgPlayAudio {}
const _: () = assert!(core::mem::size_of::<MsgPlayAudio>() == 1);

/// Haptic-command type - the `cmd` field of [`MsgTriggerHaptic`] (`0x8f` is a separate pulse). SDL
/// `haptic_type_t`, full set (we currently only send `Tick`/`Click` for the Deck command-click;
/// `Tone`/`LogSweep` - a firmware-synthesized tone/sweep with a real `freq` - are Valve-defined but
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

/// UI-intensity - the `ui_intensity` field of [`MsgTriggerHaptic`]. SDL `haptic_intensity_t`.
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

/// Payload of `TRIGGER_HAPTIC_CMD` (`0xEA`) - the Deck's `SET_HAPTIC2`. The full Valve struct,
/// `MsgTriggerHaptic` (SDL `controller_structs.h`, present since 2023-12-19). We send it for
/// the Deck's short trackpad **click** (`cmd = Tick|Click`, `ui_intensity`, `dbgain`); the remaining
/// tone/noise/lfo/sweep fields are left zero (they apply only to `Tone`/`Noise`/`LogSweep`, which we
/// don't send yet).
///
/// **Note:** the C#/InputPlumber "short packet" with a `0x04` + timestamp tail was reverse-engineered
/// before this struct was found - those tail bytes land on `freq`/`dur_ms`/`lfo` and are inert for a
/// click, which is why zeroing them (as here) makes no HW difference. `side`: 0 = left, 1 = right,
/// 2 = both (all HW-verified; note this is the **reverse** of `0x8f`'s `which_pad`).
///
/// `cmd`/`ui_intensity` are raw `u8` ([`Wire`] fields must be plain integers) holding a
/// [`HapticType`]/[`HapticIntensity`] discriminant - the caller converts (`cmd: ty as u8`). `0`
/// for both = `Off`/`System`, so `Default` (all-zero) is a valid inert packet.
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub(crate) struct MsgTriggerHaptic {
    pub side: u8,
    pub cmd: u8,          // HapticType
    pub ui_intensity: u8, // HapticIntensity
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
impl Wire for MsgTriggerHaptic {}
const _: () = assert!(core::mem::size_of::<MsgTriggerHaptic>() == 19);

/// Payload of `TRIGGER_RUMBLE_CMD` (`0xEB`) - the Deck's native dual-motor rumble. Mirrors SDL
/// `MsgSimpleRumbleCmd` exactly. `left_speed`/`right_speed` are the per-motor pulse **rate**;
/// `left_gain`/`right_gain` (dB) the amplitude trim; `intensity` a finer, **inverted** amplitude
/// lever (`0` = strongest). `rumble_type` (SDL `unRumbleType`) is HW-confirmed inert -> always 0.
/// **Deck-only** (Gordon has no motors). `Motor` mapping is via the caller (left = strong/large
/// motor, right = weak/small).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgSimpleRumbleCmd {
    pub rumble_type: u8,
    pub intensity: u16,
    pub left_speed: u16,
    pub right_speed: u16,
    pub left_gain: i8,
    pub right_gain: i8,
}
impl Wire for MsgSimpleRumbleCmd {}
const _: () = assert!(core::mem::size_of::<MsgSimpleRumbleCmd>() == 9);

// =====================================================================================
// 4. Command responses (read-back; inbound - replies to a GET, not input reports). Paired with the
//    outbound commands in 3; the attribute-tag enums they key on live in 2.
// =====================================================================================

/// `GET_ATTRIBUTES_VALUES` (`0x83`) response element - SDL `ControllerAttribute` (`{ tag, value }`).
/// `tag` is an [`attribute`] const id; `Device::get_attributes` reads the reply's 5-byte elements
/// via [`Wire::from_bytes`].
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct ControllerAttribute {
    pub tag: u8,
    pub value: u32,
}
impl Wire for ControllerAttribute {}
const _: () = assert!(core::mem::size_of::<ControllerAttribute>() == 5);

/// `GET_STRING_ATTRIBUTE` (`0xAE`) response body - SDL `MsgGetStringAttribute` (`{ tag, value[20] }`),
/// the bytes after the reply's `[cmd, len]` header. `tag` is a [`ControllerStringAttributes`]
/// discriminant; `Device::get_string_attribute` reads it via [`Wire::from_bytes`] and slices `value`
/// to the header's length.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgGetStringAttribute {
    pub tag: u8,
    pub value: [u8; 20],
}
impl Wire for MsgGetStringAttribute {}
const _: () = assert!(core::mem::size_of::<MsgGetStringAttribute>() == 21);

/// `GET_SETTINGS_DEFAULTS` (`0x8C`) / `GET_SETTINGS_MAXS` (`0x8B`) reply element - SDL
/// `SettingValueRange_t` (`short defaultminmax[3]`, indexed by [`SettingDefaultMinMax`]). **Unused -
/// we don't call the maxs/defaults getters yet; ready for [`Wire::from_bytes`] when we do.**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct SettingValueRange {
    /// `[default, min, max]` (i16), in `SettingDefaultMinMax` order.
    pub defaultminmax: [i16; 3],
}
impl Wire for SettingValueRange {}
const _: () = assert!(core::mem::size_of::<SettingValueRange>() == 6);

// =====================================================================================
// 5. Triton - haptic OUTPUT reports. Triton drives haptics via **output reports** (report id in
//    byte 0, interrupt-OUT endpoint), not feature reports. Its INPUT reports are in 6 below.
// =====================================================================================

/// Triton haptic **output**-report ids - SDL `ValveTritonOutReportMessageIDs`. Triton drives haptics
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

/// Triton `0x80` dual-motor **rumble** body - SDL `MsgHapticRumble`. `rumble_type` is HW-confirmed
/// inert (like the Deck's `unRumbleType`) -> send 0; `intensity` a finer amplitude lever (SDL sends 0);
/// per-motor `speed` (drive rate) + `gain` (dB). Same levers as the Deck's `0xeb`. **HW-verified
/// (puck).** Sent as [`TritonOutReport::Rumble`] (`device::output` prepends the report-id byte).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticRumble {
    pub rumble_type: u8,
    pub intensity: u16,
    pub left_speed: u16,
    pub left_gain: i8,
    pub right_speed: u16,
    pub right_gain: i8,
}
impl Wire for MsgHapticRumble {}
const _: () = assert!(core::mem::size_of::<MsgHapticRumble>() == 9);

/// Triton `0x81` trackpad **pulse** body - SDL `MsgHapticPulse` (Triton's analog of Gordon's `0x8f`).
/// `on_us`/`off_us` = pulse high/low us, `repeat_count` = pulses. Sent as [`TritonOutReport::Pulse`].
/// **Unused / HW-UNTESTED** (kept for completeness - a Triton beep could ride this, cf. the audio work).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticPulse {
    pub side: u8,
    pub on_us: u16,
    pub off_us: u16,
    pub repeat_count: u16,
}
impl Wire for MsgHapticPulse {}
const _: () = assert!(core::mem::size_of::<MsgHapticPulse>() == 7);

/// Triton `0x82` haptic **command / click** - SDL `MsgHapticCommand`. `command` is the haptic type
/// (SDL types it a bare `u8`; we send off/weak/strong - possibly the shared `haptic_type_t`, only 3
/// HW-verified). `gain_db` is `i8` in SDL, but HW shows this byte as a subtle **unsigned** amplitude
/// trim (`0`=medium..`255`=strong, sc-controller); the reader sends it unsigned. Sent as
/// [`TritonOutReport::Command`]. **HW-verified (puck).**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticCommand {
    pub side: u8,
    pub command: u8,
    pub gain_db: i8,
}
impl Wire for MsgHapticCommand {}
const _: () = assert!(core::mem::size_of::<MsgHapticCommand>() == 3);

/// Triton `0x83` **LFO tone** body - SDL `MsgHapticLfoTone`. A firmware-synthesized tone (the
/// promising Triton *audio* path): `frequency` Hz, `duration_ms`, `lfo_freq`/`lfo_depth` modulation.
/// Sent as [`TritonOutReport::LfoTone`]. **Unused / HW-UNTESTED.**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticLfoTone {
    pub side: u8,
    pub gain_db: i8,
    pub frequency: u16,
    pub duration_ms: u16,
    pub lfo_freq: u16,
    pub lfo_depth: u8,
}
impl Wire for MsgHapticLfoTone {}
const _: () = assert!(core::mem::size_of::<MsgHapticLfoTone>() == 9);

/// Triton `0x84` log-frequency **sweep** (chirp) body - SDL `MsgHapticLogSweep`. Sent as
/// [`TritonOutReport::LogSweep`]. **Unused / HW-UNTESTED.**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticLogSweep {
    pub side: u8,
    pub gain_db: i8,
    pub duration_ms: u16,
    pub start_freq: u16,
    pub end_freq: u16,
}
impl Wire for MsgHapticLogSweep {}
const _: () = assert!(core::mem::size_of::<MsgHapticLogSweep>() == 8);

/// Triton `0x85` named haptic **script** body - SDL `MsgHapticScript`. `script_id` selects a firmware
/// effect; `gain_db` scales. Sent as [`TritonOutReport::Script`]. **Unused / HW-UNTESTED.**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticScript {
    pub side: u8,
    pub script_id: u8,
    pub gain_db: i8,
}
impl Wire for MsgHapticScript {}
const _: () = assert!(core::mem::size_of::<MsgHapticScript>() == 3);

// =====================================================================================
// 6. Report-related inbound - input-report parsing. The device SENDS these; report **bodies** are
//    decoded in `report.rs` (-> `GordonReport`/`NeptuneReport`/`TritonReport`) and the button bits
//    live in `buttons.rs`. Here are the report **identifiers** (kept as matchable consts - they're
//    dispatched against a received byte, so an enum would force `TryFrom`/guards), the frame header,
//    the small value-enums, and the Gordon BLE framing. Sorted Generic -> Gordon (incl. BLE) ->
//    Neptune -> Triton. (GET-response structs are not here - they're command replies, see 4.)
// =====================================================================================

// --- Generic (Gordon & Neptune share the `0x01`-framed report) -------------------------

/// All 64-byte HID reports; feature reports are framed with a report-ID-0 byte.
pub(crate) const REPORT_LEN: usize = 64;
/// Report id prepended to feature-report buffers on Gordon/Neptune (outbound command framing).
pub(crate) const REPORT_ID: u8 = 0x00;
/// Report id prepended to Triton feature-report buffers. Triton's command channel rides
/// **feature report `0x01`**, not `0x00` - confirmed in SDL (`DisableSteamTritonLizardMode`
/// sets `buffer[0]=1`) and sc-controller (`wValue 0x0301`, `0x01`-prefixed payload). The
/// command *body* (`[cmd_id, len, payload...]`) is otherwise identical to Gordon/Neptune.
pub(crate) const REPORT_ID_TRITON: u8 = 0x01;

/// A raw wire 2D vector (two little-endian `i16`) - sticks/pads, shared by the fixed packets below
/// and the Gordon BLE chunk stream. Decodes into a [`crate::value::Vec2i`] in `report.rs`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct WireVec2 {
    pub x: i16,
    pub y: i16,
}
impl Wire for WireVec2 {}
const _: () = assert!(core::mem::size_of::<WireVec2>() == 4);

/// A raw wire 3D vector (three little-endian `i16`) - accel / gyro. -> [`crate::value::Vec3i`].
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct WireVec3 {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}
impl Wire for WireVec3 {}
const _: () = assert!(core::mem::size_of::<WireVec3>() == 6);

/// A raw wire quaternion (four little-endian `i16`), wire order **`w,x,y,z`** - the same across
/// Gordon USB, Neptune, and the Gordon BLE `QUAT` chunk (SDL `sGyroQuat{W,X,Y,Z}`; kernel `hid-steam`
/// table agrees: W first, at quat offset `0x28` Gordon / `0x24` Neptune). -> [`crate::value::Quati`]
/// (orientation is carried but unused downstream).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct WireQuat {
    pub w: i16,
    pub x: i16,
    pub y: i16,
    pub z: i16,
}
impl Wire for WireQuat {}
const _: () = assert!(core::mem::size_of::<WireQuat>() == 8);

/// The 4-byte input-report header - SDL `ValveInReportHeader_t`. `report_version` is `0x0001`;
/// `msg_type` (offset 2) is one of [`event_type`]; `length` (offset 3) is the body length. First
/// field of every `0x01`-framed packet ([`GordonState`]/[`NeptuneState`]/[`ControllerStatus`]).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct InReportHeader {
    pub report_version: u16,
    pub msg_type: u8,
    pub length: u8,
}
impl Wire for InReportHeader {}
const _: () = assert!(core::mem::size_of::<InReportHeader>() == 4);

/// Input-report message type at **offset 2** - SDL `ValveInReportMessageIDs` (`ID_CONTROLLER_*`,
/// prefix dropped). Kept as matchable consts (dispatched against `buf[2]` in `report.rs`). Full SDL
/// set; `(used)` = we dispatch it.
pub(crate) mod event_type {
    pub(crate) const STATE: u8 = 0x01; // (used)
    pub(crate) const DEBUG: u8 = 0x02;
    pub(crate) const WIRELESS: u8 = 0x03; // (used)
    pub(crate) const STATUS: u8 = 0x04; // (used)
    pub(crate) const DEBUG2: u8 = 0x05;
    pub(crate) const SECONDARY_STATE: u8 = 0x06;
    pub(crate) const BLE_STATE: u8 = 0x07;
    pub(crate) const DECK_STATE: u8 = 0x09; // (used)
}

/// Event byte of a `WIRELESS` (0x03) frame (offset 4) - SDL `EWirelessEventType`.
pub(crate) mod wireless {
    pub(crate) const DISCONNECTED: u8 = 0x01; // (used)
    pub(crate) const CONNECTED: u8 = 0x02; // (used)
    pub(crate) const PAIR: u8 = 0x03;
}

/// Values of [`ControllerStatus::event_code`] - the `sEventCode` of the Gordon/Neptune `0x04`
/// status frame (SDL `ControllerStatusEventCodes`). Inbound (received), so `const`s, not an enum. We
/// parse only the battery voltage/charge of that report, not these codes - trace.
pub(crate) mod status_event {
    pub(crate) const NORMAL: u8 = 0;
    pub(crate) const CRITICAL_BATTERY: u8 = 1;
    pub(crate) const GYRO_INIT_ERROR: u8 = 2;
}

/// Bits of [`ControllerStatus::state_flags`] - the `unStateFlags` of the `0x04` status frame (SDL
/// `ControllerStatusStateFlags`). Inbound -> `const`s - trace.
pub(crate) mod status_flag {
    pub(crate) const LOW_BATTERY: u8 = 0;
}

/// IMU scale constants - HW-verified on Gordon. `ControllerState` carries the raw i16 IMU readings,
/// so consumers (the engine's gyro-to-mouse) convert: `raw / GYRO_RES_PER_DPS` = deg/s,
/// `raw / ACCEL_RES_PER_G` = g. Re-exported at the crate root as the single source of truth.
pub const ACCEL_RES_PER_G: f32 = 16384.0;
pub const GYRO_RES_PER_DPS: f32 = 16.0;

/// The `0x04` battery/status frame (Gordon & Neptune) - SDL `SteamControllerStatusEvent_t`. Cast from
/// the start of the 64-byte report. Only voltage + charge are used; `event_code`/`state_flags` are
/// trace. Decodes into `report::BatteryRaw`. Wired Gordon still emits this (charge pinned 100%); the
/// dongle reports real charge (battery is by transport, not wireless-only).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct ControllerStatus {
    pub header: InReportHeader, // 0x00
    pub packet_num: u32,        // 0x04
    pub event_code: u16,        // 0x08 - a [`status_event`] code
    pub state_flags: u16,       // 0x0A - [`status_flag`] bits
    pub voltage_mv: u16,        // 0x0C (mV)
    pub charge_percent: u8,     // 0x0E (0..=100)
}
impl Wire for ControllerStatus {}
const _: () = assert!(core::mem::size_of::<ControllerStatus>() == 15);

// --- Gordon (USB `0x01` frame + BLE delta stream) --------------------------------------
//
// USB: `event_type::STATE` (state). Body offsets (decoded in `parse_gordon` -> `GordonReport`):
// buttons @0x08 (`GordonButtons`, buttons.rs), triggers @0x0B/0x0C, left pad/stick @0x10 (time-
// multiplexed - de-muxed on `LPAD_TOUCH`), right pad @0x14, accel @0x1C, gyro @0x22, quat @0x28.
// BLE: a segmented Report-ID-3 delta stream (transport framing = the `ble` module below); its input
// layout - report type + present-chunk mask - is `ble::report_type` / `ble::chunk`, accumulated into
// the same `GordonReport` by `apply_gordon_ble`.

/// Gordon **USB** input frame (`event_type::STATE`) - SDL `ValveControllerStatePacket_t` (kernel
/// `hid-steam` table agrees), the fixed 64-byte multiplexed layout (offsets in comments). The left pad
/// and analog stick share `left` (`0x10`), de-muxed on `LPAD_TOUCH` in `report.rs`. `buttons` is the
/// low 3 bytes of SDL's 8-byte button/trigger union (`_pad0` = the 24 button bits); the `u8`
/// `left/right_trigger` (`nLeft`/`nRight`) live inside that union, then `_pad1`. `trigger_l/r_16`
/// (`0x18`) are SDL's redundant *wired* 16-bit triggers - unused (we use the `u8` forms). We map
/// through the last used field (quat @ `0x30`); the frame continues with uncalibrated joystick /
/// left-pad (kernel offsets `50..61`) that are unreliable on Gordon (HW: 0 on wireless), so skipped.
/// Decodes into `report::GordonReport`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct GordonState {
    pub header: InReportHeader, // 0x00
    pub seq: u32,               // 0x04
    pub buttons: [u8; 3],       // 0x08 (SDL button-union _pad0 = low 24 button bits)
    pub left_trigger: u8,       // 0x0B (SDL nLeft)
    pub right_trigger: u8,      // 0x0C (SDL nRight)
    _pad1: [u8; 3],             // 0x0D (SDL button-union _pad1)
    pub left: WireVec2,         // 0x10 (pad/stick multiplexed)
    pub right_pad: WireVec2,    // 0x14
    pub trigger_l_16: u16,      // 0x18 (SDL sTriggerL - redundant, unused)
    pub trigger_r_16: u16,      // 0x1A (SDL sTriggerR - redundant, unused)
    pub accel: WireVec3,        // 0x1C
    pub gyro: WireVec3,         // 0x22
    pub orientation: WireQuat,  // 0x28 (w,x,y,z)
}
impl Wire for GordonState {}
const _: () = assert!(core::mem::size_of::<GordonState>() == 0x30);

/// Gordon **Bluetooth (BLE)** transport framing + compact input layout. The kernel `hid-steam`
/// driver is USB-only, so BLE is reverse-engineered from SDL (`SDL_hidapi_steam.c`) and sc-controller
/// (`sc_by_bt`). Everything rides **Report ID 3** on a 20-byte HID report (report id + 1 header byte +
/// 18 payload). Feature *and* input reports longer than 18 bytes are split into segments; the command
/// bytes themselves are identical to USB (3), so only this framing + the input layout are BLE-only.
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
        /// A status report (battery/idle) - not decoded as input yet.
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

// --- Neptune / Steam Deck --------------------------------------------------------------
//
// `event_type::DECK_STATE` (0x09) state. Decoded in `parse_neptune` -> `NeptuneReport`:
// `NeptuneButtons` (buttons.rs), 16-bit triggers, dual sticks + pads with pressure, IMU, and the raw
// stick capacitive-force bytes @0x3C/0x3E (kept raw, not exposed - `NeptuneReport::left_stick_force`).

/// Neptune (Steam Deck) input frame (`event_type::DECK_STATE`, `0x09`) - SDL
/// `SteamDeckStatePacket_t` (kernel `hid-steam` table agrees), the fixed 64-byte layout (offsets in
/// comments; no multiplex, unlike Gordon). `buttons` = the 8-byte button union ->
/// [`crate::NeptuneButtons`] (u64); triggers are 16-bit (SDL `sTriggerRawL/R`, uncalibrated);
/// `orientation` wire order `w,x,y,z`. SDL's struct **ends at the pad pressures** (`0x3C`); the
/// `*_stick_force` capacitive bytes at `0x3C`/`0x3E` are beyond SDL (InputPlumber-only), kept raw and
/// not exposed. Decodes into `report::NeptuneReport`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct NeptuneState {
    pub header: InReportHeader,  // 0x00
    pub seq: u32,                // 0x04
    pub buttons: [u8; 8],        // 0x08 (SDL button union -> NeptuneButtons u64)
    pub left_pad: WireVec2,      // 0x10
    pub right_pad: WireVec2,     // 0x14
    pub accel: WireVec3,         // 0x18
    pub gyro: WireVec3,          // 0x1E
    pub orientation: WireQuat,   // 0x24 (w,x,y,z)
    pub left_trigger: i16,       // 0x2C (SDL sTriggerRawL)
    pub right_trigger: i16,      // 0x2E (SDL sTriggerRawR)
    pub left_stick: WireVec2,    // 0x30
    pub right_stick: WireVec2,   // 0x34
    pub left_pad_pressure: i16,  // 0x38
    pub right_pad_pressure: i16, // 0x3A
    pub left_stick_force: i16,   // 0x3C (InputPlumber-only, beyond SDL)
    pub right_stick_force: i16,  // 0x3E (InputPlumber-only, beyond SDL)
}
impl Wire for NeptuneState {}
const _: () = assert!(core::mem::size_of::<NeptuneState>() == 0x40);

// --- Triton (new Steam Controller) -----------------------------------------------------
//
// Triton does NOT use the `0x01` frame: its input reports carry the **report id in byte 0**
// (dispatched in `parse_triton`). The State/NoQuat bodies decode -> `TritonReport`.

/// Triton input/status report ids (byte 0 of each read). SDL `SDL_hidapi_steam_triton.c` +
/// sc-controller `sc2.py`. `(used)` = we handle it.
pub(crate) mod triton {
    /// Input/status report ids.
    pub(crate) mod report {
        /// Main gamepad state (older firmware appends an on-controller quaternion). **HW: the puck/
        /// dongle (0x1304) + wired (0x1302) stream `0x42` by default**; parsed as NoQuat regardless.
        pub(crate) const CONTROLLER_STATE: u8 = 0x42; // (used)
        /// Battery status.
        pub(crate) const BATTERY_STATUS: u8 = 0x43; // (used)
        /// Gamepad state, "NoQuat" body - same leading fields as `CONTROLLER_STATE`. **HW: over
        /// Bluetooth (0x1303) the controller streams `0x45`.**
        pub(crate) const CONTROLLER_STATE_BLE: u8 = 0x45; // (used)
        /// Wireless connect/disconnect status (dongle), alternate id.
        pub(crate) const WIRELESS_STATUS_X: u8 = 0x46; // (used)
        /// Gamepad state with a trackpad + 16-bit IMU timestamp ("Ibex" packet). **NOT parsed** -
        /// added only if a unit is seen streaming it.
        pub(crate) const CONTROLLER_STATE_TIMESTAMP: u8 = 0x47;
        /// Wireless connect/disconnect status (dongle).
        pub(crate) const WIRELESS_STATUS: u8 = 0x79; // (used)
    }

    /// Values of [`super::TritonBatteryStatus::charge_state`] - SDL `EChargeState`. Trace.
    pub(crate) mod charge_state {
        pub(crate) const RESET: u8 = 0;
        pub(crate) const DISCHARGING: u8 = 1;
        pub(crate) const CHARGING: u8 = 2;
        pub(crate) const SRC_VALIDATE: u8 = 3;
        pub(crate) const CHARGING_DONE: u8 = 4;
    }

    /// Event byte of a wireless-status report (`WIRELESS_STATUS`/`WIRELESS_STATUS_X`) - SDL
    /// `ETritonWirelessState`.
    pub(crate) mod wireless {
        pub(crate) const DISCONNECT: u8 = 1; // (used)
        pub(crate) const CONNECT: u8 = 2; // (used)
    }
}

// Triton state has three body variants (SDL `TritonMTU*_t`) that share the same leading fields;
// **only `TritonStateNoQuat` is used for `from_bytes` decoding** (`report.rs`). `buttons` = raw
// [`crate::TritonButtons`] (u32); `imu_timestamp` unused (we synth `seq` from `seq_num`). No
// left-multiplex - separate stick/pad, both live. All decode into `report::TritonReport`.

/// `0x42` `ID_TRITON_CONTROLLER_STATE` - SDL `TritonMTUFull_t`: the NoQuat body plus a trailing
/// orientation quaternion (`w,x,y,z`). HW streams `0x42` on the puck/dongle + wired; we parse it as
/// [`TritonStateNoQuat`] (the quat is ignored as trailing bytes). Reference layout.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct TritonStateFull {
    pub report_id: u8,           // 0
    pub seq_num: u8,             // 1
    pub buttons: u32,            // 2
    pub left_trigger: i16,       // 6
    pub right_trigger: i16,      // 8
    pub left_stick: WireVec2,    // 10
    pub right_stick: WireVec2,   // 14
    pub left_pad: WireVec2,      // 18
    pub left_pad_pressure: i16,  // 22
    pub right_pad: WireVec2,     // 24
    pub right_pad_pressure: i16, // 28
    pub imu_timestamp: u32,      // 30
    pub accel: WireVec3,         // 34
    pub gyro: WireVec3,          // 40
    pub orientation: WireQuat,   // 46 (w,x,y,z)
}
impl Wire for TritonStateFull {}
const _: () = assert!(core::mem::size_of::<TritonStateFull>() == 54);

/// `0x45` `ID_TRITON_CONTROLLER_STATE_BLE` - SDL `TritonMTUNoQuat_t`. **The decoded form**, used for
/// both `0x42` and `0x45` (it is `0x42`'s common 46-byte prefix). HW streams `0x45` over Bluetooth.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct TritonStateNoQuat {
    pub report_id: u8,           // 0
    pub seq_num: u8,             // 1
    pub buttons: u32,            // 2
    pub left_trigger: i16,       // 6
    pub right_trigger: i16,      // 8
    pub left_stick: WireVec2,    // 10
    pub right_stick: WireVec2,   // 14
    pub left_pad: WireVec2,      // 18
    pub left_pad_pressure: i16,  // 22
    pub right_pad: WireVec2,     // 24
    pub right_pad_pressure: i16, // 28
    pub imu_timestamp: u32,      // 30
    pub accel: WireVec3,         // 34
    pub gyro: WireVec3,          // 40
}
impl Wire for TritonStateNoQuat {}
const _: () = assert!(core::mem::size_of::<TritonStateNoQuat>() == 46);

/// `0x47` `ID_TRITON_CONTROLLER_STATE_TIMESTAMP` ("Ibex") - SDL `TritonMTUNoQuat32TS_t`. Adds a
/// `trackpad_timestamp` before the pads (shifting them) and shrinks the IMU timestamp to `u16`, so the
/// pad/IMU offsets differ from [`TritonStateNoQuat`]. **Not parsed yet** (reference layout only) - a
/// unit seen streaming `0x47` would wire this into `report.rs`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct TritonStateTimestamp {
    pub report_id: u8,           // 0
    pub seq_num: u8,             // 1
    pub buttons: u32,            // 2
    pub left_trigger: i16,       // 6
    pub right_trigger: i16,      // 8
    pub left_stick: WireVec2,    // 10
    pub right_stick: WireVec2,   // 14
    pub trackpad_timestamp: u16, // 18
    pub left_pad: WireVec2,      // 20
    pub left_pad_pressure: i16,  // 24
    pub right_pad: WireVec2,     // 26
    pub right_pad_pressure: i16, // 30
    pub imu_timestamp: u16,      // 32
    pub accel: WireVec3,         // 34
    pub gyro: WireVec3,          // 40
}
impl Wire for TritonStateTimestamp {}
const _: () = assert!(core::mem::size_of::<TritonStateTimestamp>() == 46);

/// Triton battery status report (`0x43`) - SDL `TritonBatteryStatus_t`, full layout. `battery_level`
/// is the charge percent (used); everything else is trace. Decodes into `report::BatteryRaw`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct TritonBatteryStatus {
    pub report_id: u8,        // 0
    pub charge_state: u8,     // 1 - a [`triton::charge_state`] value
    pub battery_level: u8,    // 2 (0..=100)
    pub voltage_mv: u16,      // 3 (battery, mV)
    pub system_voltage: u16,  // 5 (mV)
    pub input_voltage: u16,   // 7 (mV)
    pub current: u16,         // 9
    pub input_current: u16,   // 11
    pub temperature: u16,     // 13
}
impl Wire for TritonBatteryStatus {}
const _: () = assert!(core::mem::size_of::<TritonBatteryStatus>() == 15);

/// Triton wireless connect/disconnect status (`0x46`/`0x79`) - SDL `TritonWirelessStatus_t` (whose
/// body is just `state`; we prepend `report_id` to match the raw read, like the other Triton packets).
/// `state` is a [`triton::wireless`] value.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct TritonWirelessStatus {
    pub report_id: u8, // 0
    pub state: u8,     // 1 - a [`triton::wireless`] value
}
impl Wire for TritonWirelessStatus {}
const _: () = assert!(core::mem::size_of::<TritonWirelessStatus>() == 2);

#[cfg(test)]
mod tests {
    use super::*;

    // --- 3 outbound: `as_bytes()` is byte-exact vs the hand-rolled LE layout it replaced ---

    #[test]
    fn haptic_pulse_bytes() {
        let m = MsgFireHapticPulse {
            which_pad: 1,
            duration: 0x0102,
            interval: 0x0304,
            count: 0x0506,
            gain: -1,
        };
        // which_pad, duration(le), interval(le), count(le), gain.
        assert_eq!(m.as_bytes(), &[1, 0x02, 0x01, 0x04, 0x03, 0x06, 0x05, 0xFF][..]);
    }

    #[test]
    fn trigger_haptic_bytes() {
        let m = MsgTriggerHaptic {
            side: 2,
            cmd: HapticType::Click as u8,     // 2
            ui_intensity: HapticIntensity::Long as u8, // 3
            dbgain: -2,
            freq: 0x1122,
            dur_ms: 0x3344,
            ..Default::default()
        };
        let b = m.as_bytes();
        assert_eq!(b.len(), 19);
        assert_eq!(&b[..4], &[2, 2, 3, 0xFE]); // side, cmd, ui_intensity, dbgain(-2)
        assert_eq!(&b[4..8], &[0x22, 0x11, 0x44, 0x33]); // freq, dur_ms (LE)
        assert_eq!(&b[8..], &[0u8; 11]); // noise/lfo/script/sweep all zero
    }

    #[test]
    fn simple_rumble_bytes() {
        let m = MsgSimpleRumbleCmd {
            rumble_type: 0,
            intensity: 0x1234,
            left_speed: 0x5678,
            right_speed: 0x9ABC,
            left_gain: -1,
            right_gain: 2,
        };
        assert_eq!(m.as_bytes(), &[0, 0x34, 0x12, 0x78, 0x56, 0xBC, 0x9A, 0xFF, 2][..]);
    }

    // --- 5 Triton output bodies: `as_bytes()` = the wire body (report id prepended by output) ---

    #[test]
    fn triton_rumble_body_bytes() {
        let m = MsgHapticRumble {
            rumble_type: 0,
            intensity: 0x1234,
            left_speed: 0x5678,
            left_gain: -1,
            right_speed: 0x9ABC,
            right_gain: 2,
        };
        // rumble_type, intensity(le), left_speed(le), left_gain, right_speed(le), right_gain.
        assert_eq!(m.as_bytes(), &[0, 0x34, 0x12, 0x78, 0x56, 0xFF, 0xBC, 0x9A, 2][..]);
    }

    #[test]
    fn triton_command_body_bytes() {
        let m = MsgHapticCommand { side: 2, command: 1, gain_db: -1 };
        assert_eq!(m.as_bytes(), &[2, 1, 0xFF][..]);
    }

    // --- 3/4: round-trips + short-slice guard ---

    #[test]
    fn controller_setting_roundtrip() {
        let s = ControllerSetting { setting_num: 0x2A, value: 0xBEEF };
        assert_eq!(s.as_bytes(), &[0x2A, 0xEF, 0xBE][..]);
        let back = ControllerSetting::from_bytes(&[0x2A, 0xEF, 0xBE]).unwrap();
        assert_eq!((back.setting_num, back.value), (0x2A, 0xBEEF));
        assert!(ControllerSetting::from_bytes(&[0, 0]).is_none());
    }

    #[test]
    fn controller_attribute_from_bytes() {
        let a = ControllerAttribute::from_bytes(&[0x05, 0xEF, 0xBE, 0xAD, 0xDE]).unwrap();
        assert_eq!((a.tag, a.value), (0x05, 0xDEAD_BEEF));
        assert!(ControllerAttribute::from_bytes(&[0; 4]).is_none());
    }

    #[test]
    fn string_attribute_from_bytes() {
        let mut b = [0u8; 21];
        b[0] = 0x01; // tag
        b[1..6].copy_from_slice(b"Hello");
        let m = MsgGetStringAttribute::from_bytes(&b).unwrap();
        assert_eq!(m.tag, 0x01);
        let value = m.value; // copy the packed array out before borrowing
        assert_eq!(&value[..5], b"Hello");
        assert!(MsgGetStringAttribute::from_bytes(&[0; 20]).is_none());
    }

    // --- 6 inbound packets: `from_bytes` lands fields at the right absolute offsets ---

    #[test]
    fn gordon_packet_offsets() {
        let mut b = [0u8; REPORT_LEN];
        b[0x04..0x08].copy_from_slice(&0x1234_5678u32.to_le_bytes()); // seq
        b[0x08..0x0B].copy_from_slice(&[0xAA, 0xBB, 0xCC]); // buttons (3 wire bytes)
        b[0x0B] = 0x11; // left_trigger
        b[0x0C] = 0x22; // right_trigger
        b[0x10..0x14].copy_from_slice(&[0x02, 0x01, 0x04, 0x03]); // left = {x:0x0102, y:0x0304}
        let p = GordonState::from_bytes(&b).unwrap();
        let (seq, btn, lt, rt, left) = (p.seq, p.buttons, p.left_trigger, p.right_trigger, p.left);
        assert_eq!(seq, 0x1234_5678);
        assert_eq!(btn, [0xAA, 0xBB, 0xCC]);
        assert_eq!((lt, rt), (0x11, 0x22));
        assert_eq!((left.x, left.y), (0x0102, 0x0304));
    }

    #[test]
    fn neptune_packet_offsets() {
        let mut b = [0u8; REPORT_LEN];
        b[0x08] = 0x01; // buttons byte 0
        b[0x2C..0x2E].copy_from_slice(&1234i16.to_le_bytes()); // left_trigger (after the 0x0F gap)
        b[0x30..0x34].copy_from_slice(&[0x02, 0x01, 0x04, 0x03]); // left_stick
        b[0x3C..0x3E].copy_from_slice(&(-99i16).to_le_bytes()); // left_stick_force
        let p = NeptuneState::from_bytes(&b).unwrap();
        let (btn, lt, ls, lsf) = (p.buttons, p.left_trigger, p.left_stick, p.left_stick_force);
        assert_eq!(btn[0], 0x01);
        assert_eq!(lt, 1234);
        assert_eq!((ls.x, ls.y), (0x0102, 0x0304));
        assert_eq!(lsf, -99);
    }

    #[test]
    fn triton_packet_offsets() {
        let mut b = [0u8; 46];
        b[0] = 0x42; // report_id
        b[1] = 7; // seq_num
        b[2..6].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // buttons
        b[40..42].copy_from_slice(&(-5000i16).to_le_bytes()); // gyro.x
        let p = TritonStateNoQuat::from_bytes(&b).unwrap();
        let (id, seq, btn, gyro) = (p.report_id, p.seq_num, p.buttons, p.gyro);
        assert_eq!((id, seq), (0x42, 7));
        assert_eq!(btn, 0xDEAD_BEEF);
        assert_eq!((gyro.x, gyro.y, gyro.z), (-5000, 0, 0));
        assert!(TritonStateNoQuat::from_bytes(&b[..45]).is_none());
    }

    #[test]
    fn battery_packets_offsets() {
        let mut b = [0u8; REPORT_LEN];
        b[0x0C..0x0E].copy_from_slice(&3700u16.to_le_bytes()); // voltage
        b[0x0E] = 88; // charge
        let g = ControllerStatus::from_bytes(&b).unwrap();
        assert_eq!((g.voltage_mv, g.charge_percent), (3700, 88));

        let mut tb = [0u8; 15];
        tb[0] = 0x43; // report_id
        tb[2] = 77; // battery_level
        tb[3..5].copy_from_slice(&0x0E8Au16.to_le_bytes()); // voltage
        let t = TritonBatteryStatus::from_bytes(&tb).unwrap();
        assert_eq!((t.battery_level, t.voltage_mv), (77, 0x0E8A));
    }
}
