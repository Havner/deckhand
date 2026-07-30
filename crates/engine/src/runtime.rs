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
//! ([`rumble_cmd`], [`program_for`]) are unit-tested below.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, unbounded};

use config::{Curve, GlobalConfig, HapticStrength, RumbleSettings, Side, StartProfile};
use steam_hid::{Device, DeviceKind, Motor, Report, Rumble as HidRumble};
use virt_out::{Rumble, Sink};

use crate::chords::{Chords, ExecReq};
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
        let (rumble_tx, rumble_rx) = unbounded::<RumbleCmd>();
        let (click_tx, click_rx) = unbounded::<Click>();
        let (control_tx, control_rx) = unbounded::<Control>();

        let r_reader = running.clone();
        let reader = thread::Builder::new()
            .name("deckhand-reader".into())
            .spawn(move || run_reader(device, cfg, frame_tx, rumble_rx, click_rx, r_reader))
            .expect("spawn reader thread");

        let r_mapper = running.clone();
        let mapper = thread::Builder::new()
            .name("deckhand-mapper".into())
            .spawn(move || {
                run_mapper(
                    sink, active, fallback, globals, frame_rx, control_rx, rumble_tx, click_tx,
                    r_mapper,
                )
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
    rumble_rx: Receiver<RumbleCmd>,
    click_rx: Receiver<Click>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    apply_device_cfg(&mut device, &cfg)?;
    let mut last_keepalive = Instant::now();
    let mut last_haptic = Instant::now();
    let mut level = RumbleCmd::default();

    while running.load(Ordering::Relaxed) {
        // Short timeout so the loop laps to check `running`, keep-alive, and rumble regularly.
        match device.poll(Duration::from_millis(4)) {
            Ok(Some(report)) => {
                match &report {
                    Report::Connected => {
                        log::info!("controller connected — re-applying device config");
                        apply_device_cfg(&mut device, &cfg)?;
                    }
                    Report::Disconnected => log::info!("controller disconnected (dongle alive)"),
                    _ => {} // State: streamed every frame, too noisy to log; Battery: surfaced later
                }
                if frame_tx.send(report).is_err() {
                    break; // mapper gone
                }
            }
            Ok(None) => {} // timeout — no frame this cycle
            Err(e) => {
                log::warn!("controller read error ({e}) — stopping reader (device → lizard)");
                break; // transport gone → end (device drops → lizard restored)
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
            apply_haptics(&mut device, &level)?;
            last_haptic = Instant::now();
        }

        // One-shot command-haptic clicks fire immediately (no arbitration — a click may briefly
        // interrupt the rumble train on its pad, which the re-fire above resumes).
        while let Ok(click) = click_rx.try_recv() {
            fire_click(&mut device, &click)?;
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
    rumble_tx: Sender<RumbleCmd>,
    click_tx: Sender<Click>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let start = Instant::now();
    // Boot into the role named by `start_profile` (read once here — it's a start-only setting).
    let mut role = start_role(&globals);
    let mut chords = Chords::new(&globals.chords, role == Role::Fallback);
    let mut mapper = Mapper::new(program_for(&role, &active, &fallback));
    let mut out: Vec<virt_out::OutputEvent> = Vec::new();
    let mut haptics: Vec<HapticReq> = Vec::new();
    let mut last_rumble = RumbleCmd::default();

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
                        let prog = program_for(&role, &active, &fallback);
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
                    mapper.tick(&masked, now, program_for(&role, &active, &fallback), &mut out, &mut haptics);
                    sink.emit(&out)?;
                    // Command-haptic pulses this tick → the reader (scaled by master rumble %).
                    for h in haptics.drain(..) {
                        let duration = click_duration(&h.strength, globals.master_rumble);
                        let _ = click_tx.send(Click { side: h.side, duration });
                    }
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
                    // Preserve the current role base across the swap — `start_profile` is
                    // start-only, so a live change to it must not retroactively yank the role.
                    chords = Chords::new(&globals.chords, chords.fallback_base());
                }
                Ok(Control::Stop) | Err(_) => stop = true,
            },
            // Insurance: devices stream ~250 Hz, but tick anyway so rumble is polled when idle.
            default(Duration::from_millis(8)) => {}
        }
        if stop {
            break;
        }

        // Rumble back-channel (game → virtual pad → real controller): scale by the global master %
        // and the *active* profile's strength/curve, carrying its pulse Hz. Send only on change.
        let prog = program_for(&role, &active, &fallback);
        let cmd = rumble_cmd(sink.poll_rumble()?, globals.master_rumble, &prog.rumble);
        if cmd != last_rumble {
            let _ = rumble_tx.send(cmd.clone());
            last_rumble = cmd;
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

/// The effective rumble to realize on the controller: per-pad drive (already scaled by master ×
/// profile strength × curve) plus the pulse frequency from the active profile.
#[derive(Debug, Clone, PartialEq, Default)]
struct RumbleCmd {
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
struct Click {
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

/// Compute the effective per-pad drive from a raw game rumble, the global master %, and the active
/// profile's rumble settings (strength % + response curve); `hz` passes through from the profile.
fn rumble_cmd(raw: Rumble, master: u8, s: &RumbleSettings) -> RumbleCmd {
    // `master` (0..=100) attenuates globally; per-profile `strength` MAY exceed 100 to *boost* a
    // game that under-drives its FF — many cap well below full range (observed: 25%), so at
    // `MAX_DUTY` they'd never reach the actuator's saturation. The boost normalizes such a game
    // back up; the drive still clamps at `u16::MAX` (→ RUMBLE_MAX_DUTY), so it can't overshoot.
    let scale = (master.min(100) as f32 / 100.0) * (s.strength as f32 / 100.0);
    let drive = |v: u16| {
        let full = (v as f32 / u16::MAX as f32) * scale;
        (apply_curve(full.clamp(0.0, 1.0), &s.curve).clamp(0.0, 1.0) * u16::MAX as f32) as u16
    };
    RumbleCmd { strong: drive(raw.strong), weak: drive(raw.weak), hz: s.hz }
}

fn apply_curve(v: f32, curve: &Curve) -> f32 {
    match curve {
        Curve::Linear => v,
        Curve::Power(e) => v.powf(*e),
    }
}

/// The program driving a given role (fallback falls back to active when unset).
fn program_for<'a>(role: &Role, active: &'a Program, fallback: &'a Option<Program>) -> &'a Program {
    match role {
        Role::Active => active,
        Role::Fallback => fallback.as_ref().unwrap_or(active),
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
        StartProfile::Active => Role::Active,
        StartProfile::Fallback => Role::Fallback,
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
        let active = prog("active");
        let none: Option<Program> = None;
        assert_eq!(program_for(&Role::Fallback, &active, &none).meta.name, "active");
        let fb = Some(prog("fallback"));
        assert_eq!(program_for(&Role::Fallback, &active, &fb).meta.name, "fallback");
        assert_eq!(program_for(&Role::Active, &active, &fb).meta.name, "active");
    }
}
