//! Engine events (PLAN §4.3) — the out-of-band signals the engine surfaces: device lifecycle,
//! battery, binding, and run-state changes. Produced by the reader/mapping threads and the handle,
//! delivered to subscribers via an [`EventSink`] broadcast → [`EventStream`] receivers (D7). The
//! wire mirror the daemon serializes onto its socket is `deckhand_ipc::Event`.

use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, unbounded};

use steam_hid::DeviceId;

use crate::handle::Status;

/// An out-of-band signal from the engine.
///
/// Deliberately **not** `#[non_exhaustive]`: adding a variant should be a compile error at every
/// consumer (notably the daemon's wire mapping), per the project's vocab policy — a missing map is a
/// bug, not a silent no-op.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// The bound controller connected (its dongle slot woke up / it powered on).
    ControllerConnected,
    /// The bound controller disconnected while its transport (the dongle) stayed alive.
    ControllerDisconnected,
    /// The bound controller's battery charge changed, in percent.
    BatteryChanged { percent: u8 },
    /// A device appeared on the bus (emitted by the hotplug monitor — D6/later).
    DeviceAdded(DeviceId),
    /// A device left the bus.
    DeviceRemoved(DeviceId),
    /// The bound device's transport went away; the engine is now waiting to reacquire it (D5).
    BindingLost,
    /// A device was (re)acquired as the bound input (D5/D6).
    BindingAcquired(DeviceId),
    /// The engine run-state changed.
    State(Status),
}

/// Broadcasts [`EngineEvent`]s to any subscribers (D7). Cloned into every thread that produces
/// events (reader, mapping loop, the handle); [`subscribe`](EventSink::subscribe) hands out
/// independent receivers, each getting every *subsequent* event. Also logs each event at info for
/// at-a-glance visibility even with no subscriber.
#[derive(Clone, Default)]
pub(crate) struct EventSink {
    subs: Arc<Mutex<Vec<Sender<EngineEvent>>>>,
}

impl EventSink {
    /// Emit an event: log it, then broadcast to all live subscribers (pruning any that dropped).
    pub(crate) fn emit(&self, ev: EngineEvent) {
        log::info!("event: {ev:?}");
        self.subs.lock().unwrap().retain(|tx| tx.send(ev.clone()).is_ok());
    }

    /// A new subscription: an independent stream of every subsequent event.
    pub(crate) fn subscribe(&self) -> EventStream {
        let (tx, rx) = unbounded();
        self.subs.lock().unwrap().push(tx);
        EventStream { rx }
    }
}

/// A subscriber's stream of [`EngineEvent`]s (D7). Hides the channel type from the public API.
pub struct EventStream {
    rx: Receiver<EngineEvent>,
}

impl EventStream {
    /// Block until the next event, or `None` once the engine has gone away (all senders dropped).
    pub fn recv(&self) -> Option<EngineEvent> {
        self.rx.recv().ok()
    }
}
