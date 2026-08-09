//! `deckhandctl` — the thin CLI client for the deckhand daemon (PLAN §4.4).
//!
//! Parses a **sequence** of commands and runs them in order over **one connection** — the daemon
//! serves multiple request/reply pairs per connection. Global flags (`--socket`, `-h`, `-V`) must
//! precede the commands. Depends only on `ipc` (+ `config` to read RON profiles), never on `engine`.
//!
//! `shutdown` and `monitor` are terminal (they end / take over the connection), so they must be the
//! last command in a chain.

use std::process::ExitCode;

use clap::Parser;
use config::{ConfigDoc, GlobalConfig};
use ipc::{Client, Event, ProfileRole, Request, Response, StatusSnapshot};

/// The command reference, shown under `--help` (the commands are raw args, so clap can't describe
/// them itself).
const COMMANDS_HELP: &str = "\
Commands (run in sequence; put --socket/-h/-V first):
  status                show engine status (state, staged input/output, profiles, globals)
  list-devices          list the enumerated devices (id, kind, transport, slot)
  input <spec>          stage input: auto | dongle | wired | bt | <device-id> | host:port
  output <spec>         stage output: local | host:port
  main <file.ron>       load + apply a Main profile
  fallback <file.ron>   load + apply a Fallback profile
  globals <file.ron>    load + apply the global config
  start                 acquire hardware and start the mapping loop
  stop                  stop the mapping loop (release hardware, keep config)
  shutdown              shut the daemon down (must be last)
  monitor               follow the event stream until Ctrl-C (must be last)

Examples:
  deckhandctl status
  deckhandctl main game.ron fallback desktop.ron input wired start
  deckhandctl --socket /run/user/1000/dev.sock stop";

/// Control the deckhand daemon.
#[derive(Parser)]
#[command(name = "deckhandctl", version, about, after_help = COMMANDS_HELP)]
struct Cli {
    /// Control socket path (Unix) / pipe name (Windows). Overrides $DECKHAND_SOCKET and the
    /// default. Must precede the commands.
    #[arg(short = 'k', long, value_name = "PATH")]
    socket: Option<String>,
    /// One or more commands to run in sequence (see the command reference under `--help`).
    #[arg(value_name = "COMMAND", trailing_var_arg = true, allow_hyphen_values = true)]
    commands: Vec<String>,
}

/// One parsed step of the chain.
enum Step {
    /// A request/reply command. `label` is the command as typed (e.g. `main game.ron`), so a chain's
    /// replies can be told apart.
    Call { label: String, req: Request },
    /// The streaming follow mode — terminal (subscribes on the connection and never returns).
    Monitor,
}

/// Build a request/reply step with its display label.
fn call(label: &str, req: Request) -> Step {
    Step::Call { label: label.to_string(), req }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Parse + read any profile files up front, so a typo fails before we touch the daemon and the
    // chain never half-runs.
    let steps = match parse_steps(&cli.commands) {
        Ok(steps) => steps,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if steps.is_empty() {
        eprintln!("no command given — try `deckhandctl --help`");
        return ExitCode::FAILURE;
    }

    // One connection carries the whole chain (the daemon loops over requests on it).
    let mut client = match connect(cli.socket.as_deref()) {
        Ok(c) => c,
        Err(code) => return code,
    };

    for step in steps {
        match step {
            Step::Call { label, req } => match client.call(&req) {
                Ok(resp) => {
                    let code = print_response(&label, resp);
                    if code != ExitCode::SUCCESS {
                        return code; // fail-fast: stop the chain on the first error
                    }
                }
                Err(e) => {
                    eprintln!("{label}: error: {e}");
                    return ExitCode::FAILURE;
                }
            },
            // Terminal: subscribe on this same connection and stream until close/Ctrl-C.
            Step::Monitor => return run_monitor(&mut client),
        }
    }
    ExitCode::SUCCESS
}

/// Parse the raw command tokens into a sequence of steps (reading profile files as it goes).
/// `shutdown`/`monitor` are terminal, so nothing may follow them.
fn parse_steps(tokens: &[String]) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        i += 1;
        let step = match tok {
            "status" => call("status", Request::Status),
            "list-devices" => call("list-devices", Request::ListDevices),
            "start" => call("start", Request::Start),
            "stop" => call("stop", Request::Stop),
            "shutdown" => call("shutdown", Request::Shutdown),
            "monitor" => Step::Monitor,
            "input" => {
                let a = take_arg(tokens, &mut i, "input")?;
                call(&format!("input {a}"), Request::SetInput(a))
            }
            "output" => {
                let a = take_arg(tokens, &mut i, "output")?;
                call(&format!("output {a}"), Request::SetOutput(a))
            }
            "main" => {
                let p = take_arg(tokens, &mut i, "main")?;
                let doc = load_doc(&p)?;
                call(&format!("main {p}"), Request::Apply {
                    role: ProfileRole::Main,
                    config: Box::new(doc),
                })
            }
            "fallback" => {
                let p = take_arg(tokens, &mut i, "fallback")?;
                let doc = load_doc(&p)?;
                call(&format!("fallback {p}"), Request::Apply {
                    role: ProfileRole::Fallback,
                    config: Box::new(doc),
                })
            }
            "globals" => {
                let p = take_arg(tokens, &mut i, "globals")?;
                let g = load_globals(&p)?;
                call(&format!("globals {p}"), Request::SetGlobals(Box::new(g)))
            }
            other => return Err(format!("unknown command '{other}' — try `deckhandctl --help`")),
        };
        steps.push(step);
    }

    // `shutdown` and `monitor` end / take over the connection — nothing may follow them.
    let last = steps.len().saturating_sub(1);
    for (idx, step) in steps.iter().enumerate() {
        let terminal = match step {
            Step::Monitor => Some("monitor"),
            Step::Call { req: Request::Shutdown, .. } => Some("shutdown"),
            _ => None,
        };
        if let Some(name) = terminal
            && idx != last
        {
            return Err(format!("'{name}' must be the last command"));
        }
    }
    Ok(steps)
}

/// Consume `tokens[*i]` as `cmd`'s argument, advancing `i`.
fn take_arg(tokens: &[String], i: &mut usize, cmd: &str) -> Result<String, String> {
    let a = tokens.get(*i).cloned().ok_or_else(|| format!("'{cmd}' requires an argument"))?;
    *i += 1;
    Ok(a)
}

/// Connect to the daemon, resolving the `--socket` override: `None` → the env/default
/// ([`ipc::default_socket_path`], honoring `$DECKHAND_SOCKET`); otherwise the given path (Unix) /
/// pipe name (Windows). Mirrors `deckhandd`'s own resolution so client and daemon agree.
fn connect(socket: Option<&str>) -> Result<Client, ExitCode> {
    open(socket).map_err(|e| {
        eprintln!("cannot reach deckhandd ({e}) — is it running?");
        ExitCode::FAILURE
    })
}

#[cfg(unix)]
fn open(socket: Option<&str>) -> std::io::Result<Client> {
    let path = socket.map(std::path::PathBuf::from).unwrap_or_else(ipc::default_socket_path);
    Client::connect_path(&path)
}
#[cfg(windows)]
fn open(socket: Option<&str>) -> std::io::Result<Client> {
    Client::connect_name(socket.unwrap_or(ipc::DEFAULT_PIPE_NAME))
}

/// Subscribe on `client` and print events until the daemon closes the stream or the user Ctrl-Cs.
fn run_monitor(client: &mut Client) -> ExitCode {
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

/// A one-line human rendering of a pushed event.
fn fmt_event(ev: &Event) -> String {
    match ev {
        Event::ControllerConnected => "controller connected".into(),
        Event::ControllerDisconnected => "controller disconnected".into(),
        Event::Battery { percent: Some(p) } => format!("battery: {p}%"),
        Event::Battery { percent: None } => "battery: unknown".into(),
        Event::BindingRemoved => "binding removed".into(),
        Event::BindingAcquired(id) => format!("binding acquired: {id}"),
        Event::State(s) => format!("state: {s:?}"),
        Event::InputStaged(i) => format!("input staged: {i}"),
        Event::OutputStaged(o) => format!("output staged: {o}"),
        Event::ProfileSet { role, name } => {
            format!("profile set: {role:?} = {}", name.as_deref().unwrap_or("(none)"))
        }
        Event::GlobalConfigSet(g) => format!(
            "globals set: start={:?}, master_rumble={}%, {} chord(s)",
            g.start_profile,
            g.master_rumble,
            g.chords.len(),
        ),
        // `Event` is #[non_exhaustive] — a newer daemon sent something we don't render yet.
        other => format!("{other:?}"),
    }
}

/// Print a reply, prefixed with the command `label` so a chain's acks/errors are attributable.
/// `status`/`list-devices` print their own (self-describing) block without a prefix.
fn print_response(label: &str, resp: Response) -> ExitCode {
    match resp {
        Response::Ok => {
            println!("{label}: ok");
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
            eprintln!("{label}: error: {msg}");
            ExitCode::FAILURE
        }
        Response::Diagnostics(diags) => {
            eprintln!("{label}: config rejected:");
            for d in &diags {
                eprintln!("  {d}");
            }
            ExitCode::FAILURE
        }
        // `Response` is #[non_exhaustive] — a reply from a newer daemon.
        other => {
            eprintln!("{label}: unexpected reply: {other:?}");
            ExitCode::FAILURE
        }
    }
}

fn print_status(s: &StatusSnapshot) {
    println!("state:    {:?}", s.state);
    println!("output:   {}", s.output);
    println!("input:    {}", s.input);
    println!("bound:    {}", s.bound.as_deref().unwrap_or("(none)"));
    println!("main:     {}", s.main.as_deref().unwrap_or("(none)"));
    println!("fallback: {}", s.fallback.as_deref().unwrap_or("(none)"));
    println!(
        "globals:  start={:?}, master_rumble={}%, {} chord(s)",
        s.globals.start_profile,
        s.globals.master_rumble,
        s.globals.chords.len(),
    );
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

fn load_doc(path: &str) -> Result<ConfigDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    ron::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

fn load_globals(path: &str) -> Result<GlobalConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    ron::from_str(&text).map_err(|e| format!("{path}: {e}"))
}
