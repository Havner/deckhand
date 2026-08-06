//! The transport seam between the reader (device side) and the mapping loop (mapper side) — PLAN
//! §6.1. Today this is a **loopback link**: both ends are co-located in one process, wired by
//! in-process crossbeams. The network adapter (later) will add a second construction path behind the
//! same [`LinkClient`]/[`LinkServer`] method surface, so `reader`/`mapping` stay identical and
//! network-free.
//!
//! - [`LinkClient`] — device/reader side (the "client" role: controller/Deck). Owns the current
//!   session's device-side channels; across a transport outage it re-mints a session and hands the
//!   mapper its fresh ends (the old reader.rs channel-swap dance, now internal here).
//! - [`LinkServer`] — mapper/output side (the "server" role: PC). Owns the current session's
//!   mapper-side channels plus the persistent control + reattach receivers.
//!
//! The reader↔mapper *lifecycle* stays observable to the mapper (it changes phases), but the
//! *mechanism* is hidden: the reader calls [`LinkClient::detach`]/[`LinkClient::reattach`], the
//! mapper reads [`LinkServer::is_detached`] and swaps sessions via [`LinkServer::reattach`].
//!
//! [`LinkClient`]/[`LinkServer`] are `Local | Network` enums dispatching to the loopback session
//! ([`LocalClient`]/[`LocalServer`]) or the socket bridge ([`net`]). The network variants are
//! constructed via [`LinkClient::connect`]/[`LinkServer::bind`]; the handle wires those to
//! `set_output(Network)`/`set_input(Network)` in slice 4b. Network reconnect/lifecycle is slice 5 —
//! for now those arms are single-connection stubs (`detach`/`reattach` no-op, `is_detached` false).

mod net;
mod wire;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, unbounded};

use steam_hid::Report;

use super::{Click, Control, RumbleCmd};

/// The mapper-side endpoints for one connected session (re-minted on each reattach). Opaque to the
/// mapping loop — it only shuttles it from `reattach_rx` into [`LinkServer::reattach`].
pub(crate) struct ServerSession {
    frame_rx: Receiver<Report>,
    rumble_tx: Sender<RumbleCmd>,
    click_tx: Sender<Click>,
}

/// The device-side endpoints for one connected session.
struct ClientSession {
    frame_tx: Sender<Report>,
    rumble_rx: Receiver<RumbleCmd>,
    click_rx: Receiver<Click>,
}

impl ClientSession {
    /// A dead session: senders whose receivers are already dropped (and vice-versa), so all I/O
    /// fails harmlessly. Installed on `detach` so the *previous* session's ends drop — which
    /// disconnects the mapper's `frame_rx` and drives it into the waiting phase.
    fn dead() -> Self {
        let (frame_tx, _) = unbounded();
        let (_, rumble_rx) = unbounded();
        let (_, click_rx) = unbounded();
        ClientSession { frame_tx, rumble_rx, click_rx }
    }
}

/// Mint a paired session (device-side + mapper-side ends of the three per-session channels).
fn session_pair() -> (ClientSession, ServerSession) {
    let (frame_tx, frame_rx) = unbounded();
    let (rumble_tx, rumble_rx) = unbounded();
    let (click_tx, click_rx) = unbounded();
    (
        ClientSession { frame_tx, rumble_rx, click_rx },
        ServerSession { frame_rx, rumble_tx, click_tx },
    )
}

// ------------------------------------------------------------------------------------------------
// Local (loopback) ends — plain crossbeam sessions.
// ------------------------------------------------------------------------------------------------

/// Device-side (reader) end of a loopback link.
pub(crate) struct LocalClient {
    session: ClientSession,
    /// Delivers a fresh mapper-side session to the mapper on reattach (persistent).
    reattach_tx: Sender<ServerSession>,
    /// Shared "device transport gone" flag, read by the mapper and the handle's status.
    detached: Arc<AtomicBool>,
}

impl LocalClient {
    /// Flag transport-gone, then drop the session's device-side ends so the mapper's `frame_rx`
    /// disconnects (→ waiting phase). Order matters — flag *before* the drop.
    fn detach(&mut self) {
        self.detached.store(true, Ordering::SeqCst);
        self.session = ClientSession::dead();
    }

    /// Mint a fresh session, hand the mapper its ends, keep ours, clear the flag. `false` if the
    /// mapper is gone (reattach channel closed).
    fn reattach(&mut self) -> bool {
        let (client, server) = session_pair();
        self.session = client;
        self.detached.store(false, Ordering::SeqCst);
        self.reattach_tx.send(server).is_ok()
    }
}

/// Mapper-side end of a loopback link.
pub(crate) struct LocalServer {
    session: ServerSession,
    control_rx: Receiver<Control>,
    reattach_rx: Receiver<ServerSession>,
    detached: Arc<AtomicBool>,
}

// ------------------------------------------------------------------------------------------------
// The transport-agnostic ends — `Local | Network`, one method surface for reader/mapper.
// ------------------------------------------------------------------------------------------------

/// Device-side (reader) end of the link. The reader talks only to this; whether it is a crossbeam
/// loopback or real sockets is chosen at construction (PLAN §6.1).
pub(crate) enum LinkClient {
    Local(LocalClient),
    Network(net::NetClient),
}

impl LinkClient {
    pub(crate) fn frame_tx(&self) -> &Sender<Report> {
        match self {
            LinkClient::Local(c) => &c.session.frame_tx,
            LinkClient::Network(c) => c.frame_tx(),
        }
    }
    pub(crate) fn rumble_rx(&self) -> &Receiver<RumbleCmd> {
        match self {
            LinkClient::Local(c) => &c.session.rumble_rx,
            LinkClient::Network(c) => c.rumble_rx(),
        }
    }
    pub(crate) fn click_rx(&self) -> &Receiver<Click> {
        match self {
            LinkClient::Local(c) => &c.session.click_rx,
            LinkClient::Network(c) => c.click_rx(),
        }
    }

    /// The device's transport went away → drive the mapper into its waiting phase. Network reconnect
    /// (drop the connection so the server sees link-down) is slice 5.
    pub(crate) fn detach(&mut self) {
        match self {
            LinkClient::Local(c) => c.detach(),
            LinkClient::Network(_) => {} // TODO(slice 5): drop the connection.
        }
    }

    /// The device returned → resume mapping. `false` if the mapper/server is gone.
    pub(crate) fn reattach(&mut self) -> bool {
        match self {
            LinkClient::Local(c) => c.reattach(),
            LinkClient::Network(_) => true, // TODO(slice 5): re-dial.
        }
    }

    /// Dial a remote server (client/forwarder role) — the handle wires this to `set_output(Network)`.
    pub(crate) fn connect(server: std::net::SocketAddr) -> std::io::Result<LinkClient> {
        Ok(LinkClient::Network(net::NetClient::connect(server)?))
    }

    /// The config uplink — the handle routes `apply`/`set_globals` here (Network only). `None` for a
    /// loopback client: its control reaches the co-located mapper via [`LocalLink::control_tx`].
    pub(crate) fn control_tx(&self) -> Option<&Sender<Control>> {
        match self {
            LinkClient::Local(_) => None,
            LinkClient::Network(c) => Some(c.control_tx()),
        }
    }
}

/// Mapper-side end of the link. The mapper talks only to this.
pub(crate) enum LinkServer {
    Local(LocalServer),
    Network(NetworkServer),
}

impl LinkServer {
    pub(crate) fn frame_rx(&self) -> &Receiver<Report> {
        match self {
            LinkServer::Local(s) => &s.session.frame_rx,
            LinkServer::Network(s) => s.net.frame_rx(),
        }
    }
    pub(crate) fn control_rx(&self) -> &Receiver<Control> {
        match self {
            LinkServer::Local(s) => &s.control_rx,
            LinkServer::Network(s) => s.net.control_rx(),
        }
    }
    pub(crate) fn rumble_tx(&self) -> &Sender<RumbleCmd> {
        match self {
            LinkServer::Local(s) => &s.session.rumble_tx,
            LinkServer::Network(s) => s.net.rumble_tx(),
        }
    }
    pub(crate) fn click_tx(&self) -> &Sender<Click> {
        match self {
            LinkServer::Local(s) => &s.session.click_tx,
            LinkServer::Network(s) => s.net.click_tx(),
        }
    }

    /// True while the device/link is gone (mapper's cue to wait; handle's cue for `WaitingForDevice`).
    pub(crate) fn is_detached(&self) -> bool {
        match self {
            LinkServer::Local(s) => s.detached.load(Ordering::SeqCst),
            LinkServer::Network(s) => s.net.is_detached(),
        }
    }

    /// Poll (from the waiting phase) whether the link has (re)attached — if so, leave the waiting
    /// phase. **Local:** a fresh session arrived on `reattach_rx`, swap it in (the pad never left).
    /// **Network:** the server re-accepted a client, resume on the same persistent channel (no swap).
    /// `false` = keep waiting.
    pub(crate) fn poll_reattach(&mut self) -> bool {
        match self {
            LinkServer::Local(s) => {
                if let Ok(session) = s.reattach_rx.try_recv() {
                    s.session = session;
                    true
                } else {
                    false
                }
            }
            LinkServer::Network(s) => !s.net.is_detached(),
        }
    }

    /// Bind for a remote client (server role). Returns the server end plus the `control_tx` for the
    /// server's *own* handle (merged with the client's wire config — the two-feeder `control_rx`).
    /// The handle wires this to `set_input(Network)`.
    pub(crate) fn bind(
        addr: std::net::SocketAddr,
    ) -> std::io::Result<(LinkServer, Sender<Control>)> {
        let net = net::NetServer::bind(addr)?;
        let control_tx = net.control_tx().clone();
        Ok((LinkServer::Network(NetworkServer { net }), control_tx))
    }
}

/// The network mapper-side end: just the socket bridge. "Reattach" = the server re-accepting a client
/// (observed via `is_detached`), so there is no session to swap (unlike the loopback path).
pub(crate) struct NetworkServer {
    net: net::NetServer,
}

/// What [`Runtime`](super::Runtime) keeps after wiring a local link: the two ends (moved into the
/// reader and mapper threads), the control sender (for the `Engine` handle's live hot-swap), and the
/// shared detached flag (for `Runtime::is_waiting`).
pub(crate) struct LocalLink {
    pub(crate) client: LinkClient,
    pub(crate) server: LinkServer,
    pub(crate) control_tx: Sender<Control>,
    pub(crate) detached: Arc<AtomicBool>,
}

/// Build a loopback link: device side ↔ mapper side, wired by in-process crossbeams.
pub(crate) fn local_link() -> LocalLink {
    let detached = Arc::new(AtomicBool::new(false));
    let (control_tx, control_rx) = unbounded();
    let (reattach_tx, reattach_rx) = unbounded();
    let (client_session, server_session) = session_pair();
    let client =
        LinkClient::Local(LocalClient { session: client_session, reattach_tx, detached: detached.clone() });
    let server = LinkServer::Local(LocalServer {
        session: server_session,
        control_rx,
        reattach_rx,
        detached: detached.clone(),
    });
    LocalLink { client, server, control_tx, detached }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Frames cross the wire through the transport-agnostic `LinkClient`/`LinkServer` enum surface
    /// (i.e. the reader/mapper-facing methods dispatch to the network adapter).
    #[test]
    fn network_link_dispatches_through_the_enum() {
        let (server, _ctl) = LinkServer::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = match &server {
            LinkServer::Network(s) => s.net.addr(),
            _ => unreachable!(),
        };
        let client = LinkClient::connect(addr).unwrap();
        // A lifecycle event rides reliable TCP → deterministic without UDP retries.
        client.frame_tx().send(Report::Connected).unwrap();
        assert_eq!(
            server.frame_rx().recv_timeout(Duration::from_secs(2)).unwrap(),
            Report::Connected
        );
    }
}
