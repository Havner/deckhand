//! Discovery, opening, and per-device I/O (PLAN §1.5, §1.6).

use std::ffi::CString;
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::backend::{HidapiDevice, RawHid};
use crate::command::{ImuMode, Motor, Rumble};
use crate::error::{Error, Result};
use crate::event::Events;
use crate::protocol::{self, cmd, setting, trackpad_mode};
use crate::report::{self, RawReport};
use crate::state::{Battery, Report};
use crate::value::Timestamp;

/// Internal read timeout for the "blocking" `read_*`; looped so it can later be
/// made cancellable for cooperative shutdown (PLAN §1.6).
const READ_TIMEOUT_MS: i32 = 1000;

/// Which Steam device this is (Valve codenames; unified with [`RawReport`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceKind {
    /// Original Steam Controller.
    Gordon,
    /// Steam Deck built-in controls.
    Neptune,
}

impl DeviceKind {
    /// Canonical lowercase token (used in [`DeviceId`] strings).
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceKind::Gordon => "gordon",
            DeviceKind::Neptune => "neptune",
        }
    }

    /// Parse a canonical token (see [`DeviceKind::as_str`]).
    pub fn from_token(s: &str) -> Option<DeviceKind> {
        match s {
            "gordon" => Some(DeviceKind::Gordon),
            "neptune" => Some(DeviceKind::Neptune),
            _ => None,
        }
    }
}

/// How the device is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Transport {
    UsbWired,
    UsbDongle,
}

impl Transport {
    /// Canonical lowercase token (used in [`DeviceId`] strings).
    pub fn as_str(&self) -> &'static str {
        match self {
            Transport::UsbWired => "wired",
            Transport::UsbDongle => "dongle",
        }
    }

    /// Parse a canonical token (see [`Transport::as_str`]).
    pub fn from_token(s: &str) -> Option<Transport> {
        match s {
            "wired" => Some(Transport::UsbWired),
            "dongle" => Some(Transport::UsbDongle),
            _ => None,
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
        protocol::PID_NEPTUNE => Some((DeviceKind::Neptune, Transport::UsbWired)),
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
            // Gamepad interface discriminator (PLAN §1.6): vendor usage page.
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
}

impl Device {
    fn new(backend: Box<dyn RawHid>, info: DeviceInfo) -> Result<Self> {
        let connected = matches!(info.transport, Transport::UsbWired);
        let mut dev = Device {
            backend,
            info,
            start: Instant::now(),
            connected,
            battery: None,
            buf: [0u8; protocol::REPORT_LEN],
        };
        // On a wireless endpoint, prompt the current connection status so an
        // already-connected controller surfaces without waiting (PLAN §1.6).
        if matches!(dev.info.transport, Transport::UsbDongle) {
            let _ = dev.feature(cmd::DONGLE_GET_WIRELESS_STATE, &[]);
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

    fn now(&self) -> Timestamp {
        Timestamp(self.start.elapsed())
    }

    // --- commands (PLAN §1.4; byte layouts provisional, verify on HW) ---

    /// Enable ("lizard") or disable raw mode.
    ///
    /// Lizard-off clears digital mappings and sets both trackpads to `NONE`
    /// (raw). TODO(PLAN §1.4): the Deck also needs click-pressure + watchdog
    /// writes and the ~1 s keep-alive thread — added with the Deck path.
    pub fn set_lizard_mode(&mut self, on: bool) -> Result<()> {
        if on {
            self.feature(cmd::SET_DEFAULT_DIGITAL_MAPPINGS, &[])?;
            self.feature(cmd::LOAD_DEFAULT_SETTINGS, &[])?;
        } else {
            self.feature(cmd::CLEAR_DIGITAL_MAPPINGS, &[])?;
            self.set_settings(&[
                (setting::LEFT_TRACKPAD_MODE, trackpad_mode::NONE as u16),
                (setting::RIGHT_TRACKPAD_MODE, trackpad_mode::NONE as u16),
            ])?;
        }
        Ok(())
    }

    /// Set the IMU mode bits (gyro/accel/orientation), PLAN §1.4.
    pub fn set_imu_mode(&mut self, mode: ImuMode) -> Result<()> {
        self.set_settings(&[(setting::IMU_MODE, mode.bits())])
    }

    /// Convenience: enable raw accel + raw gyro, or turn the IMU off.
    pub fn set_gyro(&mut self, on: bool) -> Result<()> {
        let mode = if on {
            ImuMode::raw_motion()
        } else {
            ImuMode::empty()
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

    /// Trigger a trackpad haptic pulse (`0x8f`), kernel 8-byte form.
    ///
    /// **Verified on Gordon** (PLAN §1.9): `Motor::Right`→wire pad 0, `Motor::Left`→wire
    /// pad 1 (the kernel's legacy left/right swap). `params.gain` is honored on the Deck
    /// but ignored on Gordon. (`0xeb` rumble / `0xea` haptic2 are Deck-only.)
    pub fn rumble(&mut self, motor: Motor, params: Rumble) -> Result<()> {
        let position: u8 = match motor {
            Motor::Right => 0,
            Motor::Left => 1,
        };
        let [d0, d1] = params.duration.to_le_bytes();
        let [i0, i1] = params.interval.to_le_bytes();
        let [c0, c1] = params.count.to_le_bytes();
        self.feature(
            cmd::TRIGGER_HAPTIC_PULSE,
            &[position, d0, d1, i0, i1, c0, c1, params.gain as u8],
        )
    }

    /// Power the controller off.
    pub fn power_off(&mut self) -> Result<()> {
        self.feature(cmd::TURN_OFF_CONTROLLER, b"off!")
    }

    // --- escape hatch (PLAN §1.5) ---

    /// Send a raw feature report. Takes the **logical** command
    /// `[cmd_id, len, payload…]`; the report-ID-0 prepend + pad-to-64 framing
    /// is applied here, not by the caller (PLAN §1.4).
    pub fn send_feature_report(&mut self, cmd: &[u8]) -> Result<()> {
        self.backend.send_feature_report(&frame(cmd))
    }

    /// Get a raw feature report into `buf` (report id in `buf[0]` on entry).
    pub fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.backend.get_feature_report(buf)
    }

    // --- command helpers ---

    /// Build `[id, len, payload…]` and send it framed.
    fn feature(&mut self, id: u8, payload: &[u8]) -> Result<()> {
        let mut cmd = Vec::with_capacity(2 + payload.len());
        cmd.push(id);
        cmd.push(payload.len() as u8);
        cmd.extend_from_slice(payload);
        self.send_feature_report(&cmd)
    }

    /// Write settings via `SET_SETTINGS_VALUES`: `(id, lo, hi)…` little-endian.
    fn set_settings(&mut self, pairs: &[(u8, u16)]) -> Result<()> {
        let mut payload = Vec::with_capacity(pairs.len() * 3);
        for &(id, val) in pairs {
            let [lo, hi] = val.to_le_bytes();
            payload.extend_from_slice(&[id, lo, hi]);
        }
        self.feature(cmd::SET_SETTINGS_VALUES, &payload)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Best-effort: restore lizard mode so the controller isn't left dead
        // after we exit (PLAN §1.6). TODO: join the Deck keep-alive thread.
        let _ = self.set_lizard_mode(true);
    }
}

/// Frame a logical command into a feature-report buffer: report-ID-0 byte, then
/// the command padded to 64 bytes (PLAN §1.4).
fn frame(cmd: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; 1 + protocol::REPORT_LEN];
    buf[0] = protocol::REPORT_ID;
    let n = cmd.len().min(protocol::REPORT_LEN);
    buf[1..1 + n].copy_from_slice(&cmd[..n]);
    buf
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
        let f = frame(&[cmd::SET_SETTINGS_VALUES, 0x02, 0xAA]);
        assert_eq!(f.len(), 1 + protocol::REPORT_LEN);
        assert_eq!(f[0], protocol::REPORT_ID);
        assert_eq!(&f[1..4], &[cmd::SET_SETTINGS_VALUES, 0x02, 0xAA]);
        assert!(f[4..].iter().all(|&x| x == 0));
    }

    #[test]
    fn frame_truncates_overlong_command() {
        let f = frame(&[0xAB; 100]);
        assert_eq!(f.len(), 1 + protocol::REPORT_LEN);
    }
}
