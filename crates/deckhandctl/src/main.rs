//! `deckhandctl` — the thin CLI client for the deckhand daemon (PLAN §4.4).
//!
//! One-shot: parse a subcommand → one [`Request`] → connect to `deckhandd`'s control socket → print
//! the reply → exit. Depends only on `ipc` (+ `config` to read RON profiles), never on
//! `engine`. The `monitor` follow mode lands with the real event stream (D7).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use config::{ConfigDoc, GlobalConfig};
use ipc::{Client, Event, ProfileRole, Request, Response, StatusSnapshot};

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
    /// Stage the input source: auto | dongle | wired | <device-id> | host:port.
    ///
    /// A <device-id> is `kind:transport:interface:serial` — e.g. `gordon:dongle:1:` (see
    /// `deckhandctl list-devices`).
    Input { spec: String },
    /// Stage the output sink: local | host:port.
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
    /// Follow the engine's event stream, printing events as they happen (Ctrl-C to stop).
    Monitor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // `monitor` is a long-lived stream, not a one-shot request/reply.
    if matches!(cli.cmd, Command::Monitor) {
        return run_monitor();
    }

    // Build the request first — file reads (Main/Fallback/Globals) can fail before we connect.
    let req = match build_request(&cli.cmd) {
        Ok(req) => req,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut client = match connect() {
        Ok(c) => c,
        Err(code) => return code,
    };

    match client.call(&req) {
        Ok(resp) => print_response(resp),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn connect() -> Result<Client, ExitCode> {
    Client::connect_default().map_err(|e| {
        eprintln!("cannot reach deckhandd ({e}) — is it running?");
        ExitCode::FAILURE
    })
}

/// Subscribe and print events until the daemon closes the stream or the user Ctrl-Cs.
fn run_monitor() -> ExitCode {
    let mut client = match connect() {
        Ok(c) => c,
        Err(code) => return code,
    };
    if let Err(e) = client.subscribe() {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    loop {
        match client.next_event() {
            Ok(Some(ev)) => println!("{}", fmt_event(&ev)),
            Ok(None) => {
                eprintln!("daemon closed the event stream");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
}

fn build_request(cmd: &Command) -> Result<Request, String> {
    Ok(match cmd {
        Command::Status => Request::Status,
        Command::ListDevices => Request::ListDevices,
        Command::Input { spec } => Request::SetInput(spec.clone()),
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
        Command::Monitor => unreachable!("monitor is handled before build_request"),
    })
}

/// A one-line human rendering of a pushed event.
fn fmt_event(ev: &Event) -> String {
    match ev {
        Event::ControllerConnected => "controller connected".into(),
        Event::ControllerDisconnected => "controller disconnected".into(),
        Event::Battery { percent: Some(p) } => format!("battery: {p}%"),
        Event::Battery { percent: None } => "battery: unknown".into(),
        Event::BindingLost => "binding lost (waiting for device)".into(),
        Event::BindingAcquired(id) => format!("binding acquired: {id}"),
        Event::State(s) => format!("state: {s:?}"),
        // `Event` is #[non_exhaustive] — a newer daemon sent something we don't render yet.
        other => format!("{other:?}"),
    }
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

fn print_status(s: &StatusSnapshot) {
    println!("state:    {:?}", s.state);
    println!("input:    {}", s.input);
    println!("output:   {}", s.output);
    println!("main:     {}", s.main.as_deref().unwrap_or("(none)"));
    println!("fallback: {}", s.fallback.as_deref().unwrap_or("(none)"));
}

fn print_devices(ids: &[String]) {
    if ids.is_empty() {
        println!("no devices");
        return;
    }
    for id in ids {
        println!("{id}");
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
