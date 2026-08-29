//! The reader thread (PLAN §4.2 S9, D6): a persistent **device-session loop** that owns the current
//! `Device`, is its **only writer** (read loop + keep-alive + config-on-`Connected` + rumble/click),
//! and survives a transport outage by **reacquiring** the same pinned device — [`LinkClient::detach`]
//! then [`LinkClient::reattach`] re-mint the session behind the link, so the virtual pad never leaves.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use config::{DeviceConfig, GordonTuning, HapticStrength, RumbleTuning, Side};
use crossbeam_channel::Receiver;
use steam_hid::{
    Device, DeviceId, DeviceKind, HapticIntensity, HapticPulse, HapticStyle, Manager, Motor, Report,
    Transport,
};

use crate::Result;
use crate::event::{EngineEvent, EventSink};
use crate::handle::Status;

use super::link::LinkClient;
use super::{Click, RumbleCmd};

/// How often the reader re-enumerates while waiting for the pinned device to return (D6).
const REACQUIRE_POLL_MS: u64 = 1000;

/// Device-level settings the reader applies on start, on every `Connected` (the controller resets
/// its config when it re-joins a dongle; PLAN §1.9), and whenever a live [`DeviceConfig`] update
/// arrives. Derived from the bound device's [`DeviceConfig`] with each device-specific setting
/// masked to `None`/`false` where the hardware can't honor it — so the apply path is blind.
/// Reader-internal: it's rebuilt in-place from the device on every change, so it never crosses a
/// thread boundary or the §6 wire (device settings are machine-local).
struct ReaderCfg {
    /// LED intensity `0..=100 %`, or the device default. `None` where there's no settable LED
    /// ([`DeviceKind::has_led_intensity`]).
    led_brightness: Option<u8>,
    /// Sleep/idle timeout in seconds, or the device default. `None` where idle is meaningless for
    /// the transport ([`Transport::has_idle`]).
    idle_timeout: Option<u16>,
    /// Rumble shaping for the **bound** device — exactly the one kind's tuning, so the reader never
    /// carries a config for a device that isn't attached.
    rumble: ReaderRumble,
    /// Periodically re-assert lizard-off ([`DeviceKind::needs_keepalive`]).
    keepalive: bool,
}

/// The bound device's rumble configuration — the shaping plus which command path drives it. One
/// variant per device kind so there are no inert fields (Gordon has no gain, the motor devices have
/// no pulse frequency), and the emit path dispatches by matching this rather than the device kind.
enum ReaderRumble {
    /// Gordon `0x8f` pulse-train: a duty lever + pulse frequency.
    Gordon(GordonTuning),
    /// Steam Deck `0xeb` dual-motor: speed + gain levers.
    Neptune(RumbleTuning),
    /// Triton `0x80` dual-motor: speed + gain levers (its own motors).
    Triton(RumbleTuning),
}

impl ReaderCfg {
    /// Resolve the reader config for a device from the (machine-local) [`DeviceConfig`], dropping
    /// each device-specific setting to `None`/`false` where the hardware can't honor it, and picking
    /// the bound kind's rumble config.
    fn for_device(kind: &DeviceKind, transport: &Transport, device_config: &DeviceConfig) -> Self {
        ReaderCfg {
            led_brightness: kind.has_led_intensity().then_some(device_config.led_brightness).flatten(),
            idle_timeout: transport.has_idle().then_some(device_config.idle_timeout).flatten(),
            rumble: match kind {
                DeviceKind::Gordon => ReaderRumble::Gordon(device_config.gordon),
                DeviceKind::Neptune => ReaderRumble::Neptune(device_config.neptune),
                DeviceKind::Triton => ReaderRumble::Triton(device_config.triton),
            },
            keepalive: kind.needs_keepalive(),
        }
    }
}

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
// A thread entry taking the reader's devices, link, and readback handles; grouping them into a
// struct would only add indirection.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_reader(
    mut device: Device,
    pinned_id: DeviceId,
    mut device_config: DeviceConfig,
    device_config_rx: Receiver<DeviceConfig>,
    mut link: LinkClient,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    battery: Arc<AtomicU16>,
    events: EventSink,
) -> Result<()> {
    // A separate `Manager` (own hidapi context) used only for reacquire enumeration/open — created
    // lazily so a session that never disconnects pays nothing.
    let mut manager: Option<Manager> = None;

    loop {
        // `device_config` is owned here so live updates persist across reacquire; `read_session`
        // rebuilds its `ReaderCfg` from the device each session (kind/transport are stable across
        // the outage — the pinned id doesn't change).
        match read_session(
            &mut device, &mut device_config, &device_config_rx, &link, &running, &connected, &battery,
            &events,
        )? {
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

/// Read one device session: forward frames, surface connect/disconnect/battery events, apply live
/// device-config updates, keep the controller alive, and write rumble/click. Returns when the
/// session ends (stop or transport-gone).
#[allow(clippy::too_many_arguments)]
fn read_session(
    device: &mut Device,
    device_config: &mut DeviceConfig,
    device_config_rx: &Receiver<DeviceConfig>,
    link: &LinkClient,
    running: &AtomicBool,
    connected: &AtomicBool,
    battery: &AtomicU16,
    events: &EventSink,
) -> Result<SessionEnd> {
    // Drain any config that arrived while reacquiring, so this session starts on the latest.
    while let Ok(c) = device_config_rx.try_recv() {
        *device_config = c;
    }
    let mut cfg = ReaderCfg::for_device(&device.info().kind, &device.info().transport, device_config);
    apply_device_cfg(device, &cfg);
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

    while running.load(Ordering::Relaxed) {
        // Short timeout so the loop laps to check `running`, keep-alive, and rumble regularly.
        match device.poll(Duration::from_millis(4)) {
            Ok(Some(report)) => {
                match &report {
                    Report::Connected => {
                        set_connected(connected, events, true);
                        apply_device_cfg(device, &cfg);
                    }
                    Report::Disconnected => set_connected(connected, events, false),
                    // Edge-triggered: the 0x04 report streams ~1 Hz, so only surface a change. The
                    // readback atomic is the last-value source (persists across sessions), so a
                    // reconnect at the same charge stays quiet — `status()` already reads it directly.
                    Report::Battery(b) if battery.load(Ordering::Relaxed) != b.charge_percent as u16 => {
                        set_battery(battery, events, b.charge_percent);
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

        // Live device-config updates (latest wins, machine-local — never via the link/§6 wire).
        // Rebuild `cfg` and re-apply LED/idle immediately so they don't wait for the next
        // `Connected`; the rumble levers are read from `cfg` at emit below, so they pick up too.
        let mut cfg_changed = false;
        while let Ok(c) = device_config_rx.try_recv() {
            *device_config = c;
            cfg_changed = true;
        }
        if cfg_changed {
            cfg = ReaderCfg::for_device(&device.info().kind, &device.info().transport, device_config);
            apply_device_cfg(device, &cfg);
        }

        if cfg.keepalive && last_keepalive.elapsed() >= Duration::from_secs(3) {
            let _ = device.set_lizard_mode(false);
            last_keepalive = Instant::now();
        }

        // Latest rumble level wins. The mapper already folded in the game FF and the profile
        // strength/curve; the per-device levers (in `apply_*`) do the rest at emit, so nothing is
        // scaled here — a live lever change lands via the `cfg` rebuild above.
        let mut changed = false;
        while let Ok(r) = link.rumble_rx().try_recv() {
            level = r;
            changed = true;
        }

        // Emit the rumble for the bound device. Each variant carries its own tuning + re-fire cadence
        // (the Deck's fixed `0xeb` burst vs Gordon's pulse train). Non-fatal on write error: a
        // transient hiccup must not kill the reader (a real disconnect is caught by the read above →
        // TransportGone). Same for clicks below.
        // TODO: consider moving the reader-side rumble constants (refire intervals, train/duty knobs)
        // down into `steam-hid` next to the packets they parameterize — hardware facts, not policy.
        let idle = level.strong == 0 && level.weak == 0;
        // Re-fire is due once the cadence has elapsed and the level is non-zero (a zero level never
        // re-fires — the last command plays out and stops). `since` is passed in so the closure
        // doesn't borrow `last_haptic`, which the arms re-assign after firing.
        let due = |ms: u64, since: Instant| !idle && since.elapsed() >= Duration::from_millis(ms);
        let fire = |r: Result<()>, last: &mut Instant| {
            if let Err(e) = r {
                log::warn!("rumble write failed: {e}");
            }
            *last = Instant::now();
        };
        match &cfg.rumble {
            // Gordon: re-issue the pulse train just before it ends so a sustained rumble is one
            // contiguous drive (the actuator rings up), not a mid-train restart — so it fires on the
            // re-fire tick only, never on `changed`.
            ReaderRumble::Gordon(t) => {
                if due(RUMBLE_REFIRE_MS, last_haptic) {
                    fire(apply_gordon(device, &level, t), &mut last_haptic);
                }
            }
            // Deck / Triton: each dual-motor command is a fixed short burst (no length field), so a
            // sustained rumble must be re-issued while non-zero — send on change and on the re-fire
            // tick; the change to `(0,0)` stops it. `strong`→left motor, `weak`→right (PLAN §1.9).
            ReaderRumble::Neptune(t) => {
                if changed || due(NEPTUNE_REFIRE_MS, last_haptic) {
                    fire(apply_rumble(device, &level, t), &mut last_haptic);
                }
            }
            ReaderRumble::Triton(t) => {
                if changed || due(TRITON_REFIRE_MS, last_haptic) {
                    fire(apply_rumble_triton(device, &level, t), &mut last_haptic);
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

/// Publish the battery charge to the shared readback and emit [`EngineEvent::BatteryChanged`].
/// Store-before-emit so a concurrent `status()` never reads staler than the last event. The caller
/// change-guards against the readback, so this only runs on a genuine change.
fn set_battery(flag: &AtomicU16, events: &EventSink, percent: u8) {
    flag.store(percent as u16, Ordering::SeqCst);
    events.emit(EngineEvent::BatteryChanged { percent });
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
fn apply_device_cfg(device: &mut Device, cfg: &ReaderCfg) {
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

/// How often the reader re-issues the Deck's `0xeb` rumble while non-zero. Each command is a fixed
/// short burst (no length field), so this must be **shorter than that burst** to sound continuous.
/// **Starting point — HW-tune with fftest** (the burst is ~0.3–0.5 s, so 0.5 s may be a hair long).
const NEPTUNE_REFIRE_MS: u64 = 500;

/// How often the reader re-issues Triton's `0x80` rumble while non-zero. The firmware sustains each
/// command well past this (HW: no change 40–500 ms; it only starts to gap above ~500 ms), so a
/// relaxed cadence keeps it continuous with far less bus traffic than the Deck needs.
const TRITON_REFIRE_MS: u64 = 400;

/// Route a rumble command to Gordon's trackpad actuators as pulse-trains (strong→left, weak→right;
/// PLAN §1.9). Each motor's strength drives the pulse **duty** via the [`GordonTuning`] `duty` lever
/// (constant, or scaled into a band), at the tuning's pulse `hz`. Re-fired by the reader while the
/// level stays non-zero. (Gordon only; the motor devices use [`apply_rumble`] — see `read_session`.)
fn apply_gordon(device: &mut Device, cmd: &RumbleCmd, tuning: &GordonTuning) -> Result<()> {
    if cmd.strong > 0 {
        device.haptic_pulse(Motor::Left, train(tuning.duty.drive(cmd.strong), tuning.hz))?;
    }
    if cmd.weak > 0 {
        device.haptic_pulse(Motor::Right, train(tuning.duty.drive(cmd.weak), tuning.hz))?;
    }
    Ok(())
}

/// A pulse train at `hz` whose duty cycle encodes the resolved `drive` (Gordon's only amplitude
/// lever — there is no gain field). Maps full drive onto the actuator's useful duty band
/// ([`RUMBLE_MAX_DUTY`]) at µs resolution, so even a fraction-of-a-percent effective strength
/// produces a distinct (small) pulse.
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

/// Drive the Deck's dual motors from a rumble command via `0xeb` (`strong`→left, `weak`→right; PLAN
/// §1.9). Each motor's per-motor strength drives both the **speed** and **gain** fields through the
/// [`RumbleTuning`] levers (either constant, or scaled into a band). Re-issued by the reader while
/// the level stays non-zero. (Neptune only; Gordon uses [`apply_gordon`] — see `read_session`.)
fn apply_rumble(device: &mut Device, cmd: &RumbleCmd, tuning: &RumbleTuning) -> Result<()> {
    // intensity 0 = strongest (finer amplitude lever, unused for now — §1.9). A zero-strength motor
    // resolves to speed 0 (silent) inside the lever, regardless of the tuning.
    device.rumble_cmd(
        0,
        tuning.speed.drive(cmd.strong),
        tuning.speed.drive(cmd.weak),
        tuning.gain.gain(cmd.strong),
        tuning.gain.gain(cmd.weak),
    )?;
    Ok(())
}

/// Drive Triton's dual motors from a rumble command via the `0x80` output report (`strong`→left,
/// `weak`→right). Same lever shaping as the Deck ([`apply_rumble`]) but its own [`RumbleTuning`]
/// (different motors). Re-issued by the reader while the level stays non-zero (Triton only — see
/// `read_session`).
fn apply_rumble_triton(device: &mut Device, cmd: &RumbleCmd, tuning: &RumbleTuning) -> Result<()> {
    // (intensity, left, right, left_gain, right_gain) — order mirrors `rumble_cmd`. strong→left,
    // weak→right; intensity 0 = no attenuation (finer lever, unused for now).
    device.rumble_triton(
        0,
        tuning.speed.drive(cmd.strong),
        tuning.speed.drive(cmd.weak),
        tuning.gain.gain(cmd.strong),
        tuning.gain.gain(cmd.weak),
    )?;
    Ok(())
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
        DeviceKind::Gordon => {
            let duration = gordon_click_duration(&click.strength);
            device.haptic_pulse(motor, HapticPulse { duration, interval: CLICK_INTERVAL_US, count: 1, gain: 0 })?;
        }
        DeviceKind::Neptune => {
            let gain = neptune_click_gain(&click.side, &click.strength);
            // Intensity stays Default: 0..2 are identical on HW, and gain is the strength lever here.
            device.haptic_cmd(motor, HapticStyle::Strong, HapticIntensity::Default, gain)?;
        }
        DeviceKind::Triton => {
            let (style, amp) = triton_click(&click.strength);
            device.haptic_command_triton(motor, style, amp)?;
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

/// Triton command-click (output report `0x82`) per strength level: a [`HapticStyle`] effect + an
/// amplitude byte. **HW-confirmed:** the amplitude byte is **inert** (no audible change across
/// `0..255`) and even `Weak` is a fairly firm click, so `style` is the only working lever — in
/// practice there are just **two** distinct strengths. Low/Med both use `Weak` (Med carries a
/// max amplitude only so there's a gradient if a firmware ever activates the byte); High = `Strong`.
fn triton_click(strength: &HapticStrength) -> (HapticStyle, u8) {
    match strength {
        HapticStrength::Low => (HapticStyle::Weak, 0),
        HapticStrength::Medium => (HapticStyle::Weak, 255),
        HapticStrength::High => (HapticStyle::Strong, 0),
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
}
