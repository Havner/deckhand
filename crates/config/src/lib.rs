//! `config` — the deckhand mapping **configuration** (PLAN §3).
//!
//! This crate owns the *data* half of the mapper: a declarative, feature-rich model of
//! all bindings (`ConfigDoc`), its RON (de)serialization, and validation. It is
//! deliberately **pure data** — no platform backends, no runtime — so the UI can depend
//! on it without pulling in `steam-hid`/`virt-out`/timers.
//!
//! The design mirrors Steam Input's paradigm (action sets, layers, per-source behaviors,
//! activators/commands, actions) — see PLAN §3 rounds A–E. Compilation of a `ConfigDoc`
//! into the engine's flat, index-based `Program` is **deferred** and co-designed with the
//! engine (it's the engine's input contract); this crate stops at the model + validation.
//!
//! Layers of the design that live elsewhere: the **output** vocabulary (`Key`,
//! `MouseButton`, `GamepadButton`, `GamepadAxis`) is the shared [`vocab`] crate; the
//! **input** vocabulary ([`InputSource`]) is our own, hardware-independent.

mod action;
mod binding;
mod chords;
mod command;
mod device;
mod input;
mod profile;
mod settings;
mod validate;

pub use action::{Action, ActionSetRef, LayerRef};
pub use binding::SourceBinding;
pub use chords::{Chord, ChordAction, Chords, SwitchMode};
pub use device::DeviceConfig;
pub use profile::{ActionSet, ConfigDoc, Layer, RumbleSettings};
pub use command::{
    Activator, Command, CommandSettings, HapticEdge, HapticStrength, Haptics, Turbo,
};
pub use input::{InputSource, Shape, Side, SourceKind};
pub use settings::{
    Acceleration, Activation, ActivationMode, AntiDeadzone, AsMouseSettings, Curve, Deadzone,
    DirectionalPadSettings, DpadLayout, GyroSpace, GyroToMouseSettings, Invert, JoystickMouseSettings,
    JoystickSettings, MouseOutput, OneEuroFilter, OuterRing, Rotation, Sensitivity, SoftPull,
    StickOutput, TriggerOutput, TriggerSettings,
};
pub use validate::{Diagnostic, Severity};
