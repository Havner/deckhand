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
use steam_hid::{Device, DeviceInfo, Manager, RawReport, Transport};
use std::time::Duration;
use virt_out::Sink;

use crate::program::{Program, Role};
use crate::runtime::{Control, DeviceCfg, Runtime};
use crate::{Error, Result};

/// Where input comes from. `Network` (a bound UDP receiver) is deferred; the seam is kept.
pub enum Input {
    Local(DeviceSelect),
}

/// How to pick the local controller.
pub enum DeviceSelect {
    /// The first controller that streams frames (mirrors the examples/bridge).
    Auto,
    /// Restrict to a transport (wired vs dongle).
    Transport(Transport),
    /// A specific enumerated device.
    Explicit(DeviceInfo),
}

/// Where output goes. `Network` (a UDP sender to a remote sink) is deferred; the seam is kept.
pub enum Output {
    Local,
}

/// Whether the engine is currently running its loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Idle,
    Running,
}

/// The engine handle. Holds the staged input/output, the program(s) + globals (retained across
/// start/stop), and the live [`Runtime`] while running.
pub struct Engine {
    manager: Option<Manager>,
    input: Input,
    output: Output,
    active: Option<Program>,
    fallback: Option<Program>,
    globals: GlobalConfig,
    runtime: Option<Runtime>,
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
            active: None,
            fallback: None,
            globals: GlobalConfig::default(),
            runtime: None,
        }
    }

    // --- staging (take effect at the next start) ---------------------------------------

    /// Stage the input source (Local only for now). Applied at the next `start()`.
    pub fn set_input(&mut self, input: Input) {
        self.input = input;
    }

    /// Stage the output target (Local only for now). Applied at the next `start()`.
    pub fn set_output(&mut self, output: Output) {
        self.output = output;
    }

    // --- live-or-staged config ---------------------------------------------------------

    /// Apply a program to a role. Retained (survives stop/start); hot-swapped live if running.
    pub fn apply(&mut self, program: Program, role: Role) {
        match role {
            Role::Active => self.active = Some(program.clone()),
            Role::Fallback => self.fallback = Some(program.clone()),
        }
        if let Some(rt) = &self.runtime {
            let _ = rt.control().send(Control::Apply { program: Box::new(program), role });
        }
    }

    /// Set the global config (master rumble + chords). Retained; hot-swapped live if running.
    pub fn set_globals(&mut self, globals: GlobalConfig) {
        self.globals = globals.clone();
        if let Some(rt) = &self.runtime {
            let _ = rt.control().send(Control::SetGlobals(Box::new(globals)));
        }
    }

    // --- lifecycle ---------------------------------------------------------------------

    /// Acquire hardware and start the mapping loop. Errors if no active program is applied, or
    /// on any device/sink failure. A no-op if already running.
    pub fn start(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Ok(());
        }
        let active = self.active.clone().ok_or(Error::NotReady("no active program applied"))?;
        let Output::Local = self.output; // Network output deferred.
        let device = self.open_device()?;
        let cfg = DeviceCfg::for_device(&device.info().kind, &self.globals);
        let sink = Sink::new()?;
        self.runtime = Some(Runtime::start(
            device,
            cfg,
            sink,
            active,
            self.fallback.clone(),
            self.globals.clone(),
        ));
        Ok(())
    }

    /// Halt the loop and release hardware (device → lizard restored, virtual pad unplugged).
    /// Config is retained; `start()` resumes. A no-op if idle.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut rt) = self.runtime.take() {
            rt.stop()?;
        }
        Ok(())
    }

    /// Whether the engine is running.
    pub fn status(&self) -> Status {
        if self.runtime.is_some() { Status::Running } else { Status::Idle }
    }

    /// Enumerate the attached controllers (any time — no HW is retained).
    pub fn devices(&mut self) -> Result<Vec<DeviceInfo>> {
        self.ensure_manager()?;
        Ok(self.manager.as_ref().unwrap().enumerate()?)
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
        let manager = self.manager.as_ref().unwrap();
        let Input::Local(select) = &self.input;

        if let DeviceSelect::Explicit(info) = select {
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
    /// The shipped `deckhand-run` example profile must parse and compile (keeps the sample
    /// `.ron` valid as the config model evolves).
    #[test]
    fn bridge_profile_compiles() {
        let ron = include_str!("../examples/bridge_profile.ron");
        let doc: config::ConfigDoc = ron::from_str(ron).expect("parse bridge_profile.ron");
        crate::compile(&doc).expect("compile bridge_profile.ron");
    }
}
