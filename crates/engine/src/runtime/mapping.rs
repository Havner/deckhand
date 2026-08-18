//! The central mapping loop (PLAN §4.2 S9): owns the `Sink` + `Mapper` + programs + globals, maps
//! one tick per device frame, evaluates global chords first, and polls the pad's rumble back to the
//! reader. It alternates a **connected phase** with a **waiting phase** (transport gone → release
//! outputs, keep the pad plugged, await reattach/stop), swapping the frame/rumble/click channels on
//! reattach (D6) so the same `Sink` serves across an outage.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvError, select};

use config::{GlobalConfig, RumbleSettings};
use steam_hid::Report;
use virt_out::{OutputEvent, Rumble, Sink};

use crate::chords::{Chords, ExecReq};
use crate::event::{EngineEvent, EventSink};
use crate::handle::Status;
use crate::logical::LogicalFrame;
use crate::program::{Program, Role, empty_program};
use crate::{HapticReq, Mapper, Result, Tick};

use super::link::LinkServer;
use super::{Click, Control, RumbleCmd};

/// The central mapping loop. Owns the `Sink` + `Mapper` + programs + globals; alternates a connected
/// phase (map frames, poll rumble) with a waiting phase (transport gone → release + keep pad plugged
/// until reattach/stop). Frame/rumble/click channels are swapped on reattach.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_mapper(
    mut sink: Sink,
    mut main: Option<Program>,
    mut fallback: Option<Program>,
    mut globals: GlobalConfig,
    mut link: LinkServer,
    running: Arc<AtomicBool>,
    fallback_active: Arc<AtomicBool>,
    events: EventSink,
) -> Result<()> {
    let start = Instant::now();
    // The engine always boots into Main; chords flip the role from there.
    let mut role = Role::Main;
    // Publish the initial live role unconditionally so a subscriber can seed purely from the event;
    // subsequent emits are on the chord switch only.
    set_active(&fallback_active, &events, role.clone());
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
                            set_active(&fallback_active, &events, role.clone());
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
                        // Command-haptic clicks this tick → the reader, which maps the strength
                        // level to the device (Gordon pulse duration / Deck gain).
                        for h in haptics.drain(..) {
                            let _ = link.click_tx().send(Click { side: h.side, strength: h.strength });
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
                    Ok(Report::Connected | Report::Battery(_)) => {}
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
            // Network: the mapper's `frame_rx` stays connected across a client outage (it's fed by the
            // persistent bridge threads), so detect the outage via the flag rather than a disconnect.
            if link.is_detached() {
                lost = true;
                break; // → waiting phase
            }

            // Rumble back-channel (game → virtual pad → real controller): scale by the *main*
            // profile's strength/curve, carrying its pulse Hz. The global master % is applied
            // reader-side (device-local, see `DeviceCfg::master_rumble`). On change.
            let prog = program_for(&role, &main, &fallback);
            let cmd = rumble_cmd(sink.poll_rumble()?, &prog.rumble);
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
            // Reattached → back to `Running` and resume the connected phase (local: on the reacquired
            // device with channels swapped; network: on the reconnected client). The mapper owns this
            // `State(Running)` edge so it covers both transports (the reader only emits the
            // device-specific `BindingAcquired`).
            WaitOutcome::Reattached => {
                events.emit(EngineEvent::State(Status::Running));
                continue 'session;
            }
        }
    }
}

/// Handle one control message (shared by the connected and waiting phases). Returns true to stop.
fn apply_control(
    msg: std::result::Result<Control, RecvError>,
    main: &mut Option<Program>,
    fallback: &mut Option<Program>,
    globals: &mut GlobalConfig,
    mapper: &mut Mapper,
    role: &Role,
    chords: &mut Chords,
) -> bool {
    match msg {
        Ok(Control::Apply { program, role: target }) => {
            let program = program.map(|p| *p); // `None` clears the role
            match target {
                Role::Main => *main = program,
                Role::Fallback => *fallback = program,
            }
            // Re-seed the mapper only if the applied role is the one currently live — on a clear this
            // resolves through `program_for` to the other role (e.g. main→fallback).
            if *role == target {
                mapper.switch_program(program_for(role, main, fallback));
            }
            false
        }
        Ok(Control::SetGlobals(g)) => {
            *globals = *g;
            // Preserve the current role base across the swap.
            *chords = Chords::new(&globals.chords, chords.fallback_base());
            // REVISIT (globals): a live SetGlobals only takes effect for chords here.
            // `master_rumble`/`led_brightness`/`idle_timeout` live in the reader's `DeviceCfg` (built
            // once at start, re-applied on each `Connected`), so changing them via SetGlobals does
            // NOT push to the hardware until the next start/reconnect. To make LED/idle live too, the new
            // values must reach the reader (e.g. thread them through the link so the reader re-runs
            // its apply). The UI can't work around this itself — it only sends SetGlobals. Part of a
            // broader globals rework (see the UI Globals screen).
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
    main: &mut Option<Program>,
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

    // Owned clone so the `select!` doesn't borrow `link` — `poll_reattach` below needs `&mut link`.
    let control = link.control_rx().clone();
    while running.load(Ordering::Relaxed) {
        select! {
            recv(control) -> msg => {
                if apply_control(msg, main, fallback, globals, mapper, role, chords) {
                    return Ok(WaitOutcome::Stopped);
                }
            }
            default(Duration::from_millis(50)) => {}
        }
        // Reattached? (local: the reader minted a fresh session; network: a client reconnected.)
        if link.poll_reattach() {
            return Ok(WaitOutcome::Reattached);
        }
        // Discard rumble (no controller to feed) — but drain it so the game's FF thread isn't stuck.
        let _ = sink.poll_rumble();
    }
    Ok(WaitOutcome::Stopped)
}

/// Compute the effective per-pad drive from a raw game rumble and the main profile's rumble settings
/// (strength % + response curve). The global master % and the pulse frequency are NOT applied here —
/// the reader does that (device-local; see `DeviceCfg::master_rumble` / `DeviceCfg::rumble_hz`).
fn rumble_cmd(raw: Rumble, s: &RumbleSettings) -> RumbleCmd {
    // Per-profile `strength` MAY exceed 100 to *boost* a game that under-drives its FF — many cap
    // well below full range (observed: 25%), so at `MAX_DUTY` they'd never reach the actuator's
    // saturation. The boost normalizes such a game back up; the drive still clamps at `u16::MAX`
    // (→ RUMBLE_MAX_DUTY), so it can't overshoot.
    let scale = s.strength as f32 / 100.0;
    let drive = |v: u16| {
        let full = (v as f32 / u16::MAX as f32) * scale;
        (s.curve.apply(full.clamp(0.0, 1.0)).clamp(0.0, 1.0) * u16::MAX as f32) as u16
    };
    RumbleCmd { strong: drive(raw.strong), weak: drive(raw.weak) }
}

/// The program driving a given role. Each role prefers its own slot, falls through to the other,
/// and only when **both** are unset uses the [`empty_program`] placeholder (which maps nothing) —
/// so the engine can run before any profile is applied (PLAN §6).
fn program_for<'a>(
    role: &Role,
    main: &'a Option<Program>,
    fallback: &'a Option<Program>,
) -> &'a Program {
    match role {
        Role::Main => main.as_ref().or(fallback.as_ref()).unwrap_or_else(|| empty_program()),
        Role::Fallback => fallback.as_ref().or(main.as_ref()).unwrap_or_else(|| empty_program()),
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

/// Publish the live role to the shared flag (store-before-emit) and emit [`EngineEvent::ActiveRole`].
/// Called with the initial role at loop start (always emitted) and on each chord switch (which only
/// fires on a real change), so no dedup is needed here — every call is a genuine set.
fn set_active(flag: &AtomicBool, events: &EventSink, role: Role) {
    flag.store(role == Role::Fallback, Ordering::SeqCst);
    events.emit(EngineEvent::ActiveRole(role));
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
    fn rumble_cmd_applies_strength() {
        let full = Rumble { strong: u16::MAX, weak: u16::MAX / 2 };

        // strength 100% (default) passes through unchanged. Master and frequency are NOT applied
        // here — the reader does that (see `reader::scale_master` / `DeviceCfg::rumble_hz`).
        let s = RumbleSettings { strength: 100, curve: Curve::Linear };
        let cmd = rumble_cmd(full.clone(), &s);
        assert_eq!(cmd.strong, u16::MAX);
        assert!((cmd.weak as i32 - (u16::MAX / 2) as i32).abs() <= 1);

        // strength 50% halves.
        let s = RumbleSettings { strength: 50, curve: Curve::Linear };
        let cmd = rumble_cmd(full, &s);
        assert!((cmd.strong as i32 - (u16::MAX / 2) as i32).abs() <= 1);

        // strength > 100 boosts a game that under-drives its FF: a quarter-range input at 200%
        // reaches half drive, and the boost clamps at the packet max instead of overflowing.
        let boost = RumbleSettings { strength: 200, curve: Curve::Linear };
        let quarter = Rumble { strong: u16::MAX / 4, weak: 0 };
        let cmd = rumble_cmd(quarter, &boost);
        assert!((cmd.strong as i32 - (u16::MAX / 2) as i32).abs() <= 2);
        let cmd = rumble_cmd(Rumble { strong: u16::MAX, weak: 0 }, &boost);
        assert_eq!(cmd.strong, u16::MAX);
    }

    #[test]
    fn program_for_resolves_and_falls_back_to_empty() {
        let main = Some(prog("main"));
        let fb = Some(prog("fallback"));
        let none: Option<Program> = None;
        // Both present → each role uses its own slot.
        assert_eq!(program_for(&Role::Main, &main, &fb).meta.name, "main");
        assert_eq!(program_for(&Role::Fallback, &main, &fb).meta.name, "fallback");
        // One present → both roles fall through to it (symmetric).
        assert_eq!(program_for(&Role::Fallback, &main, &none).meta.name, "main");
        assert_eq!(program_for(&Role::Main, &none, &fb).meta.name, "fallback");
        // Neither present → the empty placeholder (maps nothing).
        assert_eq!(program_for(&Role::Main, &none, &none).meta.name, "EMPTY_PROFILE");
        assert_eq!(program_for(&Role::Fallback, &none, &none).meta.name, "EMPTY_PROFILE");
    }
}
