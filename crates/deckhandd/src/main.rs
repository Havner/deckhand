//! `deckhandd` - the deckhand control daemon (PLAN 4.4).
//!
//! One binary, two uses: a **standalone runner** (pass everything on the CLI, Ctrl-C to quit) and
//! a **controllable daemon** (a client drives it over the control socket). It always opens the
//! socket; the CLI just *seeds* the engine, then clients mutate it live.
//!
//! Seeding is pass-through - only options actually given are applied - with one convenience:
//! `--start` defaults a missing `-i`/`-o` to `auto`/`local` so the engine has a source/sink.
//! Profiles are never defaulted, so `--start` with no `-m` logs a `NotReady` and keeps serving.

mod daemon;
#[cfg(target_os = "linux")]
mod inhibit;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use config::{Chords, ConfigDoc, DeviceConfig};
use engine::{EventStream, Program, Role, compile};
use ipc::{Client, Conn, Request, Response, Server};

use daemon::{Daemon, format_diags, to_wire_event};

/// The deckhand control daemon: runs the headless engine and serves a control socket.
#[derive(Parser)]
#[command(name = "deckhandd", version, about)]
struct Args {
    /// List available devices and quit (the one non-persistent option - no socket is served).
    #[arg(short = 'l', long)]
    list_devices: bool,
    /// Main profile (RON) -> applied to the Main role.
    #[arg(short, long, value_name = "RON")]
    main: Option<PathBuf>,
    /// Fallback profile (RON) -> applied to the Fallback role.
    #[arg(short, long, value_name = "RON")]
    fallback: Option<PathBuf>,
    /// Chords (RON): the top-level switch/command chords.
    #[arg(short = 'c', long, value_name = "RON")]
    chords: Option<PathBuf>,
    /// Device config (RON): LED/idle, master rumble, frequency.
    #[arg(short = 'd', long, value_name = "RON")]
    devcfg: Option<PathBuf>,
    /// Input source: auto | dongle | wired | bt | <device-id> | host:port.
    ///
    /// A <device-id> is `kind:transport:interface:serial` - e.g. `gordon:dongle:1:` (see
    /// `deckhandctl list-devices`).
    #[arg(short, long, value_name = "SPEC")]
    input: Option<String>,
    /// Output sink: local | host:port.
    #[arg(short, long, value_name = "SPEC")]
    output: Option<String>,
    /// Acquire hardware and start immediately (defaults a missing -i/-o to auto/local).
    #[arg(short, long)]
    start: bool,
    /// Prevent auto-suspend while running (Linux only).
    #[arg(short = 'p', long, value_name = "MODE", num_args = 0..=1, default_missing_value = "auto")]
    prevent_sleep: Option<PreventSleep>,
    /// Control socket path (Unix) / pipe name (Windows). Overrides $DECKHAND_SOCKET and the default.
    #[arg(short = 'k', long, value_name = "PATH")]
    socket: Option<String>,
    /// Run under systemd socket activation (Linux user unit).
    #[arg(long)]
    systemd: bool,
    /// Increase log verbosity: -v info, -vv debug, -vvv trace (default warn). RUST_LOG overrides.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

/// `--prevent-sleep` backend. See `inhibit.rs` for what each holds. The enum is always compiled
/// (the flag parses on every platform, accepted-and-ignored off-Linux); it's consumed only on Linux.
#[derive(Copy, Clone, Debug, clap::ValueEnum)]
#[allow(dead_code)] // fields read only on Linux (inhibit.rs is cfg(linux))
pub enum PreventSleep {
    /// Try powermanagement, then gnome, then login1 (skips screensaver). Screen still blanks.
    Auto,
    /// org.freedesktop.ScreenSaver - inhibits idle, so also stops screen blanking.
    Screensaver,
    /// org.freedesktop.PowerManagement.Inhibit - suspend only (KDE/XFCE/MATE).
    Powermanagement,
    /// org.gnome.SessionManager (flags=4) - suspend only (GNOME).
    Gnome,
    /// org.freedesktop.login1 block sleep - suspend only, system bus, any systemd host.
    Login1,
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
    let mut logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(args.log_level()));
    // Under systemd the journal already timestamps every line - drop env_logger's own to avoid
    // duplicating it.
    if args.systemd {
        logger.format_timestamp(None);
    }
    logger.init();

    // Hold a sleep/idle inhibitor for the daemon's lifetime (best-effort). Kept in a binding so it
    // lives across `serve` and releases on a clean exit.
    #[cfg(target_os = "linux")]
    let _inhibitor = args.prevent_sleep.and_then(inhibit::SleepInhibitor::acquire);
    #[cfg(not(target_os = "linux"))]
    if args.prevent_sleep.is_some() {
        log::warn!("--prevent-sleep is only supported on Linux - ignored");
    }

    let mut daemon = Daemon::new();

    // --- --list-devices: the one non-persistent option - enumerate, print (same format as
    // `deckhandctl list-devices`), and quit before any seeding or socket is served. -------------
    if args.list_devices {
        return match daemon.devices() {
            Ok(ids) => {
                print_devices(&ids);
                Ok(())
            }
            Err(e) => Err(e.into()),
        };
    }

    // --- CLI seeding: pass-through - apply only what was given (PLAN 4.4). ------------------
    if let Some(p) = &args.main {
        daemon.apply(Role::Main, load_program(p)?);
        log::info!("main profile: {}", p.display());
    }
    if let Some(p) = &args.fallback {
        daemon.apply(Role::Fallback, load_program(p)?);
        log::info!("fallback profile: {}", p.display());
    }
    if let Some(p) = &args.chords {
        daemon.set_chords(Some(load_chords(p)?));
        log::info!("chords: {}", p.display());
    }
    if let Some(p) = &args.devcfg {
        daemon.set_device_config(load_device_config(p)?);
        log::info!("devcfg: {}", p.display());
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

    // In systemd mode the server is *adopted* from the LISTEN_FDS socket (its real bound path comes
    // back too, for the log + the Ctrl-C self-connect wake); otherwise we bind our own here and the
    // target is the resolved `--socket`/default.
    let (server, target) = if args.systemd {
        adopt_systemd_socket()?
    } else {
        let target = resolve_socket(args.socket.as_deref());
        (bind_socket(&target)?, target)
    };

    serve(daemon, server, target, args.systemd)
}

/// Adopt the listening control socket systemd passed via `LISTEN_FDS` (socket activation), plus its
/// real bound path (from `getsockname`, for logging + the shutdown wake). The fd is validated + set
/// close-on-exec by [`sd_notify::listen_fds`]; we require **exactly one**. The daemon never binds or
/// removes this socket - systemd owns its lifecycle. Linux only.
#[cfg(target_os = "linux")]
fn adopt_systemd_socket() -> Result<(Server, SocketTarget), Box<dyn Error>> {
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixListener;

    let mut fds = sd_notify::listen_fds().map_err(|e| format!("--systemd: reading LISTEN_FDS: {e}"))?;
    let fd = fds.next().ok_or(
        "--systemd: no socket passed by systemd (LISTEN_FDS unset). Start via deckhandd.socket, \
         or a service with Requires=deckhandd.socket",
    )?;
    if fds.next().is_some() {
        return Err("--systemd: more than one socket passed by systemd; expected exactly one".into());
    }
    // SAFETY: `fd` is the listening AF_UNIX socket systemd bound and handed us; nothing else owns
    // it, and `listen_fds` already set it close-on-exec.
    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    // Use the socket's actual bound path (not the default name) so the wake self-connect reaches
    // *this* socket regardless of the unit's ListenStream; fall back to the default if unnamed.
    let target = listener
        .local_addr()
        .ok()
        .and_then(|a| a.as_pathname().map(Path::to_path_buf))
        .unwrap_or_else(|| resolve_socket(None));
    Ok((Server::from_unix_listener(listener), target))
}

#[cfg(not(target_os = "linux"))]
fn adopt_systemd_socket() -> Result<(Server, SocketTarget), Box<dyn Error>> {
    Err("--systemd is only supported on Linux".into())
}

/// The socket the daemon serves, resolved from `--socket` (`SocketTarget` is a filesystem path on
/// Unix, a pipe name on Windows).
#[cfg(unix)]
type SocketTarget = PathBuf;
#[cfg(windows)]
type SocketTarget = String;

/// Resolve the `--socket` override: `None` (unset) -> the env/default ([`ipc::default_socket_path`],
/// honoring `$DECKHAND_SOCKET`); otherwise the given path / pipe name.
#[cfg(unix)]
fn resolve_socket(over: Option<&str>) -> SocketTarget {
    over.map(PathBuf::from).unwrap_or_else(ipc::default_socket_path)
}
#[cfg(windows)]
fn resolve_socket(over: Option<&str>) -> SocketTarget {
    over.map(str::to_string).unwrap_or_else(|| ipc::DEFAULT_PIPE_NAME.to_string())
}

/// Run the accept/serve loop on an already-bound (or systemd-adopted) `server` until Shutdown or
/// Ctrl-C/SIGTERM. `target` is the socket's path/name (for logging + the Ctrl-C wake); `systemd`
/// selects the socket-activation lifecycle - signal `sd-notify` readiness, and leave the socket
/// file for systemd to reap (no [`cleanup_socket`]).
///
/// Each accepted connection is served on its **own thread** over a shared `Arc<Mutex<Daemon>>`
/// (PLAN 4.4): a request briefly locks the engine, so multiple clients - a UI holding a persistent
/// command connection, an event-stream subscriber, and an occasional `deckhandctl` - are served
/// concurrently and a long-lived connection never wedges the others. Engine access stays serialized
/// by the mutex (handling is fast; the mapping loop runs on its own threads regardless).
fn serve(
    daemon: Daemon,
    server: Server,
    target: SocketTarget,
    systemd: bool,
) -> Result<(), Box<dyn Error>> {
    log::info!("listening on {}", display_target(&target));

    // Tell systemd we're ready to accept (Type=notify). Only reachable on Linux (adopt errors
    // elsewhere), so the sd-notify call is Linux-gated.
    #[cfg(target_os = "linux")]
    if systemd && let Err(e) = sd_notify::notify(&[sd_notify::NotifyState::Ready]) {
        log::warn!("sd_notify READY: {e}");
    }

    let running = Arc::new(AtomicBool::new(true));
    let daemon = Arc::new(Mutex::new(daemon));
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
        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                log::warn!("accept: {e}");
                continue;
            }
        };
        let daemon = daemon.clone();
        let running = running.clone();
        let target = target.clone();
        std::thread::spawn(move || serve_conn(conn, daemon, running, target));
    }

    // Accept loop ended (Shutdown or Ctrl-C/SIGTERM). Detached per-connection threads exit with the
    // process; we release hardware here so lizard/pad are restored on a clean exit.
    if systemd {
        // The socket is systemd's - leave the file in place; just tell systemd we're going down.
        #[cfg(target_os = "linux")]
        let _ = sd_notify::notify(&[sd_notify::NotifyState::Stopping]);
    } else {
        // We created the socket file - remove it.
        cleanup_socket(&target);
    }
    log::info!("shutting down - releasing controller and unplugging virtual pad");
    if let Err(e) = daemon.lock().expect("daemon mutex poisoned").shutdown() {
        log::warn!("shutdown: {e}");
    }
    Ok(())
}

/// Serve one client connection to completion (its own thread). Runs the request/reply loop, locking
/// the shared engine per request; a `Subscribe` turns this thread into the client's event pump, and
/// a `Shutdown` flips `running` and wakes the accept loop so the daemon exits.
fn serve_conn(
    mut conn: Conn,
    daemon: Arc<Mutex<Daemon>>,
    running: Arc<AtomicBool>,
    target: SocketTarget,
) {
    loop {
        match conn.recv() {
            Ok(Some(Request::Shutdown)) => {
                let _ = conn.reply(&Response::Ok);
                log::info!("shutdown requested by client");
                running.store(false, Ordering::Relaxed);
                wake(&target); // unblock the accept loop so it observes !running and exits
                break;
            }
            Ok(Some(Request::Subscribe)) => {
                log::info!("client subscribed to the event stream");
                let stream = daemon.lock().expect("daemon mutex poisoned").subscribe();
                monitor(conn, stream); // runs the event stream on this thread until the client goes
                break;
            }
            Ok(Some(req)) => {
                let resp = daemon.lock().expect("daemon mutex poisoned").handle(req);
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

/// Stream engine events to a subscribed client (D7) until the engine goes away (all senders dropped
/// -> `recv` returns `None`) or the client disconnects (a send fails). Called from the connection's
/// own serve thread, which it takes over for the stream's lifetime.
fn monitor(mut conn: Conn, stream: EventStream) {
    while let Some(ev) = stream.recv() {
        if conn.send_event(&to_wire_event(ev)).is_err() {
            break; // client gone
        }
    }
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
            // is a genuine second instance; otherwise the file is stale - remove it and rebind.
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

fn load_chords(path: &Path) -> Result<Chords, Box<dyn Error>> {
    Ok(ron::from_str(&std::fs::read_to_string(path)?)?)
}

fn load_device_config(path: &Path) -> Result<DeviceConfig, Box<dyn Error>> {
    Ok(ron::from_str(&std::fs::read_to_string(path)?)?)
}

fn cli_err(e: String) -> Box<dyn Error> {
    e.into()
}

/// Print the enumerated device ids to stdout, one per line (same format as `deckhandctl
/// list-devices`).
fn print_devices(ids: &[String]) {
    if ids.is_empty() {
        println!("no devices");
        return;
    }
    for id in ids {
        println!("{id}");
    }
}
