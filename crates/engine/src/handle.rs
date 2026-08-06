//! The `Engine` control API — one owned handle over the runtime (PLAN §4.1, §4.2 S10).
//!
//! Deliberately narrow and transport-agnostic: the same calls work embedded (direct) or, later,
//! behind a socket/daemon. **No profile concept** — just program(s) + globals. `new()` is idle
//! and acquires **no** hardware (HW errors surface at `start()`); `start()`/`stop()` acquire and
//! release the device + sink while **config is retained** across the pair; `shutdown()` consumes
//! the handle.
//!
//! **Mutability:** `apply`/`set_globals` are live hot-swaps while running (and stage while idle);
//! `set_input`/`set_output` are **staged-only** — they take effect at the next `start()` and are
//! fixed within a start/stop pair. The network variants of input/output are stubs for now
//! (Local-only built; the seam is kept — PLAN §4.1/§6).

use config::GlobalConfig;
use steam_hid::{Device, DeviceId, DeviceInfo, Manager, RawReport, Transport};
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use virt_out::Sink;

use crate::event::{EngineEvent, EventSink, EventStream};
use crate::program::{Program, Role};
use crate::runtime::{Control, DeviceCfg, Runtime};
use crate::{Error, Result};

/// Where input comes from: a local controller, or a bound network endpoint that receives a remote
/// controller's frames (the server role — PLAN §6).
#[derive(Debug, Clone)]
pub enum Input {
    Local(DeviceSelect),
    /// Bind `host:port` and map frames arriving from a remote client (server/PC role).
    Network(SocketAddr),
}

/// How to pick the local controller.
#[derive(Debug, Clone)]
pub enum DeviceSelect {
    /// The first controller that streams frames (mirrors the examples/bridge).
    Auto,
    /// Restrict to a transport (wired vs dongle).
    Transport(Transport),
    /// A specific device, pinned by its stable [`DeviceId`] (resolved to a fresh path at each
    /// `start()`, so it survives the OS path changing across replug — PLAN §4.3).
    Explicit(DeviceId),
}

/// Where output goes: the local virtual devices, or a network endpoint we forward the controller's
/// frames to (the client/forwarder role — PLAN §6).
#[derive(Debug, Clone)]
pub enum Output {
    Local,
    /// Dial `host:port` and forward this machine's controller frames there (client/Deck role).
    Network(SocketAddr),
}

// --- string conversions -------------------------------------------------------------------
//
// `Display` and `FromStr` round-trip: `Display` emits exactly what `FromStr` accepts, so a
// staged selection can be reported (e.g. over the control socket) and passed straight back to
// `set_input`/`set_output`. `Transport`/`DeviceId` already round-trip; this extends the same
// contract up to the whole `Input`/`Output`, ready for the `Network` variants (PLAN §6).

impl fmt::Display for DeviceSelect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceSelect::Auto => f.write_str("auto"),
            DeviceSelect::Transport(t) => f.write_str(t.as_str()),
            DeviceSelect::Explicit(id) => write!(f, "{id}"),
        }
    }
}

impl FromStr for DeviceSelect {
    type Err = String;

    /// Most specific first: a full [`DeviceId`], then a transport token, then `auto`. The short
    /// aliases `a`/`d`/`w` are accepted as a convenience; `Display` emits the canonical token.
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        if let Ok(id) = s.parse::<DeviceId>() {
            return Ok(DeviceSelect::Explicit(id));
        }
        match s {
            "auto" | "a" => Ok(DeviceSelect::Auto),
            "dongle" | "d" => Ok(DeviceSelect::Transport(Transport::UsbDongle)),
            "wired" | "w" => Ok(DeviceSelect::Transport(Transport::UsbWired)),
            _ => Err(format!("unrecognized input '{s}' (want auto|dongle|wired|<id>)")),
        }
    }
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::Local(sel) => write!(f, "{sel}"),
            Input::Network(addr) => write!(f, "{addr}"),
        }
    }
}

impl FromStr for Input {
    type Err = String;

    /// A local selection (see [`DeviceSelect`]) first, else a `host:port` bind address (server role).
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        if let Ok(sel) = s.parse::<DeviceSelect>() {
            return Ok(Input::Local(sel));
        }
        if let Ok(addr) = s.parse::<SocketAddr>() {
            return Ok(Input::Network(addr));
        }
        Err(format!("unrecognized input '{s}' (want auto|dongle|wired|<id>|host:port)"))
    }
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Output::Local => f.write_str("local"),
            Output::Network(addr) => write!(f, "{addr}"),
        }
    }
}

impl FromStr for Output {
    type Err = String;

    /// `local`, or a `host:port` server address to forward frames to (client role).
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "local" | "l" => Ok(Output::Local),
            other => other
                .parse::<SocketAddr>()
                .map(Output::Network)
                .map_err(|_| format!("unrecognized output '{other}' (want local|host:port)")),
        }
    }
}

/// The engine's run state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No loop running (freshly constructed, or stopped). No hardware held.
    Idle,
    /// Running the mapping loop with a bound device.
    Running,
    /// Running, but the bound device's transport went away — waiting to reacquire it. The
    /// transition is wired in D5 (PLAN §4.3); the variant is defined here (D4).
    WaitingForDevice,
}

/// A point-in-time snapshot of the engine: run state, the staged input/output (**typed** — each
/// round-trips through its `Display`/`FromStr`), and the loaded program names. This is the engine's
/// side of the status contract; the transport-agnostic wire mirror is `ipc::StatusSnapshot` (the
/// daemon maps one to the other, stringifying `input`/`output`). Read atomically via
/// [`Engine::status`] rather than field-by-field so the two APIs stay in lock-step.
#[derive(Debug, Clone)]
pub struct StatusInfo {
    pub state: Status,
    /// The staged input source (always set; defaults to `auto`).
    pub input: Input,
    /// The staged output target (always set; defaults to `local`).
    pub output: Output,
    /// Name of the loaded **Main** program, or `None` if none is applied.
    pub main: Option<String>,
    /// Name of the loaded **Fallback** program, or `None`.
    pub fallback: Option<String>,
    /// The **bound** device — the id the reader resolved at `start()` and (across an outage) keeps
    /// reacquiring — or `None` when idle. Unlike `input` (the *staged* selection, which may be a
    /// policy like `auto`/`dongle`), this is the concrete device actually in use, so a UI that
    /// connects to an already-running daemon learns what's bound. Stays set through
    /// `WaitingForDevice` (the device it's waiting to reacquire); cleared on `stop()`.
    pub bound: Option<DeviceId>,
    /// The full global config (master rumble, chords, device toggles, `start_profile`). Included
    /// whole so a client connecting to a running daemon can seed its complete view in one call;
    /// subsequent changes arrive as `GlobalConfigSet` events. (`start_profile` here is the boot
    /// setting, read once at `start()` — not the currently-live role.)
    pub globals: GlobalConfig,
}

/// The engine handle. Holds the staged input/output, the program(s) + globals (retained across
/// start/stop), and the live [`Runtime`] while running.
pub struct Engine {
    manager: Option<Manager>,
    input: Input,
    output: Output,
    main: Option<Program>,
    fallback: Option<Program>,
    globals: GlobalConfig,
    runtime: Option<Runtime>,
    /// The concrete device the running loop is bound to (the reader's pinned id), or `None` when
    /// idle. Captured at `start()` and cleared at `stop()` — see [`StatusInfo::bound`]. Held here
    /// because the id itself lives across the thread boundary in the reader; the handle keeps a copy
    /// so `status()` can report it without a round-trip.
    bound: Option<DeviceId>,
    /// Broadcasts engine events to subscribers; shared with the reader/mapping threads (D7).
    events: EventSink,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// A fresh idle engine. Acquires no hardware — construction always succeeds.
    pub fn new() -> Engine {
        Engine {
            manager: None,
            input: Input::Local(DeviceSelect::Auto),
            output: Output::Local,
            main: None,
            fallback: None,
            globals: GlobalConfig::default(),
            runtime: None,
            bound: None,
            events: EventSink::default(),
        }
    }

    /// Subscribe to the engine's event stream (D7). Each call returns an independent [`EventStream`]
    /// that receives every *subsequent* [`EngineEvent`] until it is dropped.
    pub fn subscribe(&self) -> EventStream {
        self.events.subscribe()
    }

    // --- staging (take effect at the next start) ---------------------------------------

    /// Stage the input source (Local only for now). Applied at the next `start()`.
    pub fn set_input(&mut self, input: Input) {
        log::debug!("set_input: {input} (staged for next start)");
        self.input = input;
        self.events.emit(EngineEvent::InputStaged(self.input.clone()));
    }

    /// Stage the output target (Local only for now). Applied at the next `start()`.
    pub fn set_output(&mut self, output: Output) {
        log::debug!("set_output: {output} (staged for next start)");
        self.output = output;
        self.events.emit(EngineEvent::OutputStaged(self.output.clone()));
    }

    // --- live-or-staged config ---------------------------------------------------------

    /// Apply a program to a role. Retained (survives stop/start); hot-swapped live if running.
    pub fn apply(&mut self, program: Program, role: Role) {
        let mode = if self.runtime.is_some() { "live hot-swap" } else { "staged" };
        log::info!("apply: program '{}' → {role:?} ({mode})", program.meta.name);
        let name = program.meta.name.clone();
        match role {
            Role::Main => self.main = Some(program.clone()),
            Role::Fallback => self.fallback = Some(program.clone()),
        }
        if let Some(rt) = &self.runtime {
            let _ = rt.control().send(Control::Apply { program: Box::new(program), role: role.clone() });
        }
        self.events.emit(EngineEvent::ProfileSet { role, name: Some(name) });
    }

    /// Set the global config (master rumble + chords). Retained; hot-swapped live if running.
    pub fn set_globals(&mut self, globals: GlobalConfig) {
        let mode = if self.runtime.is_some() { "live" } else { "staged" };
        log::info!(
            "set_globals: master_rumble={}%, {} chord(s) ({mode})",
            globals.master_rumble,
            globals.chords.len(),
        );
        self.globals = globals.clone();
        if let Some(rt) = &self.runtime {
            let _ = rt.control().send(Control::SetGlobals(Box::new(globals)));
        }
        self.events.emit(EngineEvent::GlobalConfigSet(self.globals.clone()));
    }

    // --- lifecycle ---------------------------------------------------------------------

    /// Acquire hardware and start the mapping loop, in the role chosen by the staged input/output
    /// (PLAN §6): `Local`/`Local` maps here; `output=Network` forwards this device's frames to a
    /// server; `input=Network` maps a remote client's frames to the local sink. A no-op if already
    /// running.
    pub fn start(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Ok(());
        }
        match (self.input.clone(), self.output.clone()) {
            (Input::Local(_), Output::Local) => self.start_local(),
            (Input::Local(_), Output::Network(addr)) => self.start_forwarder(addr),
            (Input::Network(addr), Output::Local) => self.start_server(addr),
            (Input::Network(_), Output::Network(_)) => {
                Err(Error::NotReady("input and output cannot both be network"))
            }
        }
    }

    /// `Local`/`Local`: open the device + create the sink and map here (the original behaviour).
    fn start_local(&mut self) -> Result<()> {
        let main = self.main.clone().ok_or(Error::NotReady("no main program applied"))?;
        let device = self.open_device()?;
        let cfg = DeviceCfg::for_device(&device.info().kind, &self.globals);
        // Pin the resolved device's stable id so the reader reacquires *this* device if its
        // transport drops (D6), regardless of how the selection policy chose it.
        let pinned_id = device.info().id();
        {
            let info = device.info();
            let fb = self
                .fallback
                .as_ref()
                .map(|f| format!(", fallback '{}'", f.meta.name))
                .unwrap_or_default();
            log::info!(
                "starting: {:?} via {:?}, main '{}'{fb}",
                info.kind,
                info.transport,
                main.meta.name,
            );
        }
        // Remember the concrete bound device so `status()` can report it (the id itself moves into
        // the reader below), and announce the bind. `BindingAcquired` fires on *every* bind — here
        // for the initial one and in the reader for a reacquire (D6) — so a subscriber never has to
        // special-case the first; `bound` seeds the same fact for a client that connects afterwards.
        self.bound = Some(pinned_id.clone());
        let sink = Sink::new()?;
        self.runtime = Some(Runtime::start_local(
            device,
            pinned_id.clone(),
            cfg,
            sink,
            main,
            self.fallback.clone(),
            self.globals.clone(),
            self.events.clone(),
        ));
        self.events.emit(EngineEvent::BindingAcquired(pinned_id));
        self.events.emit(EngineEvent::State(Status::Running));
        Ok(())
    }

    /// `output=Network` (client/forwarder): open the device and forward its frames to the server at
    /// `addr`. **No main program is required** — the server maps, with its own config or the config
    /// we push here on connect (config is ordinary `Apply`/`SetGlobals`, PLAN §6.1).
    fn start_forwarder(&mut self, addr: SocketAddr) -> Result<()> {
        let device = self.open_device()?;
        let cfg = DeviceCfg::for_device(&device.info().kind, &self.globals);
        let pinned_id = device.info().id();
        {
            let info = device.info();
            log::info!("starting (forwarder): {:?} via {:?} → {addr}", info.kind, info.transport);
        }
        let rt = Runtime::start_client(device, pinned_id.clone(), cfg, addr, self.events.clone())?;
        self.bound = Some(pinned_id.clone());
        self.runtime = Some(rt);
        self.push_staged_config(); // seed the server with our staged programs + globals
        self.events.emit(EngineEvent::BindingAcquired(pinned_id));
        self.events.emit(EngineEvent::State(Status::Running));
        Ok(())
    }

    /// `input=Network` (server): bind `addr` and map a remote client's frames to the local sink.
    /// Requires a main program (its own config); the client may push more over the wire.
    fn start_server(&mut self, addr: SocketAddr) -> Result<()> {
        let main = self.main.clone().ok_or(Error::NotReady("no main program applied"))?;
        let sink = Sink::new()?;
        log::info!("starting (server): binding {addr}, main '{}'", main.meta.name);
        let rt = Runtime::start_server(
            sink,
            main,
            self.fallback.clone(),
            self.globals.clone(),
            addr,
            self.events.clone(),
        )?;
        self.runtime = Some(rt);
        // No local device → no `bound` and no `BindingAcquired` (the "binding" is the network link,
        // surfaced in slice 5).
        self.events.emit(EngineEvent::State(Status::Running));
        Ok(())
    }

    /// Ship the currently-staged programs + globals to a running (network) runtime as ordinary
    /// control messages — the forwarder uses this to seed the server on connect.
    fn push_staged_config(&self) {
        let Some(rt) = &self.runtime else { return };
        if let Some(m) = &self.main {
            let _ = rt
                .control()
                .send(Control::Apply { program: Box::new(m.clone()), role: Role::Main });
        }
        if let Some(f) = &self.fallback {
            let _ = rt
                .control()
                .send(Control::Apply { program: Box::new(f.clone()), role: Role::Fallback });
        }
        let _ = rt.control().send(Control::SetGlobals(Box::new(self.globals.clone())));
    }

    /// Halt the loop and release hardware (device → lizard restored, virtual pad unplugged).
    /// Config is retained; `start()` resumes. A no-op if idle.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut rt) = self.runtime.take() {
            log::info!("stopping: releasing device (→ lizard) and virtual pad");
            rt.stop()?;
            self.bound = None;
            self.events.emit(EngineEvent::State(Status::Idle));
        }
        Ok(())
    }

    /// A point-in-time [`StatusInfo`] snapshot: run state, staged input/output, and the loaded
    /// program names, read together. Run state is `Idle` when no loop is up, `WaitingForDevice`
    /// while the loop runs but the bound device's transport is gone (D5), else `Running`. The
    /// staged input/output are typed and round-trip through their `Display`/`FromStr` form, so a
    /// caller can render them and pass the same string back to `set_input`/`set_output`.
    pub fn status(&self) -> StatusInfo {
        let state = match &self.runtime {
            None => Status::Idle,
            Some(rt) if rt.is_waiting() => Status::WaitingForDevice,
            Some(_) => Status::Running,
        };
        StatusInfo {
            state,
            input: self.input.clone(),
            output: self.output.clone(),
            main: self.main.as_ref().map(|p| p.meta.name.clone()),
            fallback: self.fallback.as_ref().map(|p| p.meta.name.clone()),
            bound: self.bound.clone(),
            globals: self.globals.clone(),
        }
    }

    /// Enumerate the attached controllers (any time — no HW is retained).
    pub fn devices(&mut self) -> Result<Vec<DeviceInfo>> {
        self.ensure_manager()?;
        Ok(self.manager.as_mut().unwrap().enumerate()?)
    }

    /// Full teardown: stop if running, then consume the handle.
    pub fn shutdown(mut self) -> Result<()> {
        self.stop()
    }

    // --- internals ---------------------------------------------------------------------

    fn ensure_manager(&mut self) -> Result<()> {
        if self.manager.is_none() {
            self.manager = Some(Manager::new()?);
        }
        Ok(())
    }

    /// Open the staged local device, mirroring the examples' selection: a single/explicit
    /// candidate is opened directly (wired is idle until moved), multiple dongle slots are polled
    /// to find the one that streams.
    fn open_device(&mut self) -> Result<Device> {
        self.ensure_manager()?;
        let manager = self.manager.as_mut().unwrap();
        // Only the local + forwarder roles open a device, and both stage `Input::Local`.
        let Input::Local(select) = &self.input else {
            return Err(Error::NotReady("open_device requires a local input"));
        };

        if let DeviceSelect::Explicit(id) = select {
            // Resolve the pinned DeviceId against the *current* enumeration (the OS path may
            // have changed since it was chosen — PLAN §4.3), then open it directly (a specific
            // device is idle until moved, so no frame gate).
            let infos = manager.enumerate()?;
            let info = infos
                .iter()
                .find(|i| &i.id() == id)
                .ok_or(Error::NotReady("selected device not present"))?;
            return Ok(manager.open(info)?);
        }
        let want: Option<&Transport> = match select {
            DeviceSelect::Transport(t) => Some(t),
            _ => None,
        };

        let infos = manager.enumerate()?;
        let candidates: Vec<&DeviceInfo> =
            infos.iter().filter(|i| want.is_none_or(|t| &i.transport == t)).collect();
        match candidates.as_slice() {
            [] => Err(Error::NotReady("no matching controller found")),
            [info] => Ok(manager.open(info)?),
            many => {
                for info in many {
                    let mut device = manager.open(info)?;
                    for _ in 0..8 {
                        if matches!(
                            device.poll_raw(Duration::from_millis(200))?,
                            Some(RawReport::Gordon(_) | RawReport::Neptune(_) | RawReport::Connected)
                        ) {
                            return Ok(device);
                        }
                    }
                }
                Err(Error::NotReady("no matching controller streamed"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample profile (`examples/test_profile.ron`) must parse and compile — keeps it valid
    /// as the config model evolves (exercises a layer, a HoldLayer action, gyro invert, and every
    /// behavior kind).
    #[test]
    fn test_profile_compiles() {
        let ron = include_str!("../examples/test_profile.ron");
        let doc: config::ConfigDoc = ron::from_str(ron).expect("parse test_profile.ron");
        crate::compile(&doc).expect("compile test_profile.ron");
    }

    #[test]
    fn input_parses_transports_and_ids() {
        assert!(matches!(
            "dongle".parse::<Input>(),
            Ok(Input::Local(DeviceSelect::Transport(Transport::UsbDongle)))
        ));
        assert!(matches!("a".parse::<Input>(), Ok(Input::Local(DeviceSelect::Auto))));
        assert!(matches!(
            "gordon:dongle:1:".parse::<Input>(),
            Ok(Input::Local(DeviceSelect::Explicit(_)))
        ));
    }

    #[test]
    fn input_parses_network_and_rejects_garbage() {
        assert!(matches!("192.168.0.5:9000".parse::<Input>(), Ok(Input::Network(_))));
        assert!("wat".parse::<Input>().unwrap_err().contains("unrecognized"));
    }

    #[test]
    fn output_parses_local_and_network() {
        assert!(matches!("local".parse::<Output>(), Ok(Output::Local)));
        assert!(matches!("127.0.0.1:9000".parse::<Output>(), Ok(Output::Network(_))));
        assert!("wat".parse::<Output>().unwrap_err().contains("unrecognized"));
    }

    /// `Display` emits what `FromStr` accepts, for every `Input`/`Output` the daemon reports.
    #[test]
    fn input_output_round_trip() {
        for spec in
            ["auto", "dongle", "wired", "gordon:dongle:1:", "gordon:wired:2:ABC", "127.0.0.1:9000"]
        {
            let parsed: Input = spec.parse().unwrap();
            assert_eq!(parsed.to_string().parse::<Input>().unwrap().to_string(), parsed.to_string());
            assert_eq!(parsed.to_string(), spec);
        }
        for spec in ["local", "127.0.0.1:9000"] {
            let parsed: Output = spec.parse().unwrap();
            assert_eq!(parsed.to_string(), spec);
            assert_eq!(parsed.to_string().parse::<Output>().unwrap().to_string(), spec);
        }
    }
}
