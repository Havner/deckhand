//! Error and result types for the output HAL (PLAN §2).

/// `virt-out` result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors from creating or driving the virtual devices.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A platform I/O error (Linux: uinput/`/dev/uinput` perms — see the udev rule;
    /// Windows: a failed `SendInput`).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A ViGEmBus error (Windows virtual-gamepad backend) — e.g. the driver isn't
    /// installed, or the target couldn't be plugged in. Only present with the `vigem` feature.
    #[cfg(all(target_os = "windows", feature = "vigem"))]
    #[error("ViGEm error: {0}")]
    Vigem(#[from] vigem_client::Error),

    /// A VIIPER error (Windows virtual-gamepad backend over USB/IP) — e.g. the server isn't
    /// reachable, auth failed, or the device couldn't be created. Only present with the
    /// `viiper` feature.
    #[cfg(all(target_os = "windows", feature = "viiper"))]
    #[error("VIIPER error: {0}")]
    Viiper(#[from] viiper_client::ViiperError),
}
