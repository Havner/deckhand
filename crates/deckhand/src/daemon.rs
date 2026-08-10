//! A thin, blocking client over `ipc` driving the top/bottom bars, plus the resilient connect loop
//! that owns the whole "on connect attempt" behaviour.
//!
//! Two pieces, because the daemon's socket serves *either* request/reply *or* a terminal event
//! stream per connection:
//!
//! - [`Client`] holds a **command** connection (status / list-devices / apply / set-input/output /
//!   start / stop). It lazily connects and, on any I/O error, drops the connection so the next call
//!   reconnects — so a daemon that comes and goes just works.
//! - [`run_event_loop`] owns a **second** connection subscribed to the event stream. It runs on a
//!   dedicated thread (the iced subscription bridges its callback into the async event pump) and
//!   reconnects forever. Each attempt runs the on-connect sequence — see [`run_event_loop`].
//!
//! The loop reads [`AppSettings`] fresh from disk on every attempt (the UI persists them on every
//! edit), so option changes take effect on the next retry with no shared state between threads.

use std::io;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::ConfigDoc;
use ipc::{Client as IpcClient, Event, ProfileRole, Request, Response, StatusSnapshot};

use crate::settings::AppSettings;

/// Open a fresh connection to the daemon (honoring the optional socket/pipe override, else the
/// shared default — the same resolution `deckhandctl` uses).
#[cfg(unix)]
fn open(socket: Option<&str>) -> io::Result<IpcClient> {
    let path = socket.map(PathBuf::from).unwrap_or_else(ipc::default_socket_path);
    IpcClient::connect_path(&path)
}
#[cfg(windows)]
fn open(socket: Option<&str>) -> io::Result<IpcClient> {
    IpcClient::connect_name(socket.unwrap_or(ipc::DEFAULT_PIPE_NAME))
}

/// A blocking command client to the daemon. Reconnects transparently after the daemon restarts.
pub struct Client {
    socket: Option<String>,
    conn: Option<IpcClient>,
}

impl Client {
    /// A client for the given socket/pipe override (`None` = the shared default). Does not connect
    /// until the first call.
    pub fn new(socket: Option<String>) -> Self {
        Client { socket, conn: None }
    }

    /// Send one request, (re)connecting as needed; on any I/O failure the connection is dropped so
    /// the next call starts fresh.
    fn call(&mut self, req: Request) -> io::Result<Response> {
        if self.conn.is_none() {
            self.conn = Some(open(self.socket.as_deref())?);
        }
        let result = self.conn.as_mut().unwrap().call(&req);
        if result.is_err() {
            self.conn = None;
        }
        result
    }

    /// The current engine status.
    pub fn status(&mut self) -> io::Result<StatusSnapshot> {
        match self.call(Request::Status)? {
            Response::Status(s) => Ok(s),
            other => Err(unexpected(&other)),
        }
    }

    /// The daemon's currently-enumerated device ids.
    pub fn list_devices(&mut self) -> io::Result<Vec<String>> {
        match self.call(Request::ListDevices)? {
            Response::Devices(d) => Ok(d),
            other => Err(unexpected(&other)),
        }
    }

    /// Stage the input source (spec string — `auto|dongle|wired|bt|<device-id>|host:port`).
    pub fn set_input(&mut self, spec: String) -> io::Result<()> {
        expect_ok(self.call(Request::SetInput(spec))?)
    }

    /// Stage the output sink (spec string — `local|host:port`).
    pub fn set_output(&mut self, spec: String) -> io::Result<()> {
        expect_ok(self.call(Request::SetOutput(spec))?)
    }

    /// Replace the daemon's global config (rumble master, boot role, LED/idle, chords).
    pub fn set_globals(&mut self, globals: config::GlobalConfig) -> io::Result<()> {
        expect_ok(self.call(Request::SetGlobals(Box::new(globals)))?)
    }

    /// Acquire hardware and start the mapping loop.
    pub fn start(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Start)?)
    }

    /// Stop the mapping loop (release hardware, keep config).
    pub fn stop(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Stop)?)
    }

    /// Apply a profile to a role, or clear it (`config: None`). Compile diagnostics come back as
    /// [`Response::Diagnostics`] — folded into the error string so callers see one uniform result.
    pub fn apply(&mut self, role: ProfileRole, config: Option<Box<ConfigDoc>>) -> io::Result<()> {
        match self.call(Request::Apply { role, config })? {
            Response::Ok => Ok(()),
            Response::Diagnostics(diags) => Err(io::Error::other(diags.join("; "))),
            other => Err(unexpected(&other)),
        }
    }

    /// Ask the daemon to exit (full teardown). Graceful — it runs its clean shutdown (restores
    /// lizard mode), unlike `Child::kill`. The reply may not arrive (the daemon can close first), so
    /// callers treat this as best-effort.
    pub fn shutdown(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Shutdown)?)
    }
}

/// Shared state for the UI-managed daemon: the child process we launched (if any) plus a `quitting`
/// flag. Both live under one lock so the connect loop and the UI-exit path can't race — the loop
/// never spawns a replacement daemon once the UI has begun tearing down.
#[derive(Default)]
pub struct Managed {
    child: Option<Child>,
    quitting: bool,
}

/// A handle to the [`Managed`] daemon, shared between the connect-loop thread and `main` (which owns
/// the UI-exit teardown).
pub type Handle = Arc<Mutex<Managed>>;

/// Create an empty [`Handle`] (no daemon launched yet, not quitting).
pub fn handle() -> Handle {
    Arc::new(Mutex::new(Managed::default()))
}

/// Whether the UI currently owns a launched daemon child — i.e. we started it and will shut it down
/// on exit. Drives the "managed" vs plain "connected" status label.
pub fn is_managed(managed: &Handle) -> bool {
    managed.lock().unwrap().child.is_some()
}

/// On UI exit, gracefully stop the daemon we launched (if any). Flags `quitting` and takes the child
/// atomically so the loop can't spawn a replacement mid-exit; then asks the daemon to exit over IPC
/// and reaps it. A daemon we did *not* launch is left running.
pub fn shutdown_managed(managed: &Handle) {
    let child = {
        let mut m = managed.lock().unwrap();
        m.quitting = true;
        m.child.take()
    };
    let Some(mut child) = child else { return };
    if matches!(child.try_wait(), Ok(Some(_))) {
        return; // already gone (external kill) — nothing to stop
    }
    // Uses the default socket — the UI never overrides it (there's no UI to set one).
    let _ = Client::new(None).shutdown();
    // Bounded wait so a wedged daemon can't hang UI exit; SIGKILL only as a last resort (it skips
    // the daemon's cleanup, leaving the controller in lizard-off).
    for _ in 0..40 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// An update pushed by [`run_event_loop`]: connection lifecycle, each daemon [`Event`], plus
/// non-fatal errors from the connect/setup sequence.
#[derive(Debug, Clone)]
pub enum DaemonUpdate {
    /// The event connection was (re)established — the consumer should seed a fresh status /
    /// device list, since the stream itself carries only deltas.
    Connected,
    /// The event connection dropped (daemon stopped / restarting).
    Disconnected,
    /// A daemon event.
    Event(Event),
    /// A non-fatal problem during a connect attempt (daemon launch failed, a profile failed to
    /// load/apply, start refused, …) — surfaced in the status bar; the loop keeps going.
    Error(String),
}

const RETRY: Duration = Duration::from_millis(1000);

/// The resilient connect loop. Runs on a dedicated thread and reconnects forever with a short
/// backoff; returns when `on` returns `false` (the consumer is shutting down).
///
/// Every attempt performs the "on connect attempt" sequence, re-reading [`AppSettings`] from disk
/// each time so the user's latest toggles apply on the next retry:
///
/// 0. Reap a daemon *we* launched that has since died (external kill / crash): drop the managed
///    handle so a daemon that appeared externally isn't mistaken for ours, and we're free to
///    relaunch. (Also the point where a `quitting` UI stops the loop.)
/// 1. Try to connect to a running daemon (the subscribe connection doubles as the probe) — this may
///    be one we launched, or an external one that appeared while ours was down.
/// 2. If it isn't reachable: launch one when *Launch the daemon* is set (a UI-owned child process,
///    marked managed, never double-spawned), else just retry next tick.
/// 3. Once connected: load the Main profile from its path (empty path → clear the role).
/// 4. Same for the Fallback profile.
/// 5. Push the app's saved global config (unconditional), read fresh from `globals.ron`.
/// 6. Start the engine when *Start the engine* is set.
///
/// Steps 3–6 run on a **separate** command connection (subscribe is terminal) and are best-effort —
/// each failure is reported as [`DaemonUpdate::Error`] and the sequence continues.
pub fn run_event_loop(socket: Option<String>, managed: Handle, mut on: impl FnMut(DaemonUpdate) -> bool) {
    loop {
        // Step 0: honor a quitting UI, and reap a managed daemon that died on us. Same lock as the
        // UI-exit path, so we never relaunch once teardown has started.
        {
            let mut m = managed.lock().unwrap();
            if m.quitting {
                return;
            }
            if let Some(c) = m.child.as_mut()
                && matches!(c.try_wait(), Ok(Some(_)) | Err(_))
            {
                m.child = None;
            }
        }

        let settings = AppSettings::load();

        // Step 1: connect. The subscribe connection is also the reachability probe.
        if let Ok(mut sub) = open(socket.as_deref()) {
            if sub.subscribe().is_ok() {
                // Announce the connection first: its handler clears stale errors and seeds status,
                // so running the setup *after* it lets setup errors stick (and the seed + the
                // setup's own events reconcile the bars, since events are absolute-valued).
                if !on(DaemonUpdate::Connected) {
                    return;
                }
                // Steps 3–6, on their own command connection.
                run_on_connect(socket.as_deref(), &settings, &mut on);

                // Loops while events arrive; a clean close or I/O error ends it → reconnect.
                while let Ok(Some(ev)) = sub.next_event() {
                    if !on(DaemonUpdate::Event(ev)) {
                        return;
                    }
                }
                if !on(DaemonUpdate::Disconnected) {
                    return;
                }
            }
            // subscribe failed (raced the daemon going away) — fall through and retry.
        } else if settings.start_daemon {
            // Step 2: daemon not reachable and the user asked us to launch one.
            spawn_daemon(socket.as_deref(), &managed, &mut on);
        }

        // Daemon absent / stream dropped / just-launched and still binding — wait before retrying.
        std::thread::sleep(RETRY);
    }
}

/// Steps 3–6 of a connect attempt, on a fresh command connection (the just-subscribed daemon is
/// reachable). Best-effort: each failure is reported and the rest still runs.
fn run_on_connect(socket: Option<&str>, s: &AppSettings, on: &mut dyn FnMut(DaemonUpdate) -> bool) {
    let mut cmd = Client::new(socket.map(str::to_owned));

    if s.load_main {
        apply_profile(&mut cmd, ProfileRole::Main, &s.main_path, on);
    }
    if s.load_fallback {
        apply_profile(&mut cmd, ProfileRole::Fallback, &s.fallback_path, on);
    }
    // Step 5 (unconditional): push the app's saved global config, read fresh from disk (the UI
    // owns it — see `crate::globals`; it stays in lock-step with the file and the daemon).
    report(on, cmd.set_globals(crate::globals::load()));

    // Restore the last-used input/output (before start, so they take effect at start).
    if s.restore_io {
        if !s.last_input.is_empty() {
            report(on, cmd.set_input(s.last_input.clone()));
        }
        if !s.last_output.is_empty() {
            report(on, cmd.set_output(s.last_output.clone()));
        }
    }
    if s.start_engine {
        report(on, cmd.start());
    }
}

/// Apply one profile role from a path: an empty path clears the role, otherwise the RON is loaded
/// and shipped. Load/apply failures are reported and skip that role (never a partial clear).
fn apply_profile(
    cmd: &mut Client,
    role: ProfileRole,
    path: &str,
    on: &mut dyn FnMut(DaemonUpdate) -> bool,
) {
    let config = if path.is_empty() {
        None
    } else {
        match load_doc(path) {
            Ok(doc) => Some(Box::new(doc)),
            Err(e) => {
                on(DaemonUpdate::Error(e));
                return;
            }
        }
    };
    report(on, cmd.apply(role, config));
}

/// Launch a UI-owned `deckhandd` and record it as managed. Does nothing if the UI is quitting or a
/// daemon we launched is still alive (step 0 has already reaped any dead one, so `child.is_some()`
/// here means it's alive — just-launched and still binding; the connect retries next tick). The
/// lock is held across the spawn so the UI-exit path adopts the fresh child rather than orphaning it.
fn spawn_daemon(socket: Option<&str>, managed: &Handle, on: &mut dyn FnMut(DaemonUpdate) -> bool) {
    let mut m = managed.lock().unwrap();
    if m.quitting || m.child.is_some() {
        return;
    }
    let exe = daemon_bin();
    let mut cmd = std::process::Command::new(&exe);
    if let Some(s) = socket {
        cmd.arg("--socket").arg(s);
    }
    // The UI is a GUI-subsystem app with no console (see main.rs), so spawning the console-subsystem
    // daemon would otherwise pop a fresh console window for it. `CREATE_NO_WINDOW` runs it headless.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        Ok(c) => m.child = Some(c),
        Err(e) => {
            on(DaemonUpdate::Error(format!("launch daemon ({}): {e}", exe.display())));
        }
    }
}

/// Resolve the `deckhandd` binary: next to this executable (dev target dir + installed layouts keep
/// the two binaries side by side), falling back to the bare name on `PATH`.
fn daemon_bin() -> PathBuf {
    let name = format!("deckhandd{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(&name);
        if sibling.exists() {
            return sibling;
        }
    }
    PathBuf::from(name)
}

/// Read + parse a profile RON into a [`ConfigDoc`] (mirrors `deckhandctl`'s loader).
fn load_doc(path: &str) -> Result<ConfigDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    ron::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

/// Report a command result to the consumer: nothing on success, a [`DaemonUpdate::Error`] on
/// failure. The `bool` from `on` (consumer shutting down) is ignored — setup is a short burst.
fn report(on: &mut dyn FnMut(DaemonUpdate) -> bool, r: io::Result<()>) {
    if let Err(e) = r {
        on(DaemonUpdate::Error(e.to_string()));
    }
}

/// Map an `Ok`/`Error` reply to `()`/an error.
fn expect_ok(resp: Response) -> io::Result<()> {
    match resp {
        Response::Ok => Ok(()),
        Response::Error(msg) => Err(io::Error::other(msg)),
        other => Err(unexpected(&other)),
    }
}

fn unexpected(resp: &Response) -> io::Error {
    io::Error::other(format!("unexpected reply: {resp:?}"))
}
