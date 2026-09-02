//! Device identity vocabulary (PLAN 1.5, 4.3): what a device *is* and how it's named, independent
//! of the HID library. `DeviceKind` / `Transport` classify it, `DeviceInfo` is a discovered entry
//! (produced by the backend's `Manager`), and `DeviceId` is its stable, path-independent name.
//! Pure data - no `hidapi` here (that lives in `backend`).

use std::ffi::CString;

use crate::error::{Error, Result};
use crate::protocol;

/// Which Steam device this is (Valve codenames).
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
    /// watchdog); Gordon holds its config and needs none (PLAN 1.9). The reader uses a single
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
    pub(crate) fn is_triton(&self) -> bool {
        matches!(self, DeviceKind::Triton)
    }
}

/// How the device is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    UsbWired,
    UsbDongle,
    /// Bluetooth (BLE). Two very different framings by device: **Gordon** uses the segmented
    /// Report-ID-3 delta format (`BleState`, PLAN 1.4); **Triton** does NOT segment - the OS
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
    pub(crate) fn is_bluetooth(&self) -> bool {
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

/// A stable, path-independent identity for a device (PLAN 4.3). Built from the fields that
/// survive a replug - `kind`, `transport`, slot (`interface`), and `serial` - **not** the
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
    /// HID interface number - identifies the wired gamepad iface / dongle slot.
    pub interface: i32,
    /// Opaque OS path used to open the device.
    pub(crate) path: CString,
}

impl DeviceInfo {
    /// This device's stable, path-independent [`DeviceId`] (PLAN 4.3). An empty serial is
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

/// Map a Valve product id to its (kind, transport). `None` for a pid we don't recognize.
pub(crate) fn classify(pid: u16) -> Option<(DeviceKind, Transport)> {
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
        // The Gordon dongle reports serial = Some("") - id() must canonicalize it to None so the
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
}
