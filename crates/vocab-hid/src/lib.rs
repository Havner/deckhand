//! `vocab-hid` - the hardware-independent **input vocabulary**, shared by `config` (which names
//! controller buttons in chords/gaters) and `steam-hid` (which produces the snapshot from the
//! per-device wire bitfields). The input counterpart of `vocab-out` (outputs).
//!
//! Split into two modules purely for clarity about who needs what (everything is re-exported flat
//! at the crate root, so consumers see one namespace):
//! - [`core`] - the [`Button`] enum. **What `config` needs**: naming binding targets, nothing else
//!   pulled in. Dependency-light (just the enum + optional serde).
//! - [`state`] - the [`Buttons`] bitfield, [`Axis`], the value types, and the [`ControllerState`]
//!   snapshot. **What the mapper additionally needs**: the per-frame input it reads (produced by
//!   `steam-hid`). This is what pulls in `bitflags`.

mod core;
mod state;

// core: the config-facing vocabulary.
pub use core::Button;
// state: the mapper-facing snapshot vocabulary.
pub use state::{
    ACCEL_RES_PER_G, Axis, Buttons, ControllerState, GYRO_RES_PER_DPS, Quati, Timestamp, TrackPad,
    Vec2, Vec2i, Vec3i, button_flag,
};
