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
//    (3-6), plus the small shared wire chunk types (WireVec2/WireVec3/WireQuat) those packets embed -
//    both used everywhere, so kept here rather than in a device-specific group. The `unsafe` lives
//    here, once, guarded by a `size_of` check.
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
            core::slice::from_raw_parts(
                (self as *const Self).cast::<u8>(),
                core::mem::size_of::<Self>(),
            )
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

/// A raw wire 2D vector (two little-endian `i16`) - sticks/pads, shared by the fixed packets below
/// and the Gordon BLE chunk stream. Decodes into a [`crate::value::Vec2i`] in `state.rs`.
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
/// NOTE: the `SetDigitalMappings` (`0x80`) mapping vocabulary (SDL's `DigitalIO`/`DeviceTypes`/
/// `HIDKeyboardKeys`/`MouseButtons`/`GamepadButtons`/`ModeAdjustModes`, + `AnalogIO`) is mirrored at
/// the **end of section 3** as a reference - we bypass on-controller mapping (`ClearDigitalMappings`
/// + map in our own engine), so it's unused.
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
    SetControllerMode = 0x8D,   // SDL/IP only (not in kernel)
    LoadDefaultSettings = 0x8E, // (used) lizard-on (no-payload)
    // --- haptics (pulse; the `0xEA`/`0xEB` command haptics live in section 3 structs) ---
    TriggerHapticPulse = 0x8F, // (used) -> MsgFireHapticPulse
    TurnOffController = 0x9F,  // (used) power off - takes the "off!" magic (NOT no-payload)
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
    DongleCommitDevice = 0xB3, // (no-payload) - the empty struct we dropped
    DongleGetWirelessState = 0xB4, // (used) dongle prompt (no-payload)
    CalibrateGyro = 0xB5,      // (no-payload?)
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
/// (SDL's trailing `SETTING_DEFAULTMINMAXCOUNT` is only the C array-length sentinel - the length lives
/// in [`SettingValueRange`]'s `[i16; 3]`, so it's dropped here.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingDefaultMinMax {
    Default = 0,
    Min = 1,
    Max = 2,
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
//    The tail of the section (after the divider) is the `SET_DIGITAL_MAPPINGS` vocabulary - grouped
//    there rather than in command-id order, and unused (we map in our own engine).
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

/// Which trackpad actuator a Gordon `0x8f` pulse drives - the `which_pad` byte of
/// [`MsgFireHapticPulse`]. **Gordon's wire values are swapped** (the kernel's legacy convention):
/// `Right = 0`, `Left = 1`, and there is no "both" (pad 2 no-ops on Gordon HW - the caller fires the
/// two pads separately). Distinct from [`HapticSide`], the `0/1/2` Left/Right/Both convention the
/// Deck's `0xEA` and the Triton output reports use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum HapticPosition {
    Right = 0,
    Left = 1,
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
    pub which_pad: u8, // a [`HapticPosition`] value (Gordon's swapped Right=0/Left=1)
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

/// Which actuator(s) a haptic drives on the paths that carry a native side byte: the Deck's `0xEA`
/// [`MsgTriggerHaptic`] and every Triton output report (`0x81`-`0x85`). `Left = 0`, `Right = 1`,
/// `Both = 2` (all HW-verified). Distinct from Gordon's [`HapticPosition`], whose `0x8f` wire values
/// are swapped and carry no "both".
///
/// **Divergence:** SDL's `controller_structs.h` comments the `0xEA` side as `1=L/2=R/3=Both`, but our
/// HW verification found `0/1/2` (what we send). The Triton side structs carry no value comment in
/// SDL, so only the `0xEA` value is contested; kept at `0/1/2` to match the shipped, verified path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum HapticSide {
    Left = 0,
    Right = 1,
    Both = 2,
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
    pub side: u8,         // a [`HapticSide`] value
    pub cmd: u8,          // a [`HapticType`] value
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
/// **Deck-only** (Gordon has no motors). The caller drives `left`/`right` directly (left =
/// strong/large motor, right = weak/small).
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

// --- SET_DIGITAL_MAPPINGS (0x80): the on-controller input->output mapping vocabulary. ----------
//
// From here to the end of section 3 is the mapping command. **We bypass on-controller mapping
// entirely** (CLEAR_DIGITAL_MAPPINGS + map in our own engine), so all of this is a REFERENCE and
// is UNUSED. A mapping is an array of `DigitalMapping`: each entry binds a `DigitalIo` source to a
// (`DeviceType`, target) output, where the target is a `HidKey` (+ optional `Modifier` mask),
// `MouseButton`, `GamepadButton`, or `ModeAdjust`. SDL builds it inline for its mouse-mode fallback
// (`SDL_hidapi_steam.c`); Valve's enums say "only add, never reorder". These id spaces are
// outbound-only (we send a mapping, never dispatch on one) -> enums (`as u8` at the wire), plus the
// combinable `Modifier` bitmask.

/// `DigitalIO` - generic digital inputs (mapping *source*); SDL `IO_DIGITAL_` prefix dropped. "Only
/// add, never reorder." SDL's `BUTTON_Y == BUTTON_1` etc. aliases are noted as comments (a Rust enum
/// can't repeat a discriminant); `None` is SDL's `-1` sentinel, which lands as the byte `0xFF`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum DigitalIo {
    ButtonRightTrigger = 0,
    ButtonLeftTrigger,
    Button1, // = Y
    Button2, // = B
    Button3, // = X
    Button4, // = A
    ButtonRightBumper,
    ButtonLeftBumper,
    ButtonLeftJoystickClick,
    ButtonEscape,
    ButtonSteam,
    ButtonMenu,
    StickUp,
    StickDown,
    StickLeft,
    StickRight,
    Touch1, // = dpad Up
    Touch2, // = dpad Right
    Touch3, // = dpad Left
    Touch4, // = dpad Down
    ButtonBackLeft,
    ButtonBackRight,
    LeftTrackpadN,
    LeftTrackpadNe,
    LeftTrackpadE,
    LeftTrackpadSe,
    LeftTrackpadS,
    LeftTrackpadSw,
    LeftTrackpadW,
    LeftTrackpadNw,
    RightTrackpadN,
    RightTrackpadNe,
    RightTrackpadE,
    RightTrackpadSe,
    RightTrackpadS,
    RightTrackpadSw,
    RightTrackpadW,
    RightTrackpadNw,
    LeftTrackpadDoubleTap,
    RightTrackpadDoubleTap,
    LeftTrackpadOuterRadius,
    RightTrackpadOuterRadius,
    LeftTrackpadClick,
    RightTrackpadClick,
    BatteryLow,
    LeftTriggerThreshold,
    RightTriggerThreshold,
    ButtonBackLeft2,
    ButtonBackRight2,
    ButtonAlwaysOn,
    ButtonAncillary1,
    ButtonMacro0,
    ButtonMacro1,
    ButtonMacro2,
    ButtonMacro3,
    ButtonMacro4,
    ButtonMacro5,
    ButtonMacro6,
    ButtonMacro7,
    ButtonMacro1Finger,
    ButtonMacro2Finger,
    None = 0xFF, // SDL IO_DIGITAL_BUTTON_NONE (-1): 'no input' sentinel
}

/// `AnalogIO` - generic analog inputs; SDL `IO_` prefix dropped. There is **no SET_ANALOG_MAPPINGS**
/// command (mapping is digital-only), so this is the analog-input vocabulary for reference only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum AnalogIo {
    LeftStickX = 0,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
    Mouse1X,
    Mouse1Y,
    Mouse1Z,
    AccelX,
    AccelY,
    AccelZ,
    GyroX,
    GyroY,
    GyroZ,
    GyroQuatW,
    GyroQuatX,
    GyroQuatY,
    GyroQuatZ,
    GyroSteeringVec,
    RawTriggerLeft,
    RawTriggerRight,
    RawJoystickX,
    RawJoystickY,
    GyroTiltVec,
    PressureLeftPad,
    PressureRightPad,
    PressureLeftBumper,
    PressureRightBumper,
    PressureLeftGrip,
    PressureRightGrip,
    LeftTriggerThreshold,
    RightTriggerThreshold,
    PressureRightPadThreshold,
    PressureLeftPadThreshold,
    PressureRightBumperThreshold,
    PressureLeftBumperThreshold,
    PressureRightGripThreshold,
    PressureLeftGripThreshold,
    PressureRightPadRaw,
    PressureLeftPadRaw,
    PressureRightBumperRaw,
    PressureLeftBumperRaw,
    PressureRightGripRaw,
    PressureLeftGripRaw,
    PressureRightGrip2Threshold,
    PressureLeftGrip2Threshold,
    PressureLeftGrip2,
    PressureRightGrip2,
    PressureRightGrip2Raw,
    PressureLeftGrip2Raw,
}

/// `DeviceTypes` - which emulated device a mapping targets (the `device` byte of a [`DigitalMapping`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum DeviceType {
    Keyboard = 0,
    Mouse,
    Gamepad,
    ModeAdjust, // virtual: sensitivity / pad secondary mode while held
}

/// `HIDKeyboardKeys` - HID keyboard scancodes (mapping *target* when `device == DeviceType::Keyboard`);
/// SDL `KEY_` prefix dropped. `A`..`KeypadPeriod` are standard HID usages `0x04..0x63`; Valve then
/// appends its own modifiers/media past `0x63`. Top-row digits are `Num1`..`Num0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum HidKey {
    Invalid = 0,
    A = 0x04, // HID usage 0x04; auto-increments to 0x63
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Num1, // top-row digit '1'
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    Num0, // top-row digit '0'
    Return,
    Escape,
    Backspace,
    Tab,
    Space,
    Dash,
    Equals,
    LeftBracket,
    RightBracket,
    Backslash,
    Unused1,
    Semicolon,
    SingleQuote,
    BackTick,
    Comma,
    Period,
    ForwardSlash,
    Capslock,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    PrintScreen,
    ScrollLock,
    Break,
    Insert,
    Home,
    PageUp,
    Delete,
    End,
    PageDown,
    RightArrow,
    LeftArrow,
    DownArrow,
    UpArrow,
    NumLock,
    KeypadForwardSlash,
    KeypadAsterisk,
    KeypadDash,
    KeypadPlus,
    KeypadEnter,
    Keypad1,
    Keypad2,
    Keypad3,
    Keypad4,
    Keypad5,
    Keypad6,
    Keypad7,
    Keypad8,
    Keypad9,
    Keypad0,
    KeypadPeriod,
    LAlt, // Valve appends modifiers/media past HID 0x63
    LShift,
    LWin,
    LControl,
    RAlt,
    RShift,
    RWin,
    RControl,
    VolUp,
    VolDown,
    Mute,
    Play,
    Stop,
    Next,
    Prev,
}

bitflags::bitflags! {
    /// `ModifierMasks` - keyboard modifier **bitmask** OR'd alongside a [`HidKey`] (`device ==
    /// DeviceType::Keyboard`); SDL `KEY_`/`_MASK` dropped. A combinable mask that's held as a value,
    /// so `bitflags!` (like the button sets) rather than a `const` bit list.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) struct Modifier: u8 {
        const LCONTROL = 1 << 0;
        const LSHIFT   = 1 << 1;
        const LALT     = 1 << 2;
        const LWIN     = 1 << 3;
        const RCONTROL = 1 << 4;
        const RSHIFT   = 1 << 5;
        const RALT     = 1 << 6;
        const RWIN     = 1 << 7;
    }
}

/// `MouseButtons` - mapping *target* when `device == DeviceType::Mouse`; SDL `MOUSE_` prefix dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum MouseButton {
    Left = 0,
    Right,
    Middle,
    Back,
    Forward,
    ScrollUp,
    ScrollDown,
}

/// `GamepadButtons` - mapping *target* when `device == DeviceType::Gamepad`; SDL `GAMEPAD_BTN_` prefix
/// dropped. Numbered from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum GamepadButton {
    TriggerLeft = 1,
    TriggerRight,
    A,
    B,
    Y,
    X,
    ShoulderLeft,
    ShoulderRight,
    LeftJoystick,
    RightJoystick,
    Start,
    Select,
    Steam,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    LStickUp,
    LStickDown,
    LStickLeft,
    LStickRight,
    RStickUp,
    RStickDown,
    RStickLeft,
    RStickRight,
}

/// `ModeAdjustModes` - mapping *target* when `device == DeviceType::ModeAdjust`; SDL `MODE_ADJUST_`
/// dropped. A held source adjusts sensitivity or switches a trackpad's secondary mode (an on-controller
/// momentary mode-shift). Numbered from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ModeAdjust {
    Sensitivity = 1,
    LeftPadSecondaryMode,
    RightPadSecondaryMode,
}

/// One `SET_DIGITAL_MAPPINGS` (`0x80`) entry: bind a [`DigitalIo`] source to an output. The command
/// payload is an **array** of these (SDL sets the `[cmd, len]` header's length to `count * 3`, which
/// [`FeatureReportHeader`] gets via `payload.len()` in `device::feature`); SDL builds it inline (no
/// named struct there). `target` is a [`HidKey`] / [`MouseButton`] / [`GamepadButton`] / [`ModeAdjust`]
/// discriminant per `device` (`as u8`; for a keyboard target a [`Modifier`] mask may be OR'd in).
/// **UNUSED** - we clear the on-controller map and do output ourselves.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct DigitalMapping {
    pub source: u8, // a [`DigitalIo`] value
    pub device: u8, // a [`DeviceType`] value
    pub target: u8, // key / mouse-button / gamepad-button / mode, per `device`
}
impl Wire for DigitalMapping {}
const _: () = assert!(core::mem::size_of::<DigitalMapping>() == 3);

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

/// `GET_TRACKPAD_CALIBRATION`/`_FACTORY` (`0xAA`/`0xAB`) reply body - SDL `ValveControllerTrackpadImage_t`.
/// A 20-cell capacitance image for pad `pad_num`, plus the `noise` floor. Unlike the input reports in
/// section 6, this has **no [`InReportHeader`]** - it's the payload of a GET reply. **Unused,
/// HW-UNTESTED** (we never request trackpad images; the exact framing needs a USB dump - see the
/// advanced-features note). Reference layout.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct TrackpadImage {
    pub pad_num: u8,     // 0
    _pad: [u8; 3],       // 1 (word-align `data`)
    pub data: [i16; 20], // 4
    pub noise: u16,      // 44
}
impl Wire for TrackpadImage {}
const _: () = assert!(core::mem::size_of::<TrackpadImage>() == 46);

/// `GET_TRACKPAD_RAW` (`0xAC`) reply body - SDL `ValveControllerRawTrackpadImage_t`. As
/// [`TrackpadImage`] but a larger raw 28-cell block delivered in `offset`-indexed pieces (no header).
/// **Unused, HW-UNTESTED.** Reference layout.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct RawTrackpadImage {
    pub pad_num: u8,     // 0
    pub offset: u8,      // 1
    _pad: [u8; 2],       // 2 (word-align `data`)
    pub data: [i16; 28], // 4
}
impl Wire for RawTrackpadImage {}
const _: () = assert!(core::mem::size_of::<RawTrackpadImage>() == 60);

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

/// Triton `0x81` trackpad **pulse** body - SDL `MsgHapticPulse` (structurally Gordon's `0x8f` analog).
/// `on_us`/`off_us` = pulse high/low us, `repeat_count` = pulses. Report id [`TritonOutReport::Pulse`].
/// Sent by [`Device::pulse_triton`]. **HW-usable both ways.** As a **train** (`repeat_count > 1`) it
/// plays a pulse/rumble across the swept frequency range - a narrow band ~600-700 Hz oscillates oddly,
/// the rest is clean; it is NOT a clean *tone* path (`0x83` LfoTone / `0x84` LogSweep are - see
/// `beep-triton`), but works well as a rumble. As a **single** pulse (`repeat_count = 1`) it is a
/// discrete **click** whose width (`on_us`) sets strength - finer than the two-step `0x82` click.
/// LEFT/RIGHT are physically SWAPPED (as Gordon's `0x8f`); BOTH works. Probed in `haptic-triton`
/// (`pulse` train / `clicks-pulse` single).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticPulse {
    pub side: u8, // a [`HapticSide`] value
    pub on_us: u16,
    pub off_us: u16,
    pub repeat_count: u16,
}
impl Wire for MsgHapticPulse {}
const _: () = assert!(core::mem::size_of::<MsgHapticPulse>() == 7);

/// Style for Triton's `0x82` [`MsgHapticCommand`] click - the `command` byte. **Triton-only**, and
/// **not** the Deck's [`HapticType`]: HW shows only `0` off / `1` weak / `2` strong do anything, and
/// `3`+ have no effect (so it is *not* the 8-value `haptic_type_t` we once guessed). `Weak` is a
/// light click, `Strong` a firm one - the main strength lever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(u8)]
pub enum HapticStyle {
    Off = 0,
    Weak = 1,
    Strong = 2,
}

/// Triton `0x82` haptic **command / click** - SDL `MsgHapticCommand`. `command` is a [`HapticStyle`]
/// (SDL types it a bare `u8`; only off/weak/strong are HW-verified). `gain_db` is `i8` in SDL, but HW
/// shows this byte as a subtle **unsigned** amplitude trim (`0`=medium..`255`=strong, sc-controller);
/// the reader sends it unsigned. Sent as [`TritonOutReport::Command`]. **HW-verified (puck).**
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct MsgHapticCommand {
    pub side: u8,    // a [`HapticSide`] value
    pub command: u8, // a [`HapticStyle`] value
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
    pub side: u8, // a [`HapticSide`] value
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
    pub side: u8, // a [`HapticSide`] value
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
    pub side: u8, // a [`HapticSide`] value
    pub script_id: u8,
    pub gain_db: i8,
}
impl Wire for MsgHapticScript {}
const _: () = assert!(core::mem::size_of::<MsgHapticScript>() == 3);

// =====================================================================================
// 6. Report-related inbound - input-report parsing. The device SENDS these; report **bodies** are
//    decoded in `state.rs` (straight into `ControllerState`/`Report`). Here are the report
//    **identifiers** (kept as matchable consts - they're dispatched against a received byte, so an
//    enum would force `TryFrom`/guards), the frame header, the raw per-device button bitfields
//    (`GordonButtons`/`NeptuneButtons`/`TritonButtons`, each before the report it decodes), the small
//    value-enums, and the Gordon BLE framing. Sorted Gordon (with the report bits Neptune shares:
//    wire types, header, event consts, IMU scales) -> Neptune -> Gordon BLE -> Triton.
//    (GET-response structs are not here - they're command replies, see 4.)
// =====================================================================================

// --- Gordon (USB `0x01` frame) + the report bits Neptune shares ------------------------

/// All 64-byte HID reports; feature reports are framed with a report-ID-0 byte.
pub(crate) const REPORT_LEN: usize = 64;
/// Report id prepended to feature-report buffers on Gordon/Neptune (outbound command framing).
pub(crate) const REPORT_ID: u8 = 0x00;

/// IMU scale constants - HW-verified on Gordon. `ControllerState` carries the raw i16 IMU readings,
/// so consumers (the engine's gyro-to-mouse) convert: `raw / GYRO_RES_PER_DPS` = deg/s,
/// `raw / ACCEL_RES_PER_G` = g. Re-exported at the crate root as the single source of truth.
pub const ACCEL_RES_PER_G: f32 = 16384.0;
pub const GYRO_RES_PER_DPS: f32 = 16.0;

/// Input-report message type at **offset 2** - SDL `ValveInReportMessageIDs` (`ID_CONTROLLER_*`,
/// prefix dropped). Kept as matchable consts (dispatched against `buf[2]` in `state.rs`). Full SDL
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

bitflags::bitflags! {
    /// Raw Gordon (original Steam Controller) button bits, packed as
    /// `buttons0 | buttons1 << 8 | buttons2 << 16` (the low 3 bytes of the `0x01` frame's button
    /// union). Folded into the unified [`crate::Buttons`] in `state.rs`.
    ///
    /// Shared by **USB and Bluetooth** Gordon - identical bit layout; over BLE the same bits arrive
    /// in the compact input's button chunk. `dpad` (bits 8..11) is firmware-synthesized from left-pad
    /// directional clicks on both transports. `LPAD_AND_JOY` and the shared left-click bit are USB-wire
    /// multiplex artifacts resolved in `state::from_gordon` (never set over BLE).
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct GordonButtons: u32 {
        // buttons0
        const RT           = 1 << 0; // right trigger full-pull
        const LT           = 1 << 1; // left trigger full-pull
        const RB           = 1 << 2;
        const LB           = 1 << 3;
        const Y            = 1 << 4;
        const B            = 1 << 5;
        const X            = 1 << 6;
        const A            = 1 << 7;
        // buttons1
        const DPAD_UP      = 1 << 8;
        const DPAD_RIGHT   = 1 << 9;
        const DPAD_LEFT    = 1 << 10;
        const DPAD_DOWN    = 1 << 11;
        const VIEW         = 1 << 12; // BTN_SELECT - Valve "View" (kernel "menu left")
        const STEAM        = 1 << 13;
        const MENU         = 1 << 14; // BTN_START - Valve "Menu" (kernel "menu right")
        const LGRIP        = 1 << 15;
        // buttons2
        const RGRIP        = 1 << 16;
        const LPAD_PRESS   = 1 << 17;
        const RPAD_PRESS   = 1 << 18;
        const LPAD_TOUCH   = 1 << 19;
        const RPAD_TOUCH   = 1 << 20;
        const LSTICK_PRESS = 1 << 22;
        const LPAD_AND_JOY = 1 << 23;
    }
}

// Gordon USB body offsets (decoded in `state::from_gordon`): buttons @0x08
// (`GordonButtons` above), triggers @0x0B/0x0C, left pad/stick @0x10 (time-multiplexed - de-muxed
// on `LPAD_TOUCH`), right pad @0x14, accel @0x1C, gyro @0x22, quat @0x28. The BLE delta stream is a
// separate transport (see the Gordon BLE group below), accumulating into the same snapshot.

/// Gordon **USB** input frame (`event_type::STATE`) - SDL `ValveControllerStatePacket_t` (kernel
/// `hid-steam` table agrees), the fixed 64-byte multiplexed layout (offsets in comments). The left pad
/// and analog stick share `left` (`0x10`), de-muxed on `LPAD_TOUCH` in `state::from_gordon`. `buttons` is the
/// low 3 bytes of SDL's 8-byte button/trigger union (`_pad0` = the 24 button bits); the `u8`
/// `left/right_trigger` (`nLeft`/`nRight`) live inside that union, then `_pad1`. `trigger_l/r_16`
/// (`0x18`) are SDL's redundant *wired* 16-bit triggers - unused (we use the `u8` forms). We map
/// through the last used field (quat @ `0x30`); the frame continues with uncalibrated joystick /
/// left-pad (kernel offsets `50..61`) that are unreliable on Gordon (HW: 0 on wireless), so skipped.
/// Decoded by `state::from_gordon`.
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

/// The `0x02` debug frame - SDL `ValveControllerDebugPacket_t`. Pad coordinates, raw + filtered mouse
/// deltas, per-pad Z/pressure, finger-present, timestamps, tap state, and the two digital-IO state
/// words. Only Steam's own tooling requests this; **we never enable debug reporting, so this is a
/// reference layout** (`from_bytes` ready if we ever do). Order-of-fields matches SDL exactly.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct DebugPacket {
    pub header: InReportHeader,             // 0x00
    pub left_pad: WireVec2,                 // 0x04
    pub right_pad: WireVec2,                // 0x08
    pub left_pad_mouse: WireVec2,           // 0x0C (raw mouse delta)
    pub right_pad_mouse: WireVec2,          // 0x10
    pub left_pad_mouse_filtered: WireVec2,  // 0x14
    pub right_pad_mouse_filtered: WireVec2, // 0x18
    pub left_z: u8,                         // 0x1C (pad pressure)
    pub right_z: u8,                        // 0x1D
    pub left_finger_present: u8,            // 0x1E
    pub right_finger_present: u8,           // 0x1F
    pub left_timestamp: u8,                 // 0x20
    pub right_timestamp: u8,                // 0x21
    pub left_tap_state: u8,                 // 0x22
    pub right_tap_state: u8,                // 0x23
    pub digital_io_states0: u32,            // 0x24
    pub digital_io_states1: u32,            // 0x28
}
impl Wire for DebugPacket {}
const _: () = assert!(core::mem::size_of::<DebugPacket>() == 0x2C);

/// The `0x03` wireless-metadata frame - SDL `SteamControllerWirelessEvent_t` (`{ ucEventType }`).
/// `event` is a [`wireless`] value (connect/disconnect/pair). `state.rs` dispatches this on the
/// dongle to surface connect/disconnect; it reads `event` **via this struct** rather than a bare
/// `buf[4]`, so the offset is named in one place.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct WirelessEvent {
    pub header: InReportHeader, // 0x00
    pub event: u8,              // 0x04 - a [`wireless`] value (SDL ucEventType)
}
impl Wire for WirelessEvent {}
const _: () = assert!(core::mem::size_of::<WirelessEvent>() == 5);

/// The `0x04` battery/status frame (Gordon & Neptune) - SDL `SteamControllerStatusEvent_t`. Cast from
/// the start of the 64-byte report. Only voltage + charge are used; `event_code`/`state_flags` are
/// trace. Decodes into `state::Battery`. Wired Gordon still emits this (charge pinned 100%); the
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

/// The `0x07` `BLE_STATE` frame - SDL `ValveControllerBLEStatePacket_t`. **A DIFFERENT BLE path from
/// the one we use** - and we do NOT handle it.
///
/// SDL has two BLE routes, chosen by the report *version* word (`buf[0..2]`):
/// - **Segmented delta stream** (what OUR Gordon BLE does): when the version word is *not* the
///   `ValveInReport_t` magic and `buf[0] & 0x0F == 4` (`ble::report_type::STATE`), SDL's
///   `UpdateBLESteamControllerState` walks the [`ble::chunk`] present-mask and copies whatever chunks
///   are present (buttons/triggers/sticks/pads/accel/gyro/quat - many per packet). This is our
///   `apply_gordon_ble`, and our unit (`0x1106`) streams exactly this.
/// - **This `0x07` report** (what we DON'T see): a full `ValveInReport_t` (real header, `ucType == 7`)
///   carrying buttons/pads/triggers **plus a single IMU group per packet** - `gyro_data_type` selects
///   what `gyro[4]` holds (1 = quat, 2 = accel, 3 = gyro). So its "accumulation" is just that each
///   packet refreshes buttons/pads/triggers but fills only one IMU group, the others persisting from
///   prior packets. It is a *distinct* representation, not our chunk stream.
///
/// **Verdict: not worth pursuing.** Our segmented path is HW-verified and richer (a packet can carry
/// accel+gyro+quat at once via the chunk mask, vs one IMU group here); this `0x07` form is a
/// lower-bandwidth alternative for a firmware/stack that emits `ValveInReport_t`-framed BLE state,
/// which ours does not. Kept as a reference layout only.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct BleStatePacket {
    pub header: InReportHeader, // 0x00
    pub packet_num: u32,        // 0x04
    pub buttons: [u8; 3],       // 0x08 (SDL button-union _pad0)
    pub left_trigger: u8,       // 0x0B (SDL nLeft)
    pub right_trigger: u8,      // 0x0C (SDL nRight)
    _pad1: [u8; 3],             // 0x0D (SDL button-union _pad1)
    pub left_pad: WireVec2,     // 0x10
    pub right_pad: WireVec2,    // 0x14
    pub gyro_data_type: u8,     // 0x18 (selects `gyro`: 1 = quat, 2 = accel, 3 = gyro)
    pub gyro: [i16; 4],         // 0x19 (one IMU group per packet, per `gyro_data_type`)
}
impl Wire for BleStatePacket {}
const _: () = assert!(core::mem::size_of::<BleStatePacket>() == 33);

// --- Neptune / Steam Deck (Deck-specific `0x09` frame; shares everything above) --------
//
// `event_type::DECK_STATE` (0x09) state. Decoded in `state::from_neptune`:
// `NeptuneButtons`, 16-bit triggers, dual sticks + pads with pressure, IMU, and the raw stick
// capacitive-force bytes @0x3C/0x3E (kept raw, not exposed - `NeptuneState::left_stick_force`).

bitflags::bitflags! {
    /// Raw Neptune (Steam Deck) button bits, packed from `buttons0..6` (bytes 0x08..0x0E). Byte N
    /// occupies bits `8*N..`. Folded into the unified [`crate::Buttons`] in `state.rs`.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct NeptuneButtons: u64 {
        // buttons0
        const RT           = 1 << 0; // right trigger full-pull
        const LT           = 1 << 1; // left trigger full-pull
        const RB           = 1 << 2;
        const LB           = 1 << 3;
        const Y            = 1 << 4;
        const B            = 1 << 5;
        const X            = 1 << 6;
        const A            = 1 << 7;
        // buttons1
        const DPAD_UP      = 1 << 8;
        const DPAD_RIGHT   = 1 << 9;
        const DPAD_LEFT    = 1 << 10;
        const DPAD_DOWN    = 1 << 11;
        const VIEW         = 1 << 12;
        const STEAM        = 1 << 13;
        const MENU         = 1 << 14;
        const LGRIP2       = 1 << 15;
        // buttons2
        const RGRIP2       = 1 << 16;
        const LPAD_PRESS   = 1 << 17;
        const RPAD_PRESS   = 1 << 18;
        const LPAD_TOUCH   = 1 << 19;
        const RPAD_TOUCH   = 1 << 20;
        const LSTICK_PRESS = 1 << 22;
        // buttons3
        const RSTICK_PRESS = 1 << 26; // bit 2 of byte 3
        // buttons5 (byte 0x0D -> bits 40..)
        const LGRIP        = 1 << 41;
        const RGRIP        = 1 << 42;
        const LSTICK_TOUCH = 1 << 46;
        const RSTICK_TOUCH = 1 << 47;
        // buttons6 (byte 0x0E -> bits 48..)
        const QUICK_ACCESS = 1 << 50;
    }
}

/// Neptune (Steam Deck) input frame (`event_type::DECK_STATE`, `0x09`) - SDL
/// `SteamDeckStatePacket_t` (kernel `hid-steam` table agrees), the fixed 64-byte layout (offsets in
/// comments; no multiplex, unlike Gordon). `buttons` = the 8-byte button union ->
/// [`crate::NeptuneButtons`] (u64); triggers are 16-bit (SDL `sTriggerRawL/R`, uncalibrated);
/// `orientation` wire order `w,x,y,z`. SDL's struct **ends at the pad pressures** (`0x3C`); the
/// `*_stick_force` capacitive bytes at `0x3C`/`0x3E` are beyond SDL (InputPlumber-only), kept raw and
/// not exposed. Decoded by `state::from_neptune`.
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

// --- Gordon BLE (segmented Report-ID-3 delta stream; USB command bytes, BLE framing) ---

/// Gordon **Bluetooth (BLE)** transport framing + compact input layout. The kernel `hid-steam`
/// driver is USB-only, so BLE is reverse-engineered from SDL (`SDL_hidapi_steam.c`) and sc-controller
/// (`sc_by_bt`). Everything rides **Report ID 3** on a 20-byte HID report (report id + 1 header byte +
/// 18 payload). Feature *and* input reports longer than 18 bytes are split into segments; the command
/// bytes themselves are identical to USB (3), so only this framing + the input layout are BLE-only.
/// The reassembled input accumulates into the same snapshot (via `state::apply_gordon_ble`).
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

// --- Triton (new Steam Controller) -----------------------------------------------------
//
// Triton does NOT use the `0x01` frame: its input reports carry the **report id in byte 0**
// (dispatched in `state::parse_triton`). The State/NoQuat bodies decode -> `ControllerState`.

/// Report id prepended to Triton feature-report buffers. Triton's command channel rides
/// **feature report `0x01`**, not `0x00` - confirmed in SDL (`DisableSteamTritonLizardMode`
/// sets `buffer[0]=1`) and sc-controller (`wValue 0x0301`, `0x01`-prefixed payload). The
/// command *body* (`[cmd_id, len, payload...]`) is otherwise identical to Gordon/Neptune.
pub(crate) const REPORT_ID_TRITON: u8 = 0x01;

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

bitflags::bitflags! {
    /// Raw Triton (new Steam Controller, 2026) button bits, packed as a `u32` from the four button
    /// bytes of report `0x42`: `byte2 | byte3<<8 | byte4<<16 | byte5<<24`. Bit assignments verified
    /// against SDL `SDL_hidapi_steam_triton.c` (`TritonButtons`) and sc-controller `sc2.py`
    /// (`SC2Button`) - the two agree. Folded into the unified [`crate::Buttons`] in `state.rs`.
    ///
    /// Named with the unified scheme (so the fold is 1:1): the back paddles follow the Deck
    /// convention - upper `R4/L4` -> `RGRIP/LGRIP`, lower `R5/L5` -> `RGRIP2/LGRIP2`. `RT/LT` are the
    /// trigger digital full-pull bits. `L/RGRIP_TOUCH` are the capacitive handle sensors this
    /// controller adds over the Deck (on whenever the handles are held - including resting on a
    /// table). Two high bits (`1<<30`, `1<<31`) are unidentified on the test units.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    pub struct TritonButtons: u32 {
        // byte2
        const A            = 1 << 0;
        const B            = 1 << 1;
        const X            = 1 << 2;
        const Y            = 1 << 3;
        const QUICK_ACCESS = 1 << 4;  // the "..." QAM button
        const RSTICK_PRESS = 1 << 5;  // R3
        const MENU         = 1 << 6;  // (right/start)
        const RGRIP        = 1 << 7;  // R4 (upper right paddle)
        // byte3
        const RGRIP2       = 1 << 8;  // R5 (lower right paddle)
        const RB           = 1 << 9;  // R1 bumper
        const DPAD_DOWN    = 1 << 10;
        const DPAD_RIGHT   = 1 << 11;
        const DPAD_LEFT    = 1 << 12;
        const DPAD_UP      = 1 << 13;
        const VIEW         = 1 << 14; // (left/select)
        const LSTICK_PRESS = 1 << 15; // L3
        // byte4
        const STEAM        = 1 << 16;
        const LGRIP        = 1 << 17; // L4 (upper left paddle)
        const LGRIP2       = 1 << 18; // L5 (lower left paddle)
        const LB           = 1 << 19; // L1 bumper
        const RSTICK_TOUCH = 1 << 20;
        const RPAD_TOUCH   = 1 << 21;
        const RPAD_PRESS   = 1 << 22;
        const RT           = 1 << 23; // right trigger full-pull (digital)
        // byte5
        const LSTICK_TOUCH = 1 << 24;
        const LPAD_TOUCH   = 1 << 25;
        const LPAD_PRESS   = 1 << 26;
        const LT           = 1 << 27; // left trigger full-pull (digital)
        const RGRIP_TOUCH  = 1 << 28; // capacitive right handle
        const LGRIP_TOUCH  = 1 << 29; // capacitive left handle
    }
}

// Triton state has three body variants (SDL `TritonMTU*_t`) that share the same leading fields;
// **only `TritonStateNoQuat` is used for `from_bytes` decoding** (`state.rs`). `buttons` = raw
// [`crate::TritonButtons`] (u32); `imu_timestamp` unused (we synth `seq` from `seq_num`). No
// left-multiplex - separate stick/pad, both live. All decode into `ControllerState` (`state::from_triton`).

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
/// unit seen streaming `0x47` would wire this into `state.rs`.
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
/// is the charge percent (used); everything else is trace. Decodes into `state::Battery`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct TritonBatteryStatus {
    pub report_id: u8,       // 0
    pub charge_state: u8,    // 1 - a [`triton::charge_state`] value
    pub battery_level: u8,   // 2 (0..=100)
    pub voltage_mv: u16,     // 3 (battery, mV)
    pub system_voltage: u16, // 5 (mV)
    pub input_voltage: u16,  // 7 (mV)
    pub current: u16,        // 9
    pub input_current: u16,  // 11
    pub temperature: u16,    // 13
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
        assert_eq!(
            m.as_bytes(),
            &[1, 0x02, 0x01, 0x04, 0x03, 0x06, 0x05, 0xFF][..]
        );
    }

    #[test]
    fn trigger_haptic_bytes() {
        let m = MsgTriggerHaptic {
            side: 2,
            cmd: HapticType::Click as u8,              // 2
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
        assert_eq!(
            m.as_bytes(),
            &[0, 0x34, 0x12, 0x78, 0x56, 0xBC, 0x9A, 0xFF, 2][..]
        );
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
        assert_eq!(
            m.as_bytes(),
            &[0, 0x34, 0x12, 0x78, 0x56, 0xFF, 0xBC, 0x9A, 2][..]
        );
    }

    #[test]
    fn triton_command_body_bytes() {
        let m = MsgHapticCommand {
            side: 2,
            command: 1,
            gain_db: -1,
        };
        assert_eq!(m.as_bytes(), &[2, 1, 0xFF][..]);
    }

    // --- 3/4: round-trips + short-slice guard ---

    #[test]
    fn controller_setting_roundtrip() {
        let s = ControllerSetting {
            setting_num: 0x2A,
            value: 0xBEEF,
        };
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

    #[test]
    fn wireless_event_offset() {
        let mut b = [0u8; REPORT_LEN];
        b[0x04] = wireless::CONNECTED; // event byte at the payload start
        assert_eq!(
            WirelessEvent::from_bytes(&b).unwrap().event,
            wireless::CONNECTED
        );
        assert!(WirelessEvent::from_bytes(&[0u8; 4]).is_none()); // too short
    }

    #[test]
    fn gordon_reference_packet_offsets() {
        // DebugPacket: right_pad_mouse_filtered @0x18, digital_io_states1 @0x28.
        let mut b = [0u8; REPORT_LEN];
        b[0x18..0x1C].copy_from_slice(&[0x02, 0x01, 0x04, 0x03]);
        b[0x28..0x2C].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes());
        let d = DebugPacket::from_bytes(&b).unwrap();
        let (rmf, io1) = (d.right_pad_mouse_filtered, d.digital_io_states1);
        assert_eq!((rmf.x, rmf.y), (0x0102, 0x0304));
        assert_eq!(io1, 0xCAFE_F00D);

        // BleStatePacket: gyro_data_type @0x18, gyro[4] @0x19.
        let mut b = [0u8; REPORT_LEN];
        b[0x18] = 2; // accel group
        b[0x19..0x1B].copy_from_slice(&(-1234i16).to_le_bytes());
        let s = BleStatePacket::from_bytes(&b).unwrap();
        let (gdt, gyro) = (s.gyro_data_type, s.gyro);
        assert_eq!(gdt, 2);
        assert_eq!(gyro[0], -1234);
    }
}
