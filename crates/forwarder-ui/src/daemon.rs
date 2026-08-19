//! A thin blocking client over `ipc` plus the resilient connect loop — a trimmed copy of the main
//! UI's `daemon.rs`, with the forwarder's **fixed** connect policy baked in (no toggles):
//!
//! - **Launch the daemon** if it isn't running (always — the forwarder's whole job is to run a
//!   client on the Deck), spawned with `--prevent-sleep` so the Deck doesn't idle-sleep while
//!   forwarding (the flag is Linux-only; it's accepted-and-ignored elsewhere).
//! - **No** profile / chords loading.
//! - **Push the device config** (master rumble etc.) on every connect.
//! - **Restore the last input** (only) — the output is set from the text field at Start, never here.
//! - **Never auto-start** the engine — the user always presses Start.
//!
//! Same two-piece shape as the main UI: [`Client`] is a reconnecting command connection; the event
//! subscription runs on its own connection via [`run_event_loop`].

use std::io;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ipc::{Client as IpcClient, Event, Request, Response, StatusSnapshot};

use crate::settings::Settings;

/// Open a fresh connection to the daemon (default socket/pipe — the forwarder never overrides it).
#[cfg(unix)]
fn open(socket: Option<&str>) -> io::Result<IpcClient> {
    let path = socket
        .map(PathBuf::from)
        .unwrap_or_else(ipc::default_socket_path);
    IpcClient::connect_path(&path)
}
#[cfg(windows)]
fn open(socket: Option<&str>) -> io::Result<IpcClient> {
    IpcClient::connect_name(socket.unwrap_or(ipc::DEFAULT_PIPE_NAME))
}

/// A blocking command client to the daemon. Reconnects transparently after the daemon restarts.
pub(crate) struct Client {
    socket: Option<String>,
    conn: Option<IpcClient>,
}

impl Client {
    /// A client for the given socket/pipe override (`None` = the shared default). Does not connect
    /// until the first call.
    pub(crate) fn new(socket: Option<String>) -> Self {
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
    pub(crate) fn status(&mut self) -> io::Result<StatusSnapshot> {
        match self.call(Request::Status)? {
            Response::Status(s) => Ok(s),
            other => Err(unexpected(&other)),
        }
    }

    /// The daemon's currently-enumerated device ids.
    pub(crate) fn list_devices(&mut self) -> io::Result<Vec<String>> {
        match self.call(Request::ListDevices)? {
            Response::Devices(d) => Ok(d),
            other => Err(unexpected(&other)),
        }
    }

    /// Stage the input source (`auto|dongle|wired|bt|<device-id>`).
    pub(crate) fn set_input(&mut self, spec: String) -> io::Result<()> {
        expect_ok(self.call(Request::SetInput(spec))?)
    }

    /// Stage the output sink (`host:port` — the forwarder's network target).
    pub(crate) fn set_output(&mut self, spec: String) -> io::Result<()> {
        expect_ok(self.call(Request::SetOutput(spec))?)
    }

    /// Replace the daemon's device config (LED/idle, master rumble, frequency).
    pub(crate) fn set_device_config(
        &mut self,
        device_config: config::DeviceConfig,
    ) -> io::Result<()> {
        expect_ok(self.call(Request::SetDeviceConfig(device_config))?)
    }

    /// Acquire hardware and start the mapping loop.
    pub(crate) fn start(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Start)?)
    }

    /// Stop the mapping loop (release hardware, keep config).
    pub(crate) fn stop(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Stop)?)
    }

    /// Ask the daemon to exit (graceful teardown — restores lizard mode). Best-effort.
    pub(crate) fn shutdown(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Shutdown)?)
    }
}

/// Shared state for the UI-managed daemon: the child process we launched (if any) plus a `quitting`
/// flag, under one lock so the connect loop and the UI-exit path can't race.
#[derive(Default)]
pub(crate) struct Managed {
    child: Option<Child>,
    quitting: bool,
}

/// A handle to the [`Managed`] daemon, shared between the connect-loop thread and `main`.
pub(crate) type Handle = Arc<Mutex<Managed>>;

/// Create an empty [`Handle`] (no daemon launched yet, not quitting).
pub(crate) fn handle() -> Handle {
    Arc::new(Mutex::new(Managed::default()))
}

/// Whether the UI currently owns a launched daemon child (drives the "managed" status label).
pub(crate) fn is_managed(managed: &Handle) -> bool {
    managed.lock().unwrap().child.is_some()
}

/// On UI exit, gracefully stop the daemon we launched (if any). A daemon we did not launch is left
/// running.
pub(crate) fn shutdown_managed(managed: &Handle) {
    let child = {
        let mut m = managed.lock().unwrap();
        m.quitting = true;
        m.child.take()
    };
    let Some(mut child) = child else { return };
    if matches!(child.try_wait(), Ok(Some(_))) {
        return; // already gone (external kill) — nothing to stop
    }
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
pub(crate) enum DaemonUpdate {
    /// The event connection was (re)established — seed a fresh status / device list.
    Connected,
    /// The event connection dropped (daemon stopped / restarting).
    Disconnected,
    /// A daemon event.
    Event(Event),
    /// A non-fatal problem during a connect attempt (daemon launch failed, a setup call refused, …).
    Error(String),
}

const RETRY: Duration = Duration::from_millis(1000);

/// The resilient connect loop. Runs on a dedicated thread and reconnects forever with a short
/// backoff; returns when `on` returns `false` (the consumer is shutting down).
///
/// Every attempt performs the forwarder's fixed on-connect sequence — see [`run_on_connect`]. The
/// only setting it consults is the last input (re-read fresh from disk each attempt).
pub(crate) fn run_event_loop(
    socket: Option<String>,
    managed: Handle,
    mut on: impl FnMut(DaemonUpdate) -> bool,
) {
    loop {
        // Honor a quitting UI, and reap a managed daemon that died on us.
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

        let settings = Settings::load();

        // Connect. The subscribe connection is also the reachability probe.
        if let Ok(mut sub) = open(socket.as_deref()) {
            if sub.subscribe().is_ok() {
                if !on(DaemonUpdate::Connected) {
                    return;
                }
                run_on_connect(socket.as_deref(), &settings, &mut on);
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
        } else {
            // Not reachable → the forwarder always launches its own daemon.
            spawn_daemon(socket.as_deref(), &managed, &mut on);
        }

        std::thread::sleep(RETRY);
    }
}

/// The forwarder's fixed on-connect sequence, on a fresh command connection. Best-effort: each
/// failure is reported and the rest still runs.
///
/// 1. Push the device config (fresh from `devcfg.ron` — the master rumble etc.).
/// 2. Re-stage the last input (only; the output is set from the text field at Start).
///
/// No profile/chords load and no auto-start — those are the forwarder's non-negotiable policy.
fn run_on_connect(socket: Option<&str>, s: &Settings, on: &mut dyn FnMut(DaemonUpdate) -> bool) {
    let mut cmd = Client::new(socket.map(str::to_owned));
    report(on, cmd.set_device_config(crate::device::load()));
    if !s.last_input.is_empty() {
        report(on, cmd.set_input(s.last_input.clone()));
    }
}

/// Launch a UI-owned `deckhandd` (with `--prevent-sleep`) and record it as managed.
fn spawn_daemon(socket: Option<&str>, managed: &Handle, on: &mut dyn FnMut(DaemonUpdate) -> bool) {
    let mut m = managed.lock().unwrap();
    if m.quitting || m.child.is_some() {
        return;
    }
    let exe = daemon_bin();
    let mut cmd = std::process::Command::new(&exe);
    // The Deck acting as a network forwarder grabs the controller, so the compositor sees no local
    // input and would idle-sleep mid-session — hold an inhibitor for the daemon's lifetime. The flag
    // is Linux-only in effect (accepted-and-ignored off-Linux), so it's passed unconditionally.
    cmd.arg("--prevent-sleep");
    if let Some(s) = socket {
        cmd.arg("--socket").arg(s);
    }
    // GUI-subsystem app with no console (see main.rs) — run the console-subsystem daemon headless so
    // it doesn't pop a console window.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        Ok(c) => m.child = Some(c),
        Err(e) => {
            on(DaemonUpdate::Error(format!(
                "launch daemon ({}): {e}",
                exe.display()
            )));
        }
    }
}

/// Resolve the `deckhandd` binary: next to this executable, falling back to the bare name on `PATH`.
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

/// Report a command result: nothing on success, a [`DaemonUpdate::Error`] on failure.
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
