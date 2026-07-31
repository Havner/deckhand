//! The daemon's engine wrapper: owns the [`Engine`], tracks the little bit of state the control
//! protocol reports (staged specs, which programs are loaded), and turns a [`Request`] into a
//! [`Response`]. The spec parsers and diagnostic formatting are shared by the CLI seed path and the
//! socket handler (PLAN §4.4).

use config::{ConfigDoc, Diagnostic, Severity};
use deckhand_ipc::{DeviceEntry, ProfileRole, Request, Response, RunState, StatusInfo};
use engine::{
    DeviceId, DeviceInfo, DeviceSelect, Engine, Input, Output, Program, Role, Status, Transport,
    compile,
};

/// The daemon's view of the engine's *default* selection. `Engine::new` stages `Local(Auto)` /
/// `Local`, so these are the specs it starts with — we report them rather than call `set_input`/
/// `set_output` with values that would be a no-op. (If the engine's default ever changes, this is
/// the spot that goes stale.)
const DEFAULT_INPUT: &str = "auto";
const DEFAULT_OUTPUT: &str = "local";

/// The running daemon state around the engine.
pub struct Daemon {
    engine: Engine,
    input_spec: String,
    output_spec: String,
    has_main: bool,
    has_fallback: bool,
}

impl Daemon {
    pub fn new() -> Self {
        Daemon {
            engine: Engine::new(),
            input_spec: DEFAULT_INPUT.to_owned(),
            output_spec: DEFAULT_OUTPUT.to_owned(),
            has_main: false,
            has_fallback: false,
        }
    }

    /// Apply an already-compiled program to a role (the CLI seed path compiles up front).
    pub fn apply(&mut self, role: Role, program: Program) {
        match role {
            Role::Main => self.has_main = true,
            Role::Fallback => self.has_fallback = true,
        }
        self.engine.apply(program, role);
    }

    /// Stage the input from a spec string (`auto|dongle|wired|<device-id>|host:port`). Shared by
    /// the CLI `-i` and the `SetInput` request. `Err` is a human-readable reason.
    pub fn set_input(&mut self, spec: &str) -> Result<(), String> {
        let input = parse_input_spec(spec)?;
        self.engine.set_input(input);
        self.input_spec = spec.to_owned();
        Ok(())
    }

    /// Stage the output from a spec string (`local|host:port`). Shared by `-o` and `SetOutput`.
    pub fn set_output(&mut self, spec: &str) -> Result<(), String> {
        let output = parse_output_spec(spec)?;
        self.engine.set_output(output);
        self.output_spec = spec.to_owned();
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
            Request::ListDevices => match self.engine.devices() {
                Ok(list) => Response::Devices(list.iter().map(device_entry).collect()),
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Status => Response::Status(self.status_info()),
            // Acknowledged here; the serve loop watches for it and shuts down.
            Request::Shutdown => Response::Ok,
            // Event streaming lands in D7; until then, say so rather than silently hang.
            Request::Subscribe => Response::Error("event streaming not available yet (D7)".into()),
            // `Request` is #[non_exhaustive]; a future variant this daemon predates.
            _ => Response::Error("unsupported request".into()),
        }
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

    fn status_info(&self) -> StatusInfo {
        StatusInfo {
            state: match self.engine.status() {
                Status::Idle => RunState::Idle,
                Status::Running => RunState::Running,
                Status::WaitingForDevice => RunState::WaitingForDevice,
            },
            input: self.input_spec.clone(),
            output: self.output_spec.clone(),
            has_main: self.has_main,
            has_fallback: self.has_fallback,
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

/// Parse an input spec into an engine [`Input`]. `host:port` is recognized but rejected (the
/// engine's `Network` input is a stubbed seam — PLAN §6).
pub fn parse_input_spec(spec: &str) -> Result<Input, String> {
    let select = match spec {
        "auto" | "a" => DeviceSelect::Auto,
        "dongle" | "d" => DeviceSelect::Transport(Transport::UsbDongle),
        "wired" | "w" => DeviceSelect::Transport(Transport::UsbWired),
        other => {
            if let Ok(id) = other.parse::<DeviceId>() {
                DeviceSelect::Explicit(id)
            } else if other.contains(':') {
                return Err(format!("network input '{other}' not supported yet (PLAN §6)"));
            } else {
                return Err(format!("unrecognized input '{other}' (want auto|dongle|wired|<id>)"));
            }
        }
    };
    Ok(Input::Local(select))
}

/// Parse an output spec into an engine [`Output`]. Only `local` for now; `host:port` (Network
/// sender) is a stubbed seam (PLAN §6).
pub fn parse_output_spec(spec: &str) -> Result<Output, String> {
    match spec {
        "local" | "l" => Ok(Output::Local),
        other if other.contains(':') => {
            Err(format!("network output '{other}' not supported yet (PLAN §6)"))
        }
        other => Err(format!("unrecognized output '{other}' (want local|host:port)")),
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

fn device_entry(info: &DeviceInfo) -> DeviceEntry {
    DeviceEntry {
        id: info.id().to_string(),
        kind: format!("{:?}", info.kind),
        transport: format!("{:?}", info.transport),
        interface: info.interface,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_spec_parses_transports_and_ids() {
        assert!(matches!(
            parse_input_spec("dongle"),
            Ok(Input::Local(DeviceSelect::Transport(Transport::UsbDongle)))
        ));
        assert!(matches!(parse_input_spec("a"), Ok(Input::Local(DeviceSelect::Auto))));
        assert!(matches!(
            parse_input_spec("gordon:dongle:1:"),
            Ok(Input::Local(DeviceSelect::Explicit(_)))
        ));
    }

    #[test]
    fn input_spec_rejects_network_and_garbage() {
        assert!(parse_input_spec("192.168.0.5:9000").unwrap_err().contains("network"));
        assert!(parse_input_spec("wat").unwrap_err().contains("unrecognized"));
    }

    #[test]
    fn output_spec_only_local() {
        assert!(matches!(parse_output_spec("local"), Ok(Output::Local)));
        assert!(parse_output_spec("host:1").unwrap_err().contains("network"));
        assert!(parse_output_spec("wat").unwrap_err().contains("unrecognized"));
    }
}
