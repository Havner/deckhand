//! `virt-out` — the output HAL (PLAN §2). Emits virtual mouse/keyboard/gamepad input
//! to the OS. Platform code sits behind the backend module; the public API is a
//! platform-agnostic [`Sink`] plus the output vocabulary in [`OutputEvent`]. Sync.
//!
//! Backends: Linux uinput via `evdev`; Windows `SendInput` (kb/mouse) + ViGEm (virtual
//! Xbox 360 pad). macOS comes later (PLAN §2). See PLAN §2.1.

mod event;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Sink;

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use win::Sink;

pub use event::{OutputEvent, Rumble};
// Re-export the shared output vocabulary so consumers keep using `virt_out::Key` etc.
pub use vocab::{GamepadAxis, GamepadButton, Key, MouseButton};

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
}
