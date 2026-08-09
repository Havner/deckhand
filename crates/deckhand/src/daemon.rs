//! A thin, blocking client over `ipc` driving the top/bottom bars.
//!
//! Two pieces, because the daemon's socket serves *either* request/reply *or* a terminal event
//! stream per connection:
//!
//! - [`Client`] holds a **command** connection (status / list-devices / set-input / set-output /
//!   start / stop). It lazily connects and, on any I/O error, drops the connection so the next call
//!   reconnects — so a daemon that comes and goes just works.
//! - [`run_event_loop`] owns a **second** connection subscribed to the event stream, forwarding
//!   updates through a callback and reconnecting forever. It runs on a dedicated thread; the iced
//!   subscription bridges its callback into the async event pump.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use ipc::{Client as IpcClient, Event, Request, Response, StatusSnapshot};

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

    /// Acquire hardware and start the mapping loop.
    pub fn start(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Start)?)
    }

    /// Stop the mapping loop (release hardware, keep config).
    pub fn stop(&mut self) -> io::Result<()> {
        expect_ok(self.call(Request::Stop)?)
    }
}

/// An update pushed by [`run_event_loop`]: connection lifecycle plus each daemon [`Event`].
#[derive(Debug, Clone)]
pub enum DaemonUpdate {
    /// The event connection was (re)established — the consumer should seed a fresh status /
    /// device list, since the stream itself carries only deltas.
    Connected,
    /// The event connection dropped (daemon stopped / restarting).
    Disconnected,
    /// A daemon event.
    Event(Event),
}

/// Resilient blocking subscribe loop: connect, subscribe, forward every [`DaemonUpdate`] to `on`,
/// and reconnect forever with a short backoff. Returns when `on` returns `false` (the consumer is
/// shutting down). Intended to run on a dedicated thread.
pub fn run_event_loop(socket: Option<String>, mut on: impl FnMut(DaemonUpdate) -> bool) {
    loop {
        if let Ok(mut client) = open(socket.as_deref())
            && client.subscribe().is_ok()
        {
            if !on(DaemonUpdate::Connected) {
                return;
            }
            // Loops while events arrive; a clean close or I/O error ends it → reconnect.
            while let Ok(Some(ev)) = client.next_event() {
                if !on(DaemonUpdate::Event(ev)) {
                    return;
                }
            }
            if !on(DaemonUpdate::Disconnected) {
                return;
            }
        }
        // Daemon absent or the stream dropped — wait before retrying so we don't busy-spin.
        std::thread::sleep(Duration::from_millis(1000));
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
