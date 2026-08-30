//! `steam-hid` — read input from and send commands to Steam controllers over HID.
//!
//! Input layer (HAL) for the `deckhand` mapper: discovers and opens Steam
//! controllers (original Steam Controller / Steam Deck), decodes their full input
//! state, and sends commands (lizard mode, IMU, haptics, LED, power off, …).
//!
//! The design this implements is documented in `PLAN.md` §1. Field offsets and
//! command layouts are taken from that reference and are **unverified on hardware**
//! (PLAN §1.9); development proceeds Gordon-first, so the Steam Deck (Neptune)
//! input path is currently stubbed.
//!
//! # Model at a glance
//! - Every input frame is a **full snapshot**; the read stream also carries
//!   lifecycle frames (connect/disconnect/battery). One read → one [`RawReport`]
//!   (low level) or [`Report`] (unified), never a bare state.
//! - Snapshots come from [`Device::read`]/[`Device::poll`]; [`Device::events`] is
//!   a change-driven view over the same stream; [`ControllerState::diff`] is the
//!   stateless primitive underneath it.

mod backend;
mod buttons;
mod command;
mod device;
mod error;
mod event;
mod protocol;
mod report;
mod state;
mod value;

pub use buttons::{Axis, Button, Buttons, GordonButtons, NeptuneButtons, TritonButtons, button_flag};
pub use command::{HapticPulse, HapticStyle, ImuMode, Motor};
pub use protocol::{ACCEL_RES_PER_G, GYRO_RES_PER_DPS, HapticIntensity, HapticType};
pub use device::{Device, DeviceId, DeviceInfo, DeviceKind, Manager, Transport};
pub use error::{Error, Result};
pub use event::{Event, Events};
pub use report::{BatteryRaw, GordonReport, NeptuneReport, RawReport, TritonReport};
pub use state::{Battery, ControllerState, Report};
pub use value::{Quati, Timestamp, TrackPad, Vec2, Vec2i, Vec3i};
