//! `virt-out` — the output HAL (PLAN §2). Emits virtual mouse/keyboard/gamepad input
//! to the OS. Platform code sits behind the backend module; the public API is a
//! platform-agnostic [`Sink`] plus the output vocabulary in [`OutputEvent`]. Sync.
//!
//! Linux backend only for now (uinput via `evdev`), so the crate is Linux-gated.
//! Windows (SendInput + ViGEm) / macOS come later (PLAN §2). See PLAN §2.1.

#![cfg(target_os = "linux")]

mod event;
mod linux;

pub use event::{GamepadAxis, GamepadButton, Key, MouseButton, OutputEvent, Rumble};
pub use linux::Sink;

/// `virt-out` result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors from creating or driving the virtual devices.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A uinput / evdev I/O error (often `/dev/uinput` permissions — see the udev rule).
    #[error("uinput I/O error: {0}")]
    Io(#[from] std::io::Error),
}
