//! Engine events (PLAN 4.3) - the out-of-band signals the engine surfaces: device lifecycle,
//! battery, binding, and run-state changes. Produced by the reader/mapping threads and the handle,
//! delivered to subscribers via an [`EventSink`] broadcast -> [`EventStream`] receivers (D7). The
//! wire mirror the daemon serializes onto its socket is `ipc::Event`.

use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, unbounded};

use config::{Chords, DeviceConfig};
use steam_hid::DeviceId;

use crate::handle::{Input, Output, Status};
use crate::program::Role;

/// An out-of-band signal from the engine.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// The bound controller's presence changed: `true` = connected, `false` = disconnected. On the
    /// dongle this is the slot powering on/off while the transport stays alive; on wired/BT the
    /// controller *is* the transport, so it's `true` for the whole session and `false` only when the
    /// transport goes (-> `WaitingForDevice`). Carries the absolute value (seed-then-subscribe safe).
    ControllerConnected(bool),
    /// The bound controller's battery charge changed, in percent.
    BatteryChanged { percent: u8 },
    /// The binding was torn down: the engine stopped and no device is bound any more (-> `Idle`).
    /// Brackets [`BindingAcquired`](Self::BindingAcquired) - together they track the bound-device
    /// lifetime so subscribers stay consistent with `status().bound`. A transport outage does **not**
    /// emit this (the device stays pinned); that surfaces as `State(WaitingForDevice)`.
    BindingRemoved,
    /// A device was acquired as the bound input at `start()` (D5/D6). Not re-emitted on reacquire
    /// after a transport outage - that surfaces as `State(Running)`.
    BindingAcquired(DeviceId),
    /// The engine run-state changed.
    State(Status),
    /// The **live** role switched (the chord flipped main<->fallback, or the initial role at start).
    /// Carries the absolute new role - the parallel of [`State`](Self::State) for the profile mode.
    /// Distinct from [`ProfileSet`](Self::ProfileSet), which reports a program loaded *into* a slot;
    /// this reports which slot is now *active*.
    ActiveRole(Role),
    /// The **staged** input selection changed (takes effect at the next `start()`). Lets a client
    /// that connects to a running daemon stay in sync when another controls it on the side. Carries
    /// the absolute new value (not a delta), so it's safe to seed-then-subscribe race-free.
    InputStaged(Input),
    /// The **staged** output selection changed (takes effect at the next `start()`). Absolute value.
    OutputStaged(Output),
    /// A program was applied to a role (live hot-swap if running, else staged). Carries the role and
    /// the program's name (`None` reserved for a future clear). Absolute value.
    ProfileSet { role: Role, name: Option<String> },
    /// The chords were set (live if running, else staged); `None` = no chords. Carries the whole
    /// set so a client can mirror it without a round-trip. Absolute value.
    ChordsSet(Option<Chords>),
    /// The device config was set (live if running, else staged). Carries the whole config so a client
    /// can mirror it without a round-trip. Absolute value.
    DeviceConfigSet(DeviceConfig),

    // --- Live layer-stack view (mapper-side) - the ONLY events NOT reflected in `StatusInfo`. ---
    // Every event above mirrors a `StatusInfo`/`StatusSnapshot` field, so a client can seed on connect
    // then ride events. These three deliberately do NOT: they are a transient debug/awareness view of
    // the effective layer stack, surfaced through `monitor` only, not seeded on connect. All three are
    // absolute-valued - each carries the FULL new set (the runtime diffs and emits only on a change).
    /// The active **action set** changed - its name.
    ActiveSet(String),
    /// The set of **held layers** (from `HoldLayer`) changed - the full new set of names, in
    /// declared-order (id) order. Empty = no held layers.
    HeldLayers(Vec<String>),
    /// The set of **persistent layers** (from `AddLayer`/`RemoveLayer`) changed - the full new set of
    /// names. Empty = none.
    PersistentLayers(Vec<String>),
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
        // Debug, not info: events fire per connect/role-switch/etc. and get noisy - `status`/`monitor`
        // are the normal-usage surface for this.
        log::debug!("event: {ev:?}");
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
