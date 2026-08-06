//! The reader thread (PLAN §4.2 S9, D6): a persistent **device-session loop** that owns the current
//! `Device`, is its **only writer** (read loop + keep-alive + config-on-`Connected` + rumble/click),
//! and survives a transport outage by **reacquiring** the same pinned device — [`LinkClient::detach`]
//! then [`LinkClient::reattach`] re-mint the session behind the link, so the virtual pad never leaves.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use config::Side;
use steam_hid::{Device, DeviceId, Manager, Motor, Report, Rumble as HidRumble};

use crate::Result;
use crate::event::{EngineEvent, EventSink};

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
/// (which flags `WaitingForDevice` and disconnects the mapper's frames) + emits `BindingLost`, waits
/// for the pinned `DeviceId` to reappear (its own `Manager`), reopens it, and [`LinkClient::reattach`]es
/// (mint a fresh session behind the link) so the mapper resumes on the same virtual pad.
pub(super) fn run_reader(
    mut device: Device,
    pinned_id: DeviceId,
    cfg: DeviceCfg,
    mut link: LinkClient,
    running: Arc<AtomicBool>,
    events: EventSink,
) -> Result<()> {
    // A separate `Manager` (own hidapi context) used only for reacquire enumeration/open — created
    // lazily so a session that never disconnects pays nothing.
    let mut manager: Option<Manager> = None;

    loop {
        match read_session(&mut device, &cfg, &link, &running, &events)? {
            SessionEnd::Stop => return Ok(()),
            SessionEnd::TransportGone => {}
        }

        // Transport gone. `detach` flags it and drops the session's device-side ends so the mapper's
        // `frame_rx` disconnects and it enters the waiting phase (release_all + WaitingForDevice).
        link.detach();
        events.emit(EngineEvent::BindingLost);
        drop(device);

        let mgr = manager.get_or_insert_with(|| Manager::new().expect("hidapi context for reacquire"));
        let Some(new_device) = reacquire(mgr, &pinned_id, &running) else {
            return Ok(()); // stopped while waiting
        };

        // Reacquired: emit the device-specific `BindingAcquired`, then `reattach` mints a fresh
        // session and hands the mapper its ends; the mapper swaps, emits `State(Running)`, and
        // resumes the connected phase (the pad never left).
        events.emit(EngineEvent::BindingAcquired(pinned_id.clone()));
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
    events: &EventSink,
) -> Result<SessionEnd> {
    apply_device_cfg(device, cfg);
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
                        events.emit(EngineEvent::ControllerConnected);
                        apply_device_cfg(device, cfg);
                    }
                    Report::Disconnected => events.emit(EngineEvent::ControllerDisconnected),
                    // Edge-triggered: the 0x04 report streams ~1 Hz, so only surface a change.
                    Report::Battery(b) if last_battery != Some(b.charge_percent) => {
                        events.emit(EngineEvent::BatteryChanged { percent: b.charge_percent });
                        last_battery = Some(b.charge_percent);
                    }
                    _ => {} // State (per-frame, too noisy); unchanged Battery.
                }
                if link.frame_tx().send(report).is_err() {
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
        while let Ok(r) = link.rumble_rx().try_recv() {
            level = r;
        }
        if (level.strong > 0 || level.weak > 0)
            && last_haptic.elapsed() >= Duration::from_millis(RUMBLE_REFIRE_MS)
        {
            // Non-fatal: a transient write hiccup must not kill the reader (a real disconnect is
            // caught by the read above → TransportGone). Same for clicks below.
            if let Err(e) = apply_haptics(device, &level) {
                log::warn!("rumble write failed: {e}");
            }
            last_haptic = Instant::now();
        }

        // One-shot command-haptic clicks fire immediately (no arbitration — a click may briefly
        // interrupt the rumble train on its pad, which the re-fire above resumes).
        while let Ok(click) = link.click_rx().try_recv() {
            if let Err(e) = fire_click(device, &click) {
                log::warn!("click write failed: {e}");
            }
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

/// Apply lizard-off + gyro + optional LED/idle (on start and every `Connected`). **Non-fatal:** a
/// transient feature-write hiccup is logged and ignored — it must not tear down the reader thread; a
/// genuinely-gone device surfaces as a read error → reacquire.
fn apply_device_cfg(device: &mut Device, cfg: &DeviceCfg) {
    let result: Result<()> = (|| {
        device.set_lizard_mode(false)?;
        device.set_gyro(cfg.gyro)?;
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

/// Trailing off-phase of the single click pulse (irrelevant to the felt tick at `count=1`).
const CLICK_INTERVAL_US: u16 = 1000;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
