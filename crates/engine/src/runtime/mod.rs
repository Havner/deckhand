//! The manager shell — the runtime that owns the two device-driving threads and the control channel
//! (PLAN §4.1, §4.2 S9). Split across three files:
//! - **this module** — the [`Runtime`] lifecycle (spawn/join the threads) + the inter-thread message
//!   types ([`Control`], [`RumbleCmd`], [`Click`]) + per-device [`DeviceCfg`];
//! - [`reader`] — the reader thread: a persistent device-session loop that owns the `Device`, is its
//!   only writer, and reacquires the pinned device across a transport outage (D6);
//! - [`mapping`] — the central mapping loop: owns the `Sink` + `Mapper`, maps frames to outputs, and
//!   keeps the pad plugged through an outage (connected/waiting phases).
//!
//! Concurrency is sync, no async (CLAUDE / PLAN §4): a reader thread (which owns the `Send`-not-
//! `Sync` `Device`) forwards frames over a channel to the mapping loop; commands/rumble flow back
//! over channels. The pure helpers are unit-tested in `reader`/`mapping`.

mod link;
mod mapping;
mod reader;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};

use config::{GlobalConfig, HapticStrength, Side};
use steam_hid::{Device, DeviceId, DeviceKind, Transport};
use virt_out::Sink;

use crate::Result;
use crate::event::EventSink;
use crate::program::{Program, Role};

use link::{LinkClient, LinkServer, LocalLink};
use mapping::run_mapper;
use reader::run_reader;

/// Device-level settings the reader applies on start and on every `Connected` (the controller
/// resets its config when it re-joins a dongle; PLAN §1.9). Profile-independent.
pub(crate) struct DeviceCfg {
    /// LED intensity `0..=100 %`, or leave the device default. `None` where the device has no
    /// settable LED ([`DeviceKind::has_led_intensity`]).
    pub led_brightness: Option<u8>,
    /// Sleep/idle timeout in seconds, or leave the device default. `None` where idle is meaningless
    /// for the transport ([`Transport::has_idle`]).
    pub idle_timeout: Option<u16>,
    /// Periodically re-assert lizard-off ([`DeviceKind::needs_keepalive`]).
    pub keepalive: bool,
}

impl DeviceCfg {
    /// The config for a device from the globals, with each device-specific setting dropped to
    /// `None`/`false` where the hardware can't honor it (so the reader applies it blindly).
    pub fn for_device(kind: &DeviceKind, transport: &Transport, globals: &GlobalConfig) -> Self {
        DeviceCfg {
            led_brightness: kind.has_led_intensity().then_some(globals.led_brightness).flatten(),
            idle_timeout: transport.has_idle().then_some(globals.idle_timeout).flatten(),
            keepalive: kind.needs_keepalive(),
        }
    }
}

/// A control message to the mapping loop, from the `Engine` handle (live hot-swap). Device
/// reattach after an outage (D6) is **not** here — it goes through the [`link`] seam.
pub(crate) enum Control {
    /// Replace one role's program (main↔fallback), or clear it (`program: None`), re-seeding the
    /// mapper if the affected role is the one live.
    Apply { program: Option<Box<Program>>, role: Role },
    /// Replace the global config (master rumble + chords).
    SetGlobals(Box<GlobalConfig>),
    /// Stop the loop (the running flag also gates it; this just wakes the `select!`).
    Stop,
}

/// The effective rumble to realize on the controller: per-pad drive (already scaled by master ×
/// profile strength × curve) plus the pulse frequency from the main profile. Produced by the
/// mapping loop's `rumble_cmd`, realized by the reader's `apply_haptics`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct RumbleCmd {
    pub(crate) strong: u16,
    pub(crate) weak: u16,
    pub(crate) hz: u16,
}

/// One-shot command-haptic click for the reader to fire immediately on `side`'s pad, at one of three
/// `strength` levels — **the reader maps the level to the device** (Gordon `0x8f` pulse duration,
/// Deck `0xea` gain), distinct from the sustained rumble, with **no arbitration** (it briefly
/// interrupts a rumble on the shared pad, which resumes next re-fire; the opposite pad is untouched
/// — PLAN §1.9 haptics v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Click {
    pub(crate) side: Side,
    pub(crate) strength: HapticStrength,
}

/// A running engine: the two threads + the control channel. Created by [`Runtime::start`] on
/// `Engine::start`, torn down by [`Runtime::stop`] on `Engine::stop` (config lives in the handle).
pub(crate) struct Runtime {
    running: Arc<AtomicBool>,
    /// Set by the link when the bound device's transport goes away — the loop stays up (pad
    /// plugged, outputs neutral) but is `WaitingForDevice` (PLAN §4.3, D5). Shared with both
    /// `Link*` ends (the client sets it, the mapper reads it).
    detached: Arc<AtomicBool>,
    /// Published by the reader: is the bound controller currently present? `Some` **iff a reader
    /// runs** (local + client roles) — `None` in the server role (no local device). Read by
    /// `status()`; the reader emits the matching `ControllerConnected(bool)` event on each change.
    controller_connected: Option<Arc<AtomicBool>>,
    /// Published by the mapper: is the live role the **fallback**? `Some` **iff a mapper runs**
    /// (local + server roles) — `None` in the client role (the live role lives on the remote server).
    /// Read by `status()`; the mapper emits the matching `ActiveRole` event on each change.
    fallback_active: Option<Arc<AtomicBool>>,
    control_tx: Sender<Control>,
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
        cfg: DeviceCfg,
        sink: Sink,
        main: Option<Program>,
        fallback: Option<Program>,
        globals: GlobalConfig,
        events: EventSink,
    ) -> Runtime {
        let running = Arc::new(AtomicBool::new(true));
        // The reader gets the client end, the mapper the server end; the handle keeps `control_tx`,
        // and `detached` is the shared `WaitingForDevice` flag (PLAN §6.1).
        let LocalLink { client, server, control_tx, detached } = link::local_link();
        // Both threads run locally → both readback flags are live.
        let connected = Arc::new(AtomicBool::new(false));
        let fallback_active = Arc::new(AtomicBool::new(false));
        let reader = spawn_reader(
            device, pinned_id, cfg, client, running.clone(), connected.clone(), events.clone(),
        );
        let mapper = spawn_mapper(
            sink, main, fallback, globals, server, running.clone(), fallback_active.clone(), events,
        );
        Runtime {
            running,
            detached,
            controller_connected: Some(connected),
            fallback_active: Some(fallback_active),
            control_tx,
            reader: Some(reader),
            mapper: Some(mapper),
        }
    }

    /// **Client** role (output=Network): reader only — it reads the device and forwards frames to
    /// the remote server at `addr`; there is no local mapper or `Sink`. The link's config uplink
    /// becomes the runtime's `control_tx`, so the handle's `apply`/`set_globals` travel to the
    /// server. Errors if the dial fails.
    pub fn start_client(
        addr: SocketAddr,
        device: Device,
        pinned_id: DeviceId,
        cfg: DeviceCfg,
        events: EventSink,
    ) -> Result<Runtime> {
        let running = Arc::new(AtomicBool::new(true));
        let link = LinkClient::connect(addr)?;
        let control_tx = link.control_tx().cloned().expect("a network client has a control uplink");
        // The reader flags this (its shared link flag) on device-loss, so `is_waiting()`/`status()`
        // report `WaitingForDevice` — matching the `State` event the reader emits.
        let detached = link.detached();
        // Client role: a reader (→ `controller_connected`), but no local mapper (the live role is
        // on the remote server, so `fallback_active` is `None`).
        let connected = Arc::new(AtomicBool::new(false));
        let reader =
            spawn_reader(device, pinned_id, cfg, link, running.clone(), connected.clone(), events);
        Ok(Runtime {
            running,
            detached,
            controller_connected: Some(connected),
            fallback_active: None,
            control_tx,
            reader: Some(reader),
            mapper: None,
        })
    }

    /// **Server** role (input=Network): mapper only — it binds `addr`, receives a remote client's
    /// frames, and maps them to the local `Sink`; there is no local device or reader. The returned
    /// `control_tx` merges the server's *own* handle config with the client's wire config into
    /// `control_rx`. Errors if the bind fails.
    pub fn start_server(
        addr: SocketAddr,
        sink: Sink,
        main: Option<Program>,
        fallback: Option<Program>,
        globals: GlobalConfig,
        events: EventSink,
    ) -> Result<Runtime> {
        let running = Arc::new(AtomicBool::new(true));
        // `detached` is the server link's shared flag: true while no client is connected, so
        // `is_waiting()`/`status()` report `WaitingForDevice` (matching the mapper's event).
        let (link, control_tx, detached) = LinkServer::bind(addr)?;
        // Server role: a mapper (→ `fallback_active`), but no local device/reader (input arrives from
        // a remote client, so `controller_connected` is `None`).
        let fallback_active = Arc::new(AtomicBool::new(false));
        let mapper = spawn_mapper(
            sink, main, fallback, globals, link, running.clone(), fallback_active.clone(), events,
        );
        Ok(Runtime {
            running,
            detached,
            controller_connected: None,
            fallback_active: Some(fallback_active),
            control_tx,
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

    /// The live role, or `None` if this role has no local mapper (client — the role lives on the
    /// remote server). Read by `status()`; kept in lock-step with the `ActiveRole` event.
    pub(crate) fn active_role(&self) -> Option<Role> {
        self.fallback_active
            .as_ref()
            .map(|f| if f.load(Ordering::SeqCst) { Role::Fallback } else { Role::Main })
    }

    /// The control channel, for live `apply`/`set_globals` while running.
    pub fn control(&self) -> &Sender<Control> {
        &self.control_tx
    }

    /// Halt the loop and join both threads, releasing hardware (device → lizard restored on drop,
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
    cfg: DeviceCfg,
    link: LinkClient,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    events: EventSink,
) -> JoinHandle<Result<()>> {
    thread::Builder::new()
        .name("deckhand-reader".into())
        .spawn(move || {
            let result = run_reader(device, pinned_id, cfg, link, running, connected, events);
            // A reader error would otherwise be invisible until stop() joins it — log it now.
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
    globals: GlobalConfig,
    link: LinkServer,
    running: Arc<AtomicBool>,
    fallback_active: Arc<AtomicBool>,
    events: EventSink,
) -> JoinHandle<Result<()>> {
    thread::Builder::new()
        .name("deckhand-mapper".into())
        .spawn(move || {
            run_mapper(sink, main, fallback, globals, link, running, fallback_active, events)
        })
        .expect("spawn mapper thread")
}
