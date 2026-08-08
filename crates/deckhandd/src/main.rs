//! `deckhandd` — the deckhand control daemon (PLAN §4.4).
//!
//! One binary, two uses: a **standalone runner** (pass everything on the CLI, Ctrl-C to quit) and
//! a **controllable daemon** (a client drives it over the control socket). It always opens the
//! socket; the CLI just *seeds* the engine, then clients mutate it live.
//!
//! Seeding is pass-through — only options actually given are applied — with one convenience:
//! `--start` defaults a missing `-i`/`-o` to `auto`/`local` so the engine has a source/sink.
//! Profiles are never defaulted, so `--start` with no `-m` logs a `NotReady` and keeps serving.

mod daemon;
#[cfg(target_os = "linux")]
mod inhibit;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;
use config::{ConfigDoc, GlobalConfig};
use engine::{EventStream, Program, Role, compile};
use ipc::{Client, Conn, Request, Response, Server};

use daemon::{Daemon, format_diags, to_wire_event};

/// The deckhand control daemon: runs the headless engine and serves a control socket.
#[derive(Parser)]
#[command(name = "deckhandd", version, about)]
struct Args {
    /// Main profile (RON) → applied to the Main role.
    #[arg(short, long, value_name = "RON")]
    main: Option<PathBuf>,
    /// Fallback profile (RON) → applied to the Fallback role.
    #[arg(short, long, value_name = "RON")]
    fallback: Option<PathBuf>,
    /// Global config (RON): master rumble, boot role, switch chords.
    #[arg(short, long, value_name = "RON")]
    globals: Option<PathBuf>,
    /// Input source: auto | dongle | wired | bt | <device-id> | host:port.
    ///
    /// A <device-id> is `kind:transport:interface:serial` — e.g. `gordon:dongle:1:` (see
    /// `deckhandctl list-devices`).
    #[arg(short, long, value_name = "SPEC")]
    input: Option<String>,
    /// Output sink: local | host:port.
    #[arg(short, long, value_name = "SPEC")]
    output: Option<String>,
    /// Acquire hardware and start immediately (defaults a missing -i/-o to auto/local).
    #[arg(short, long)]
    start: bool,
    /// Keep the session awake while running (Linux only). Holds a freedesktop
    /// ScreenSaver inhibitor.
    #[arg(short = 'p', long)]
    prevent_sleep: bool,
    /// Control socket path (Unix) / pipe name (Windows). Overrides $DECKHAND_SOCKET and the default.
    #[arg(short = 'k', long, value_name = "PATH")]
    socket: Option<String>,
    /// Increase log verbosity: -v info, -vv debug, -vvv trace (default warn). RUST_LOG overrides.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

impl Args {
    fn log_level(&self) -> &'static str {
        match self.verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(args.log_level()))
        .init();

    // Hold a sleep/idle inhibitor for the daemon's lifetime (best-effort). Kept in a binding so it
    // lives across `serve` and releases on a clean exit.
    #[cfg(target_os = "linux")]
    let _inhibitor = args.prevent_sleep.then(inhibit::SleepInhibitor::acquire).flatten();
    #[cfg(not(target_os = "linux"))]
    if args.prevent_sleep {
        log::warn!("--prevent-sleep is only supported on Linux — ignored");
    }

    let mut daemon = Daemon::new();

    // --- CLI seeding: pass-through — apply only what was given (PLAN §4.4). ------------------
    if let Some(p) = &args.main {
        daemon.apply(Role::Main, load_program(p)?);
        log::info!("main profile: {}", p.display());
    }
    if let Some(p) = &args.fallback {
        daemon.apply(Role::Fallback, load_program(p)?);
        log::info!("fallback profile: {}", p.display());
    }
    if let Some(p) = &args.globals {
        daemon.set_globals(load_globals(p)?);
        log::info!("globals: {}", p.display());
    }
    if let Some(spec) = &args.input {
        daemon.set_input(spec).map_err(cli_err)?;
    }
    if let Some(spec) = &args.output {
        daemon.set_output(spec).map_err(cli_err)?;
    }

    // --- --start: acquire hardware and run. The engine already defaults its input/output to
    // Local(Auto)/Local, so a missing -i/-o needs no set_* call (the daemon just reports those
    // defaults in status). Starting under-configured (no main, no device) is not fatal. ---------
    if args.start {
        match daemon.start() {
            Ok(()) => log::info!("engine started"),
            Err(e) => log::error!("--start: {e}; serving socket, waiting for a client"),
        }
    }

    serve(daemon, resolve_socket(args.socket.as_deref()))
}

/// The socket the daemon serves, resolved from `--socket` (`SocketTarget` is a filesystem path on
/// Unix, a pipe name on Windows).
#[cfg(unix)]
type SocketTarget = PathBuf;
#[cfg(windows)]
type SocketTarget = String;

/// Resolve the `--socket` override: `None` (unset) → the env/default ([`ipc::default_socket_path`],
/// honoring `$DECKHAND_SOCKET`); otherwise the given path / pipe name.
#[cfg(unix)]
fn resolve_socket(over: Option<&str>) -> SocketTarget {
    over.map(PathBuf::from).unwrap_or_else(ipc::default_socket_path)
}
#[cfg(windows)]
fn resolve_socket(over: Option<&str>) -> SocketTarget {
    over.map(str::to_string).unwrap_or_else(|| ipc::DEFAULT_PIPE_NAME.to_string())
}

/// Bind the control socket and run the accept/serve loop until Shutdown or Ctrl-C/SIGTERM. A bind
/// failure is **fatal** (propagated).
fn serve(mut daemon: Daemon, target: SocketTarget) -> Result<(), Box<dyn Error>> {
    let server = bind_socket(&target)?;
    log::info!("listening on {}", display_target(&target));

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = running.clone();
        let target = target.clone();
        // Ctrl-C / SIGTERM: flip the flag and unblock the blocking accept by self-connecting.
        ctrlc::set_handler(move || {
            running.store(false, Ordering::Relaxed);
            wake(&target);
        })?;
    }

    for conn in server.incoming() {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        let mut conn = match conn {
            Ok(c) => c,
            Err(e) => {
                log::warn!("accept: {e}");
                continue;
            }
        };
        // Request/reply is served one connection at a time (each is short: connect → request →
        // reply → close). A `Subscribe` is the exception — a long-lived event stream — so it's
        // handed to its own thread and the accept loop moves on (D7).
        let mut shutdown = false;
        loop {
            match conn.recv() {
                Ok(Some(Request::Shutdown)) => {
                    let _ = conn.reply(&Response::Ok);
                    log::info!("shutdown requested by client");
                    shutdown = true;
                    break;
                }
                Ok(Some(Request::Subscribe)) => {
                    log::info!("client subscribed to the event stream");
                    spawn_monitor(conn, daemon.subscribe()); // moves conn onto its own thread
                    break;
                }
                Ok(Some(req)) => {
                    let resp = daemon.handle(req);
                    if let Err(e) = conn.reply(&resp) {
                        log::warn!("reply: {e}");
                        break;
                    }
                }
                Ok(None) => break, // client closed the connection
                Err(e) => {
                    log::warn!("recv: {e}");
                    break;
                }
            }
        }
        if shutdown || !running.load(Ordering::Relaxed) {
            break;
        }
    }

    cleanup_socket(&target);
    log::info!("shutting down — releasing controller and unplugging virtual pad");
    if let Err(e) = daemon.shutdown() {
        log::warn!("shutdown: {e}");
    }
    Ok(())
}

/// Wake the blocking accept loop by opening (and dropping) a throwaway connection to our socket.
#[cfg(unix)]
fn wake(target: &Path) {
    let _ = Client::connect_path(target);
}
#[cfg(windows)]
fn wake(target: &str) {
    let _ = Client::connect_name(target);
}

/// A human-readable form of the socket target for logging.
#[cfg(unix)]
fn display_target(target: &Path) -> String {
    target.display().to_string()
}
#[cfg(windows)]
fn display_target(target: &str) -> String {
    target.to_string()
}

/// Remove the socket file on exit (Unix only; Windows named pipes need no cleanup).
#[cfg(unix)]
fn cleanup_socket(target: &Path) {
    let _ = std::fs::remove_file(target);
}
#[cfg(windows)]
fn cleanup_socket(_target: &str) {}

/// Stream engine events to a subscribed client on its own thread (D7) until the engine goes away
/// (all senders dropped → `recv` returns `None`) or the client disconnects (a send fails). Detached:
/// on daemon shutdown the engine drops, the stream ends, and the thread exits with the process.
fn spawn_monitor(mut conn: Conn, stream: EventStream) {
    std::thread::spawn(move || {
        while let Some(ev) = stream.recv() {
            if conn.send_event(&to_wire_event(ev)).is_err() {
                break; // client gone
            }
        }
    });
}

/// Bind the control socket at `path`, with a stale-socket / single-instance dance: if the file
/// exists but nothing is listening, it's removed and rebound; a live listener is a genuine second
/// instance (fatal). The path is removed on exit ([`cleanup_socket`]).
#[cfg(unix)]
fn bind_socket(path: &Path) -> Result<Server, Box<dyn Error>> {
    match Server::bind_path(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // Bind failed because the socket file exists. If a daemon is actually listening, this
            // is a genuine second instance; otherwise the file is stale — remove it and rebind.
            if Client::connect_path(path).is_ok() {
                return Err(
                    format!("another deckhandd is already running on {}", path.display()).into()
                );
            }
            log::warn!("removing stale control socket {}", path.display());
            std::fs::remove_file(path)?;
            Ok(Server::bind_path(path)?)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(windows)]
fn bind_socket(name: &str) -> Result<Server, Box<dyn Error>> {
    Ok(Server::bind_name(name)?)
}

/// Read a RON profile and compile it to a `Program`, formatting any diagnostics.
fn load_program(path: &Path) -> Result<Program, Box<dyn Error>> {
    let doc: ConfigDoc = ron::from_str(&std::fs::read_to_string(path)?)?;
    compile(&doc).map_err(|diags| {
        let msg = format_diags(&diags).join("\n  ");
        Box::<dyn Error>::from(format!("{}: did not compile:\n  {msg}", path.display()))
    })
}

fn load_globals(path: &Path) -> Result<GlobalConfig, Box<dyn Error>> {
    Ok(ron::from_str(&std::fs::read_to_string(path)?)?)
}

fn cli_err(e: String) -> Box<dyn Error> {
    e.into()
}
