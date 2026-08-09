//! The **network** adapter of the transport seam (PLAN §6.1/§6.2 slice 3): hidden bridge threads and
//! sockets that carry the reader↔mapper channels over the wire, so the mapper's `select!` composes
//! over *local* in-process crossbeams exactly as in the loopback case.
//!
//! Two peers (roles chosen by the handle's input/output staging — slice 4):
//! - [`NetServer`] (output/PC side) **binds** TCP+UDP on one port and exposes the *mapper-side*
//!   channels: `frame_rx` (fed by UDP snapshots + TCP lifecycle events), `control_rx` (fed by TCP
//!   config), and `rumble_tx`/`click_tx` (drained to the client over UDP).
//! - [`NetClient`] (controller/Deck side) **dials** the server and exposes the *device-side*
//!   channels: `frame_tx` (Reports; `State`→UDP, lifecycle→TCP), `control_tx` (config→TCP), and
//!   `rumble_rx`/`click_rx` (fed by the UDP back-channel).
//!
//! A **single connection** for now (no reconnect yet — slice 5). Idempotency split per §6.1:
//! `State` snapshots ride UDP (latest-wins, `state.seq` drops stale); lifecycle + config ride TCP.
//! Wired into [`LinkClient`](super::LinkClient)/[`LinkServer`](super::LinkServer) as the `Network`
//! variants, which the handle selects via `set_output(Network)` / `set_input(Network)`.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, unbounded};

use steam_hid::Report;

use super::super::{Click, Control, RumbleCmd};
use super::wire::{self, Downlink, FramePacket, PROTOCOL_VERSION, Uplink};

/// Poll granularity for socket loops so a thread notices `running` cleared on shutdown.
const POLL: Duration = Duration::from_millis(200);
/// How often the client sends a TCP keep-alive `Ping` (to detect a dead server promptly).
const PING_INTERVAL: Duration = Duration::from_millis(1000);
/// Max UDP datagram we'll accept (a `ControllerState` is well under this; guards the recv buffer).
const UDP_BUF: usize = 2048;

// ---------------------------------------------------------------------------------------------
// Server (output/PC side)
// ---------------------------------------------------------------------------------------------

/// State shared between the server's bridge threads and its `Drop`.
struct ServerShared {
    running: AtomicBool,
    /// True when no client is connected — before the first `Hello` and between reconnects. The
    /// mapper's cue for `WaitingForDevice`; shared with the `Runtime` (via [`NetServer::detached`])
    /// so `Runtime::is_waiting` / `status()` reflect it too.
    detached: Arc<AtomicBool>,
    /// The client's UDP return address, learned from the first frame datagram (for the back-channel).
    client_udp: Mutex<Option<SocketAddr>>,
    /// A clone of the accepted TCP stream, so `Drop` can `shutdown` it to unblock the reader thread.
    tcp: Mutex<Option<TcpStream>>,
}

/// The mapper-side end of a network link. Owns the bridge threads; exposes the same channel surface
/// as the loopback `LinkServer` session.
pub(crate) struct NetServer {
    /// The actually-bound address (resolves an ephemeral `:0` port). Only the tests read it — a real
    /// server is given a fixed address — so it's `allow(dead_code)` rather than removed.
    #[allow(dead_code)]
    addr: SocketAddr,
    frame_rx: Receiver<Report>,
    control_rx: Receiver<Control>,
    /// A clone of the control sender the TCP thread feeds — exposed so the server's *own* handle can
    /// merge local `apply`/`set_globals` into the same stream as the client's wire config (the
    /// two-feeder `control_rx`, PLAN §6.1).
    control_tx: Sender<Control>,
    rumble_tx: Sender<RumbleCmd>,
    click_tx: Sender<Click>,
    shared: Arc<ServerShared>,
    threads: Vec<JoinHandle<()>>,
}

impl NetServer {
    /// Bind TCP+UDP on `addr` (port `0` picks an ephemeral one, exposed via [`Self::addr`]) and spawn
    /// the bridge threads. Serves clients one at a time, **re-accepting after each disconnect** so the
    /// server survives a client reconnect (the well-behaved server, PLAN §6.1).
    pub(super) fn bind(addr: SocketAddr) -> io::Result<NetServer> {
        let listener = TcpListener::bind(addr)?;
        let bound = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let udp = Arc::new(UdpSocket::bind(bound)?);
        udp.set_read_timeout(Some(POLL))?;

        let (frame_tx, frame_rx) = unbounded::<Report>();
        let (control_tx, control_rx) = unbounded::<Control>();
        let (rumble_tx, rumble_rx) = unbounded::<RumbleCmd>();
        let (click_tx, click_rx) = unbounded::<Click>();
        let shared = Arc::new(ServerShared {
            running: AtomicBool::new(true),
            detached: Arc::new(AtomicBool::new(true)), // no client yet
            client_udp: Mutex::new(None),
            tcp: Mutex::new(None),
        });

        let mut threads = Vec::new();
        threads.push(spawn("net-srv-tcp", {
            let (shared, frame_tx, ctl) = (shared.clone(), frame_tx.clone(), control_tx.clone());
            move || server_tcp(listener, &shared, &frame_tx, &ctl)
        }));
        threads.push(spawn("net-srv-udp", {
            let (shared, udp, frame_tx) = (shared.clone(), udp.clone(), frame_tx);
            move || server_udp(&udp, &shared, &frame_tx)
        }));
        threads.push(spawn("net-srv-back", {
            let shared = shared.clone();
            move || server_backchannel(&udp, &shared, &rumble_rx, &click_rx)
        }));

        Ok(NetServer {
            addr: bound,
            frame_rx,
            control_rx,
            control_tx,
            rumble_tx,
            click_tx,
            shared,
            threads,
        })
    }

    #[allow(dead_code)] // read only by tests (ephemeral-port bind); see the `addr` field.
    pub(super) fn addr(&self) -> SocketAddr {
        self.addr
    }
    pub(super) fn frame_rx(&self) -> &Receiver<Report> {
        &self.frame_rx
    }
    pub(super) fn control_rx(&self) -> &Receiver<Control> {
        &self.control_rx
    }
    pub(super) fn control_tx(&self) -> &Sender<Control> {
        &self.control_tx
    }
    /// True while no client is connected (before the first connect, and between reconnects).
    pub(super) fn is_detached(&self) -> bool {
        self.shared.detached.load(Ordering::SeqCst)
    }
    /// A shared handle to the detached flag, for `Runtime::is_waiting` / `status()`.
    pub(super) fn detached(&self) -> Arc<AtomicBool> {
        self.shared.detached.clone()
    }
    pub(super) fn rumble_tx(&self) -> &Sender<RumbleCmd> {
        &self.rumble_tx
    }
    pub(super) fn click_tx(&self) -> &Sender<Click> {
        &self.click_tx
    }
}

impl Drop for NetServer {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        // Unblock the TCP reader (a blocking `read_frame`) by shutting the stream.
        if let Some(s) = self.shared.tcp.lock().unwrap().take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Serve clients one at a time, re-accepting after each disconnect so the server survives a client
/// reconnect. Each connection is [`serve_connection`]; a disconnect flags `detached` + forgets the
/// old client's UDP address (an epoch boundary for the frame stream).
fn server_tcp(
    listener: TcpListener,
    shared: &ServerShared,
    frame_tx: &Sender<Report>,
    control_tx: &Sender<Control>,
) {
    while shared.running.load(Ordering::SeqCst) {
        let Some(mut stream) = accept_client(&listener, shared) else {
            return; // stopped
        };
        let peer = stream.peer_addr().ok();
        log::info!("net: client connected{}", peer.map(|a| format!(" from {a}")).unwrap_or_default());
        // Store a clone so `Drop` can unblock the blocking reads in `serve_connection`.
        match stream.try_clone() {
            Ok(c) => *shared.tcp.lock().unwrap() = Some(c),
            Err(_) => continue,
        }
        serve_connection(&mut stream, shared, frame_tx, control_tx);
        // Connection ended → back to detached; forget the client's UDP address so a stale datagram
        // can't reach the next client's back-channel.
        shared.detached.store(true, Ordering::SeqCst);
        *shared.client_udp.lock().unwrap() = None;
        log::info!("net: client disconnected — waiting for reconnect");
    }
}

/// Nonblocking-accept one client, or `None` if the server is stopping.
fn accept_client(listener: &TcpListener, shared: &ServerShared) -> Option<TcpStream> {
    loop {
        if !shared.running.load(Ordering::SeqCst) {
            return None;
        }
        match listener.accept() {
            Ok((s, _)) => return Some(s),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Pump one client connection: the first `Hello` (validated) marks it connected; then config →
/// `control_tx`, lifecycle events → `frame_tx`, until EOF/error. Blocking reads; `Drop` shuts the
/// stream down to unblock this loop.
fn serve_connection(
    stream: &mut TcpStream,
    shared: &ServerShared,
    frame_tx: &Sender<Report>,
    control_tx: &Sender<Control>,
) {
    let mut greeted = false;
    loop {
        if !shared.running.load(Ordering::SeqCst) {
            return;
        }
        match wire::read_frame::<Uplink>(stream) {
            Ok(Some(msg)) => match msg {
                Uplink::Hello { version } => {
                    if version != PROTOCOL_VERSION {
                        log::warn!("net: client protocol {version} != {PROTOCOL_VERSION} — dropping");
                        return;
                    }
                    greeted = true;
                    shared.detached.store(false, Ordering::SeqCst); // connected
                }
                _ if !greeted => {
                    log::warn!("net: client sent data before Hello — dropping");
                    return;
                }
                Uplink::Ping => {} // keep-alive — no-op (its arrival keeps this read loop live)
                Uplink::Apply { program, role } => {
                    let _ = control_tx.send(Control::Apply { program: program.map(Box::new), role });
                }
                Uplink::SetGlobals(g) => {
                    let _ = control_tx.send(Control::SetGlobals(Box::new(g)));
                }
                Uplink::Event(report) => {
                    let _ = frame_tx.send(report);
                }
            },
            Ok(None) => return, // clean EOF — client closed
            Err(_) => return,   // error or Drop-shutdown — done
        }
    }
}

/// Receive UDP frame snapshots, drop stale by `seq`, and forward as `Report::State`. Learns the
/// client's return address for the back-channel.
fn server_udp(udp: &UdpSocket, shared: &ServerShared, frame_tx: &Sender<Report>) {
    let mut buf = [0u8; UDP_BUF];
    let mut last_seq: Option<u32> = None;
    while shared.running.load(Ordering::SeqCst) {
        let (n, from) = match udp.recv_from(&mut buf) {
            Ok(x) => x,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
                continue;
            }
            Err(_) => continue,
        };
        // Between connections (detached), drop frames and reset the seq gate so the *next* client's
        // stream isn't gated by the previous one's sequence (epoch boundary, PLAN §6.1).
        if shared.detached.load(Ordering::SeqCst) {
            last_seq = None;
            continue;
        }
        let state: FramePacket = match wire::decode(&buf[..n]) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Latest-wins: drop out-of-order / duplicate datagrams.
        if last_seq.is_some_and(|s| state.seq <= s) {
            continue;
        }
        last_seq = Some(state.seq);
        *shared.client_udp.lock().unwrap() = Some(from);
        if frame_tx.send(Report::State(state)).is_err() {
            return; // mapper gone
        }
    }
}

/// Drain the mapper's rumble/click and send them to the client over the UDP back-channel (once its
/// address is known from an inbound frame).
fn server_backchannel(
    udp: &UdpSocket,
    shared: &ServerShared,
    rumble_rx: &Receiver<RumbleCmd>,
    click_rx: &Receiver<Click>,
) {
    while shared.running.load(Ordering::SeqCst) {
        let msg = select! {
            recv(rumble_rx) -> m => match m { Ok(r) => Downlink::Rumble(r), Err(_) => return },
            recv(click_rx) -> m => match m { Ok(c) => Downlink::Click(c), Err(_) => return },
            default(POLL) => continue,
        };
        let addr = *shared.client_udp.lock().unwrap();
        if let (Some(addr), Ok(bytes)) = (addr, wire::encode(&msg)) {
            let _ = udp.send_to(&bytes, addr);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Client (controller/Deck side)
// ---------------------------------------------------------------------------------------------

struct ClientShared {
    running: AtomicBool,
    /// True while the local device is gone. The reader sets it on device-loss (`detach`) and clears
    /// it on reacquire (`reattach`); the uplink thread only (re)dials while it's *false*, so a device
    /// outage drops the link (→ server `WaitingForDevice`) instead of reconnecting to nothing. Shared
    /// with the `Runtime` (via [`NetClient::detached`]) so `is_waiting`/`status()` report it too.
    detached: Arc<AtomicBool>,
    tcp: Mutex<Option<TcpStream>>,
}

/// (Re)dial the server: connect TCP and send `Hello`.
fn dial(server: SocketAddr) -> io::Result<TcpStream> {
    let mut tcp = TcpStream::connect(server)?;
    wire::write_frame(&mut tcp, &Uplink::Hello { version: PROTOCOL_VERSION })?;
    Ok(tcp)
}

/// The device-side end of a network link. Owns the bridge threads; exposes the same channel surface
/// as the loopback `LinkClient` session, plus a `control_tx` for the handle's config uplink.
pub(crate) struct NetClient {
    frame_tx: Sender<Report>,
    control_tx: Sender<Control>,
    rumble_rx: Receiver<RumbleCmd>,
    click_rx: Receiver<Click>,
    shared: Arc<ClientShared>,
    threads: Vec<JoinHandle<()>>,
}

impl NetClient {
    /// Dial the server: bind+connect UDP (so datagrams default to the server and the server learns
    /// our return address), connect TCP + `Hello`, and spawn the bridge threads. The uplink thread
    /// re-dials on its own after a drop (§6.1), so the initial dial here is just fail-fast.
    pub(super) fn connect(server: SocketAddr) -> io::Result<NetClient> {
        // Bind an ephemeral UDP port on the matching family and connect it to the server.
        let local: SocketAddr =
            if server.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
        let udp = Arc::new(UdpSocket::bind(local)?);
        udp.connect(server)?;
        udp.set_read_timeout(Some(POLL))?;

        let tcp = dial(server)?; // initial dial, fail-fast

        let (frame_tx, frame_rx) = unbounded::<Report>();
        let (control_tx, control_rx) = unbounded::<Control>();
        let (rumble_tx, rumble_rx) = unbounded::<RumbleCmd>();
        let (click_tx, click_rx) = unbounded::<Click>();
        let shared = Arc::new(ClientShared {
            running: AtomicBool::new(true),
            detached: Arc::new(AtomicBool::new(false)), // device present at connect
            tcp: Mutex::new(Some(tcp.try_clone()?)),
        });

        let mut threads = Vec::new();
        threads.push(spawn("net-cli-up", {
            let (udp, shared) = (udp.clone(), shared.clone());
            move || client_uplink(tcp, server, &udp, &shared, &frame_rx, &control_rx)
        }));
        threads.push(spawn("net-cli-down", {
            let shared = shared.clone();
            move || client_downlink(&udp, &shared, &rumble_tx, &click_tx)
        }));

        Ok(NetClient { frame_tx, control_tx, rumble_rx, click_rx, shared, threads })
    }

    pub(super) fn frame_tx(&self) -> &Sender<Report> {
        &self.frame_tx
    }
    pub(super) fn control_tx(&self) -> &Sender<Control> {
        &self.control_tx
    }
    pub(super) fn rumble_rx(&self) -> &Receiver<RumbleCmd> {
        &self.rumble_rx
    }
    pub(super) fn click_rx(&self) -> &Receiver<Click> {
        &self.click_rx
    }

    /// The local device went away → drop the connection so the server sees link-down (→
    /// `WaitingForDevice`), and don't reconnect until the device returns.
    pub(super) fn detach(&self) {
        self.shared.detached.store(true, Ordering::SeqCst);
        if let Some(tcp) = self.shared.tcp.lock().unwrap().as_ref() {
            let _ = tcp.shutdown(std::net::Shutdown::Both);
        }
    }

    /// The device returned → let the uplink thread re-dial the server.
    pub(super) fn reattach(&self) -> bool {
        self.shared.detached.store(false, Ordering::SeqCst);
        true
    }

    /// A shared handle to the "waiting" flag (local device gone), for `Runtime::is_waiting` /
    /// `status()`.
    pub(super) fn detached(&self) -> Arc<AtomicBool> {
        self.shared.detached.clone()
    }
}

impl Drop for NetClient {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        // Dropping frame_tx/control_tx disconnects the uplink thread's receivers; shut the TCP
        // stream too so a blocked write returns.
        if let Some(s) = self.shared.tcp.lock().unwrap().take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Split the reader's Reports and the handle's Control onto the wire (`State`→UDP, lifecycle/config→
/// TCP) and **re-dial** the server whenever the TCP link drops — but only while the device is present
/// (a device outage drops the link so the server waits). Exits on stop / the reader closing.
fn client_uplink(
    mut tcp: TcpStream,
    server: SocketAddr,
    udp: &UdpSocket,
    shared: &ClientShared,
    frame_rx: &Receiver<Report>,
    control_rx: &Receiver<Control>,
) {
    loop {
        match pump(&mut tcp, udp, shared, frame_rx, control_rx) {
            PumpEnd::Stop => return,
            PumpEnd::Broke => log::warn!("net: connection to {server} lost — reconnecting"),
        }
        // The link dropped — re-dial (only while running and the device is present).
        *shared.tcp.lock().unwrap() = None;
        loop {
            if !shared.running.load(Ordering::SeqCst) {
                return;
            }
            if shared.detached.load(Ordering::SeqCst) {
                thread::sleep(POLL); // device gone — nothing to forward, don't reconnect yet
                continue;
            }
            match dial(server) {
                Ok(new) => {
                    if let Ok(c) = new.try_clone() {
                        *shared.tcp.lock().unwrap() = Some(c);
                    }
                    tcp = new;
                    log::info!("net: (re)connected to {server}");
                    break;
                }
                Err(_) => thread::sleep(POLL), // server down — keep retrying
            }
        }
    }
}

/// Why [`pump`] returned.
enum PumpEnd {
    /// Clean stop — the reader/handle closed the channels, or `running` was cleared.
    Stop,
    /// The TCP link dropped (or the device went away) — the caller should re-dial.
    Broke,
}

/// Pump the current connection until the reader/handle close (→ `Stop`) or the TCP link drops / the
/// device goes away (→ `Broke`). A periodic `Ping` detects a dead server over the otherwise-idle TCP
/// (`State` frames ride UDP, which can't surface a broken peer).
fn pump(
    tcp: &mut TcpStream,
    udp: &UdpSocket,
    shared: &ClientShared,
    frame_rx: &Receiver<Report>,
    control_rx: &Receiver<Control>,
) -> PumpEnd {
    let mut last_ping = Instant::now();
    loop {
        if !shared.running.load(Ordering::SeqCst) {
            return PumpEnd::Stop;
        }
        if shared.detached.load(Ordering::SeqCst) {
            return PumpEnd::Broke; // device gone → drop the link (re-dial gated until it returns)
        }
        select! {
            recv(frame_rx) -> m => match m {
                Ok(Report::State(state)) => {
                    if let Ok(bytes) = wire::encode::<FramePacket>(&state) {
                        let _ = udp.send(&bytes);
                    }
                }
                // Connected / Disconnected / Battery → reliable TCP (never dropped).
                Ok(report) => {
                    if wire::write_frame(tcp, &Uplink::Event(report)).is_err() {
                        return PumpEnd::Broke;
                    }
                }
                Err(_) => return PumpEnd::Stop, // reader gone
            },
            recv(control_rx) -> m => {
                let msg = match m {
                    Ok(Control::Apply { program, role }) => Uplink::Apply { program: program.map(|p| *p), role },
                    Ok(Control::SetGlobals(g)) => Uplink::SetGlobals(*g),
                    Ok(Control::Stop) => return PumpEnd::Stop, // local stop
                    Err(_) => return PumpEnd::Stop,
                };
                if wire::write_frame(tcp, &msg).is_err() {
                    return PumpEnd::Broke;
                }
            },
            default(POLL) => {}
        }
        if last_ping.elapsed() >= PING_INTERVAL {
            if wire::write_frame(tcp, &Uplink::Ping).is_err() {
                return PumpEnd::Broke;
            }
            last_ping = Instant::now();
        }
    }
}

/// Receive the UDP back-channel and route it to the reader's rumble/click channels.
fn client_downlink(
    udp: &UdpSocket,
    shared: &ClientShared,
    rumble_tx: &Sender<RumbleCmd>,
    click_tx: &Sender<Click>,
) {
    let mut buf = [0u8; UDP_BUF];
    while shared.running.load(Ordering::SeqCst) {
        let n = match udp.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
                continue;
            }
            Err(_) => continue,
        };
        match wire::decode::<Downlink>(&buf[..n]) {
            Ok(Downlink::Rumble(r)) => {
                let _ = rumble_tx.send(r);
            }
            Ok(Downlink::Click(c)) => {
                let _ = click_tx.send(c);
            }
            Err(_) => {}
        }
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    thread::Builder::new().name(name.into()).spawn(f).expect("spawn net bridge thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{HapticStrength, Side};
    use steam_hid::ControllerState;

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    #[test]
    fn frames_control_and_backchannel_cross_the_wire() {
        let server = NetServer::bind(loopback()).unwrap();
        let client = NetClient::connect(server.addr()).unwrap();

        // Client → server: a controller snapshot (UDP) + a config apply is exercised via lifecycle.
        let state = ControllerState { seq: 7, left_trigger: 0.5, ..Default::default() };
        // UDP can (in principle) drop on loopback; resend a few times — latest-wins makes this safe.
        let recv_state = loop_send_until(&client, &server, Report::State(state.clone()));
        assert_eq!(recv_state, Report::State(state));

        // Client → server: a lifecycle event over reliable TCP.
        client.frame_tx().send(Report::Connected).unwrap();
        assert_eq!(server.frame_rx().recv_timeout(secs(2)).unwrap(), Report::Connected);

        // Server → client: rumble over the UDP back-channel (address learned from the frame above).
        let cmd = RumbleCmd { strong: 30000, weak: 0, hz: 80 };
        let got = loop_backchannel(&server, &client, cmd.clone());
        assert_eq!(got, cmd);

        // Server → client: a one-shot click.
        server.click_tx().send(Click { side: Side::Left, strength: HapticStrength::Medium }).unwrap();
        assert_eq!(
            client.click_rx().recv_timeout(secs(2)).unwrap(),
            Click { side: Side::Left, strength: HapticStrength::Medium }
        );
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Send a `State` frame (UDP) until the server surfaces it, tolerating loopback UDP loss.
    fn loop_send_until(client: &NetClient, server: &NetServer, report: Report) -> Report {
        for _ in 0..50 {
            client.frame_tx().send(report.clone()).unwrap();
            if let Ok(r) = server.frame_rx().recv_timeout(Duration::from_millis(100)) {
                return r;
            }
        }
        panic!("frame never arrived");
    }

    /// Send rumble (UDP back-channel) until the client surfaces it.
    fn loop_backchannel(server: &NetServer, client: &NetClient, cmd: RumbleCmd) -> RumbleCmd {
        for _ in 0..50 {
            server.rumble_tx().send(cmd.clone()).unwrap();
            if let Ok(r) = client.rumble_rx().recv_timeout(Duration::from_millis(100)) {
                return r;
            }
        }
        panic!("rumble never arrived");
    }

    /// Poll `cond` (up to ~3 s) — for the async connect/disconnect transitions.
    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        for _ in 0..300 {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn server_survives_client_reconnect() {
        let server = NetServer::bind(loopback()).unwrap();
        assert!(server.is_detached(), "no client yet");

        let client = NetClient::connect(server.addr()).unwrap();
        assert!(wait_until(|| !server.is_detached()), "server never registered the client");
        // A lifecycle event over reliable TCP flows.
        client.frame_tx().send(Report::Connected).unwrap();
        assert_eq!(server.frame_rx().recv_timeout(secs(2)).unwrap(), Report::Connected);

        // Disconnect → the server re-detaches.
        drop(client);
        assert!(wait_until(|| server.is_detached()), "server never noticed the disconnect");

        // Reconnect with a fresh client → the server re-accepts and frames resume on the same channel.
        let client2 = NetClient::connect(server.addr()).unwrap();
        assert!(wait_until(|| !server.is_detached()), "server never registered the reconnect");
        client2.frame_tx().send(Report::Connected).unwrap();
        assert_eq!(server.frame_rx().recv_timeout(secs(2)).unwrap(), Report::Connected);
    }

    #[test]
    fn client_redials_after_device_loss() {
        let server = NetServer::bind(loopback()).unwrap();
        let client = NetClient::connect(server.addr()).unwrap();
        assert!(wait_until(|| !server.is_detached()), "server never registered the client");

        // Device-loss on the client (reader `detach`) → it drops the link, the server sees link-down.
        client.detach();
        assert!(wait_until(|| server.is_detached()), "server never saw the client drop the link");

        // Device back (`reattach`) → the client re-dials, the server re-accepts, frames resume.
        client.reattach();
        assert!(wait_until(|| !server.is_detached()), "client never re-dialed after reattach");
        client.frame_tx().send(Report::Connected).unwrap();
        assert_eq!(server.frame_rx().recv_timeout(secs(2)).unwrap(), Report::Connected);
    }
}
