//! The daemon's engine wrapper: owns the [`Engine`] — the *sole* source of truth — and turns a
//! [`Request`] into a [`Response`]. It keeps no shadow state; everything the control protocol
//! reports (staged input/output, which programs are loaded) is read back from the engine. Input/
//! output specs round-trip through the engine's `Input`/`Output` `FromStr`/`Display` (PLAN §4.4).

use config::{ConfigDoc, Diagnostic, Severity};
use engine::{Engine, EngineEvent, EventStream, Input, Output, Program, Role, Status, compile};
use ipc::{Event, ProfileRole, Request, Response, RunState, StatusSnapshot};

/// The running daemon state around the engine — just the engine, no shadow copies.
pub struct Daemon {
    engine: Engine,
}

impl Daemon {
    pub fn new() -> Self {
        Daemon {
            engine: Engine::new(),
        }
    }

    /// Apply an already-compiled program to a role (the CLI seed path compiles up front).
    pub fn apply(&mut self, role: Role, program: Program) {
        self.engine.apply(program, role);
    }

    /// Stage the input from a spec string (`auto|dongle|wired|bt|<device-id>|host:port`). Shared by
    /// the CLI `-i` and the `SetInput` request. `Err` is a human-readable reason.
    pub fn set_input(&mut self, spec: &str) -> Result<(), String> {
        self.engine.set_input(spec.parse::<Input>()?);
        Ok(())
    }

    /// Stage the output from a spec string (`local|host:port`). Shared by `-o` and `SetOutput`.
    pub fn set_output(&mut self, spec: &str) -> Result<(), String> {
        self.engine.set_output(spec.parse::<Output>()?);
        Ok(())
    }

    /// Replace the global config (CLI `-g` and `SetGlobals`).
    pub fn set_globals(&mut self, globals: config::GlobalConfig) {
        self.engine.set_globals(globals);
    }

    /// Acquire hardware and start the mapping loop (CLI `--start`).
    pub fn start(&mut self) -> engine::Result<()> {
        self.engine.start()
    }

    /// Enumerate the currently-attached devices as their stable `DeviceId` strings — shared by the
    /// CLI `--list-devices` and the `ListDevices` request. `Err` is a human-readable reason.
    pub fn devices(&mut self) -> Result<Vec<String>, String> {
        self.engine
            .devices()
            .map(|list| list.iter().map(|i| i.id().to_string()).collect())
            .map_err(|e| e.to_string())
    }

    /// Handle one control request. `Shutdown` is dealt with by the caller (it stops the serve
    /// loop); here it is a no-op `Ok` so the client still gets an acknowledgement.
    pub fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Apply { role, config } => self.apply_config(role, *config),
            Request::SetGlobals(g) => {
                self.set_globals(*g);
                Response::Ok
            }
            Request::SetInput(spec) => match self.set_input(&spec) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e),
            },
            Request::SetOutput(spec) => match self.set_output(&spec) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e),
            },
            Request::Start => match self.engine.start() {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Stop => match self.engine.stop() {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::ListDevices => match self.devices() {
                Ok(list) => Response::Devices(list),
                Err(e) => Response::Error(e),
            },
            Request::Status => Response::Status(self.status_info()),
            // Shutdown + Subscribe are intercepted by the serve loop (they change how the connection
            // is served); if one reaches here it's a no-op ack / a `_` catch below.
            Request::Shutdown => Response::Ok,
            // `Request` is #[non_exhaustive]; a future variant this daemon predates (Subscribe is
            // handled by the serve loop, so it never lands here).
            _ => Response::Error("unsupported request".into()),
        }
    }

    /// Subscribe to the engine's event stream (D7) — the serve loop hands the resulting stream to a
    /// per-connection monitor thread.
    pub fn subscribe(&self) -> EventStream {
        self.engine.subscribe()
    }

    /// Compile a shipped `ConfigDoc` and, iff it has no errors, apply it — else reject with the
    /// diagnostics (client ships config, daemon compiles; PLAN §4.4).
    fn apply_config(&mut self, role: ProfileRole, config: ConfigDoc) -> Response {
        match compile(&config) {
            Ok(program) => {
                self.apply(role_of(role), program);
                Response::Ok
            }
            Err(diags) => Response::Diagnostics(format_diags(&diags)),
        }
    }

    fn status_info(&self) -> StatusSnapshot {
        let s = self.engine.status();
        StatusSnapshot {
            state: run_state(s.state),
            output: s.output.to_string(),
            input: s.input.to_string(),
            bound: s.bound.map(|id| id.to_string()),
            main: s.main,
            fallback: s.fallback,
            globals: s.globals,
        }
    }

    /// Full teardown (releases hardware → lizard restored, pad unplugged).
    pub fn shutdown(self) -> engine::Result<()> {
        self.engine.shutdown()
    }
}

/// Map a wire [`ProfileRole`] to the engine's `Role`.
fn role_of(r: ProfileRole) -> Role {
    match r {
        ProfileRole::Main => Role::Main,
        ProfileRole::Fallback => Role::Fallback,
    }
}

/// Map the engine's `Role` to a wire [`ProfileRole`] (the reverse of [`role_of`]).
fn profile_role(r: Role) -> ProfileRole {
    match r {
        Role::Main => ProfileRole::Main,
        Role::Fallback => ProfileRole::Fallback,
    }
}

/// Format compile/validation diagnostics as severity-prefixed lines for the wire / the CLI.
pub fn format_diags(diags: &[Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .map(|d| {
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            format!("{sev}: {}", d.message)
        })
        .collect()
}

/// Map an engine `Status` to the wire `RunState`.
pub fn run_state(status: Status) -> RunState {
    match status {
        Status::Idle => RunState::Idle,
        Status::Running => RunState::Running,
        Status::WaitingForDevice => RunState::WaitingForDevice,
    }
}

/// Map an engine [`EngineEvent`] to its wire [`Event`] (D7). Exhaustive on purpose — `EngineEvent`
/// is not `#[non_exhaustive]`, so a new variant is a compile error here until it's mapped.
pub fn to_wire_event(ev: EngineEvent) -> Event {
    match ev {
        EngineEvent::ControllerConnected => Event::ControllerConnected,
        EngineEvent::ControllerDisconnected => Event::ControllerDisconnected,
        EngineEvent::BatteryChanged { percent } => Event::Battery { percent: Some(percent) },
        EngineEvent::BindingLost => Event::BindingLost,
        EngineEvent::BindingAcquired(id) => Event::BindingAcquired(id.to_string()),
        EngineEvent::State(s) => Event::State(run_state(s)),
        EngineEvent::InputStaged(i) => Event::InputStaged(i.to_string()),
        EngineEvent::OutputStaged(o) => Event::OutputStaged(o.to_string()),
        EngineEvent::ProfileSet { role, name } => Event::ProfileSet { role: profile_role(role), name },
        EngineEvent::GlobalConfigSet(g) => Event::GlobalConfigSet(g),
    }
}
