//! `steam-hid` - read input from and send commands to Steam controllers over HID.
//!
//! Input layer (HAL) for the `deckhand` mapper: discovers and opens Steam
//! controllers (original Steam Controller / Steam Deck), decodes their full input
//! state, and sends commands (lizard mode, IMU, haptics, LED, power off, ...).
//!
//! The design this implements is documented in `PLAN.md` 1. Field offsets and
//! command layouts are taken from that reference and are **unverified on hardware**
//! (PLAN 1.9); development proceeds Gordon-first, so the Steam Deck (Neptune)
//! input path is currently stubbed.
//!
//! # Model at a glance
//! - Every input frame is a **full snapshot**; the read stream also carries
//!   lifecycle frames (connect/disconnect/battery). One read -> one [`Report`]:
//!   [`Report::State`] (a unified [`ControllerState`]) or a lifecycle signal,
//!   never a bare state. The wire packet decodes straight to it (in `state`).
//! - Snapshots come from [`Device::read`]/[`Device::poll`]; [`Device::events`] is
//!   a change-driven view over the same stream; [`diff`] is the
//!   stateless primitive underneath it.

mod backend;
mod device;
mod error;
mod event;
mod info;
mod protocol;
mod report;

// The input vocabulary (`Buttons`/`ControllerState`/value types/IMU scales) now lives in `vocab-hid`
// so the mapper can consume it without a HAL dep; `steam-hid` produces it. Re-exported so this
// crate's own consumers keep using `steam_hid::ControllerState`, `::Buttons`, etc.
pub use vocab_hid::{
    ACCEL_RES_PER_G, Axis, Button, Buttons, ControllerState, GYRO_RES_PER_DPS, Quati, Timestamp,
    TrackPad, Vec2, Vec2i, Vec3i, button_flag,
};
pub use protocol::{
    ControllerStringAttributes, GordonButtons, GyroMode, HapticIntensity, HapticPosition,
    HapticSide, HapticStyle, HapticType, NeptuneButtons, TritonButtons,
};
pub use backend::Manager;
pub use device::{Device, HapticPulse};
pub use info::{DeviceId, DeviceInfo, DeviceKind, Transport};
pub use error::{Error, Result};
pub use event::{Event, Events, diff};
pub use report::{Battery, Report};
