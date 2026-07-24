//! Internal HID backend abstraction (PLAN §1.6).
//!
//! Kept behind this trait so the `hidapi` backend can be swapped for a raw
//! `hidraw`/`nusb` Linux backend later without touching the public API. Not
//! exported.

use crate::error::Result;

/// The minimal raw-HID surface `Device` needs: timed reads + feature reports.
///
/// `Send` so a `Device` can be moved onto a worker thread (PLAN §1.6).
pub(crate) trait RawHid: Send {
    /// Read one input report, waiting up to `timeout_ms` (0 returned on timeout).
    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize>;
    /// Send an already-framed feature report (report-ID byte included).
    fn send_feature_report(&self, data: &[u8]) -> Result<()>;
    /// Get a feature report; `buf[0]` should carry the report id on entry.
    fn get_feature_report(&self, buf: &mut [u8]) -> Result<usize>;
}

/// `hidapi`-backed implementation.
pub(crate) struct HidapiDevice {
    dev: hidapi::HidDevice,
}

impl HidapiDevice {
    pub(crate) fn new(dev: hidapi::HidDevice) -> Self {
        HidapiDevice { dev }
    }
}

impl RawHid for HidapiDevice {
    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize> {
        Ok(self.dev.read_timeout(buf, timeout_ms)?)
    }

    fn send_feature_report(&self, data: &[u8]) -> Result<()> {
        self.dev.send_feature_report(data)?;
        Ok(())
    }

    fn get_feature_report(&self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.dev.get_feature_report(buf)?)
    }
}
