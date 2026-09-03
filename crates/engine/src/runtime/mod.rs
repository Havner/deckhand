//! The manager shell - the runtime that owns the two device-driving threads and the control channel
//! (PLAN 4.1, 4.2 S9). Split across three files:
//! - **this module** - the [`Runtime`] lifecycle (spawn/join the threads) + the inter-thread message
//!   types ([`Control`], [`RumbleCmd`]; command feedback rides `mapper::FeedbackReq` over the
//!   downlink); the reader-side per-device `ReaderCfg` lives in
//!   [`reader`] (it's rebuilt there from the live [`DeviceConfig`]);
//! - [`reader`] - the reader thread: a persistent device-session loop that owns the `Device`, is its
//!   only writer, and reacquires the pinned device across a transport outage (D6);
//! - [`mapping`] - the central mapping loop: owns the `Sink` + `Mapper`, maps frames to outputs, and
//!   keeps the pad plugged through an outage (connected/waiting phases).
//!
//! Concurrency is sync, no async (CLAUDE / PLAN 4): a reader thread (which owns the `Send`-not-
//! `Sync` `Device`) forwards frames over a channel to the mapping loop; commands/rumble flow back
//! over channels. The pure helpers are unit-tested in `reader`/`mapping`.

mod feedback;
mod link;
mod mapping;
mod reader;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, unbounded};
use serde::{Deserialize, Serialize};

use config::{Chords, DeviceConfig};
use steam_hid::{Device, DeviceId};
use virt_out::Sink;

use crate::Result;
use crate::event::EventSink;
use mapper::{Program, Role};

use link::{LinkClient, LinkServer, LocalLink};
use mapping::run_mapper;
use reader::run_reader;

/// A control message to the mapping loop, from the `Engine` handle (live hot-swap). Device
/// reattach after an outage (D6) is **not** here - it goes through the [`link`] seam.
pub(crate) enum Control {
    /// Replace one role's program (main<->fallback), or clear it (`program: None`), re-seeding the
    /// mapper if the affected role is the one live.
    Apply { program: Option<Box<Program>>, role: Role },
    /// Replace the chords (`None` clears them). Device settings never reach the mapper - they're
    /// reader-side and machine-local - so they don't ride `Control` (which is the network uplink in
    /// the client role); they travel on the reader's own [`Runtime::device_config_tx`] channel.
    SetChords(Option<Chords>),
    /// Stop the loop (the running flag also gates it; this just wakes the `select!`).
    Stop,
}

/// The effective rumble to realize on the controller: per-pad drive (already scaled by profile
/// strength x curve). The per-device shaping (levers, pulse frequency) is applied reader-side.
/// Produced by the mapping loop's `rumble_cmd`, realized by the reader's `apply_gordon` /
/// `apply_rumble{,_triton}`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct RumbleCmd {
    pub(crate) strong: u16,
    pub(crate) weak: u16,
}

/// Sentinel stored in the reader's battery readback before any battery frame has arrived (charge is
/// always 0..=100, so `u16::MAX` can never collide). Mapped back to `None` by [`Runtime::battery`].
const BATTERY_UNKNOWN: u16 = u16::MAX;

/// A running engine: the two threads + the control channel. Created by [`Runtime::start`] on
/// `Engine::start`, torn down by [`Runtime::stop`] on `Engine::stop` (config lives in the handle).
pub(crate) struct Runtime {
    running: Arc<AtomicBool>,
    /// Set by the link when the bound device's transport goes away - the loop stays up (pad
    /// plugged, outputs neutral) but is `WaitingForDevice` (PLAN 4.3, D5). Shared with both
    /// `Link*` ends (the client sets it, the mapper reads it).
    detached: Arc<AtomicBool>,
    /// Published by the reader: is the bound controller currently present? `Some` **iff a reader
    /// runs** (local + client roles) - `None` in the server role (no local device). Read by
    /// `status()`; the reader emits the matching `ControllerConnected(bool)` event on each change.
    controller_connected: Option<Arc<AtomicBool>>,
    /// Published by the reader: the bound controller's last-known battery charge (percent), or
    /// [`BATTERY_UNKNOWN`] before any `0x04` frame arrives (wired Gordon reports a fake 100%; only
    /// wireless controllers report real charge). `Some` **iff a reader runs** (local + client roles),
    /// mirroring `controller_connected` - `None` in the server role. Read by `status()`; the reader
    /// emits the matching `BatteryChanged` event on each change.
    battery: Option<Arc<AtomicU16>>,
    /// Published by the mapper: is the live role the **fallback**? `Some` **iff a mapper runs**
    /// (local + server roles) - `None` in the client role (the live role lives on the remote server).
    /// Read by `status()`; the mapper emits the matching `ActiveRole` event on each change.
    fallback_active: Option<Arc<AtomicBool>>,
    control_tx: Sender<Control>,
    /// Live device-config channel to the reader (LED/idle, master rumble, frequency). `Some` **iff a
    /// reader runs** (local + client roles) - `None` in the server role, where device settings are a
    /// no-op (no local device). Machine-local: it deliberately does **not** ride `control_tx` (the
    /// network uplink), so device settings never cross the 6 wire.
    device_config_tx: Option<Sender<DeviceConfig>>,
    reader: Option<JoinHandle<Result<()>>>,
    mapper: Option<JoinHandle<Result<()>>>,
}

impl Runtime {
    /// **Local** role (input=Local, output=Local): reader + mapper co-located, wired by the loopback
    /// link. `device` and `sink` are already opened/created by the caller so any HW error surfaces
    /// before the threads start.
    #[allow(clippy::too_many_arguments)]
    pub fn start_local(
        device: Device,
        pinned_id: DeviceId,
        device_config: DeviceConfig,
        sink: Sink,
        main: Option<Program>,
        fallback: Option<Program>,
        chords: Option<Chords>,
        events: EventSink,
    ) -> Runtime {
        let running = Arc::new(AtomicBool::new(true));
        // The reader gets the client end, the mapper the server end; the handle keeps `control_tx`,
        // and `detached` is the shared `WaitingForDevice` flag (PLAN 6.1).
        let LocalLink { client, server, control_tx, detached } = link::local_link();
        // Reader-side live device-config channel (machine-local, off the link).
        let (device_config_tx, device_config_rx) = unbounded();
        // Both threads run locally -> both readback flags are live.
        let connected = Arc::new(AtomicBool::new(false));
        let battery = Arc::new(AtomicU16::new(BATTERY_UNKNOWN));
        let fallback_active = Arc::new(AtomicBool::new(false));
        let reader = spawn_reader(
            device, pinned_id, device_config, device_config_rx, client, running.clone(), connected.clone(),
            battery.clone(), events.clone(),
        );
        let mapper = spawn_mapper(
            sink, main, fallback, chords, server, running.clone(), fallback_active.clone(), events,
        );
        Runtime {
            running,
            detached,
            controller_connected: Some(connected),
            battery: Some(battery),
            fallback_active: Some(fallback_active),
            control_tx,
            device_config_tx: Some(device_config_tx),
            reader: Some(reader),
            mapper: Some(mapper),
        }
    }

    /// **Client** role (output=Network): reader only - it reads the device and forwards frames to
    /// the remote server at `addr`; there is no local mapper or `Sink`. The link's config uplink
    /// becomes the runtime's `control_tx`, so the handle's `apply`/`set_chords` travel to the
    /// server. Errors if the dial fails.
    pub fn start_client(
        addr: SocketAddr,
        device: Device,
        pinned_id: DeviceId,
        device_config: DeviceConfig,
        events: EventSink,
    ) -> Result<Runtime> {
        let running = Arc::new(AtomicBool::new(true));
        let link = LinkClient::connect(addr)?;
        let control_tx = link.control_tx().cloned().expect("a network client has a control uplink");
        // The reader flags this (its shared link flag) on device-loss, so `is_waiting()`/`status()`
        // report `WaitingForDevice` - matching the `State` event the reader emits.
        let detached = link.detached();
        // Reader-side live device-config channel (machine-local; the client's device settings stay
        // on this machine, never pushed to the server over the wire).
        let (device_config_tx, device_config_rx) = unbounded();
        // Client role: a reader (-> `controller_connected`), but no local mapper (the live role is
        // on the remote server, so `fallback_active` is `None`).
        let connected = Arc::new(AtomicBool::new(false));
        let battery = Arc::new(AtomicU16::new(BATTERY_UNKNOWN));
        let reader = spawn_reader(
            device, pinned_id, device_config, device_config_rx, link, running.clone(), connected.clone(),
            battery.clone(), events,
        );
        Ok(Runtime {
            running,
            detached,
            controller_connected: Some(connected),
            battery: Some(battery),
            fallback_active: None,
            control_tx,
            device_config_tx: Some(device_config_tx),
            reader: Some(reader),
            mapper: None,
        })
    }

    /// **Server** role (input=Network): mapper only - it binds `addr`, receives a remote client's
    /// frames, and maps them to the local `Sink`; there is no local device or reader. The returned
    /// `control_tx` merges the server's *own* handle config with the client's wire config into
    /// `control_rx`. Errors if the bind fails.
    pub fn start_server(
        addr: SocketAddr,
        sink: Sink,
        main: Option<Program>,
        fallback: Option<Program>,
        chords: Option<Chords>,
        events: EventSink,
    ) -> Result<Runtime> {
        let running = Arc::new(AtomicBool::new(true));
        // `detached` is the server link's shared flag: true while no client is connected, so
        // `is_waiting()`/`status()` report `WaitingForDevice` (matching the mapper's event).
        let (link, control_tx, detached) = LinkServer::bind(addr)?;
        // Server role: a mapper (-> `fallback_active`), but no local device/reader (input arrives from
        // a remote client, so `controller_connected` is `None`).
        let fallback_active = Arc::new(AtomicBool::new(false));
        let mapper = spawn_mapper(
            sink, main, fallback, chords, link, running.clone(), fallback_active.clone(), events,
        );
        Ok(Runtime {
            running,
            detached,
            controller_connected: None,
            battery: None,
            fallback_active: Some(fallback_active),
            control_tx,
            // Server role: no local reader -> device settings are a no-op here (staged in the handle,
            // never applied). The client keeps and applies its own device config on its machine.
            device_config_tx: None,
            reader: None,
            mapper: Some(mapper),
        })
    }

    /// True when the loop is up but the bound device's transport is gone (`WaitingForDevice`).
    pub(crate) fn is_waiting(&self) -> bool {
        self.detached.load(Ordering::SeqCst)
    }

    /// Whether the bound controller is currently present, or `None` if this role has no local reader
    /// (server). Read by `status()`; kept in lock-step with the `ControllerConnected` event.
    pub(crate) fn controller_connected(&self) -> Option<bool> {
        self.controller_connected.as_ref().map(|f| f.load(Ordering::SeqCst))
    }

    /// The bound controller's last-known battery charge (percent), or `None` when this role has no
    /// local reader (server) **or** no battery frame has arrived yet - both render as "unknown", so
    /// the sentinel never leaks past this method. Read by `status()`; kept in lock-step with the
    /// `BatteryChanged` event.
    pub(crate) fn battery(&self) -> Option<u8> {
        self.battery
            .as_ref()
            .map(|f| f.load(Ordering::SeqCst))
            .filter(|&v| v != BATTERY_UNKNOWN)
            .map(|v| v as u8)
    }

    /// The live role, or `None` if this role has no local mapper (client - the role lives on the
    /// remote server). Read by `status()`; kept in lock-step with the `ActiveRole` event.
    pub(crate) fn active_role(&self) -> Option<Role> {
        self.fallback_active
            .as_ref()
            .map(|f| if f.load(Ordering::SeqCst) { Role::Fallback } else { Role::Main })
    }

    /// The control channel, for live `apply`/`set_chords` while running.
    pub fn control(&self) -> &Sender<Control> {
        &self.control_tx
    }

    /// Push a live device-config update to the reader (LED/idle, master rumble, frequency). A no-op
    /// in the server role (no local reader) - device settings are machine-local, so a server just
    /// keeps them staged in the handle. Latest-wins on the reader; never crosses the 6 wire.
    pub fn set_device_config(&self, device_config: DeviceConfig) {
        if let Some(tx) = &self.device_config_tx {
            let _ = tx.send(device_config);
        }
    }

    /// Halt the loop and join both threads, releasing hardware (device -> lizard restored on drop,
    /// virtual pad unplugged). Returns the first thread error, if any.
    pub fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.control().send(Control::Stop); // wake the mapper's select immediately
        let mut result = Ok(());
        if let Some(h) = self.mapper.take()
            && let Ok(r) = h.join()
        {
            result = result.and(r);
        }
        if let Some(h) = self.reader.take()
            && let Ok(r) = h.join()
        {
            result = result.and(r);
        }
        result
    }
}

/// Spawn the reader thread (device side). Shared by the local + client roles.
#[allow(clippy::too_many_arguments)]
fn spawn_reader(
    device: Device,
    pinned_id: DeviceId,
    device_config: DeviceConfig,
    device_config_rx: crossbeam_channel::Receiver<DeviceConfig>,
    link: LinkClient,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    battery: Arc<AtomicU16>,
    events: EventSink,
) -> JoinHandle<Result<()>> {
    thread::Builder::new()
        .name("deckhand-reader".into())
        .spawn(move || {
            let result = run_reader(
                device, pinned_id, device_config, device_config_rx, link, running, connected, battery,
                events,
            );
            // A reader error would otherwise be invisible until stop() joins it - log it now.
            if let Err(ref e) = result {
                log::error!("reader thread exited with error: {e}");
            }
            result
        })
        .expect("spawn reader thread")
}

/// Spawn the mapping thread (mapper side). Shared by the local + server roles.
#[allow(clippy::too_many_arguments)]
fn spawn_mapper(
    sink: Sink,
    main: Option<Program>,
    fallback: Option<Program>,
    chords: Option<Chords>,
    link: LinkServer,
    running: Arc<AtomicBool>,
    fallback_active: Arc<AtomicBool>,
    events: EventSink,
) -> JoinHandle<Result<()>> {
    thread::Builder::new()
        .name("deckhand-mapper".into())
        .spawn(move || {
            run_mapper(sink, main, fallback, chords, link, running, fallback_active, events)
        })
        .expect("spawn mapper thread")
}
