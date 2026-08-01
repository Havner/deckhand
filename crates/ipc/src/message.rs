//! The control-plane messages (PLAN §4.4). A client sends a [`Request`]; the daemon replies with
//! a [`Response`]. After a [`Request::Subscribe`], the daemon instead streams [`Event`]s on that
//! connection. All are serialized with `postcard` (see [`crate::codec`]).
//!
//! These are the wire vocabulary — deliberately *not* the engine's own types (the crate never
//! depends on `engine`). Selection specs travel as strings the daemon parses (the same grammar as
//! its `-i`/`-o` CLI); config travels as [`config::ConfigDoc`] (the daemon compiles it).

use config::{ConfigDoc, GlobalConfig};
use serde::{Deserialize, Serialize};

/// Which profile role a config applies to (wire mirror of the engine's `Role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileRole {
    Main,
    Fallback,
}

/// A request from a client to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Request {
    /// Apply a profile to a role. The daemon **compiles** the shipped `ConfigDoc`; on failure it
    /// replies [`Response::Diagnostics`] (not applied), on success [`Response::Ok`]. Boxed to keep
    /// the enum small.
    Apply { role: ProfileRole, config: Box<ConfigDoc> },
    /// Replace the global config (rumble master, chords, boot role, …).
    SetGlobals(Box<GlobalConfig>),
    /// Stage the input source — spec string `dongle|wired|<device-id>|host:port` (the daemon
    /// parses it, same grammar as `-i`). Applied at the next `Start`.
    SetInput(String),
    /// Stage the output sink — spec string `local|host:port`. Applied at the next `Start`.
    SetOutput(String),
    /// Acquire hardware and run the mapping loop.
    Start,
    /// Halt the loop and release hardware; config is retained.
    Stop,
    /// Full teardown — the daemon exits.
    Shutdown,
    /// List the currently-enumerated devices.
    ListDevices,
    /// The current engine status.
    Status,
    /// Turn this connection into an event stream: the daemon then pushes [`Event`]s until the
    /// connection closes.
    Subscribe,
}

/// A reply from the daemon to a [`Request`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Response {
    /// The request succeeded with no payload.
    Ok,
    /// The request failed (a human-readable reason — bad spec, not ready, I/O, …).
    Error(String),
    /// Compile diagnostics for an [`Request::Apply`] that was **rejected** (errors present, not
    /// applied); each string is severity-prefixed (`error: …` / `warning: …`).
    Diagnostics(Vec<String>),
    /// The enumerated devices as their stable **`DeviceId` strings** (reply to
    /// [`Request::ListDevices`]) — each is both the display label (`gordon:dongle:1:` is
    /// self-describing) and the token to pass back as `SetInput`/select-device (PLAN §4.3).
    Devices(Vec<String>),
    /// Engine status (reply to [`Request::Status`]).
    Status(StatusInfo),
}

/// The engine's run state (wire mirror; `WaitingForDevice` lands with D4/D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RunState {
    Idle,
    Running,
    WaitingForDevice,
}

/// A snapshot of the daemon's engine (reply to [`Request::Status`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub state: RunState,
    /// The staged input spec — always set (defaults to `auto`, matching the engine's own default).
    pub input: String,
    /// The staged output spec — always set (defaults to `local`).
    pub output: String,
    pub has_main: bool,
    pub has_fallback: bool,
}

/// An asynchronous event pushed to a subscribed connection (PLAN §4.3, D7). `#[non_exhaustive]` so
/// a future native device-hotplug push (udev / `WM_DEVICECHANGE`) can add `DeviceAdded`/`Removed`
/// without breaking clients — those were dropped as YAGNI (on-demand `list-devices` + a UI refresh
/// cover topology; see §4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Event {
    ControllerConnected,
    ControllerDisconnected,
    /// Battery percentage (wireless only; `None` when unknown).
    Battery { percent: Option<u8> },
    /// The bound device's transport went away (engine now `WaitingForDevice`).
    BindingLost,
    /// A device was (re)acquired, by id.
    BindingAcquired(String),
    /// The run state changed.
    State(RunState),
}
