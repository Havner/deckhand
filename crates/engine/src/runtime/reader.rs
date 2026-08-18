//! The reader thread (PLAN §4.2 S9, D6): a persistent **device-session loop** that owns the current
//! `Device`, is its **only writer** (read loop + keep-alive + config-on-`Connected` + rumble/click),
//! and survives a transport outage by **reacquiring** the same pinned device — [`LinkClient::detach`]
//! then [`LinkClient::reattach`] re-mint the session behind the link, so the virtual pad never leaves.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use config::{HapticStrength, Side};
use steam_hid::{Device, DeviceId, DeviceKind, HapticPulse, HapticStyle, Manager, Motor, Report};

use crate::Result;
use crate::event::{EngineEvent, EventSink};
use crate::handle::Status;

use super::link::LinkClient;
use super::{Click, DeviceCfg, RumbleCmd};

/// How often the reader re-enumerates while waiting for the pinned device to return (D6).
const REACQUIRE_POLL_MS: u64 = 1000;

/// Why a device read-session ended.
enum SessionEnd {
    /// `running` was cleared or the mapper is gone → the reader should exit.
    Stop,
    /// The device's transport went away (unplug / dongle removed) → reacquire.
    TransportGone,
}

/// The reader thread. Reads until the session ends; on transport-gone it [`LinkClient::detach`]es
/// (which flags `WaitingForDevice` and disconnects the mapper's frames) + emits
/// `State(WaitingForDevice)`, waits for the pinned `DeviceId` to reappear (its own `Manager`), reopens
/// it, and [`LinkClient::reattach`]es (mint a fresh session behind the link) so the mapper resumes on
/// the same virtual pad. The device stays pinned across the outage, so no binding event fires — the
/// run-state edges (which cover the client/server-split case where reader and mapper live on
/// different machines) are the only signal.
pub(super) fn run_reader(
    mut device: Device,
    pinned_id: DeviceId,
    cfg: DeviceCfg,
    mut link: LinkClient,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    events: EventSink,
) -> Result<()> {
    // A separate `Manager` (own hidapi context) used only for reacquire enumeration/open — created
    // lazily so a session that never disconnects pays nothing.
    let mut manager: Option<Manager> = None;

    loop {
        match read_session(&mut device, &cfg, &link, &running, &connected, &events)? {
            SessionEnd::Stop => return Ok(()),
            SessionEnd::TransportGone => {}
        }

        // Transport gone → the controller is no longer present (all transports; on wired/BT this is
        // the only disconnect signal). Publish it before the state edge below.
        set_connected(&connected, &events, false);
        // `detach` flags it and drops the session's device-side ends so the mapper's `frame_rx`
        // disconnects and it enters the waiting phase (release_all + WaitingForDevice).
        link.detach();
        events.emit(EngineEvent::State(Status::WaitingForDevice));
        drop(device);

        let mgr = manager.get_or_insert_with(|| Manager::new().expect("hidapi context for reacquire"));
        let Some(new_device) = reacquire(mgr, &pinned_id, &running) else {
            return Ok(()); // stopped while waiting
        };

        // Reacquired: emit `State(Running)` (the device stays pinned, so no `BindingAcquired`), then
        // `reattach` mints a fresh session and hands the mapper its ends; the mapper swaps, emits its
        // own `State(Running)`, and resumes the connected phase (the pad never left). The duplicate
        // edge is idempotent locally and is the sole run-state signal on the reader's machine when
        // reader and mapper are split across the network.
        events.emit(EngineEvent::State(Status::Running));
        if !link.reattach() {
            return Ok(()); // mapper gone
        }
        device = new_device;
    }
}

/// Read one device session: forward frames, surface connect/disconnect/battery events, keep the
/// controller alive, and write rumble/click. Returns when the session ends (stop or transport-gone).
fn read_session(
    device: &mut Device,
    cfg: &DeviceCfg,
    link: &LinkClient,
    running: &AtomicBool,
    connected: &AtomicBool,
    events: &EventSink,
) -> Result<SessionEnd> {
    apply_device_cfg(device, cfg);
    // A live session means the controller is present — publish it. This is the ONLY "connected"
    // signal on wired/BT (the controller *is* the transport). It's also needed on the dongle: the
    // receiver sends a `Connected` report on the *first* open but NOT on a re-open of an
    // already-on controller, so presuming here is what makes a second `start()` report connected.
    // The cost is a brief `true`→`false` when a dongle slot is opened with the pad turned off (the
    // receiver then reports `Disconnected`) — accepted: a correct settled state beats no state.
    set_connected(connected, events, true);
    // Haptic strategy is per-device: the Deck (Neptune) has real motors driven by `0xeb`
    // (`rumble_cmd`, re-issued periodically — see below); Gordon has only trackpad actuators,
    // driven as a re-fired pulse train (`0x8f`, `haptic_pulse`).
    let kind = device.info().kind.clone();
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
                        set_connected(connected, events, true);
                        apply_device_cfg(device, cfg);
                    }
                    Report::Disconnected => set_connected(connected, events, false),
                    // Edge-triggered: the 0x04 report streams ~1 Hz, so only surface a change.
                    Report::Battery(b) if last_battery != Some(b.charge_percent) => {
                        events.emit(EngineEvent::BatteryChanged { percent: b.charge_percent });
                        last_battery = Some(b.charge_percent);
                    }
                    // State (per-frame, too noisy) and an unchanged Battery need no event.
                    Report::State(_) | Report::Battery(_) => {}
                }
                if link.frame_tx().send(report).is_err() {
                    return Ok(SessionEnd::Stop); // mapper gone
                }
            }
            Ok(None) => {} // timeout — no frame this cycle
            Err(e) => {
                log::warn!("controller read error ({e}) — waiting for device");
                return Ok(SessionEnd::TransportGone);
            }
        }

        if cfg.keepalive && last_keepalive.elapsed() >= Duration::from_secs(2) {
            let _ = device.set_lizard_mode(false);
            last_keepalive = Instant::now();
        }

        // Latest rumble level wins. Apply the global master attenuator here (device-local): the
        // mapper sends game+profile-scaled rumble, master scales every amplitude before the hardware.
        let mut changed = false;
        while let Ok(r) = link.rumble_rx().try_recv() {
            level = scale_master(r, cfg.master_rumble);
            changed = true;
        }
        // Non-fatal on write error: a transient hiccup must not kill the reader (a real disconnect
        // is caught by the read above → TransportGone). Same for clicks below.
        match &kind {
            DeviceKind::Neptune => {
                // Deck: each `0xeb` command is a **fixed short burst** (the packet has no length
                // field), so a sustained rumble must be **re-issued** every ~`NEPTUNE_REFIRE_MS` —
                // send on change and on the re-fire tick while non-zero; the change to `(0,0)` stops
                // it. `strong`→left motor, `weak`→right (kernel FF mapping, PLAN §1.9).
                let refire = (level.strong > 0 || level.weak > 0)
                    && last_haptic.elapsed() >= Duration::from_millis(NEPTUNE_REFIRE_MS);
                if changed || refire {
                    if let Err(e) =
                        // intensity 0 = strongest (finer amplitude lever, unused for now — §1.9).
                        device.rumble_cmd(0, level.strong, level.weak, NEPTUNE_L_GAIN, NEPTUNE_R_GAIN)
                    {
                        log::warn!("rumble write failed: {e}");
                    }
                    last_haptic = Instant::now();
                }
            }
            DeviceKind::Gordon => {
                // Gordon: re-issue the pulse train just before it ends so a sustained rumble is one
                // contiguous drive (the actuator rings up), not a mid-train restart. Zero level →
                // nothing (the train plays out and stops).
                if (level.strong > 0 || level.weak > 0)
                    && last_haptic.elapsed() >= Duration::from_millis(RUMBLE_REFIRE_MS)
                {
                    if let Err(e) = apply_haptics(device, &level) {
                        log::warn!("rumble write failed: {e}");
                    }
                    last_haptic = Instant::now();
                }
            }
        }

        // One-shot command-haptic clicks fire immediately (no arbitration — a click may briefly
        // interrupt the rumble train on its pad, which the re-fire above resumes).
        while let Ok(click) = link.click_rx().try_recv() {
            if let Err(e) = fire_click(device, &click, &kind) {
                log::warn!("click write failed: {e}");
            }
        }
    }
    Ok(SessionEnd::Stop)
}

/// Publish the controller-present state to the shared flag and emit
/// [`EngineEvent::ControllerConnected`]. Store-before-emit so a concurrent `status()` never reads
/// staler than the last event. **Always emits** — no change-detection: the caller sites are already
/// genuine transitions (session start / a `Connected`/`Disconnected` report / transport-gone), and a
/// rare duplicate (e.g. two `Connected` reports in a row) is a harmless absolute-valued repeat.
fn set_connected(flag: &AtomicBool, events: &EventSink, connected: bool) {
    flag.store(connected, Ordering::SeqCst);
    events.emit(EngineEvent::ControllerConnected(connected));
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

/// Apply lizard-off + gyro + optional LED/idle (on start and every `Connected`). **Non-fatal:** a
/// transient feature-write hiccup is logged and ignored — it must not tear down the reader thread; a
/// genuinely-gone device surfaces as a read error → reacquire.
fn apply_device_cfg(device: &mut Device, cfg: &DeviceCfg) {
    let result: Result<()> = (|| {
        device.set_lizard_mode(false)?;
        device.set_gyro(true)?; // every current device has an IMU; always on
        if let Some(b) = cfg.led_brightness {
            device.set_led_intensity(b)?;
        }
        if let Some(t) = cfg.idle_timeout {
            device.set_idle_timeout(t)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!("device cfg: {e}");
    }
}

/// Attenuate a rumble command's amplitudes by the global master percentage (`0..=100`); `hz` is
/// unchanged. Applied reader-side so `master_rumble` stays a device-local setting (PLAN §1.9): the
/// mapper already folded in the game FF and the profile strength/curve.
fn scale_master(cmd: RumbleCmd, master: u8) -> RumbleCmd {
    let scale = |v: u16| ((v as u32 * master.min(100) as u32) / 100) as u16;
    RumbleCmd { strong: scale(cmd.strong), weak: scale(cmd.weak), hz: cmd.hz }
}

/// Route a rumble command to Gordon's trackpad actuators as pulse-trains (strong→left, weak→right;
/// PLAN §1.9). Re-fired by the reader while the level stays non-zero. (Gordon only; the Deck uses
/// [`Device::rumble_cmd`] directly — see `read_session`.)
fn apply_haptics(device: &mut Device, cmd: &RumbleCmd) -> Result<()> {
    if cmd.strong > 0 {
        device.haptic_pulse(Motor::Left, train(cmd.strong, cmd.hz))?;
    }
    if cmd.weak > 0 {
        device.haptic_pulse(Motor::Right, train(cmd.weak, cmd.hz))?;
    }
    Ok(())
}

/// Deck motor gains (dB) for `rumble_cmd` — the kernel drives `FF_RUMBLE` with left = +2 dB,
/// right = 0 dB (the two motors aren't matched; PLAN §1.9). HW-tunable starting point.
const NEPTUNE_L_GAIN: i8 = 2;
const NEPTUNE_R_GAIN: i8 = 2;

/// How often the reader re-issues the Deck's `0xeb` rumble while non-zero. Each command is a fixed
/// short burst (no length field), so this must be **shorter than that burst** to sound continuous.
/// **Starting point — HW-tune with fftest** (the burst is ~0.3–0.5 s, so 0.5 s may be a hair long).
const NEPTUNE_REFIRE_MS: u64 = 500;

/// The duty cycle full drive maps to. Gordon's pad actuator saturates above ~25% duty
/// (HW-tested: the useful strength band is ~1–25%, above that feels identical), so `drive`
/// spans `0..RUMBLE_MAX_DUTY` of the period rather than `0..1`. Kept **linear** within the band
/// (close enough; the per-profile `curve` fine-tunes). **HW-specific** — likely differs on other
/// controllers / haptic packets, so revisit per-`Shape` if that turns out to matter.
const RUMBLE_MAX_DUTY: f32 = 0.25;

/// Pulse-train length (ms). The pad actuator **rings up** over many cycles, so a train must be
/// long enough to reach full amplitude — a too-short one feels weak regardless of duty. The reader
/// re-fires just *before* this elapses ([`RUMBLE_REFIRE_MS`]) so a sustained rumble is one
/// near-continuous drive, not a train restarted mid-swing (which never rings up). HW-tuned.
const RUMBLE_TRAIN_MS: u32 = 250;
/// How often the reader re-issues the train — a hair under [`RUMBLE_TRAIN_MS`] so trains are
/// contiguous (re-fire near the train's end, not mid-play) while never leaving a silent gap.
const RUMBLE_REFIRE_MS: u64 = 220;

/// A pulse train at the profile's `hz` whose duty cycle encodes the already-scaled `drive` (the
/// only amplitude lever — `gain` is ignored on Gordon). Maps full drive onto the actuator's
/// useful duty band ([`RUMBLE_MAX_DUTY`]) at µs resolution, so even a fraction-of-a-percent
/// effective strength produces a distinct (small) pulse.
fn train(drive: u16, hz: u16) -> HapticPulse {
    let hz = hz.clamp(16, 1000); // period must fit u16 (hz ≥ 16); Gordon's usable range
    let period = 1_000_000u32 / hz as u32; // µs
    let full = drive as f32 / u16::MAX as f32;
    // Full drive → MAX_DUTY of the period; keep the µs precision so tiny drives stay tiny. A
    // valid pulse still needs duration ≥ 1 and interval ≥ 1.
    let duty = (full * RUMBLE_MAX_DUTY * period as f32) as u32;
    let duty = duty.clamp(1, period - 1);
    let count = ((hz as u32 * RUMBLE_TRAIN_MS) / 1000).max(1) as u16;
    HapticPulse { duration: duty as u16, interval: (period - duty) as u16, count, gain: 0 }
}

/// Trailing off-phase of the single click pulse (irrelevant to the felt tick at `count=1`).
const CLICK_INTERVAL_US: u16 = 1000;

/// Fire one command-haptic click on its pad, mapping the `strength` level to the device: Gordon uses
/// a `0x8f` pulse whose **duration** encodes strength (gain inert → 0); the Deck uses a `0xea`
/// `haptic_cmd` (`Strong` style) whose **gain** encodes strength, **per side** (the two motors
/// differ). `Side::Left`→left actuator, `Side::Right`→right.
fn fire_click(device: &mut Device, click: &Click, kind: &DeviceKind) -> Result<()> {
    let motor = match click.side {
        Side::Left => Motor::Left,
        Side::Right => Motor::Right,
    };
    match kind {
        DeviceKind::Neptune => {
            let gain = neptune_click_gain(&click.side, &click.strength);
            device.haptic_cmd(motor, HapticStyle::Strong, gain)?;
        }
        DeviceKind::Gordon => {
            let duration = gordon_click_duration(&click.strength);
            device.haptic_pulse(motor, HapticPulse { duration, interval: CLICK_INTERVAL_US, count: 1, gain: 0 })?;
        }
    }
    Ok(())
}

/// Gordon `0x8f` click pulse duration (µs) for a strength level — the duration is the strength lever
/// (gain is inert on Gordon). HW-tuned via the `haptic` example.
fn gordon_click_duration(strength: &HapticStrength) -> u16 {
    match strength {
        HapticStrength::Low => 500,
        HapticStrength::Medium => 1000,
        HapticStrength::High => 2000,
    }
}

/// Deck `0xea` click gain (dB) for a strength level, **per side** (HW-tuned — the two motors differ,
/// the right needing a couple dB more for a comparable feel).
fn neptune_click_gain(side: &Side, strength: &HapticStrength) -> i8 {
    match (side, strength) {
        (Side::Left, HapticStrength::Low) => -2,
        (Side::Left, HapticStrength::Medium) => 1,
        (Side::Left, HapticStrength::High) => 4,
        (Side::Right, HapticStrength::Low) => -2,
        (Side::Right, HapticStrength::Medium) => 2,
        (Side::Right, HapticStrength::High) => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn train_maps_full_drive_to_the_max_duty_band() {
        let duty_frac = |t: HapticPulse| {
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
    fn scale_master_attenuates_amplitudes_only() {
        let cmd = RumbleCmd { strong: u16::MAX, weak: 10_000, hz: 80 };
        // 100% is identity; hz always passes through.
        assert_eq!(scale_master(cmd.clone(), 100), cmd);
        // 50% halves both amplitudes, hz untouched.
        let half = scale_master(cmd.clone(), 50);
        assert!((half.strong as i32 - (u16::MAX / 2) as i32).abs() <= 1);
        assert_eq!((half.weak, half.hz), (5_000, 80));
        // 0% silences.
        assert_eq!(scale_master(cmd, 0), RumbleCmd { strong: 0, weak: 0, hz: 80 });
    }
}
