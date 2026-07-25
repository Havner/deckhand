//! Shared helpers for the steam-hid examples.

use std::time::Duration;

use steam_hid::{Device, DeviceInfo, Manager, RawReport, Result, Transport};

/// Enumerate and open the target controller — the one selection path all examples
/// share.
///
/// Honors `--wired` / `--dongle` args to restrict transport. A single (or
/// explicitly-filtered) candidate is opened **directly**: a wired controller is
/// idle until you move it, so we must *not* gate on receiving a frame (that gate
/// works on the dongle only because the receiver sends periodic connect/battery
/// frames). With multiple candidates (dongle slots) it polls to pick the one that
/// actually streams. Returns the device and a human description, or `None`.
pub fn select_device(manager: &Manager) -> Result<Option<(String, Device)>> {
    let args: Vec<String> = std::env::args().collect();
    let want = if args.iter().any(|a| a == "--wired") {
        Some(Transport::UsbWired)
    } else if args.iter().any(|a| a == "--dongle") {
        Some(Transport::UsbDongle)
    } else {
        None
    };

    let devices = manager.enumerate()?;
    let candidates: Vec<&DeviceInfo> =
        devices.iter().filter(|i| want.as_ref().is_none_or(|t| i.transport == *t)).collect();
    if candidates.is_empty() {
        return Ok(None);
    }

    let describe = |i: &DeviceInfo| {
        format!("{:?} / {:?} {:04x}:{:04x} iface={}", i.kind, i.transport, i.vid, i.pid, i.interface)
    };

    // Unambiguous: open it directly (don't wait for a frame — wired is idle until moved).
    if candidates.len() == 1 {
        let info = candidates[0];
        return Ok(Some((describe(info), manager.open(info)?)));
    }

    // Multiple candidates (dongle slots): pick the one that streams / reports connected.
    for info in candidates {
        let mut device = manager.open(info)?;
        for _ in 0..8 {
            if matches!(
                device.poll_raw(Duration::from_millis(200))?,
                Some(RawReport::Gordon(_)) | Some(RawReport::Connected)
            ) {
                return Ok(Some((describe(info), device)));
            }
        }
    }
    Ok(None)
}
