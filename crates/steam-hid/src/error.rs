//! Error and result types (PLAN §1.5).

use thiserror::Error;

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors surfaced by `steam-hid`.
///
/// Note the two things that are deliberately **not** errors (PLAN §1.5):
/// - a read timeout is `Ok(None)` from the `poll_*` methods, not an error;
/// - a controller disconnecting while the transport stays alive is a
///   [`Report::Disconnected`](crate::Report) value, not an error.
///
/// A transport that genuinely goes away (wired unplug, dongle removed) *is*
/// [`Error::Disconnected`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Underlying HID backend failure.
    #[error("HID backend error: {0}")]
    Hid(#[from] hidapi::HidError),

    /// The transport endpoint went away (unplug / dongle removed).
    #[error("device transport disconnected")]
    Disconnected,

    /// A device we do not recognize / support.
    #[error("unsupported device (vid={vid:#06x}, pid={pid:#06x})")]
    UnsupportedDevice { vid: u16, pid: u16 },

    /// A report shorter than expected, or otherwise malformed.
    #[error("malformed report: expected {expected} bytes, got {got}")]
    ShortReport { expected: usize, got: usize },

    /// No matching Steam device was found during enumeration/open.
    #[error("no matching Steam device found")]
    NoDevice,
}
