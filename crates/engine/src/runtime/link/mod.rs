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
//! NB: only the local adapter exists for now, so these are plain structs; the network adapter will
//! turn the internal representation into a `Local | Network` enum (the method surface is unchanged).

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

/// Device-side (reader) end of the loopback link.
pub(crate) struct LinkClient {
    session: ClientSession,
    /// Delivers a fresh mapper-side session to the [`LinkServer`] on reattach (persistent).
    reattach_tx: Sender<ServerSession>,
    /// Shared "device transport gone" flag, read by the mapper and the handle's status.
    detached: Arc<AtomicBool>,
}

impl LinkClient {
    pub(crate) fn frame_tx(&self) -> &Sender<Report> {
        &self.session.frame_tx
    }
    pub(crate) fn rumble_rx(&self) -> &Receiver<RumbleCmd> {
        &self.session.rumble_rx
    }
    pub(crate) fn click_rx(&self) -> &Receiver<Click> {
        &self.session.click_rx
    }

    /// The device's transport went away: flag it, then drop the session's device-side ends so the
    /// mapper's `frame_rx` disconnects (→ waiting phase). Order matters — flag *before* the drop so
    /// the mapper reads the disconnect as transport-lost, not a clean stop.
    pub(crate) fn detach(&mut self) {
        self.detached.store(true, Ordering::SeqCst);
        self.session = ClientSession::dead();
    }

    /// The device returned: mint a fresh session, hand the mapper its ends, keep ours, clear the
    /// flag. Returns `false` if the mapper is gone (reattach channel closed) → the reader exits.
    pub(crate) fn reattach(&mut self) -> bool {
        let (client, server) = session_pair();
        self.session = client;
        self.detached.store(false, Ordering::SeqCst);
        self.reattach_tx.send(server).is_ok()
    }
}

/// Mapper-side end of the loopback link.
pub(crate) struct LinkServer {
    session: ServerSession,
    control_rx: Receiver<Control>,
    reattach_rx: Receiver<ServerSession>,
    detached: Arc<AtomicBool>,
}

impl LinkServer {
    pub(crate) fn frame_rx(&self) -> &Receiver<Report> {
        &self.session.frame_rx
    }
    pub(crate) fn control_rx(&self) -> &Receiver<Control> {
        &self.control_rx
    }
    pub(crate) fn reattach_rx(&self) -> &Receiver<ServerSession> {
        &self.reattach_rx
    }
    pub(crate) fn rumble_tx(&self) -> &Sender<RumbleCmd> {
        &self.session.rumble_tx
    }
    pub(crate) fn click_tx(&self) -> &Sender<Click> {
        &self.session.click_tx
    }

    /// True while the device transport is gone (the mapper's cue to stay in the waiting phase; the
    /// handle's cue to report `WaitingForDevice`).
    pub(crate) fn is_detached(&self) -> bool {
        self.detached.load(Ordering::SeqCst)
    }

    /// Swap in the fresh session delivered on `reattach_rx` (the pad never left, so the same `Sink`
    /// resumes over the new device).
    pub(crate) fn reattach(&mut self, session: ServerSession) {
        self.session = session;
    }
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
    let client = LinkClient { session: client_session, reattach_tx, detached: detached.clone() };
    let server =
        LinkServer { session: server_session, control_rx, reattach_rx, detached: detached.clone() };
    LocalLink { client, server, control_tx, detached }
}
