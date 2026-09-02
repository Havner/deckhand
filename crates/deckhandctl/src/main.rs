//! `deckhandctl` - the thin CLI client for the deckhand daemon (PLAN 4.4).
//!
//! Parses a **sequence** of commands and runs them in order over **one connection** - the daemon
//! serves multiple request/reply pairs per connection. Global flags (`--socket`, `-h`, `-V`) must
//! precede the commands. Depends only on `ipc` (+ `config` to read RON profiles), never on `engine`.
//!
//! `shutdown` and `monitor` are terminal (they end / take over the connection), so they must be the
//! last command in a chain.

use std::process::ExitCode;

use clap::Parser;
use config::{Chord, ChordAction, Chords, ConfigDoc, DeviceConfig, GordonTuning, Lever, RumbleTuning};
use ipc::{BoundDevice, Client, Event, ProfileRole, Request, Response, StatusSnapshot};

/// The command reference, shown under `--help` (the commands are raw args, so clap can't describe
/// them itself).
const COMMANDS_HELP: &str = "\
Commands (run in sequence; put --socket/-h/-V first):
  status                show engine status (state, staged input/output, profiles, chords, devcfg)
  list-devices          list the enumerated devices (id, kind, transport, slot)
  input <spec>          stage input: auto | dongle | wired | bt | <device-id> | host:port
  output <spec>         stage output: local | host:port
  main <file.ron>       load + apply a Main profile (empty string clears it: reverts to Fallback)
  fallback <file.ron>   load + apply a Fallback profile (empty string clears it)
  chords <file.ron>     load + apply the top-level switch/command chords
  devcfg <file.ron>     load + apply the device config (LED/idle, master rumble, frequency)
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
    /// The streaming follow mode - terminal (subscribes on the connection and never returns).
    Monitor,
}

/// Build a request/reply step with its display label.
fn call(label: &str, req: Request) -> Step {
    Step::Call { label: label.to_string(), req }
}

/// Build an `Apply` step for a profile role. An **empty** argument (`main ""`) clears the role
/// (`config: None`), so the other role takes over live; otherwise the path is loaded and shipped.
fn call_apply(role: ProfileRole, cmd: &str, tokens: &[String], i: &mut usize) -> Result<Step, String> {
    let p = take_arg(tokens, i, cmd)?;
    let (label, config) = if p.is_empty() {
        (format!("{cmd} (clear)"), None)
    } else {
        (format!("{cmd} {p}"), Some(Box::new(load_doc(&p)?)))
    };
    Ok(call(&label, Request::Apply { role, config }))
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
        eprintln!("no command given - try `deckhandctl --help`");
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
            "main" => call_apply(ProfileRole::Main, "main", tokens, &mut i)?,
            "fallback" => call_apply(ProfileRole::Fallback, "fallback", tokens, &mut i)?,
            "chords" => {
                let p = take_arg(tokens, &mut i, "chords")?;
                // An empty argument (`chords ""`) clears the chords, mirroring `main ""`.
                let (label, chords) = if p.is_empty() {
                    ("chords (clear)".to_string(), None)
                } else {
                    (format!("chords {p}"), Some(load_chords(&p)?))
                };
                call(&label, Request::SetChords(chords))
            }
            "devcfg" => {
                let p = take_arg(tokens, &mut i, "devcfg")?;
                let d = load_device_config(&p)?;
                call(&format!("devcfg {p}"), Request::SetDeviceConfig(d))
            }
            other => return Err(format!("unknown command '{other}' - try `deckhandctl --help`")),
        };
        steps.push(step);
    }

    // `shutdown` and `monitor` end / take over the connection - nothing may follow them.
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

/// Connect to the daemon, resolving the `--socket` override: `None` -> the env/default
/// ([`ipc::default_socket_path`], honoring `$DECKHAND_SOCKET`); otherwise the given path (Unix) /
/// pipe name (Windows). Mirrors `deckhandd`'s own resolution so client and daemon agree.
fn connect(socket: Option<&str>) -> Result<Client, ExitCode> {
    open(socket).map_err(|e| {
        eprintln!("cannot reach deckhandd ({e}) - is it running?");
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
        Event::ControllerConnected(true) => "controller connected".into(),
        Event::ControllerConnected(false) => "controller disconnected".into(),
        Event::Battery { percent } => format!("battery: {percent}%"),
        Event::BindingRemoved => "binding removed".into(),
        Event::BindingAcquired(b) => format!("binding acquired: {} ({:?})", b.id, b.shape),
        Event::State(s) => format!("state: {s:?}"),
        Event::ActiveRole(r) => format!("active role: {r:?}"),
        Event::InputStaged(i) => format!("input staged: {i}"),
        Event::OutputStaged(o) => format!("output staged: {o}"),
        Event::ProfileSet { role, name } => {
            format!("profile set: {role:?} = {}", name.as_deref().unwrap_or("(none)"))
        }
        Event::ChordsSet(c) => format!("chords set: {}", chord_summary(c)),
        Event::DeviceConfigSet(d) => format!("devcfg set: {}", devcfg_lines(d).join(" | ")),
        Event::ActiveSet(name) => format!("action set: {name}"),
        Event::HeldLayers(names) => format!("held layers: {}", name_list(names)),
        Event::PersistentLayers(names) => format!("persistent layers: {}", name_list(names)),
    }
}

/// A comma-joined name list, or `(none)` when empty - for the layer-view events.
fn name_list(names: &[String]) -> String {
    if names.is_empty() { "(none)".into() } else { names.join(", ") }
}

/// A `Lever<u8>` (percent) rendered compactly: `40%` (fixed) or `0-100%` (scaled band).
fn lever_pct(l: &Lever<u8>) -> String {
    match l {
        Lever::Fixed(v) => format!("{v}%"),
        Lever::Scaled { min, max } => format!("{min}-{max}%"),
    }
}

/// A `Lever<i8>` (dB, signed) rendered compactly: `+2dB` (fixed) or `-4..+12dB` (scaled band).
fn lever_db(l: &Lever<i8>) -> String {
    match l {
        Lever::Fixed(v) => format!("{v:+}dB"),
        Lever::Scaled { min, max } => format!("{min:+}..{max:+}dB"),
    }
}

/// Device-config summary as lines (LED/idle, then one line per device's rumble) - the status block
/// prints them stacked; the event stream joins them onto one line.
fn devcfg_lines(d: &DeviceConfig) -> Vec<String> {
    let led = d.led_brightness.map_or_else(|| "default".to_string(), |v| format!("{v}%"));
    let idle = d.idle_timeout.map_or_else(|| "default".to_string(), |s| format!("{s}s"));
    let GordonTuning { duty, hz } = &d.gordon;
    let RumbleTuning { speed: nspeed, gain: ngain } = &d.neptune;
    let RumbleTuning { speed: tspeed, gain: tgain } = &d.triton;
    vec![
        format!("led={led} idle={idle}"),
        format!("gordon(duty {}, {hz}Hz)", lever_pct(duty)),
        format!("neptune(speed {}, gain {})", lever_pct(nspeed), lever_db(ngain)),
        format!("triton(speed {}, gain {})", lever_pct(tspeed), lever_db(tgain)),
    ]
}

/// Chords as lines: `(none)`, or the count followed by one `#N buttons -> action` line per chord.
fn chord_lines(chords: &Option<Chords>) -> Vec<String> {
    let Some(c) = chords else {
        return vec!["(none)".to_string()];
    };
    let mut out = vec![c.chords.len().to_string()];
    out.extend(
        c.chords
            .iter()
            .enumerate()
            .map(|(i, ch)| format!("#{} {}", i + 1, chord_desc(ch))),
    );
    out
}

/// A one-line description of a chord: its buttons (AND-combined) -> its action.
fn chord_desc(ch: &Chord) -> String {
    let buttons = ch.buttons.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>().join("+");
    let action = match &ch.action {
        ChordAction::SwitchProfile { mode } => format!("switch profile ({mode:?})"),
        ChordAction::CommandExecute { command, args } if args.is_empty() => format!("run `{command}`"),
        ChordAction::CommandExecute { command, args } => format!("run `{command} {}`", args.join(" ")),
    };
    format!("{buttons} -> {action}")
}

/// `(none)` when no chords are set, else the count - the shared rendering for status + events.
fn chord_summary(chords: &Option<Chords>) -> String {
    chords.as_ref().map_or_else(|| "(none)".to_string(), |c| c.chords.len().to_string())
}

fn bound_summary(bound: &Option<BoundDevice>) -> String {
    bound.as_ref().map_or_else(|| "(none)".to_string(), |b| format!("{} ({:?})", b.id, b.shape))
}

fn battery_summary(battery: Option<u8>) -> String {
    battery.map_or_else(|| "(unknown)".to_string(), |p| format!("{p}%"))
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
    }
}

/// The status value column: labels are padded to the width of the longest (`controller:`) so values
/// line up. Continuation lines of a multi-line field ([`print_field`]) indent to match.
const STATUS_COL: usize = 12;

/// Print a status field whose value may span multiple lines: the first line carries `label` (padded
/// to [`STATUS_COL`]), any further lines are blank-padded to align under it.
fn print_field(label: &str, lines: &[String]) {
    for (i, line) in lines.iter().enumerate() {
        let head = if i == 0 { label } else { "" };
        println!("{head:<STATUS_COL$}{line}");
    }
}

fn print_status(s: &StatusSnapshot) {
    // Labels padded to the width of the longest (`controller:`) so values line up in one column.
    let controller = match s.controller {
        Some(true) => "Connected",
        Some(false) => "Disconnected",
        None => "(none)",
    };
    let active = match s.active {
        Some(ProfileRole::Main) => "Main",
        Some(ProfileRole::Fallback) => "Fallback",
        None => "(none)",
    };
    println!("state:      {:?}", s.state);
    println!("output:     {}", s.output);
    println!("input:      {}", s.input);
    println!("bound:      {}", bound_summary(&s.bound));
    println!("controller: {controller}");
    println!("battery:    {}", battery_summary(s.battery));
    print_field("devcfg:", &devcfg_lines(&s.device_config));
    println!("main:       {}", s.main.as_deref().unwrap_or("(none)"));
    println!("fallback:   {}", s.fallback.as_deref().unwrap_or("(none)"));
    println!("active:     {active}");
    print_field("chords:", &chord_lines(&s.chords));
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

fn load_chords(path: &str) -> Result<Chords, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    ron::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

fn load_device_config(path: &str) -> Result<DeviceConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    ron::from_str(&text).map_err(|e| format!("{path}: {e}"))
}
