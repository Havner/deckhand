//! `deckhandctl` — the thin CLI client for the deckhand daemon (PLAN §4.4).
//!
//! One-shot: parse a subcommand → one [`Request`] → connect to `deckhandd`'s control socket → print
//! the reply → exit. Depends only on `deckhand-ipc` (+ `config` to read RON profiles), never on
//! `engine`. The `monitor` follow mode lands with the real event stream (D7).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use config::{ConfigDoc, GlobalConfig};
use deckhand_ipc::{Client, DeviceEntry, ProfileRole, Request, Response, StatusInfo};

/// Control the deckhand daemon.
#[derive(Parser)]
#[command(name = "deckhandctl", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show engine status (state, staged input/output, loaded profiles).
    Status,
    /// List the currently-enumerated devices (id, kind, transport, slot).
    ListDevices,
    /// Select the input device by id (`gordon:dongle:1:`) or `auto|dongle|wired`.
    SelectDevice { spec: String },
    /// Stage the input source: `auto|dongle|wired|<device-id>|host:port`.
    Input { spec: String },
    /// Stage the output sink: `local` (host:port deferred).
    Output { spec: String },
    /// Load a RON profile and apply it as the Main program.
    Main { path: PathBuf },
    /// Load a RON profile and apply it as the Fallback program.
    Fallback { path: PathBuf },
    /// Load a RON global config and apply it.
    Globals { path: PathBuf },
    /// Acquire hardware and start the mapping loop.
    Start,
    /// Stop the mapping loop (release hardware, keep config).
    Stop,
    /// Shut the daemon down.
    Shutdown,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Build the request first — file reads (Main/Fallback/Globals) can fail before we connect.
    let req = match build_request(&cli.cmd) {
        Ok(req) => req,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut client = match Client::connect_default() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot reach deckhandd ({e}) — is it running?");
            return ExitCode::FAILURE;
        }
    };

    match client.call(&req) {
        Ok(resp) => print_response(resp),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build_request(cmd: &Command) -> Result<Request, String> {
    Ok(match cmd {
        Command::Status => Request::Status,
        Command::ListDevices => Request::ListDevices,
        Command::SelectDevice { spec } | Command::Input { spec } => Request::SetInput(spec.clone()),
        Command::Output { spec } => Request::SetOutput(spec.clone()),
        Command::Main { path } => {
            Request::Apply { role: ProfileRole::Main, config: Box::new(load_doc(path)?) }
        }
        Command::Fallback { path } => {
            Request::Apply { role: ProfileRole::Fallback, config: Box::new(load_doc(path)?) }
        }
        Command::Globals { path } => Request::SetGlobals(Box::new(load_globals(path)?)),
        Command::Start => Request::Start,
        Command::Stop => Request::Stop,
        Command::Shutdown => Request::Shutdown,
    })
}

fn print_response(resp: Response) -> ExitCode {
    match resp {
        Response::Ok => {
            println!("ok");
            ExitCode::SUCCESS
        }
        Response::Status(s) => {
            print_status(&s);
            ExitCode::SUCCESS
        }
        Response::Devices(list) => {
            print_devices(&list);
            ExitCode::SUCCESS
        }
        Response::Error(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
        Response::Diagnostics(diags) => {
            eprintln!("config rejected:");
            for d in &diags {
                eprintln!("  {d}");
            }
            ExitCode::FAILURE
        }
        // `Response` is #[non_exhaustive] — a reply from a newer daemon.
        other => {
            eprintln!("unexpected reply: {other:?}");
            ExitCode::FAILURE
        }
    }
}

fn print_status(s: &StatusInfo) {
    println!("state:    {:?}", s.state);
    println!("input:    {}", s.input);
    println!("output:   {}", s.output);
    println!("main:     {}", if s.has_main { "loaded" } else { "(none)" });
    println!("fallback: {}", if s.has_fallback { "loaded" } else { "(none)" });
}

fn print_devices(list: &[DeviceEntry]) {
    if list.is_empty() {
        println!("no devices");
        return;
    }
    for d in list {
        println!("{}  ({} / {}, iface {})", d.id, d.kind, d.transport, d.interface);
    }
}

fn load_doc(path: &Path) -> Result<ConfigDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn load_globals(path: &Path) -> Result<GlobalConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}
