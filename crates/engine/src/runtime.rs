//! The manager shell — reader-thread-per-device + the central mapping loop (PLAN §4.1, §4.2 S9).
//!
//! Concurrency is sync, no async (CLAUDE / PLAN §4): a **reader thread owns the `Device`** (which
//! is `Send`, not `Sync`) and is the *only* place that touches it — read loop, keep-alive,
//! re-apply-config-on-`Connected`, and haptic writes. It forwards frames over a channel to the
//! **central mapping loop**, which owns the `Sink` + `Mapper` + programs + globals, stamps the
//! monotonic [`Tick`], evaluates global chords first, runs the active program, emits outputs, and
//! polls the virtual pad's rumble back to the reader.
//!
//! The pieces here are wired to real hardware and are exercised end-to-end by the `Engine` handle
//! and the `deckhand-run` binary (S10, which HW-validates against the bridge). The pure helpers
//! ([`scale_rumble`], [`program_for`]) are unit-tested below.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, unbounded};

use config::GlobalConfig;
use steam_hid::{Device, DeviceKind, Motor, Report, Rumble as HidRumble};
use virt_out::{Rumble, Sink};

use crate::chords::Chords;
use crate::logical::LogicalFrame;
use crate::program::{Program, Role};
use crate::{HapticReq, Mapper, Result, Tick};

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

/// A control message from the `Engine` handle to the mapping loop (live hot-swap while running).
pub(crate) enum Control {
    /// Replace one role's program (active↔fallback), re-seeding the mapper if it's in use.
    Apply { program: Box<Program>, role: Role },
    /// Replace the global config (master rumble + chords).
    SetGlobals(Box<GlobalConfig>),
    /// Stop the loop (the running flag also gates it; this just wakes the `select!`).
    Stop,
}

/// A running engine: the two threads + the control channel. Created by [`Runtime::start`] on
/// `Engine::start`, torn down by [`Runtime::stop`] on `Engine::stop` (config lives in the handle).
pub(crate) struct Runtime {
    running: Arc<AtomicBool>,
    control_tx: Sender<Control>,
    reader: Option<JoinHandle<Result<()>>>,
    mapper: Option<JoinHandle<Result<()>>>,
}

impl Runtime {
    /// Acquire hardware and spawn the reader + mapping threads. `device` and `sink` are already
    /// opened/created by the caller so any HW error surfaces before the threads start.
    pub fn start(
        device: Device,
        cfg: DeviceCfg,
        sink: Sink,
        active: Program,
        fallback: Option<Program>,
        globals: GlobalConfig,
    ) -> Runtime {
        let running = Arc::new(AtomicBool::new(true));
        let (frame_tx, frame_rx) = unbounded::<Report>();
        let (rumble_tx, rumble_rx) = unbounded::<Rumble>();
        let (control_tx, control_rx) = unbounded::<Control>();

        let r_reader = running.clone();
        let reader = thread::Builder::new()
            .name("deckhand-reader".into())
            .spawn(move || run_reader(device, cfg, frame_tx, rumble_rx, r_reader))
            .expect("spawn reader thread");

        let r_mapper = running.clone();
        let mapper = thread::Builder::new()
            .name("deckhand-mapper".into())
            .spawn(move || {
                run_mapper(sink, active, fallback, globals, frame_rx, control_rx, rumble_tx, r_mapper)
            })
            .expect("spawn mapper thread");

        Runtime { running, control_tx, reader: Some(reader), mapper: Some(mapper) }
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

/// The reader thread: owns the `Device`, forwards frames, keeps the controller alive, re-applies
/// config on reconnect, and writes rumble pulse-trains. It is the only writer of the device.
fn run_reader(
    mut device: Device,
    cfg: DeviceCfg,
    frame_tx: Sender<Report>,
    rumble_rx: Receiver<Rumble>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    apply_device_cfg(&mut device, &cfg)?;
    let mut last_keepalive = Instant::now();
    let mut last_haptic = Instant::now();
    let mut level = Rumble::default();

    while running.load(Ordering::Relaxed) {
        // Short timeout so the loop laps to check `running`, keep-alive, and rumble regularly.
        match device.poll(Duration::from_millis(4)) {
            Ok(Some(report)) => {
                if matches!(report, Report::Connected) {
                    apply_device_cfg(&mut device, &cfg)?;
                }
                if frame_tx.send(report).is_err() {
                    break; // mapper gone
                }
            }
            Ok(None) => {} // timeout — no frame this cycle
            Err(_) => break, // transport gone → end (device drops → lizard restored)
        }

        if cfg.keepalive && last_keepalive.elapsed() >= Duration::from_secs(2) {
            let _ = device.set_lizard_mode(false);
            last_keepalive = Instant::now();
        }

        // Latest rumble level wins; re-issue the pulse train while non-zero (~60 Hz-ish, throttled
        // like the bridge). Zero level → nothing (the train stops).
        while let Ok(r) = rumble_rx.try_recv() {
            level = r;
        }
        if !level.is_zero() && last_haptic.elapsed() >= Duration::from_millis(120) {
            apply_haptics(&mut device, &level)?;
            last_haptic = Instant::now();
        }
    }
    Ok(())
}

/// The central mapping loop: owns the `Sink` + `Mapper` + programs + globals, runs one tick per
/// device frame (with an insurance timeout), and polls rumble back to the reader.
#[allow(clippy::too_many_arguments)]
fn run_mapper(
    mut sink: Sink,
    mut active: Program,
    mut fallback: Option<Program>,
    mut globals: GlobalConfig,
    frame_rx: Receiver<Report>,
    control_rx: Receiver<Control>,
    rumble_tx: Sender<Rumble>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let start = Instant::now();
    let mut mapper = Mapper::new(&active);
    let mut chords = Chords::new(&globals.chords);
    let mut role = Role::Active;
    let mut out: Vec<virt_out::OutputEvent> = Vec::new();
    let mut haptics: Vec<HapticReq> = Vec::new();
    let mut last_rumble = Rumble::default();

    while running.load(Ordering::Relaxed) {
        let mut stop = false;
        select! {
            recv(frame_rx) -> msg => match msg {
                Ok(Report::State(state)) => {
                    let frame = LogicalFrame::new(state);
                    // Chords first: they may switch the active/fallback role and consume buttons.
                    let outcome = chords.eval(&globals.chords, &frame);
                    if outcome.role != role {
                        role = outcome.role;
                        mapper.switch_program(program_for(&role, &active, &fallback));
                    }
                    let masked = frame.masked(&outcome.consumed);
                    let now = Tick(start.elapsed().as_millis() as u64);
                    out.clear();
                    haptics.clear();
                    mapper.tick(&masked, now, program_for(&role, &active, &fallback), &mut out, &mut haptics);
                    sink.emit(&out)?;
                }
                // Lifecycle/battery: the reader owns device re-apply; nothing to map here yet
                // (status surfacing is the Engine's job, S10). `Report` is non_exhaustive.
                Ok(_) => {}
                Err(_) => stop = true, // reader gone
            },
            recv(control_rx) -> msg => match msg {
                Ok(Control::Apply { program, role: target }) => match target {
                    Role::Active => {
                        active = *program;
                        if role == Role::Active {
                            mapper.switch_program(&active);
                        }
                    }
                    Role::Fallback => {
                        fallback = Some(*program);
                        if role == Role::Fallback {
                            mapper.switch_program(program_for(&role, &active, &fallback));
                        }
                    }
                },
                Ok(Control::SetGlobals(g)) => {
                    globals = *g;
                    chords = Chords::new(&globals.chords);
                }
                Ok(Control::Stop) | Err(_) => stop = true,
            },
            // Insurance: devices stream ~250 Hz, but tick anyway so rumble is polled when idle.
            default(Duration::from_millis(8)) => {}
        }
        if stop {
            break;
        }

        // Rumble back-channel (game → virtual pad → real controller), scaled by master rumble.
        // Send only on change so the reader's channel doesn't churn.
        let rumble = scale_rumble(sink.poll_rumble()?, globals.master_rumble);
        if rumble != last_rumble {
            let _ = rumble_tx.send(rumble.clone());
            last_rumble = rumble;
        }
    }
    Ok(())
}

/// Apply lizard-off + gyro + optional LED/idle. Called on start and every `Connected`.
fn apply_device_cfg(device: &mut Device, cfg: &DeviceCfg) -> Result<()> {
    device.set_lizard_mode(false)?;
    device.set_gyro(cfg.gyro)?;
    if let Some(b) = cfg.led_brightness {
        device.set_led_intensity(b)?;
    }
    if let Some(t) = cfg.idle_timeout {
        device.set_idle_timeout(t)?;
    }
    Ok(())
}

/// Route a rumble level to Gordon's trackpad actuators as pulse-trains (strong→left, weak→right;
/// PLAN §1.9). Re-fired by the reader while the level stays non-zero.
fn apply_haptics(device: &mut Device, rumble: &Rumble) -> Result<()> {
    if rumble.strong > 0 {
        device.rumble(Motor::Left, train(rumble.strong))?;
    }
    if rumble.weak > 0 {
        device.rumble(Motor::Right, train(rumble.weak))?;
    }
    Ok(())
}

/// A ~60 Hz pulse train whose duty cycle encodes magnitude (crude amplitude; PLAN §1.9 / bridge).
fn train(magnitude: u16) -> HidRumble {
    const PERIOD_US: u32 = 16_667; // ~60 Hz, within Gordon's rumble range
    const STRENGTH: f32 = 0.5; // scale duty (our amplitude lever) to 50 %
    let full = magnitude as f32 / u16::MAX as f32;
    let duty = ((full * STRENGTH * PERIOD_US as f32) as u32).clamp(600, PERIOD_US - 600);
    HidRumble { duration: duty as u16, interval: (PERIOD_US - duty) as u16, count: 12, gain: 0 }
}

/// Scale a rumble by the master percentage (`0..=100`), saturating.
fn scale_rumble(r: Rumble, master: u8) -> Rumble {
    let scaled = |v: u16| ((v as u32 * master.min(100) as u32) / 100) as u16;
    Rumble { strong: scaled(r.strong), weak: scaled(r.weak) }
}

/// The program driving a given role (fallback falls back to active when unset).
fn program_for<'a>(role: &Role, active: &'a Program, fallback: &'a Option<Program>) -> &'a Program {
    match role {
        Role::Active => active,
        Role::Fallback => fallback.as_ref().unwrap_or(active),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{ProgramMeta, SetId, SourceMap};

    fn prog(name: &str) -> Program {
        Program {
            meta: ProgramMeta { name: name.into(), role: Role::Active },
            default_set: SetId::new(0),
            sets: vec![crate::program::CompiledSet {
                name: "s".into(),
                base: SourceMap::new(),
                layers: vec![],
            }],
        }
    }

    #[test]
    fn scale_rumble_by_master_percent() {
        let r = Rumble { strong: 1000, weak: 500 };
        assert_eq!(scale_rumble(r.clone(), 100), r);
        assert_eq!(scale_rumble(r.clone(), 50), Rumble { strong: 500, weak: 250 });
        assert_eq!(scale_rumble(r.clone(), 0), Rumble::default());
        // Master is clamped at 100 (no amplification).
        assert_eq!(scale_rumble(r, 200), Rumble { strong: 1000, weak: 500 });
    }

    #[test]
    fn program_for_falls_back_to_active_when_unset() {
        let active = prog("active");
        let none: Option<Program> = None;
        assert_eq!(program_for(&Role::Fallback, &active, &none).meta.name, "active");
        let fb = Some(prog("fallback"));
        assert_eq!(program_for(&Role::Fallback, &active, &fb).meta.name, "fallback");
        assert_eq!(program_for(&Role::Active, &active, &fb).meta.name, "active");
    }
}
