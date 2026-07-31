//! Engine events (PLAN §4.3) — the vocabulary of asynchronous signals the engine surfaces: device
//! lifecycle, battery, binding, and run-state changes. The wire mirror is `deckhand_ipc::Event`.
//!
//! **Not yet delivered on a channel.** Until the real subscription stream lands (D7),
//! [`EngineEvent::emit`] just logs a stub line at info, so the event stream is observable now and
//! the emission points are already in the right places. D7 swaps `emit` for sending on a channel.

use steam_hid::DeviceId;

use crate::handle::Status;

/// An asynchronous signal from the engine.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum EngineEvent {
    /// The bound controller connected (its dongle slot woke up / it powered on).
    ControllerConnected,
    /// The bound controller disconnected while its transport (the dongle) stayed alive.
    ControllerDisconnected,
    /// The bound controller's battery charge changed (wireless only), in percent.
    BatteryChanged { percent: u8 },
    /// A device appeared on the bus (emitted by the hotplug monitor — D6).
    DeviceAdded(DeviceId),
    /// A device left the bus (D6).
    DeviceRemoved(DeviceId),
    /// The bound device's transport went away; the engine is now waiting to reacquire it (D5).
    BindingLost,
    /// A device was (re)acquired as the bound input (D5/D6).
    BindingAcquired(DeviceId),
    /// The engine run-state changed.
    State(Status),
}

impl EngineEvent {
    /// Surface this event. Until the real subscription channel lands (D7) this logs a stub line at
    /// info so the stream is observable; the call sites are already the correct emission points.
    pub(crate) fn emit(&self) {
        log::info!("event(stub): {self:?}");
    }
}
