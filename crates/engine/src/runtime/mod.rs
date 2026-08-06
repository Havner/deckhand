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

use config::{GlobalConfig, Side};
use steam_hid::{Device, DeviceId, DeviceKind};
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
    /// Enable the IMU (derived from whether any program maps gyro; simplest: on).
    pub gyro: bool,
    /// LED brightness `0..=100 %`, or leave the device default.
    pub led_brightness: Option<u8>,
    /// Sleep/idle timeout in seconds, or leave the device default.
    pub idle_timeout: Option<u16>,
    /// Periodically re-assert lizard-off — the Deck (Neptune) reverts after ~10 s (PLAN §1.9).
    pub keepalive: bool,
}

impl DeviceCfg {
    /// A sensible config for `kind` from the globals (gyro on; keep-alive only where needed).
    pub fn for_device(kind: &DeviceKind, globals: &GlobalConfig) -> Self {
        DeviceCfg {
            gyro: true,
            led_brightness: globals.led_brightness,
            idle_timeout: globals.idle_timeout,
            keepalive: matches!(kind, DeviceKind::Neptune),
        }
    }
}

/// A control message to the mapping loop, from the `Engine` handle (live hot-swap). Device
/// reattach after an outage (D6) is **not** here — it goes through the [`link`] seam.
pub(crate) enum Control {
    /// Replace one role's program (main↔fallback), re-seeding the mapper if it's in use.
    Apply { program: Box<Program>, role: Role },
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

/// One-shot command-haptic click for the reader to fire immediately: a single `0x8f` pulse
/// (`count=1`) of `duration` µs on `side`'s pad — distinct from the sustained rumble train, and
/// with **no arbitration** (it briefly interrupts a rumble on the shared pad, which resumes next
/// re-fire; the opposite pad is untouched — PLAN §1.9 haptics v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Click {
    pub(crate) side: Side,
    pub(crate) duration: u16,
}

/// A running engine: the two threads + the control channel. Created by [`Runtime::start`] on
/// `Engine::start`, torn down by [`Runtime::stop`] on `Engine::stop` (config lives in the handle).
pub(crate) struct Runtime {
    running: Arc<AtomicBool>,
    /// Set by the link when the bound device's transport goes away — the loop stays up (pad
    /// plugged, outputs neutral) but is `WaitingForDevice` (PLAN §4.3, D5). Shared with both
    /// `Link*` ends (the client sets it, the mapper reads it).
    detached: Arc<AtomicBool>,
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
        main: Program,
        fallback: Option<Program>,
        globals: GlobalConfig,
        events: EventSink,
    ) -> Runtime {
        let running = Arc::new(AtomicBool::new(true));
        // The reader gets the client end, the mapper the server end; the handle keeps `control_tx`,
        // and `detached` is the shared `WaitingForDevice` flag (PLAN §6.1).
        let LocalLink { client, server, control_tx, detached } = link::local_link();
        let reader = spawn_reader(device, pinned_id, cfg, client, running.clone(), events.clone());
        let mapper =
            spawn_mapper(sink, main, fallback, globals, server, running.clone(), events);
        Runtime { running, detached, control_tx, reader: Some(reader), mapper: Some(mapper) }
    }

    /// **Client/forwarder** role (output=Network): reader only — it reads the device and forwards
    /// frames to the remote server at `addr`; there is no local mapper or `Sink`. The link's config
    /// uplink becomes the runtime's `control_tx`, so the handle's `apply`/`set_globals` travel to the
    /// server. Errors if the dial fails.
    pub fn start_client(
        device: Device,
        pinned_id: DeviceId,
        cfg: DeviceCfg,
        addr: SocketAddr,
        events: EventSink,
    ) -> Result<Runtime> {
        let running = Arc::new(AtomicBool::new(true));
        // The client has no local mapper, so no `WaitingForDevice` of its own (slice 5).
        let detached = Arc::new(AtomicBool::new(false));
        let link = LinkClient::connect(addr)?;
        let control_tx = link.control_tx().cloned().expect("a network client has a control uplink");
        let reader = spawn_reader(device, pinned_id, cfg, link, running.clone(), events);
        Ok(Runtime { running, detached, control_tx, reader: Some(reader), mapper: None })
    }

    /// **Server** role (input=Network): mapper only — it binds `addr`, receives a remote client's
    /// frames, and maps them to the local `Sink`; there is no local device or reader. The returned
    /// `control_tx` merges the server's *own* handle config with the client's wire config into
    /// `control_rx`. Errors if the bind fails.
    pub fn start_server(
        sink: Sink,
        main: Program,
        fallback: Option<Program>,
        globals: GlobalConfig,
        addr: SocketAddr,
        events: EventSink,
    ) -> Result<Runtime> {
        let running = Arc::new(AtomicBool::new(true));
        // Slice 5 will track link state → `WaitingForDevice`; for now a single connection stays up.
        let detached = Arc::new(AtomicBool::new(false));
        let (link, control_tx) = LinkServer::bind(addr)?;
        let mapper = spawn_mapper(sink, main, fallback, globals, link, running.clone(), events);
        Ok(Runtime { running, detached, control_tx, reader: None, mapper: Some(mapper) })
    }

    /// True when the loop is up but the bound device's transport is gone (`WaitingForDevice`).
    pub(crate) fn is_waiting(&self) -> bool {
        self.detached.load(Ordering::SeqCst)
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
fn spawn_reader(
    device: Device,
    pinned_id: DeviceId,
    cfg: DeviceCfg,
    link: LinkClient,
    running: Arc<AtomicBool>,
    events: EventSink,
) -> JoinHandle<Result<()>> {
    thread::Builder::new()
        .name("deckhand-reader".into())
        .spawn(move || {
            let result = run_reader(device, pinned_id, cfg, link, running, events);
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
    main: Program,
    fallback: Option<Program>,
    globals: GlobalConfig,
    link: LinkServer,
    running: Arc<AtomicBool>,
    events: EventSink,
) -> JoinHandle<Result<()>> {
    thread::Builder::new()
        .name("deckhand-mapper".into())
        .spawn(move || run_mapper(sink, main, fallback, globals, link, running, events))
        .expect("spawn mapper thread")
}
