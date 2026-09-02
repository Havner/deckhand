//! HID backend (PLAN 1.5, 1.6): the one module that speaks `hidapi`. Owns both halves of the
//! library surface - the context ([`Manager`], which enumerates and opens devices) and the
//! per-device raw handle ([`HidapiDevice`], timed reads plus feature/output reports). Everything
//! else in the crate is hidapi-free. `Manager` is public (re-exported); `HidapiDevice` is internal.
//!
//! `hidapi::HidDevice` is `Send`, so a `HidapiDevice` (and the `Device` holding it) can be moved
//! onto a per-device reader thread (PLAN 1.6).

use hidapi::{HidApi, HidDevice, HidError};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::info::{DeviceInfo, classify};
use crate::protocol;

/// `hidapi`-backed raw HID handle.
pub(crate) struct HidapiDevice {
    dev: HidDevice,
}

impl HidapiDevice {
    pub(crate) fn new(dev: HidDevice) -> Self {
        HidapiDevice { dev }
    }

    /// Read one input report, waiting up to `timeout_ms` (0 returned on timeout).
    pub(crate) fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize> {
        match self.dev.read_timeout(buf, timeout_ms) {
            // A signal (e.g. SIGINT from Ctrl-C) can interrupt the blocking wait
            // with EINTR. hidapi doesn't expose errno, so we match its strerror
            // text; treat it like a timeout (nothing this cycle -> `Ok(0)`) so the
            // caller's read loop re-checks its run flag rather than failing - this
            // is what makes cooperative shutdown work (PLAN 1.6). Without it, a
            // Ctrl-C mid-read surfaces as a hard error on Linux (Windows never
            // interrupts the read this way).
            Err(HidError::HidApiError { message })
                if message.contains("Interrupted system call") => Ok(0),
            other => Ok(other?),
        }
    }

    /// Send an already-framed feature report (report-ID byte included).
    pub(crate) fn send_feature_report(&self, data: &[u8]) -> Result<()> {
        self.dev.send_feature_report(data)?;
        Ok(())
    }

    /// Send an **output** report (`data[0]` = report id). Triton drives haptics this way (its
    /// `0x80`-`0x85` output reports) rather than via feature reports.
    pub(crate) fn send_output_report(&self, data: &[u8]) -> Result<()> {
        self.dev.write(data)?;
        Ok(())
    }

    /// Get a feature report; `buf[0]` should carry the report id on entry.
    pub(crate) fn get_feature_report(&self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.dev.get_feature_report(buf)?)
    }
}

/// Owns the HID context; enumerates and opens devices (PLAN 1.5).
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
    /// (PLAN 1.6). Interface filtering is provisional - verify on hardware.
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
            // Gamepad interface discriminator (PLAN 1.6): the vendor usage page
            // (>= 0xFF00) picks the gamepad interface, dropping the emulated
            // mouse/keyboard. This works for BLE too - hidapi lists the single BLE
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
        Device::new(HidapiDevice::new(dev), info.clone())
    }

    /// Open the first enumerated device (convenience).
    pub fn open_first(&mut self) -> Result<Device> {
        let infos = self.enumerate()?;
        let first = infos.first().ok_or(Error::NoDevice)?;
        self.open(first)
    }
}
