//! The central mapping loop (PLAN §4.2 S9): owns the `Sink` + `Mapper` + programs + globals, maps
//! one tick per device frame, evaluates global chords first, and polls the pad's rumble back to the
//! reader. It alternates a **connected phase** with a **waiting phase** (transport gone → release
//! outputs, keep the pad plugged, await reattach/stop), swapping the frame/rumble/click channels on
//! reattach (D6) so the same `Sink` serves across an outage.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvError, select};

use config::{GlobalConfig, HapticStrength, RumbleSettings, StartProfile};
use steam_hid::Report;
use virt_out::{OutputEvent, Rumble, Sink};

use crate::chords::{Chords, ExecReq};
use crate::event::{EngineEvent, EventSink};
use crate::handle::Status;
use crate::logical::LogicalFrame;
use crate::program::{Program, Role};
use crate::{HapticReq, Mapper, Result, Tick};

use super::link::LinkServer;
use super::{Click, Control, RumbleCmd};

/// The central mapping loop. Owns the `Sink` + `Mapper` + programs + globals; alternates a connected
/// phase (map frames, poll rumble) with a waiting phase (transport gone → release + keep pad plugged
/// until reattach/stop). Frame/rumble/click channels are swapped on reattach.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_mapper(
    mut sink: Sink,
    mut main: Program,
    mut fallback: Option<Program>,
    mut globals: GlobalConfig,
    mut link: LinkServer,
    running: Arc<AtomicBool>,
    events: EventSink,
) -> Result<()> {
    let start = Instant::now();
    // Boot into the role named by `start_profile` (read once here — it's a start-only setting).
    let mut role = start_role(&globals);
    let mut chords = Chords::new(&globals.chords, role == Role::Fallback);
    let mut mapper = Mapper::new(program_for(&role, &main, &fallback));
    let mut out: Vec<OutputEvent> = Vec::new();
    let mut haptics: Vec<HapticReq> = Vec::new();
    let mut last_rumble = RumbleCmd::default();

    'session: loop {
        // ---- connected phase ----
        let mut lost = false;
        while running.load(Ordering::Relaxed) {
            let mut stop = false;
            select! {
                recv(link.frame_rx()) -> msg => match msg {
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
                            let _ = link.click_tx().send(Click { side: h.side, duration });
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
                    // Frames gone: transport-lost (link detached) → waiting phase; else stop.
                    Err(_) => {
                        if link.is_detached() {
                            lost = true;
                        } else {
                            stop = true;
                        }
                    }
                },
                recv(link.control_rx()) -> msg => {
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
                let _ = link.rumble_tx().send(cmd.clone());
                last_rumble = cmd;
            }
        }
        if !lost {
            return Ok(()); // clean stop (`running` cleared) mid-connected.
        }

        // ---- waiting phase: release outputs, keep the pad plugged, await reattach / stop ----
        match run_waiting(
            &mut sink, &mut main, &mut fallback, &mut globals, &mut mapper, &role, &mut chords,
            &mut link, &running, &events,
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
    link: &mut LinkServer,
    running: &AtomicBool,
    events: &EventSink,
) -> Result<WaitOutcome> {
    let mut out: Vec<OutputEvent> = Vec::new();
    mapper.release_all(&mut out);
    sink.emit(&out)?;
    events.emit(EngineEvent::State(Status::WaitingForDevice));

    // Owned clones so the `select!` doesn't borrow `link` — the reattach arm needs `&mut link`.
    let control = link.control_rx().clone();
    let reattach = link.reattach_rx().clone();
    while running.load(Ordering::Relaxed) {
        select! {
            recv(control) -> msg => {
                if apply_control(msg, main, fallback, globals, mapper, role, chords) {
                    return Ok(WaitOutcome::Stopped);
                }
            }
            // The reader reacquired the device and minted a fresh session → swap it in and resume.
            recv(reattach) -> msg => if let Ok(session) = msg {
                link.reattach(session);
                return Ok(WaitOutcome::Reattached);
            },
            default(Duration::from_millis(50)) => {}
        }
        // Discard rumble (no controller to feed) — but drain it so the game's FF thread isn't stuck.
        let _ = sink.poll_rumble();
    }
    Ok(WaitOutcome::Stopped)
}

/// Command-haptic click durations (µs) for Low/Med/High — a single pulse, HW-tuned via the `haptic`
/// example (`interval`/`count` are fixed; the duration is the strength). Scaled by master rumble %.
const CLICK_LOW_US: u16 = 500;
const CLICK_MED_US: u16 = 1000;
const CLICK_HIGH_US: u16 = 2000;

/// The click pulse duration for `strength`, attenuated by the global `master` % (like all haptics).
fn click_duration(strength: &HapticStrength, master: u8) -> u16 {
    let base = match strength {
        HapticStrength::Low => CLICK_LOW_US,
        HapticStrength::Medium => CLICK_MED_US,
        HapticStrength::High => CLICK_HIGH_US,
    };
    ((base as u32 * master.min(100) as u32) / 100).max(1) as u16
}

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
    use crate::program::{CompiledSet, ProgramMeta, SetId, SourceMap};
    use config::Curve;

    fn prog(name: &str) -> Program {
        Program {
            meta: ProgramMeta { name: name.into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
            sets: vec![CompiledSet { name: "s".into(), base: SourceMap::new(), layers: vec![] }],
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
    fn program_for_falls_back_to_active_when_unset() {
        let main = prog("main");
        let none: Option<Program> = None;
        assert_eq!(program_for(&Role::Fallback, &main, &none).meta.name, "main");
        let fb = Some(prog("fallback"));
        assert_eq!(program_for(&Role::Fallback, &main, &fb).meta.name, "fallback");
        assert_eq!(program_for(&Role::Main, &main, &fb).meta.name, "main");
    }
}
