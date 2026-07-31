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

mod mapping;
mod reader;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, unbounded};

use config::{GlobalConfig, Side};
use steam_hid::{Device, DeviceId, DeviceKind, Report};
use virt_out::Sink;

use crate::Result;
use crate::program::{Program, Role};

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

/// A control message to the mapping loop. Most come from the `Engine` handle (live hot-swap);
/// `Reattach` comes from the reader after it reacquires a device (D6).
pub(crate) enum Control {
    /// Replace one role's program (main↔fallback), re-seeding the mapper if it's in use.
    Apply { program: Box<Program>, role: Role },
    /// Replace the global config (master rumble + chords).
    SetGlobals(Box<GlobalConfig>),
    /// Stop the loop (the running flag also gates it; this just wakes the `select!`).
    Stop,
    /// The reader reacquired the pinned device (D6): swap to these fresh channels and resume
    /// mapping — the pad stayed plugged throughout, so the game never saw a disconnect.
    Reattach {
        frame_rx: Receiver<Report>,
        rumble_tx: Sender<RumbleCmd>,
        click_tx: Sender<Click>,
    },
}

/// The effective rumble to realize on the controller: per-pad drive (already scaled by master ×
/// profile strength × curve) plus the pulse frequency from the main profile. Produced by the
/// mapping loop's `rumble_cmd`, realized by the reader's `apply_haptics`.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct RumbleCmd {
    pub(crate) strong: u16,
    pub(crate) weak: u16,
    pub(crate) hz: u16,
}

/// One-shot command-haptic click for the reader to fire immediately: a single `0x8f` pulse
/// (`count=1`) of `duration` µs on `side`'s pad — distinct from the sustained rumble train, and
/// with **no arbitration** (it briefly interrupts a rumble on the shared pad, which resumes next
/// re-fire; the opposite pad is untouched — PLAN §1.9 haptics v1).
pub(crate) struct Click {
    pub(crate) side: Side,
    pub(crate) duration: u16,
}

/// A running engine: the two threads + the control channel. Created by [`Runtime::start`] on
/// `Engine::start`, torn down by [`Runtime::stop`] on `Engine::stop` (config lives in the handle).
pub(crate) struct Runtime {
    running: Arc<AtomicBool>,
    /// Set by the reader when the bound device's transport goes away — the loop stays up (pad
    /// plugged, outputs neutral) but is `WaitingForDevice` (PLAN §4.3, D5).
    waiting: Arc<AtomicBool>,
    control_tx: Sender<Control>,
    reader: Option<JoinHandle<Result<()>>>,
    mapper: Option<JoinHandle<Result<()>>>,
}

impl Runtime {
    /// Acquire hardware and spawn the reader + mapping threads. `device` and `sink` are already
    /// opened/created by the caller so any HW error surfaces before the threads start.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        device: Device,
        pinned_id: DeviceId,
        cfg: DeviceCfg,
        sink: Sink,
        main: Program,
        fallback: Option<Program>,
        globals: GlobalConfig,
    ) -> Runtime {
        let running = Arc::new(AtomicBool::new(true));
        let waiting = Arc::new(AtomicBool::new(false));
        let (frame_tx, frame_rx) = unbounded::<Report>();
        let (rumble_tx, rumble_rx) = unbounded::<RumbleCmd>();
        let (click_tx, click_rx) = unbounded::<Click>();
        let (control_tx, control_rx) = unbounded::<Control>();

        let r_reader = running.clone();
        let w_reader = waiting.clone();
        // The reader also sends `Reattach` after reacquiring a device (D6), so it holds a control tx.
        let reader_ctl = control_tx.clone();
        let reader = thread::Builder::new()
            .name("deckhand-reader".into())
            .spawn(move || {
                let result = run_reader(
                    device, pinned_id, cfg, reader_ctl, frame_tx, rumble_rx, click_rx, r_reader,
                    w_reader,
                );
                // A reader error would otherwise be invisible until stop() joins it — log it now.
                if let Err(ref e) = result {
                    log::error!("reader thread exited with error: {e}");
                }
                result
            })
            .expect("spawn reader thread");

        let r_mapper = running.clone();
        let w_mapper = waiting.clone();
        let mapper = thread::Builder::new()
            .name("deckhand-mapper".into())
            .spawn(move || {
                run_mapper(
                    sink, main, fallback, globals, frame_rx, control_rx, rumble_tx, click_tx,
                    r_mapper, w_mapper,
                )
            })
            .expect("spawn mapper thread");

        Runtime { running, waiting, control_tx, reader: Some(reader), mapper: Some(mapper) }
    }

    /// True when the loop is up but the bound device's transport is gone (`WaitingForDevice`).
    pub(crate) fn is_waiting(&self) -> bool {
        self.waiting.load(Ordering::SeqCst)
    }

    /// The control channel, for live `apply`/`set_globals` while running.
    pub fn control(&self) -> &Sender<Control> {
        &self.control_tx
    }

    /// Halt the loop and join both threads, releasing hardware (device → lizard restored on drop,
    /// virtual pad unplugged). Returns the first thread error, if any.
    pub fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.control_tx.send(Control::Stop); // wake the mapper's select immediately
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
