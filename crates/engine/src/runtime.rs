//! The manager shell — reader-thread-per-device + the central mapping loop (PLAN §4.1, §4.2 S9).
//!
//! Concurrency is sync, no async (CLAUDE / PLAN §4): a **reader thread owns the `Device`** (which
//! is `Send`, not `Sync`) and is the *only* place that touches it — read loop, keep-alive,
//! re-apply-config-on-`Connected`, and haptic writes. It forwards frames over a channel to the
//! **central mapping loop**, which owns the `Sink` + `Mapper` + programs + globals, stamps the
//! monotonic [`Tick`], evaluates global chords first, runs the active (Main-or-Fallback) program,
//! emits outputs, and polls the virtual pad's rumble back to the reader.
//!
//! The pieces here are wired to real hardware and are exercised end-to-end by the `Engine` handle
//! and the `deckhandd` daemon (which HW-validates against the bridge). The pure helpers
//! ([`rumble_cmd`], [`program_for`]) are unit-tested below.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvError, Sender, select, unbounded};

use config::{GlobalConfig, HapticStrength, RumbleSettings, Side, StartProfile};
use steam_hid::{Device, DeviceId, DeviceKind, Manager, Motor, Report, Rumble as HidRumble};
use virt_out::{OutputEvent, Rumble, Sink};

use crate::chords::{Chords, ExecReq};
use crate::event::EngineEvent;
use crate::handle::Status;
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
                run_reader(
                    device, pinned_id, cfg, reader_ctl, frame_tx, rumble_rx, click_rx, r_reader,
                    w_reader,
                )
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

/// How often the reader re-enumerates while waiting for the pinned device to return (D6).
const REACQUIRE_POLL_MS: u64 = 1000;

/// Why a device read-session ended.
enum SessionEnd {
    /// `running` was cleared or the mapper is gone → the reader should exit.
    Stop,
    /// The device's transport went away (unplug / dongle removed) → reacquire.
    TransportGone,
}

/// The reader thread: a persistent **device-session loop** that owns the current `Device`, is its
/// only writer, and survives a transport-gone by **reacquiring** the *same* pinned device (D6). It
/// reads until the session ends; on transport-gone it flags `waiting` + emits `BindingLost`, waits
/// for the pinned `DeviceId` to reappear (its own `Manager`), reopens it, mints fresh channels, and
/// tells the mapper to [`Control::Reattach`] — so the virtual pad never leaves.
#[allow(clippy::too_many_arguments)] // the reader owns the device + all its channels + reacquire.
fn run_reader(
    mut device: Device,
    pinned_id: DeviceId,
    cfg: DeviceCfg,
    control_tx: Sender<Control>,
    mut frame_tx: Sender<Report>,
    mut rumble_rx: Receiver<RumbleCmd>,
    mut click_rx: Receiver<Click>,
    running: Arc<AtomicBool>,
    waiting: Arc<AtomicBool>,
) -> Result<()> {
    // A separate `Manager` (own hidapi context) used only for reacquire enumeration/open — created
    // lazily so a session that never disconnects pays nothing.
    let mut manager: Option<Manager> = None;

    loop {
        match read_session(&mut device, &cfg, &frame_tx, &rumble_rx, &click_rx, &running)? {
            SessionEnd::Stop => return Ok(()),
            SessionEnd::TransportGone => {}
        }

        // Transport gone: mapper keeps the pad plugged + enters WaitingForDevice (D5). Drop the
        // dead device (→ lizard restored), then wait for the *same* device to return.
        waiting.store(true, Ordering::SeqCst);
        EngineEvent::BindingLost.emit();
        drop(device);

        let mgr = manager.get_or_insert_with(|| Manager::new().expect("hidapi context for reacquire"));
        let Some(new_device) = reacquire(mgr, &pinned_id, &running) else {
            return Ok(()); // stopped while waiting
        };

        // Reacquired: mint fresh channels, hand the mapper's ends over, and resume this loop with
        // the reader's ends. The mapper swaps and returns to the connected phase (pad never left).
        let (ftx, frx) = unbounded::<Report>();
        let (rtx, rrx) = unbounded::<RumbleCmd>();
        let (ctx, crx) = unbounded::<Click>();
        waiting.store(false, Ordering::SeqCst);
        EngineEvent::BindingAcquired(pinned_id.clone()).emit();
        EngineEvent::State(Status::Running).emit();
        if control_tx.send(Control::Reattach { frame_rx: frx, rumble_tx: rtx, click_tx: ctx }).is_err()
        {
            return Ok(()); // mapper gone
        }
        device = new_device;
        frame_tx = ftx;
        rumble_rx = rrx;
        click_rx = crx;
    }
}

/// Read one device session: forward frames, surface connect/disconnect/battery events, keep the
/// controller alive, and write rumble/click. Returns when the session ends (stop or transport-gone).
fn read_session(
    device: &mut Device,
    cfg: &DeviceCfg,
    frame_tx: &Sender<Report>,
    rumble_rx: &Receiver<RumbleCmd>,
    click_rx: &Receiver<Click>,
    running: &AtomicBool,
) -> Result<SessionEnd> {
    apply_device_cfg(device, cfg)?;
    let mut last_keepalive = Instant::now();
    let mut last_haptic = Instant::now();
    let mut level = RumbleCmd::default();
    let mut last_battery: Option<u8> = None;

    while running.load(Ordering::Relaxed) {
        // Short timeout so the loop laps to check `running`, keep-alive, and rumble regularly.
        match device.poll(Duration::from_millis(4)) {
            Ok(Some(report)) => {
                match &report {
                    Report::Connected => {
                        EngineEvent::ControllerConnected.emit();
                        apply_device_cfg(device, cfg)?;
                    }
                    Report::Disconnected => EngineEvent::ControllerDisconnected.emit(),
                    // Edge-triggered: the 0x04 report streams ~1 Hz, so only surface a change.
                    Report::Battery(b) if last_battery != Some(b.charge_percent) => {
                        EngineEvent::BatteryChanged { percent: b.charge_percent }.emit();
                        last_battery = Some(b.charge_percent);
                    }
                    _ => {} // State (per-frame, too noisy); unchanged Battery.
                }
                if frame_tx.send(report).is_err() {
                    return Ok(SessionEnd::Stop); // mapper gone
                }
            }
            Ok(None) => {} // timeout — no frame this cycle
            Err(e) => {
                log::warn!("controller read error ({e}) — binding lost, waiting for device");
                return Ok(SessionEnd::TransportGone);
            }
        }

        if cfg.keepalive && last_keepalive.elapsed() >= Duration::from_secs(2) {
            let _ = device.set_lizard_mode(false);
            last_keepalive = Instant::now();
        }

        // Latest rumble level wins; re-issue the pulse train just before it ends so a sustained
        // rumble is one contiguous drive (the actuator rings up), not a mid-train restart. Zero
        // level → nothing (the train plays out and stops).
        while let Ok(r) = rumble_rx.try_recv() {
            level = r;
        }
        if (level.strong > 0 || level.weak > 0)
            && last_haptic.elapsed() >= Duration::from_millis(RUMBLE_REFIRE_MS)
        {
            apply_haptics(device, &level)?;
            last_haptic = Instant::now();
        }

        // One-shot command-haptic clicks fire immediately (no arbitration — a click may briefly
        // interrupt the rumble train on its pad, which the re-fire above resumes).
        while let Ok(click) = click_rx.try_recv() {
            fire_click(device, &click)?;
        }
    }
    Ok(SessionEnd::Stop)
}

/// Poll (~1 Hz) for the pinned device to reappear, then open it. `None` if `running` is cleared
/// while waiting. Matches on the stable [`DeviceId`] (survives a `/dev/hidrawN` change) and retries
/// on open failure (a dongle slot can flicker mid-enumeration).
fn reacquire(mgr: &mut Manager, pinned_id: &DeviceId, running: &AtomicBool) -> Option<Device> {
    while running.load(Ordering::Relaxed) {
        if let Ok(infos) = mgr.enumerate()
            && let Some(info) = infos.iter().find(|i| &i.id() == pinned_id)
        {
            match mgr.open(info) {
                Ok(device) => {
                    log::info!("device {pinned_id} returned — reattaching");
                    return Some(device);
                }
                Err(e) => log::warn!("reacquire open failed ({e}) — retrying"),
            }
        }
        thread::sleep(Duration::from_millis(REACQUIRE_POLL_MS));
    }
    None
}

/// The central mapping loop: owns the `Sink` + `Mapper` + programs + globals. It alternates a
/// **connected phase** (map one tick per device frame, poll rumble back) with a **waiting phase**
/// (transport gone → release outputs, keep the pad plugged, idle until reattach/stop). The frame /
/// rumble / click channels are swapped on reattach (D6), so the same `Sink` serves across an outage.
#[allow(clippy::too_many_arguments)]
fn run_mapper(
    mut sink: Sink,
    mut main: Program,
    mut fallback: Option<Program>,
    mut globals: GlobalConfig,
    mut frame_rx: Receiver<Report>,
    control_rx: Receiver<Control>,
    mut rumble_tx: Sender<RumbleCmd>,
    mut click_tx: Sender<Click>,
    running: Arc<AtomicBool>,
    waiting: Arc<AtomicBool>,
) -> Result<()> {
    let start = Instant::now();
    // Boot into the role named by `start_profile` (read once here — it's a start-only setting).
    let mut role = start_role(&globals);
    let mut chords = Chords::new(&globals.chords, role == Role::Fallback);
    let mut mapper = Mapper::new(program_for(&role, &main, &fallback));
    let mut out: Vec<virt_out::OutputEvent> = Vec::new();
    let mut haptics: Vec<HapticReq> = Vec::new();
    let mut last_rumble = RumbleCmd::default();

    'session: loop {
        // ---- connected phase ----
        let mut lost = false;
        while running.load(Ordering::Relaxed) {
            let mut stop = false;
            select! {
                recv(frame_rx) -> msg => match msg {
                    Ok(Report::State(state)) => {
                        let frame = LogicalFrame::new(state);
                        // Chords first: they may switch the main/fallback role and consume buttons.
                        let outcome = chords.eval(&globals.chords, &frame);
                        if outcome.role != role {
                            role = outcome.role;
                            let prog = program_for(&role, &main, &fallback);
                            log::info!("chord: switched to {role:?} — profile '{}' now active", prog.meta.name);
                            mapper.switch_program(prog);
                        }
                        let masked = frame.masked(&outcome.consumed);
                        // CommandExecute chords: run each on its own thread so the loop never blocks.
                        for exec in outcome.execute {
                            spawn_command(exec);
                        }
                        let now = Tick(start.elapsed().as_millis() as u64);
                        out.clear();
                        haptics.clear();
                        mapper.tick(&masked, now, program_for(&role, &main, &fallback), &mut out, &mut haptics);
                        sink.emit(&out)?;
                        // Command-haptic pulses this tick → the reader (scaled by master rumble %).
                        for h in haptics.drain(..) {
                            let duration = click_duration(&h.strength, globals.master_rumble);
                            let _ = click_tx.send(Click { side: h.side, duration });
                        }
                    }
                    // Controller gone but the transport (dongle) is alive → release outputs so
                    // nothing sticks (e.g. a held stick keeping the character running). The reader
                    // stays up; a `Connected` report resumes mapping. DEFERRED (to-decide): this
                    // drops *outputs* but keeps latches (toggles, active layers), so a toggle
                    // re-asserts on reconnect — revisit whether a disconnect should reset state.
                    Ok(Report::Disconnected) => {
                        out.clear();
                        mapper.release_all(&mut out);
                        sink.emit(&out)?;
                    }
                    // Connected / Battery: surfaced by the reader (D4); nothing to map here.
                    // `Report` is non_exhaustive.
                    Ok(_) => {}
                    // Reader gone: transport-lost (it flagged `waiting`) → waiting phase; else stop.
                    Err(_) => {
                        if waiting.load(Ordering::SeqCst) {
                            lost = true;
                        } else {
                            stop = true;
                        }
                    }
                },
                recv(control_rx) -> msg => {
                    stop = apply_control(
                        msg, &mut main, &mut fallback, &mut globals, &mut mapper, &role, &mut chords,
                    );
                }
                // Insurance: devices stream ~250 Hz, but tick anyway so rumble is polled when idle.
                default(Duration::from_millis(8)) => {}
            }
            if stop {
                return Ok(());
            }
            if lost {
                break;
            }

            // Rumble back-channel (game → virtual pad → real controller): scale by the global
            // master % and the *main* profile's strength/curve, carrying its pulse Hz. On change.
            let prog = program_for(&role, &main, &fallback);
            let cmd = rumble_cmd(sink.poll_rumble()?, globals.master_rumble, &prog.rumble);
            if cmd != last_rumble {
                let _ = rumble_tx.send(cmd.clone());
                last_rumble = cmd;
            }
        }
        if !lost {
            return Ok(()); // clean stop (`running` cleared) mid-connected.
        }

        // ---- waiting phase: release outputs, keep the pad plugged, await reattach / stop ----
        match run_waiting(
            &mut sink, &mut main, &mut fallback, &mut globals, &mut mapper, &role, &mut chords,
            &control_rx, &mut frame_rx, &mut rumble_tx, &mut click_tx, &running,
        )? {
            WaitOutcome::Stopped => return Ok(()),
            // Reattached (channels swapped) → resume the connected phase on the new device.
            WaitOutcome::Reattached => continue 'session,
        }
    }
}

/// Handle one control message (shared by the connected and waiting phases). Returns true to stop.
fn apply_control(
    msg: std::result::Result<Control, RecvError>,
    main: &mut Program,
    fallback: &mut Option<Program>,
    globals: &mut GlobalConfig,
    mapper: &mut Mapper,
    role: &Role,
    chords: &mut Chords,
) -> bool {
    match msg {
        Ok(Control::Apply { program, role: target }) => {
            match target {
                Role::Main => {
                    *main = *program;
                    if *role == Role::Main {
                        mapper.switch_program(main);
                    }
                }
                Role::Fallback => {
                    *fallback = Some(*program);
                    if *role == Role::Fallback {
                        mapper.switch_program(program_for(role, main, fallback));
                    }
                }
            }
            false
        }
        Ok(Control::SetGlobals(g)) => {
            *globals = *g;
            // Preserve the current role base across the swap — `start_profile` is start-only.
            *chords = Chords::new(&globals.chords, chords.fallback_base());
            false
        }
        // Reattach is handled directly in the waiting phase; it only reaches here if it somehow
        // arrives while connected (it can't) — ignore rather than reject.
        Ok(Control::Reattach { .. }) => false,
        Ok(Control::Stop) | Err(_) => true,
    }
}

/// Why the waiting phase ended.
enum WaitOutcome {
    /// A stop was requested.
    Stopped,
    /// The reader reacquired the device and the frame/rumble/click channels were swapped in.
    Reattached,
}

/// Transport-lost phase: release every applied output so nothing sticks, keep the virtual pad
/// plugged, and idle until the reader reattaches (channels swapped) or a stop. Config hot-swaps are
/// still accepted so a reattach uses the latest programs; the pad's rumble uploads are drained so a
/// game's force-feedback thread doesn't block on the still-present pad.
#[allow(clippy::too_many_arguments)]
fn run_waiting(
    sink: &mut Sink,
    main: &mut Program,
    fallback: &mut Option<Program>,
    globals: &mut GlobalConfig,
    mapper: &mut Mapper,
    role: &Role,
    chords: &mut Chords,
    control_rx: &Receiver<Control>,
    frame_rx: &mut Receiver<Report>,
    rumble_tx: &mut Sender<RumbleCmd>,
    click_tx: &mut Sender<Click>,
    running: &AtomicBool,
) -> Result<WaitOutcome> {
    let mut out: Vec<OutputEvent> = Vec::new();
    mapper.release_all(&mut out);
    sink.emit(&out)?;
    EngineEvent::State(Status::WaitingForDevice).emit();

    while running.load(Ordering::Relaxed) {
        select! {
            recv(control_rx) -> msg => match msg {
                Ok(Control::Reattach { frame_rx: frx, rumble_tx: rtx, click_tx: ctx }) => {
                    *frame_rx = frx;
                    *rumble_tx = rtx;
                    *click_tx = ctx;
                    return Ok(WaitOutcome::Reattached);
                }
                other => {
                    if apply_control(other, main, fallback, globals, mapper, role, chords) {
                        return Ok(WaitOutcome::Stopped);
                    }
                }
            },
            default(Duration::from_millis(50)) => {}
        }
        // Discard rumble (no controller to feed) — but drain it so the game's FF thread isn't stuck.
        let _ = sink.poll_rumble();
    }
    Ok(WaitOutcome::Stopped)
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

/// The effective rumble to realize on the controller: per-pad drive (already scaled by master ×
/// profile strength × curve) plus the pulse frequency from the main profile.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct RumbleCmd {
    strong: u16,
    weak: u16,
    hz: u16,
}

/// Route a rumble command to Gordon's trackpad actuators as pulse-trains (strong→left, weak→right;
/// PLAN §1.9). Re-fired by the reader while the level stays non-zero.
fn apply_haptics(device: &mut Device, cmd: &RumbleCmd) -> Result<()> {
    if cmd.strong > 0 {
        device.rumble(Motor::Left, train(cmd.strong, cmd.hz))?;
    }
    if cmd.weak > 0 {
        device.rumble(Motor::Right, train(cmd.weak, cmd.hz))?;
    }
    Ok(())
}

/// One-shot command-haptic click for the reader to fire immediately: a single `0x8f` pulse
/// (`count=1`) of `duration` µs on `side`'s pad — distinct from the sustained rumble train, and
/// with **no arbitration** (it briefly interrupts a rumble on the shared pad, which resumes next
/// re-fire; the opposite pad is untouched — PLAN §1.9 haptics v1).
pub(crate) struct Click {
    side: Side,
    duration: u16,
}

/// Command-haptic click durations (µs) for Low/Med/High — a single pulse, HW-tuned via the `haptic`
/// example (`interval`/`count` are fixed; the duration is the strength). Scaled by master rumble %.
const CLICK_LOW_US: u16 = 500;
const CLICK_MED_US: u16 = 1000;
const CLICK_HIGH_US: u16 = 2000;
/// Trailing off-phase of the single click pulse (irrelevant to the felt tick at `count=1`).
const CLICK_INTERVAL_US: u16 = 1000;

/// The click pulse duration for `strength`, attenuated by the global `master` % (like all haptics).
fn click_duration(strength: &HapticStrength, master: u8) -> u16 {
    let base = match strength {
        HapticStrength::Low => CLICK_LOW_US,
        HapticStrength::Medium => CLICK_MED_US,
        HapticStrength::High => CLICK_HIGH_US,
    };
    ((base as u32 * master.min(100) as u32) / 100).max(1) as u16
}

/// Fire one command-haptic click on its pad (`Side::Left`→left actuator, `Side::Right`→right).
fn fire_click(device: &mut Device, click: &Click) -> Result<()> {
    let motor = match click.side {
        Side::Left => Motor::Left,
        Side::Right => Motor::Right,
    };
    device.rumble(
        motor,
        HidRumble { duration: click.duration, interval: CLICK_INTERVAL_US, count: 1, gain: 0 },
    )?;
    Ok(())
}

/// The duty cycle full drive maps to. Gordon's pad actuator saturates above ~25% duty
/// (HW-tested: the useful strength band is ~1–25%, above that feels identical), so `drive`
/// spans `0..RUMBLE_MAX_DUTY` of the period rather than `0..1`. Kept **linear** within the band
/// (close enough; the per-profile `curve` fine-tunes). **HW-specific** — likely differs on other
/// controllers / haptic packets, so revisit per-`Shape` if that turns out to matter.
const RUMBLE_MAX_DUTY: f32 = 0.25;

/// A pulse train at the profile's `hz` whose duty cycle encodes the already-scaled `drive` (the
/// only amplitude lever — `gain` is ignored on Gordon). Maps full drive onto the actuator's
/// useful duty band ([`RUMBLE_MAX_DUTY`]) at µs resolution, so even a fraction-of-a-percent
/// effective strength produces a distinct (small) pulse.
fn train(drive: u16, hz: u16) -> HidRumble {
    let hz = hz.clamp(16, 1000); // period must fit u16 (hz ≥ 16); Gordon's usable range
    let period = 1_000_000u32 / hz as u32; // µs
    let full = drive as f32 / u16::MAX as f32;
    // Full drive → MAX_DUTY of the period; keep the µs precision so tiny drives stay tiny. A
    // valid pulse still needs duration ≥ 1 and interval ≥ 1.
    let duty = (full * RUMBLE_MAX_DUTY * period as f32) as u32;
    let duty = duty.clamp(1, period - 1);
    let count = ((hz as u32 * RUMBLE_TRAIN_MS) / 1000).max(1) as u16;
    HidRumble { duration: duty as u16, interval: (period - duty) as u16, count, gain: 0 }
}

/// Pulse-train length (ms). The pad actuator **rings up** over many cycles, so a train must be
/// long enough to reach full amplitude — a too-short one feels weak regardless of duty. The reader
/// re-fires just *before* this elapses ([`RUMBLE_REFIRE_MS`]) so a sustained rumble is one
/// near-continuous drive, not a train restarted mid-swing (which never rings up). HW-tuned.
const RUMBLE_TRAIN_MS: u32 = 250;
/// How often the reader re-issues the train — a hair under [`RUMBLE_TRAIN_MS`] so trains are
/// contiguous (re-fire near the train's end, not mid-play) while never leaving a silent gap.
const RUMBLE_REFIRE_MS: u64 = 220;

/// Compute the effective per-pad drive from a raw game rumble, the global master %, and the main
/// profile's rumble settings (strength % + response curve); `hz` passes through from the profile.
fn rumble_cmd(raw: Rumble, master: u8, s: &RumbleSettings) -> RumbleCmd {
    // `master` (0..=100) attenuates globally; per-profile `strength` MAY exceed 100 to *boost* a
    // game that under-drives its FF — many cap well below full range (observed: 25%), so at
    // `MAX_DUTY` they'd never reach the actuator's saturation. The boost normalizes such a game
    // back up; the drive still clamps at `u16::MAX` (→ RUMBLE_MAX_DUTY), so it can't overshoot.
    let scale = (master.min(100) as f32 / 100.0) * (s.strength as f32 / 100.0);
    let drive = |v: u16| {
        let full = (v as f32 / u16::MAX as f32) * scale;
        (s.curve.apply(full.clamp(0.0, 1.0)).clamp(0.0, 1.0) * u16::MAX as f32) as u16
    };
    RumbleCmd { strong: drive(raw.strong), weak: drive(raw.weak), hz: s.hz }
}

/// The program driving a given role (fallback falls back to main when unset).
fn program_for<'a>(role: &Role, main: &'a Program, fallback: &'a Option<Program>) -> &'a Program {
    match role {
        Role::Main => main,
        Role::Fallback => fallback.as_ref().unwrap_or(main),
    }
}

/// Run a `CommandExecute` chord's program on a **detached thread** so the mapping loop never blocks
/// on it. Headless: a direct exec (no shell). Logs the invocation and its exit status + captured
/// stdout/stderr at `info` (spawn failure at `warn`).
fn spawn_command(exec: ExecReq) {
    std::thread::spawn(move || {
        let shown = if exec.args.is_empty() {
            exec.command.clone()
        } else {
            format!("{} {}", exec.command, exec.args.join(" "))
        };
        log::info!("chord: executing `{shown}`");
        match std::process::Command::new(&exec.command).args(&exec.args).output() {
            Ok(o) => {
                // Three lines: status, then raw stdout/stderr (Display, not Debug, so newlines
                // render and there are no wrapping quotes).
                log::info!("chord: `{shown}` exited {}", o.status);
                log::info!("chord: `{shown}` stdout:\n{}", String::from_utf8_lossy(&o.stdout).trim_end());
                log::info!("chord: `{shown}` stderr:\n{}", String::from_utf8_lossy(&o.stderr).trim_end());
            }
            Err(e) => log::warn!("chord: `{shown}` failed to spawn: {e}"),
        }
    });
}

/// The role the engine boots into, from `GlobalConfig::start_profile` (read once at loop start).
fn start_role(globals: &GlobalConfig) -> Role {
    match globals.start_profile {
        StartProfile::Main => Role::Main,
        StartProfile::Fallback => Role::Fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Curve;
    use crate::program::{ProgramMeta, SetId, SourceMap};

    fn prog(name: &str) -> Program {
        Program {
            meta: ProgramMeta { name: name.into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
            sets: vec![crate::program::CompiledSet {
                name: "s".into(),
                base: SourceMap::new(),
                layers: vec![],
            }],
        }
    }

    #[test]
    fn rumble_cmd_applies_master_strength_and_hz() {
        let full = Rumble { strong: u16::MAX, weak: u16::MAX / 2 };

        // master 50% × strength 100% (defaults) = half; hz carried from the profile.
        let s = RumbleSettings { hz: 60, strength: 100, curve: Curve::Linear };
        let cmd = rumble_cmd(full.clone(), 50, &s);
        assert_eq!(cmd.hz, 60);
        assert!((cmd.strong as i32 - (u16::MAX / 2) as i32).abs() <= 1);
        assert!((cmd.weak as i32 - (u16::MAX / 4) as i32).abs() <= 1);

        // master 100% × strength 50% = half too; different hz passes through.
        let s = RumbleSettings { hz: 200, strength: 50, curve: Curve::Linear };
        let cmd = rumble_cmd(full.clone(), 100, &s);
        assert_eq!(cmd.hz, 200);
        assert!((cmd.strong as i32 - (u16::MAX / 2) as i32).abs() <= 1);

        // master 0% → silent.
        let cmd = rumble_cmd(full, 0, &s);
        assert_eq!((cmd.strong, cmd.weak), (0, 0));

        // strength > 100 boosts a game that under-drives its FF: a quarter-range input at 200%
        // reaches half drive, and the boost clamps at the packet max instead of overflowing.
        let boost = RumbleSettings { hz: 80, strength: 200, curve: Curve::Linear };
        let quarter = Rumble { strong: u16::MAX / 4, weak: 0 };
        let cmd = rumble_cmd(quarter, 100, &boost);
        assert!((cmd.strong as i32 - (u16::MAX / 2) as i32).abs() <= 2);
        let cmd = rumble_cmd(Rumble { strong: u16::MAX, weak: 0 }, 100, &boost);
        assert_eq!(cmd.strong, u16::MAX);
    }

    #[test]
    fn click_duration_maps_strength_and_scales_with_master() {
        // Low/Med/High map to their base durations at full master.
        assert_eq!(click_duration(&HapticStrength::Low, 100), CLICK_LOW_US);
        assert_eq!(click_duration(&HapticStrength::Medium, 100), CLICK_MED_US);
        assert_eq!(click_duration(&HapticStrength::High, 100), CLICK_HIGH_US);
        // Master attenuates the click like all haptics, but never to silence.
        assert_eq!(click_duration(&HapticStrength::High, 50), CLICK_HIGH_US / 2);
        assert_eq!(click_duration(&HapticStrength::Low, 0), 1);
    }

    #[test]
    fn train_maps_full_drive_to_the_max_duty_band() {
        let duty_frac = |t: HidRumble| {
            let period = t.duration as f32 + t.interval as f32;
            t.duration as f32 / period
        };
        // Full drive → RUMBLE_MAX_DUTY (25%), half → half of that, linearly.
        assert!((duty_frac(train(u16::MAX, 80)) - RUMBLE_MAX_DUTY).abs() < 0.01);
        assert!((duty_frac(train(u16::MAX / 2, 80)) - RUMBLE_MAX_DUTY / 2.0).abs() < 0.01);
        // A fraction-of-a-percent effective drive still yields a distinct, small pulse (µs
        // resolution) rather than being clamped up or to silence.
        let tiny = train(327, 80); // ~0.5% strength
        assert!(tiny.duration >= 1 && (tiny.duration as f32) < train(u16::MAX / 10, 80).duration as f32);
    }

    #[test]
    fn program_for_falls_back_to_active_when_unset() {
        let main = prog("main");
        let none: Option<Program> = None;
        assert_eq!(program_for(&Role::Fallback, &main, &none).meta.name, "main");
        let fb = Some(prog("fallback"));
        assert_eq!(program_for(&Role::Fallback, &main, &fb).meta.name, "fallback");
        assert_eq!(program_for(&Role::Main, &main, &fb).meta.name, "main");
    }
}
