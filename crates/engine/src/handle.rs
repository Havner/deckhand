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
use std::str::FromStr;
use std::time::Duration;
use virt_out::Sink;

use crate::event::{EngineEvent, EventSink, EventStream};
use crate::program::{Program, Role};
use crate::runtime::{Control, DeviceCfg, Runtime};
use crate::{Error, Result};

/// Where input comes from. `Network` (a bound UDP receiver) is deferred; the seam is kept.
#[derive(Debug, Clone)]
pub enum Input {
    Local(DeviceSelect),
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

/// Where output goes. `Network` (a UDP sender to a remote sink) is deferred; the seam is kept.
#[derive(Debug, Clone)]
pub enum Output {
    Local,
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
        }
    }
}

impl FromStr for Input {
    type Err = String;

    /// A local selection (see [`DeviceSelect`]). A `host:port` shape is recognized but rejected —
    /// the `Network` input is a stubbed seam (PLAN §6).
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.parse::<DeviceSelect>() {
            Ok(sel) => Ok(Input::Local(sel)),
            Err(_) if s.contains(':') => {
                Err(format!("network input '{s}' not supported yet (PLAN §6)"))
            }
            Err(e) => Err(e),
        }
    }
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Output::Local => f.write_str("local"),
        }
    }
}

impl FromStr for Output {
    type Err = String;

    /// Only `local` for now. A `host:port` shape is recognized but rejected — the `Network` sender
    /// is a stubbed seam (PLAN §6).
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "local" | "l" => Ok(Output::Local),
            other if other.contains(':') => {
                Err(format!("network output '{other}' not supported yet (PLAN §6)"))
            }
            other => Err(format!("unrecognized output '{other}' (want local|host:port)")),
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
    }

    /// Stage the output target (Local only for now). Applied at the next `start()`.
    pub fn set_output(&mut self, output: Output) {
        log::debug!("set_output: {output} (staged for next start)");
        self.output = output;
    }

    // --- live-or-staged config ---------------------------------------------------------

    /// Apply a program to a role. Retained (survives stop/start); hot-swapped live if running.
    pub fn apply(&mut self, program: Program, role: Role) {
        let mode = if self.runtime.is_some() { "live hot-swap" } else { "staged" };
        log::info!("apply: program '{}' → {role:?} ({mode})", program.meta.name);
        match role {
            Role::Main => self.main = Some(program.clone()),
            Role::Fallback => self.fallback = Some(program.clone()),
        }
        if let Some(rt) = &self.runtime {
            let _ = rt.control().send(Control::Apply { program: Box::new(program), role });
        }
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
    }

    // --- lifecycle ---------------------------------------------------------------------

    /// Acquire hardware and start the mapping loop. Errors if no main program is applied, or
    /// on any device/sink failure. A no-op if already running.
    pub fn start(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Ok(());
        }
        let main = self.main.clone().ok_or(Error::NotReady("no main program applied"))?;
        let Output::Local = self.output; // Network output deferred.
        let device = self.open_device()?;
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
        let cfg = DeviceCfg::for_device(&device.info().kind, &self.globals);
        // Pin the resolved device's stable id so the reader reacquires *this* device if its
        // transport drops (D6), regardless of how the selection policy chose it.
        let pinned_id = device.info().id();
        let sink = Sink::new()?;
        self.runtime = Some(Runtime::start(
            device,
            pinned_id,
            cfg,
            sink,
            main,
            self.fallback.clone(),
            self.globals.clone(),
            self.events.clone(),
        ));
        self.events.emit(EngineEvent::State(Status::Running));
        Ok(())
    }

    /// Halt the loop and release hardware (device → lizard restored, virtual pad unplugged).
    /// Config is retained; `start()` resumes. A no-op if idle.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut rt) = self.runtime.take() {
            log::info!("stopping: releasing device (→ lizard) and virtual pad");
            rt.stop()?;
            self.events.emit(EngineEvent::State(Status::Idle));
        }
        Ok(())
    }

    /// The engine's run state: `Idle` when no loop is up, `WaitingForDevice` while the loop runs
    /// but the bound device's transport is gone (D5), else `Running`.
    pub fn status(&self) -> Status {
        match &self.runtime {
            None => Status::Idle,
            Some(rt) if rt.is_waiting() => Status::WaitingForDevice,
            Some(_) => Status::Running,
        }
    }

    /// The staged input source. Round-trips through its `Display`/`FromStr` form, so a caller can
    /// render it (e.g. in status) and pass the same string back to `set_input`.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// The staged output target (see [`Engine::input`] for the round-trip contract).
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// The name of the applied **Main** program, or `None` if none is applied.
    pub fn main_name(&self) -> Option<&str> {
        self.main.as_ref().map(|p| p.meta.name.as_str())
    }

    /// The name of the applied **Fallback** program, or `None`.
    pub fn fallback_name(&self) -> Option<&str> {
        self.fallback.as_ref().map(|p| p.meta.name.as_str())
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
        let Input::Local(select) = &self.input;

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
    fn input_rejects_network_and_garbage() {
        assert!("192.168.0.5:9000".parse::<Input>().unwrap_err().contains("network"));
        assert!("wat".parse::<Input>().unwrap_err().contains("unrecognized"));
    }

    #[test]
    fn output_only_local() {
        assert!(matches!("local".parse::<Output>(), Ok(Output::Local)));
        assert!("host:1".parse::<Output>().unwrap_err().contains("network"));
        assert!("wat".parse::<Output>().unwrap_err().contains("unrecognized"));
    }

    /// `Display` emits what `FromStr` accepts, for every `Input`/`Output` the daemon reports.
    #[test]
    fn input_output_round_trip() {
        for spec in ["auto", "dongle", "wired", "gordon:dongle:1:", "gordon:wired:2:ABC"] {
            let parsed: Input = spec.parse().unwrap();
            assert_eq!(parsed.to_string().parse::<Input>().unwrap().to_string(), parsed.to_string());
            assert_eq!(parsed.to_string(), spec);
        }
        let out: Output = "local".parse().unwrap();
        assert_eq!(out.to_string(), "local");
        assert!(matches!(out.to_string().parse::<Output>(), Ok(Output::Local)));
    }
}
