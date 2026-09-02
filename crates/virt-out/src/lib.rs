//! `virt-out` - the output HAL (PLAN 2). Emits virtual mouse/keyboard/gamepad input
//! to the OS. Platform code sits behind the backend module; the public API is a
//! platform-agnostic [`Sink`] plus the output vocabulary in [`OutputEvent`]. Sync.
//!
//! Backends: Linux uinput via `evdev`; Windows `SendInput` (kb/mouse) + ViGEm (virtual
//! Xbox 360 pad). macOS comes later (PLAN 2). See PLAN 2.1.

mod error;
mod gamepad;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Sink;

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use win::Sink;

pub use error::{Error, Result};
pub use gamepad::Rumble;
// Re-export the shared output vocabulary so consumers keep using `virt_out::Key` etc.
pub use vocab_out::{GamepadAxis, GamepadButton, Key, MouseButton, OutputEvent};
