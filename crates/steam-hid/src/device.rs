//! Discovery, opening, and per-device I/O (PLAN §1.5, §1.6).

use std::ffi::CString;
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::backend::{HidapiDevice, RawHid};
use crate::command::{HapticPulse, HapticStyle, Motor};
use crate::error::{Error, Result};
use crate::event::Events;
use crate::protocol::{
    self, Cmd, ControllerStringAttributes, GyroMode, HapticIntensity, HapticType, TrackpadDPadMode,
    TritonOutReport, Wire, setting,
};
use crate::report::{self, RawReport};
use crate::state::{Battery, Report};
use crate::value::Timestamp;

/// Internal read timeout for the "blocking" `read_*`; looped so it can later be
/// made cancellable for cooperative shutdown (PLAN §1.6).
const READ_TIMEOUT_MS: i32 = 1000;

/// Which Steam device this is (Valve codenames; unified with [`RawReport`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKind {
    /// Original Steam Controller.
    Gordon,
    /// Steam Deck built-in controls.
    Neptune,
    /// New Steam Controller (2026; SDL codename "Triton").
    Triton,
}

impl DeviceKind {
    /// Canonical lowercase token (used in [`DeviceId`] strings).
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceKind::Gordon => "gordon",
            DeviceKind::Neptune => "neptune",
            DeviceKind::Triton => "triton",
        }
    }

    /// Parse a canonical token (see [`DeviceKind::as_str`]).
    pub fn from_token(s: &str) -> Option<DeviceKind> {
        match s {
            "gordon" => Some(DeviceKind::Gordon),
            "neptune" => Some(DeviceKind::Neptune),
            "triton" => Some(DeviceKind::Triton),
            _ => None,
        }
    }

    /// Whether this device auto-reverts to lizard mode and so needs the reader to periodically
    /// re-assert lizard-off. The Deck reverts after ~10 s and Triton after ~3 s (its firmware
    /// watchdog); Gordon holds its config and needs none (PLAN §1.9). The reader uses a single
    /// ~3 s cadence for whichever devices need it.
    pub fn needs_keepalive(&self) -> bool {
        match self {
            DeviceKind::Gordon => false,
            DeviceKind::Neptune | DeviceKind::Triton => true,
        }
    }

    /// Whether this device has a front LED whose intensity is settable. Gordon and Triton do; the
    /// Deck has no front-facing LED (only a power LED we deliberately leave alone).
    pub fn has_led_intensity(&self) -> bool {
        match self {
            DeviceKind::Gordon | DeviceKind::Triton => true,
            DeviceKind::Neptune => false,
        }
    }

    /// Whether this device's command/input transport is Triton's (report-id-in-byte-0 input,
    /// feature report `0x01`, output-report haptics) rather than the `0x01`-framed Gordon/Neptune
    /// protocol.
    fn is_triton(&self) -> bool {
        matches!(self, DeviceKind::Triton)
    }
}

/// How the device is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    UsbWired,
    UsbDongle,
    /// Bluetooth (BLE). Two very different framings by device: **Gordon** uses the segmented
    /// Report-ID-3 delta format (`BleState`, PLAN §1.4); **Triton** does NOT segment — the OS
    /// HID-over-GATT stack reassembles it into plain numbered reports (state id `0x45`), so it rides
    /// the same `next_frame_triton` path as USB. Which one is chosen by `DeviceKind`, not here.
    Bluetooth,
}

impl Transport {
    /// Canonical lowercase token (used in [`DeviceId`] strings).
    pub fn as_str(&self) -> &'static str {
        match self {
            Transport::UsbWired => "wired",
            Transport::UsbDongle => "dongle",
            Transport::Bluetooth => "bt",
        }
    }

    /// Parse a canonical token (see [`Transport::as_str`]).
    pub fn from_token(s: &str) -> Option<Transport> {
        match s {
            "wired" => Some(Transport::UsbWired),
            "dongle" => Some(Transport::UsbDongle),
            "bt" => Some(Transport::Bluetooth),
            _ => None,
        }
    }

    /// Whether this transport uses the BLE segmented framing + compact input format.
    fn is_bluetooth(&self) -> bool {
        matches!(self, Transport::Bluetooth)
    }

    /// Whether the controller-side idle/sleep timeout is meaningful here. A wired controller is
    /// bus-powered and never idles; wireless (dongle/BT) runs on battery and does.
    pub fn has_idle(&self) -> bool {
        match self {
            Transport::UsbWired => false,
            Transport::UsbDongle => true,
            Transport::Bluetooth => true,
        }
    }
}

/// A stable, path-independent identity for a device (PLAN §4.3). Built from the fields that
/// survive a replug — `kind`, `transport`, slot (`interface`), and `serial` — **not** the
/// ephemeral OS path (Linux `/dev/hidrawN` is reassigned on replug). Used to pin a selection so
/// it reconnects to the *same* physical device, and to name a device on the CLI / control socket.
///
/// String form (`Display`/`FromStr`): `kind:transport:interface:serial`, e.g.
/// `gordon:dongle:1:ABCDEF`; an empty trailing field means no serial (`gordon:wired:2:`). The
/// serial is taken verbatim as the remainder, so it may itself contain `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId {
    pub kind: DeviceKind,
    pub transport: Transport,
    pub interface: i32,
    pub serial: Option<String>,
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}:{}",
            self.kind.as_str(),
            self.transport.as_str(),
            self.interface,
            self.serial.as_deref().unwrap_or(""),
        )
    }
}

impl std::str::FromStr for DeviceId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::ParseDeviceId(s.to_owned());
        let mut it = s.splitn(4, ':');
        let (Some(kind), Some(transport), Some(iface), Some(serial)) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            return Err(bad());
        };
        Ok(DeviceId {
            kind: DeviceKind::from_token(kind).ok_or_else(bad)?,
            transport: Transport::from_token(transport).ok_or_else(bad)?,
            interface: iface.parse().map_err(|_| bad())?,
            serial: (!serial.is_empty()).then(|| serial.to_owned()),
        })
    }
}

/// A discovered device (one gamepad HID interface / dongle slot).
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub kind: DeviceKind,
    pub transport: Transport,
    pub serial: Option<String>,
    pub vid: u16,
    pub pid: u16,
    /// HID interface number — identifies the wired gamepad iface / dongle slot.
    pub interface: i32,
    /// Opaque OS path used to open the device.
    pub(crate) path: CString,
}

impl DeviceInfo {
    /// This device's stable, path-independent [`DeviceId`] (PLAN §4.3). An empty serial is
    /// normalized to `None` (the Gordon dongle reports `Some("")`) so the id is canonical and
    /// round-trips through its string form.
    pub fn id(&self) -> DeviceId {
        DeviceId {
            kind: self.kind.clone(),
            transport: self.transport.clone(),
            interface: self.interface,
            serial: self.serial.clone().filter(|s| !s.is_empty()),
        }
    }
}

fn classify(pid: u16) -> Option<(DeviceKind, Transport)> {
    match pid {
        protocol::PID_GORDON_WIRED => Some((DeviceKind::Gordon, Transport::UsbWired)),
        protocol::PID_GORDON_DONGLE => Some((DeviceKind::Gordon, Transport::UsbDongle)),
        protocol::PID_GORDON_BLE => Some((DeviceKind::Gordon, Transport::Bluetooth)),
        protocol::PID_NEPTUNE => Some((DeviceKind::Neptune, Transport::UsbWired)),
        protocol::PID_TRITON_WIRED => Some((DeviceKind::Triton, Transport::UsbWired)),
        protocol::PID_TRITON_BLE => Some((DeviceKind::Triton, Transport::Bluetooth)),
        protocol::PID_TRITON_PUCK => Some((DeviceKind::Triton, Transport::UsbDongle)),
        _ => None,
    }
}

/// Owns the HID context; enumerates and opens devices (PLAN §1.5).
pub struct Manager {
    api: HidApi,
}

impl Manager {
    /// Create a manager (initializes the HID backend).
    pub fn new() -> Result<Self> {
        Ok(Manager {
            api: HidApi::new()?,
        })
    }

    /// List connected Steam controller **gamepad** interfaces.
    ///
    /// Filters to the Valve vendor gamepad interface (usage page in the
    /// `0xFF00` range), dropping the emulated mouse/keyboard interfaces
    /// (PLAN §1.6). Interface filtering is provisional — verify on hardware.
    pub fn enumerate(&mut self) -> Result<Vec<DeviceInfo>> {
        // Re-scan the bus: hidapi caches the device list at context creation, so without this a
        // long-lived `Manager` never sees hotplug changes (breaks `devices()` freshness and the
        // engine's reacquire poll, which waits for a device to *reappear*).
        self.api.refresh_devices()?;
        let mut out = Vec::new();
        for info in self.api.device_list() {
            if info.vendor_id() != protocol::VALVE_VID {
                continue;
            }
            let Some((kind, transport)) = classify(info.product_id()) else {
                continue;
            };
            // Gamepad interface discriminator (PLAN §1.6): the vendor usage page
            // (>= 0xFF00) picks the gamepad interface, dropping the emulated
            // mouse/keyboard. This works for BLE too — hidapi lists the single BLE
            // hidraw node once *per top-level collection* (mouse 0x01, keyboard
            // 0x01, vendor 0xFF00, all same path), so the filter keeps exactly the
            // one vendor entry (verified on HW).
            if info.usage_page() < 0xFF00 {
                continue;
            }
            out.push(DeviceInfo {
                kind,
                transport,
                // Normalize an empty serial to None (the Gordon dongle reports `Some("")`).
                serial: info.serial_number().filter(|s| !s.is_empty()).map(str::to_owned),
                vid: info.vendor_id(),
                pid: info.product_id(),
                interface: info.interface_number(),
                path: info.path().to_owned(),
            });
        }
        Ok(out)
    }

    /// Open a specific enumerated device.
    pub fn open(&self, info: &DeviceInfo) -> Result<Device> {
        let dev = self.api.open_path(&info.path)?;
        Device::new(Box::new(HidapiDevice::new(dev)), info.clone())
    }

    /// Open the first enumerated device (convenience).
    pub fn open_first(&mut self) -> Result<Device> {
        let infos = self.enumerate()?;
        let first = infos.first().ok_or(Error::NoDevice)?;
        self.open(first)
    }
}

/// An open device: a transport endpoint that outlives connect/disconnect (PLAN §1.5).
///
/// Driven in **one** mode at a time — snapshots (`read`/`poll`) or events
/// (`events`) — since both consume the single frame stream.
pub struct Device {
    backend: Box<dyn RawHid>,
    info: DeviceInfo,
    start: Instant,
    connected: bool,
    battery: Option<Battery>,
    buf: [u8; protocol::REPORT_LEN],
    /// BLE reassembly + accumulation state; `Some` only on the Bluetooth transport.
    ble: Option<BleState>,
}

/// Per-device state for the Bluetooth transport (PLAN §1.4).
///
/// BLE input is a segmented *delta* stream, so we reassemble 20-byte segments into
/// a full packet and accumulate chunk updates into `acc`, emitting a snapshot per
/// completed input packet. `seq` is synthesized (the wire has none).
struct BleState {
    /// Reassembly buffer for the current multi-segment packet.
    assembled: [u8; protocol::ble::SEGMENT_PAYLOAD * protocol::ble::MAX_SEGMENTS],
    /// Next segment number expected (resets to 0 on a completed/!ordered packet).
    expected_seg: usize,
    /// Accumulated controller state (only-changed chunks arrive per packet).
    acc: report::GordonReport,
    /// Synthesized sequence counter, bumped per input snapshot.
    seq: u32,
}

impl BleState {
    fn new() -> Self {
        BleState {
            assembled: [0u8; protocol::ble::SEGMENT_PAYLOAD * protocol::ble::MAX_SEGMENTS],
            expected_seg: 0,
            acc: report::GordonReport::default(),
            seq: 0,
        }
    }
}

impl Device {
    fn new(backend: Box<dyn RawHid>, info: DeviceInfo) -> Result<Self> {
        // Wired USB and Bluetooth are point-to-point (connected the moment the
        // endpoint opens); only the dongle multiplexes an absent controller.
        let connected = matches!(info.transport, Transport::UsbWired | Transport::Bluetooth);
        // Gordon's BLE segmented-delta reassembly. Triton over BLE is a *different* framing (its own
        // report ids, no segmentation) handled by the Triton path — so this state is Gordon-only.
        let ble = (info.transport.is_bluetooth() && !info.kind.is_triton()).then(BleState::new);
        let mut dev = Device {
            backend,
            info,
            start: Instant::now(),
            connected,
            battery: None,
            buf: [0u8; protocol::REPORT_LEN],
            ble,
        };
        // On a wireless endpoint, prompt the current connection status so an
        // already-connected controller surfaces without waiting (PLAN §1.6).
        // Gordon dongle only — this is the original receiver's wireless-state command; the Triton
        // puck streams state by default when a controller is present, so it needs no prompt.
        if matches!(dev.info.transport, Transport::UsbDongle) && !dev.info.kind.is_triton() {
            let _ = dev.feature(Cmd::DongleGetWirelessState, &[]);
        }
        Ok(dev)
    }

    /// The device this was opened from.
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Whether a controller is currently connected on this endpoint (cached from
    /// `0x03` frames; convenience, not load-bearing — PLAN §1.5).
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Last-known battery status, if any (wireless only).
    pub fn battery(&self) -> Option<Battery> {
        self.battery.clone()
    }

    // --- input: one physical read == one frame of some type ---

    /// Read one raw wire frame, blocking until one arrives (PLAN §1.5).
    pub fn read_raw(&mut self) -> Result<RawReport> {
        loop {
            if let Some(raw) = self.next_frame(READ_TIMEOUT_MS)? {
                return Ok(raw);
            }
        }
    }

    /// Read one raw wire frame, waiting up to `timeout`. `None` on timeout.
    pub fn poll_raw(&mut self, timeout: Duration) -> Result<Option<RawReport>> {
        self.next_frame(clamp_timeout(timeout))
    }

    /// Read one high-level frame (normalized snapshot or lifecycle), blocking.
    pub fn read(&mut self) -> Result<Report> {
        let raw = self.read_raw()?;
        Ok(Report::decode(&raw, self.now()))
    }

    /// Read one high-level frame, waiting up to `timeout`. `None` on timeout.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Report>> {
        match self.poll_raw(timeout)? {
            Some(raw) => Ok(Some(Report::decode(&raw, self.now()))),
            None => Ok(None),
        }
    }

    /// The change-driven [`Events`] view over this device (PLAN §1.5).
    pub fn events(&mut self) -> Events<'_> {
        Events::new(self)
    }

    /// Read one frame, update cached connection/battery state, and decode it.
    fn next_frame(&mut self, timeout_ms: i32) -> Result<Option<RawReport>> {
        if self.info.kind.is_triton() {
            return self.next_frame_triton(timeout_ms);
        }
        if self.ble.is_some() {
            return self.next_frame_ble(timeout_ms);
        }
        // The default path: **USB Gordon (wired + dongle) and Neptune** — one physical read is one
        // 64-byte `0x01`-framed report (`[0x01, 0x00, <event>, …]`), decoded by `report::parse`.
        let n = self.backend.read_timeout(&mut self.buf, timeout_ms)?;
        if n == 0 {
            return Ok(None);
        }
        let raw = report::parse(&self.buf)?;
        match &raw {
            RawReport::Connected => self.connected = true,
            RawReport::Disconnected => self.connected = false,
            RawReport::Battery(b) => self.battery = Some(Battery::from(b)),
            _ => {}
        }
        Ok(Some(raw))
    }

    /// Triton read path: one physical read == one report, dispatched by the **report id in byte 0**
    /// (not the `0x01`-framed event byte Gordon/Neptune use — see `report::parse_triton`). Works for
    /// every Triton transport: the puck and wire stream state as `0x42`, **Bluetooth streams `0x45`**
    /// (both the same "NoQuat" body). On Linux/Windows the OS HID-over-GATT stack reassembles BLE and
    /// prepends the report id, so BT reports arrive here exactly like USB (no segmentation, unlike
    /// Gordon BLE). Reports we don't decode as a frame (the `0x47` timestamped body, unknown ids) are
    /// skipped by reading again within the timeout budget; a read timeout returns `None`.
    fn next_frame_triton(&mut self, timeout_ms: i32) -> Result<Option<RawReport>> {
        loop {
            let n = self.backend.read_timeout(&mut self.buf, timeout_ms)?;
            if n == 0 {
                return Ok(None);
            }
            let Some(raw) = report::parse_triton(&self.buf[..n]) else {
                continue; // undecoded report — keep reading
            };
            match &raw {
                RawReport::Connected => self.connected = true,
                RawReport::Disconnected => self.connected = false,
                RawReport::Battery(b) => self.battery = Some(Battery::from(b)),
                _ => {}
            }
            return Ok(Some(raw));
        }
    }

    /// BLE read path: reassemble 20-byte segments into a packet, accumulate its
    /// chunks, and emit a full snapshot per completed **input** packet (PLAN §1.4).
    ///
    /// Loops within one call so a multi-segment frame returns as one `RawReport`;
    /// a read timeout returns `None` with partial reassembly state preserved for the
    /// next call. Non-input (status) packets are skipped (read again).
    fn next_frame_ble(&mut self, timeout_ms: i32) -> Result<Option<RawReport>> {
        use protocol::ble;
        loop {
            let mut seg = [0u8; ble::SEGMENT_SIZE];
            let n = self.backend.read_timeout(&mut seg, timeout_ms)?;
            if n == 0 {
                return Ok(None); // timeout — partial reassembly (if any) is retained
            }
            if n < ble::SEGMENT_SIZE || seg[0] != ble::REPORT_ID {
                continue;
            }
            let hdr = seg[1];
            if hdr & ble::SEG_DATA_FLAG == 0 {
                continue; // empty segment
            }
            let ble_state = self.ble.as_mut().expect("ble state present on BT transport");
            let segnum = (hdr & ble::SEG_NUM_MASK) as usize;
            if segnum != ble_state.expected_seg {
                // Out of order: resync only on a fresh packet (segment 0).
                ble_state.expected_seg = 0;
                if segnum != 0 {
                    continue;
                }
            }
            if segnum >= ble::MAX_SEGMENTS {
                ble_state.expected_seg = 0;
                continue;
            }
            let at = segnum * ble::SEGMENT_PAYLOAD;
            ble_state.assembled[at..at + ble::SEGMENT_PAYLOAD]
                .copy_from_slice(&seg[2..ble::SEGMENT_SIZE]);

            if hdr & ble::SEG_LAST_FLAG == 0 {
                ble_state.expected_seg += 1;
                continue; // need more segments
            }

            // Packet complete: apply its chunks to the accumulator.
            ble_state.expected_seg = 0;
            let len = (segnum + 1) * ble::SEGMENT_PAYLOAD;
            let assembled = ble_state.assembled;
            if report::apply_gordon_ble(&mut ble_state.acc, &assembled[..len]) {
                ble_state.seq = ble_state.seq.wrapping_add(1);
                ble_state.acc.seq = ble_state.seq;
                return Ok(Some(RawReport::Gordon(ble_state.acc.clone())));
            }
            // Non-input (status) packet — keep reading within the timeout budget.
        }
    }

    fn now(&self) -> Timestamp {
        Timestamp(self.start.elapsed())
    }

    // --- commands (PLAN §1.4; byte layouts provisional, verify on HW) ---

    /// Enable ("lizard") or disable raw mode.
    ///
    /// Lizard-off clears digital mappings and sets both trackpads to `NONE`
    /// (raw). The Deck (Neptune) reverts to lizard ~10 s after lizard-off unless
    /// it is re-asserted, so a long-lived consumer must call this periodically —
    /// the engine reader does (~2 s, Neptune-gated; HW-verified holding a real
    /// Deck alive across a multi-minute session). steam-hid spawns NO keep-alive
    /// thread by design: `Device` is the single writer (Send, not Sync), so the
    /// cadence lives in the owner's read loop, alongside its other device writes.
    pub fn set_lizard_mode(&mut self, on: bool) -> Result<()> {
        if on {
            self.feature(Cmd::SetDefaultDigitalMappings, &[])?;
            self.feature(Cmd::LoadDefaultSettings, &[])?;
        } else {
            self.feature(Cmd::ClearDigitalMappings, &[])?;
            self.set_settings(&[
                (setting::LEFT_TRACKPAD_MODE, TrackpadDPadMode::None as u16),
                (setting::RIGHT_TRACKPAD_MODE, TrackpadDPadMode::None as u16),
            ])?;
        }
        Ok(())
    }

    /// Set the IMU mode bits (gyro/accel/orientation), PLAN §1.4.
    pub fn set_imu_mode(&mut self, mode: GyroMode) -> Result<()> {
        self.set_settings(&[(setting::IMU_MODE, mode.bits())])
    }

    /// Convenience: enable raw accel + raw gyro, or turn the IMU off.
    pub fn set_gyro(&mut self, on: bool) -> Result<()> {
        let mode = if on {
            GyroMode::raw_motion()
        } else {
            GyroMode::empty()
        };
        self.set_imu_mode(mode)
    }

    /// Set the LED brightness (0..=100 %).
    pub fn set_led_intensity(&mut self, percent: u8) -> Result<()> {
        self.set_settings(&[(setting::LED_USER_BRIGHTNESS, percent.min(100) as u16)])
    }

    /// Set the sleep/idle inactivity timeout, in seconds.
    pub fn set_idle_timeout(&mut self, secs: u16) -> Result<()> {
        self.set_settings(&[(setting::SLEEP_INACTIVITY_TIMEOUT, secs)])
    }

    /// Trigger a trackpad haptic **pulse** (`0x8f`), kernel 8-byte form.
    ///
    /// **Verified on Gordon** (PLAN §1.9): `Motor::Right`→wire pad 0, `Motor::Left`→wire
    /// pad 1 (the kernel's legacy left/right swap). `params.gain` is honored on the Deck
    /// but ignored on Gordon. This drives the *trackpad* actuator (Gordon's only haptic;
    /// works on the Deck too). For the Deck's native continuous rumble use [`Self::rumble_cmd`].
    pub fn haptic_pulse(&mut self, motor: Motor, params: HapticPulse) -> Result<()> {
        let position: u8 = match motor {
            Motor::Right => 0,
            Motor::Left => 1,
            // `0x8F` has no BOTH side — pad=2 no-ops on Gordon (HW-verified). Drive Left/Right
            // separately (there is a single `HapticPulse`, so the caller loses nothing).
            Motor::Both => {
                return Err(Error::Unsupported(
                    "0x8F haptic pulse has no BOTH side; drive Left and Right separately",
                ));
            }
        };
        let msg = protocol::MsgFireHapticPulse {
            which_pad: position,
            duration: params.duration,
            interval: params.interval,
            count: params.count,
            gain: params.gain,
        };
        self.feature(Cmd::TriggerHapticPulse, msg.as_bytes())
    }

    /// Drive the Deck's dual haptic motors — **rumble** (`0xeb` `TRIGGER_RUMBLE_CMD`), kernel
    /// 9-byte form.
    ///
    /// The Deck's native rumble, what the Linux `hid-steam` driver wires `FF_RUMBLE` to. Each
    /// command plays a **fixed short burst** (~0.5 s, HW-measured — the packet has no length field),
    /// so a sustained rumble must be **re-issued** periodically; `(0, 0)` stops it. Character is
    /// **pulsating**: `left`/`right` set the **pulse rate** (higher = faster; *not* a rumble
    /// frequency), while amplitude has two levers — `left_gain`/`right_gain` (dB, coarse) and
    /// **`intensity`** (a finer amplitude control gain lacks, but **inverted**: `0` = strongest,
    /// larger = weaker, ~unfelt near `u16::MAX`; usable ~`0..16k`). We currently pass `intensity = 0`
    /// (strongest) everywhere — plumbed but not yet used as a mapping lever.
    ///
    /// **`intensity` is a `u16` (LE), kernel- and SDL-confirmed** (`report[3]` = LSB, `report[4]` =
    /// MSB — kernel `steam_haptic_rumble`; SDL `MsgSimpleRumbleCmd.unIntensity`). A HW sweep of the
    /// low byte *alone* feels like it does nothing, but that's only because it's the least-significant
    /// byte (0..255 of a 0..65535 range) — it's fine resolution, not a dead field (the low bytes of
    /// `left`/`right` behave the same). InputPlumber's "single-byte intensity + event_type" split is
    /// wrong. The leading `report[2]` (kernel 0 / SDL `unRumbleType`) is a rumble-type selector we
    /// leave at 0.
    ///
    /// **Deck-only:** Gordon has no motors, so `0xeb` no-ops there — use [`Self::haptic_pulse`] for
    /// Gordon. (`left` = strong/large motor, `right` = weak/small, matching the kernel's
    /// `rumble_left`/`rumble_right` ← FF strong/weak.)
    pub fn rumble_cmd(
        &mut self,
        intensity: u16,
        left: u16,
        right: u16,
        left_gain: i8,
        right_gain: i8,
    ) -> Result<()> {
        let msg = protocol::MsgSimpleRumbleCmd {
            rumble_type: 0,
            intensity,
            left_speed: left,
            right_speed: right,
            left_gain,
            right_gain,
        };
        self.feature(Cmd::TriggerRumbleCmd, msg.as_bytes())
    }

    /// Fire the Deck's `0xEA` `SET_HAPTIC2` — a short, finely-tuned trackpad **click** haptic (much
    /// better than `0x8f` for command clicks; the strongest setting beats a full `0x8f` click). `cmd`
    /// picks the haptic type (we use [`HapticType::Tick`]/[`HapticType::Click`]), `ui_intensity` is a
    /// second HW-confirmed lever (see [`HapticIntensity`]), and `gain` (dB) scales it — together a
    /// wide range of click strengths.
    ///
    /// **Deck-only** (no-ops on Gordon). Payload is the full [`protocol::MsgTriggerHaptic`] (SDL);
    /// only `side`/`cmd`/`ui_intensity`/`dbgain` are set, the tone/lfo/sweep fields stay zero.
    /// `Motor::Left → 0`, `Motor::Right → 1`, `Motor::Both → 2` (all HW-verified; the **reverse** of
    /// `0x8f`'s pads).
    pub fn haptic_cmd(
        &mut self,
        motor: Motor,
        cmd: HapticType,
        intensity: HapticIntensity,
        gain: i8,
    ) -> Result<()> {
        let side: u8 = match motor {
            Motor::Left => 0,
            Motor::Right => 1,
            Motor::Both => 2, // HW-verified: 0xEA honors a BOTH side.
        };
        let msg = protocol::MsgTriggerHaptic {
            side,
            cmd: cmd as u8,
            ui_intensity: intensity as u8,
            dbgain: gain,
            ..Default::default()
        };
        self.feature(Cmd::TriggerHapticCmd, msg.as_bytes())
    }

    /// Drive Triton's dual-motor **continuous rumble** — output report `0x80` (`HapticRumble`,
    /// 10 bytes). Unlike the Deck's `0xeb`, this rides an **output** report on the interrupt-OUT
    /// endpoint. `left`/`right` are the per-motor drive (SDL feeds the 16-bit rumble magnitudes here
    /// as the field it calls `speed`); `*_gain` are per-motor dB trims; `intensity` is a finer
    /// amplitude lever (SDL passes 0). The firmware safety-times out in ~50 ms, so a sustained rumble
    /// must be **re-issued** (the reader does, ~40 ms); `(0, 0)` stops it.
    ///
    /// **HW-verified on the puck (PLAN §1.9), same levers as the Deck's `0xeb`:** `left`/`right` are
    /// the per-motor **rate** (SDL's "speed"; higher = stronger, the coarse amplitude), `*_gain` (dB)
    /// the real strength trim, and `intensity` a finer **inverted** amplitude lever (`0` = no change,
    /// larger = weaker). Param order mirrors [`Self::rumble_cmd`]. `Motor` side 0=left/1=right
    /// (verified). **Triton-only.**
    pub fn rumble_triton(
        &mut self,
        intensity: u16,
        left: u16,
        right: u16,
        left_gain: i8,
        right_gain: i8,
    ) -> Result<()> {
        // rumble_type (SDL MsgHapticRumble.type) is HW-confirmed inert (swept 0..255, no effect);
        // every reference sends 0, so we do too.
        let msg = protocol::MsgHapticRumble {
            rumble_type: 0,
            intensity,
            left_speed: left,
            left_gain,
            right_speed: right,
            right_gain,
        };
        self.output(TritonOutReport::Rumble, msg.as_bytes())
    }

    /// Fire a Triton **haptic command / click** — output report `0x82` (`HapticCommand`, 4 bytes):
    /// `[side, style, amplitude]`. `style` is a [`HapticStyle`] (`0` off / `1` weak / `2` strong —
    /// HW: `Weak` is a light click, `Strong` a firm one; this is the **main strength lever**).
    /// `amplitude` is an **unsigned** trim, `0x00` = medium … `0xFF` = strong (sc-controller's
    /// observed layout — SDL's struct misleadingly types this byte as a signed `gain_db`, but its own
    /// driver never sends `0x82`; HW confirms the audible effect of this byte is subtle). `Motor`
    /// side 0=left/1=right. **Triton-only.**
    pub fn haptic_command_triton(&mut self, motor: Motor, style: HapticStyle, amplitude: u8) -> Result<()> {
        let side: u8 = match motor {
            Motor::Left => 0,
            Motor::Right => 1,
            Motor::Both => 2, // HW-verified: Triton's 0x82 click honors a BOTH side.
        };
        // `command` = the haptic type (off/weak/strong); `gain_db` carries our unsigned amplitude
        // trim (SDL types the byte i8, but HW treats it as 0=medium..255=strong — see the struct doc).
        let msg = protocol::MsgHapticCommand { side, command: style as u8, gain_db: amplitude as i8 };
        self.output(TritonOutReport::Command, msg.as_bytes())
    }

    /// Power the controller off.
    pub fn power_off(&mut self) -> Result<()> {
        self.feature(Cmd::TurnOffController, b"off!")
    }

    // --- reads / queries (GET round-trips; **HW-UNTESTED** — probe with the `getters` example) ---

    /// Read a string attribute (e.g. the unit serial) via `GET_STRING_ATTRIBUTE` (`0xAE`). Request is
    /// `[0xAE, max_len, tag]` (kernel `steam_get_serial` uses `max_len` = 0x16); the reply is
    /// `[cmd, str_len, echoed_tag, string…]`. **USB only** (BLE feature-report segmentation not
    /// handled here). **HW-UNTESTED.**
    pub fn get_string_attribute(&mut self, tag: ControllerStringAttributes) -> Result<String> {
        let tag = tag as u8;
        // Accept only a reply for *this* tag whose string is non-empty (rejects a stale other-tag
        // reply and the mid-update empty-buffer race). `body` = `MsgGetStringAttribute { tag,
        // value[20] }`; `len` = the reply header's body length (tag + value = 21, HW-observed), an
        // upper bound only — the string itself is NUL-terminated, so we cut `value` at the first NUL.
        let (body, len) = self.get_roundtrip(
            Cmd::GetStringAttribute,
            0x16, // requested max response length (kernel `steam_get_serial`)
            &[tag],
            |_len, body| body.first() == Some(&tag) && body.get(1).is_some_and(|&b| b != 0),
        )?;
        let msg = protocol::MsgGetStringAttribute::from_bytes(&body)
            .ok_or(Error::ShortReport { expected: 21, got: body.len() })?;
        let value = msg.value; // copy the packed array out by value before slicing
        let raw = &value[..len.min(value.len())];
        let raw = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        Ok(String::from_utf8_lossy(raw).into_owned())
    }

    /// Read the controller's read-only attributes via `GET_ATTRIBUTES_VALUES` (`0x83`) — the **full**
    /// list of `(tag, value)` (SDL sends a bare `[0x83]`; the firmware returns all — there's no
    /// per-tag request). `tag` is a [`protocol::ControllerAttributes`] (name it via
    /// [`protocol::ControllerAttributes::from_tag`]), `value` a `u32`. **USB only.**
    pub fn get_attributes(&mut self) -> Result<Vec<(u8, u32)>> {
        let (data, len) = self.get_roundtrip(
            Cmd::GetAttributesValues,
            0, // no request payload — returns the full set
            &[],
            |len, _| len > 0, // non-empty attribute list
        )?;
        Ok(data[..len]
            .as_chunks::<5>() // ControllerAttribute = {tag:u8, value:u32}
            .0
            .iter()
            .filter_map(|c| protocol::ControllerAttribute::from_bytes(c))
            .map(|a| (a.tag, a.value))
            .collect())
    }

    /// Read specific settings via `GET_SETTINGS_VALUES` (`0x89`) — returns `(id, value)` pairs. The
    /// **request uses the same `ControllerSetting[]` shape as `SET_SETTINGS_VALUES`** — one `(id,
    /// value)` triple per wanted setting (the value is ignored on a read; HW-confirmed on Gordon).
    /// **USB only.**
    pub fn get_settings(&mut self, ids: &[u8]) -> Result<Vec<(u8, u16)>> {
        let mut entries = Vec::with_capacity(ids.len() * 3);
        for &id in ids {
            entries.extend_from_slice(
                protocol::ControllerSetting { setting_num: id, value: 0 }.as_bytes(),
            );
        }
        let (data, len) =
            self.get_roundtrip(Cmd::GetSettingsValues, entries.len() as u8, &entries, |_, _| true)?;
        Ok(data[..len]
            .as_chunks::<3>() // ControllerSetting = {id:u8, value:u16}
            .0
            .iter()
            .filter_map(|c| protocol::ControllerSetting::from_bytes(c))
            .map(|s| (s.setting_num, s.value))
            .collect())
    }

    // --- escape hatch (PLAN §1.5) ---

    /// Send a raw feature report. Takes the **logical** command
    /// `[cmd_id, len, payload…]`; the transport framing is applied here, not by the
    /// caller (PLAN §1.4): USB prepends report id 0 and pads to 64; Bluetooth splits
    /// the command into Report-ID-3 segments (`[0x03][0x80|seg|(0x40 if last)]
    /// [<=18 data]`, zero-padded to 20 — the command bytes are identical to USB).
    pub fn send_feature_report(&mut self, cmd: &[u8]) -> Result<()> {
        if self.info.transport.is_bluetooth() && !self.info.kind.is_triton() {
            for seg in frame_ble(cmd) {
                self.backend.send_feature_report(&seg)?;
            }
            Ok(())
        } else if self.info.kind.is_triton() {
            // Triton's command channel rides feature report **0x01** and the whole HID report is
            // **exactly 64 bytes** (report-id byte + 63 payload) — the device stalls a SET_REPORT of
            // any other length (Broken pipe otherwise). Gordon/Neptune use report id 0x00 with 64
            // *data* bytes (65-byte buffer; report 0 is unnumbered so nothing extra goes on the wire).
            self.backend
                .send_feature_report(&frame(cmd, protocol::REPORT_ID_TRITON, protocol::REPORT_LEN))
        } else {
            self.backend
                .send_feature_report(&frame(cmd, protocol::REPORT_ID, 1 + protocol::REPORT_LEN))
        }
    }

    /// Get a raw feature report into `buf` (report id in `buf[0]` on entry).
    pub fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.backend.get_feature_report(buf)
    }

    // --- command helpers ---

    /// Build `[FeatureReportHeader, payload…]` and send it framed.
    fn feature(&mut self, cmd: Cmd, payload: &[u8]) -> Result<()> {
        let header = protocol::FeatureReportHeader { cmd: cmd as u8, length: payload.len() as u8 };
        let mut bytes = Vec::with_capacity(2 + payload.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(payload);
        self.send_feature_report(&bytes)
    }

    /// Write settings via `SET_SETTINGS_VALUES` — a concatenation of [`protocol::ControllerSetting`]
    /// `(id, value-le)` triples. Takes `(id, value)` pairs for call-site ergonomics.
    fn set_settings(&mut self, pairs: &[(u8, u16)]) -> Result<()> {
        let mut payload = Vec::with_capacity(pairs.len() * 3);
        for &(setting_num, value) in pairs {
            payload.extend_from_slice(protocol::ControllerSetting { setting_num, value }.as_bytes());
        }
        self.feature(Cmd::SetSettingsValues, &payload)
    }

    /// Write a GET request (`request[0]` = command id) and read the reply, retrying a few times (the
    /// device may return other reports first — mirrors SDL's `ReadResponse`). Locates the echoed
    /// command id (at offset 0 or 1, absorbing hidapi's report-id-byte ambiguity for report 0),
    /// strips the `[cmd, len]` header, and returns `(body, len)`: `body` = the reply payload ready to
    /// cast (the bytes after the header), `len` = the reply's length byte clamped to the bytes
    /// available. `valid(len, body)` gates acceptance (rejects stale/other-report replies).
    fn get_roundtrip(
        &mut self,
        cmd: Cmd,
        length: u8,
        payload: &[u8],
        valid: impl Fn(usize, &[u8]) -> bool,
    ) -> Result<(Vec<u8>, usize)> {
        // Build the request through the shared `FeatureReportHeader` (same header as `feature()`) —
        // GET commands vary the header's `length` field: payload length for SETTINGS, `0` for
        // ATTRIBUTES, and the requested max response length for the STRING getter, so it's passed in
        // rather than derived from `payload.len()`.
        let echo = cmd as u8;
        let mut request = protocol::FeatureReportHeader { cmd: echo, length }.as_bytes().to_vec();
        request.extend_from_slice(payload);
        // The dongle's feature endpoint is flaky under back-to-back I/O: a `SetFeature` too soon after
        // a prior `GetFeature` EPIPEs, and a `GetFeature` can race the device mid-updating its reply
        // buffer (stale/half-written). So re-send the whole exchange each attempt, settling around
        // both halves, and only accept a reply whose `cmd` echo matches AND passes `valid`.
        let mut buf = vec![0u8; 1 + protocol::REPORT_LEN];
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(10));
            if self.send_feature_report(&request).is_err() {
                continue; // busy → retry the SetFeature
            }
            std::thread::sleep(Duration::from_millis(20)); // let the reply compute
            buf.iter_mut().for_each(|b| *b = 0);
            buf[0] = protocol::REPORT_ID;
            let n = self.get_feature_report(&mut buf)?;
            // The reply is `[report-id(0), cmd, len, body…]` — hidapi keeps the report-id byte at [0]
            // for report 0 (matches SDL's `ReadResponse`, HW-confirmed on Gordon). Reject a stale /
            // other-report reply whose echoed cmd at [1] doesn't match (the device replies out of
            // order under back-to-back GETs).
            if n < 3 || buf[1] != echo {
                continue;
            }
            let body = buf[3..].to_vec();
            let len = (buf[2] as usize).min(body.len());
            if valid(len, &body) {
                return Ok((body, len));
            }
        }
        Err(Error::Unsupported("GET response not received/validated"))
    }

    /// Build `[report_id, payload…]` and send it as a Triton haptic **output** report. The output-
    /// report analog of [`Self::feature`], but the framing is a single report-id byte (no length
    /// field) and it rides the interrupt-OUT endpoint. Output reports carry their own fixed lengths,
    /// so no `frame`-style padding.
    fn output(&mut self, cmd: TritonOutReport, payload: &[u8]) -> Result<()> {
        let mut report = Vec::with_capacity(1 + payload.len());
        report.push(cmd as u8);
        report.extend_from_slice(payload);
        self.backend.send_output_report(&report)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Best-effort: restore lizard mode so the controller isn't left dead
        // after we exit (PLAN §1.6). No keep-alive thread to join — the owning
        // consumer (the engine reader) drives the Deck keep-alive from its read
        // loop, so it stops the instant this Device is dropped.
        let _ = self.set_lizard_mode(true);
    }
}

/// Frame a logical command into a feature-report buffer of `buf_len` bytes: the `report_id` byte,
/// then the command zero-padded to fill the buffer (PLAN §1.4). Gordon/Neptune use `report_id`
/// `0x00` and `buf_len = 1 + 64` (64 data bytes); Triton uses `0x01` and `buf_len = 64` (the whole
/// report is 64 bytes — report id + 63 payload — or the device stalls the transfer).
fn frame(cmd: &[u8], report_id: u8, buf_len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; buf_len];
    buf[0] = report_id;
    let n = cmd.len().min(buf_len - 1);
    buf[1..1 + n].copy_from_slice(&cmd[..n]);
    buf
}

/// Frame a logical command into BLE feature segments (PLAN §1.4): split `cmd`
/// (`[id, len, payload…]`) into ≤18-byte chunks, each a 20-byte Report-ID-3 report
/// `[0x03][0x80 | seg | (0x40 if last)][chunk, zero-padded]`. A single-segment
/// command's header is `0xC0`.
fn frame_ble(cmd: &[u8]) -> Vec<[u8; protocol::ble::SEGMENT_SIZE]> {
    use protocol::ble;
    let mut out = Vec::new();
    // An empty command still needs one (terminal) segment.
    let mut chunks = cmd.chunks(ble::SEGMENT_PAYLOAD).peekable();
    let mut seg_num: u8 = 0;
    loop {
        let chunk = chunks.next().unwrap_or(&[]);
        let last = chunks.peek().is_none();
        let mut buf = [0u8; ble::SEGMENT_SIZE];
        buf[0] = ble::REPORT_ID;
        buf[1] = ble::SEG_DATA_FLAG
            | (seg_num & ble::SEG_NUM_MASK)
            | if last { ble::SEG_LAST_FLAG } else { 0 };
        buf[2..2 + chunk.len()].copy_from_slice(chunk);
        out.push(buf);
        if last {
            break;
        }
        seg_num += 1;
    }
    out
}

fn clamp_timeout(timeout: Duration) -> i32 {
    timeout.as_millis().min(i32::MAX as u128) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_round_trips() {
        for id in [
            DeviceId {
                kind: DeviceKind::Gordon,
                transport: Transport::UsbDongle,
                interface: 1,
                serial: Some("ABCDEF".into()),
            },
            DeviceId {
                kind: DeviceKind::Neptune,
                transport: Transport::UsbWired,
                interface: 2,
                serial: None,
            },
            // A serial containing ':' survives (it's the verbatim remainder).
            DeviceId {
                kind: DeviceKind::Gordon,
                transport: Transport::UsbWired,
                interface: 2,
                serial: Some("aa:bb".into()),
            },
        ] {
            let s = id.to_string();
            assert_eq!(s.parse::<DeviceId>().unwrap(), id, "round-trip for {s:?}");
        }
    }

    #[test]
    fn device_id_display_form() {
        let id = DeviceId {
            kind: DeviceKind::Gordon,
            transport: Transport::UsbDongle,
            interface: 1,
            serial: Some("ABCDEF".into()),
        };
        assert_eq!(id.to_string(), "gordon:dongle:1:ABCDEF");
        assert_eq!(
            DeviceId { serial: None, ..id }.to_string(),
            "gordon:dongle:1:"
        );
    }

    #[test]
    fn device_info_id_normalizes_empty_serial() {
        // The Gordon dongle reports serial = Some("") — id() must canonicalize it to None so the
        // id round-trips through its string form (and a CLI `--input gordon:dongle:1:` matches).
        let info = DeviceInfo {
            kind: DeviceKind::Gordon,
            transport: Transport::UsbDongle,
            serial: Some(String::new()),
            vid: protocol::VALVE_VID,
            pid: protocol::PID_GORDON_DONGLE,
            interface: 1,
            path: CString::new("dummy").unwrap(),
        };
        let id = info.id();
        assert_eq!(id.serial, None);
        assert_eq!(id.to_string(), "gordon:dongle:1:");
        assert_eq!(id.to_string().parse::<DeviceId>().unwrap(), id);
    }

    #[test]
    fn device_id_rejects_garbage() {
        for bad in ["", "gordon", "gordon:dongle", "gordon:dongle:x:s", "bogus:dongle:1:s"] {
            assert!(bad.parse::<DeviceId>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn frame_prepends_report_id_and_pads_to_64() {
        let f = frame(&[Cmd::SetSettingsValues as u8, 0x02, 0xAA], protocol::REPORT_ID, 1 + protocol::REPORT_LEN);
        assert_eq!(f.len(), 1 + protocol::REPORT_LEN);
        assert_eq!(f[0], protocol::REPORT_ID);
        assert_eq!(&f[1..4], &[Cmd::SetSettingsValues as u8, 0x02, 0xAA]);
        assert!(f[4..].iter().all(|&x| x == 0));
    }

    #[test]
    fn frame_truncates_overlong_command() {
        let f = frame(&[0xAB; 100], protocol::REPORT_ID, 1 + protocol::REPORT_LEN);
        assert_eq!(f.len(), 1 + protocol::REPORT_LEN);
    }
}
