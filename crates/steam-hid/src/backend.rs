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
    /// Send an **output** report (`data[0]` = report id). Triton drives haptics this way (its
    /// `0x80`–`0x85` output reports) rather than via feature reports.
    fn send_output_report(&self, data: &[u8]) -> Result<()>;
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
        match self.dev.read_timeout(buf, timeout_ms) {
            // A signal (e.g. SIGINT from Ctrl-C) can interrupt the blocking wait
            // with EINTR. hidapi doesn't expose errno, so we match its strerror
            // text; treat it like a timeout (nothing this cycle → `Ok(0)`) so the
            // caller's read loop re-checks its run flag rather than failing — this
            // is what makes cooperative shutdown work (PLAN §1.6). Without it, a
            // Ctrl-C mid-read surfaces as a hard error on Linux (Windows never
            // interrupts the read this way).
            Err(hidapi::HidError::HidApiError { message })
                if message.contains("Interrupted system call") => Ok(0),
            other => Ok(other?),
        }
    }

    fn send_feature_report(&self, data: &[u8]) -> Result<()> {
        self.dev.send_feature_report(data)?;
        Ok(())
    }

    fn send_output_report(&self, data: &[u8]) -> Result<()> {
        self.dev.write(data)?;
        Ok(())
    }

    fn get_feature_report(&self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.dev.get_feature_report(buf)?)
    }
}
