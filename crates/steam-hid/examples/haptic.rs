//! `haptic` — Gordon's trackpad **pulse** (`0x8F` TRIGGER_HAPTIC_PULSE), the rumble/click side of the
//! `0x8F` command (the audible-tone side is `beep`). The Gordon counterpart to `haptic-neptune`
//! (Deck `0xEA`/`0xEB`) and `haptic-triton` (Triton `0x80`/`0x82`).
//!
//! The `0x8F` pulse is a square wave on a trackpad actuator: each cycle is `duration` µs ON then
//! `interval` µs OFF, repeated `count` times, so **frequency ≈ 1e6/(dur+interval) Hz**, the on/off
//! ratio is the **duty cycle**, and `count·(dur+interval)` the total length. Low frequencies
//! (~30–150 Hz) feel like **rumble**; a single pulse (`count=1`) is a **click**. On Gordon amplitude
//! is the **duty cycle** (`gain` ignored); on the Deck the `gain` byte *is* honored. Pad map
//! (verified): wire 0 = RIGHT, 1 = LEFT; **`pad=2` (BOTH) no-ops on Gordon** → fire wire 0 + 1.
//!
//! Command set is **kept parallel with `haptic-neptune`/`haptic-triton`**: the common functional
//! modes first (`rumble`/`clicks`), then Gordon's own primary levers (`duty`/`freq`), then the rest.
//! Everything is driven the way the **engine** does — a short pulse train re-fired contiguously
//! (`TRAIN_MS`/`REFIRE_MS`). `--wired`/`--dongle` pick the transport. Run:
//!   `cargo run -p steam-hid --example haptic -- [--wired|--dongle] [MODE]`
//! Common modes (same in haptic-neptune/haptic-triton):
//!   rumble                    sustained mid rumble, both pads (default)
//!   clicks                    singular Low/Med/High click, each pad + both
//! Gordon levers (the device-specific primaries):
//!   duty                      amplitude sweep via DUTY CYCLE (Gordon's lever)
//!   freq                      frequency sweep 25..1000 Hz (rumble → tone)
//! Further Gordon-only modes:
//!   gain                      pulse GAIN sweep (Deck-honored; Gordon ignores it)
//!   longstop                  how to STOP a long (no-re-fire) train early (count=1 vs count=0)
//!   custom <dur> <int> <cnt> [pad]   fire one custom pulse (pad 0=R/1=L/2=both, default both)
//! Ctrl-C to stop a sweep early.

mod common;

use std::thread::sleep;
use std::time::{Duration, Instant};

use steam_hid::{Device, HapticPosition, HapticPulse, Result};

// Mirror the engine's rumble cadence (engine `RUMBLE_TRAIN_MS`/`RUMBLE_REFIRE_MS`): a *short* pulse
// train re-fired contiguously (so the actuator rings up) rather than one long clean blast.
const TRAIN_MS: u32 = 250;
const REFIRE_MS: u64 = 220;
/// How long to sustain each sweep step (the engine sustains while the game commands rumble).
const HOLD_MS: u64 = 1200;
/// How long to hold each rumble L/R/BOTH step (a bit longer, so the feel is clear).
const RUMBLE_HOLD_MS: u64 = 1500;
/// dB-gain sweep points, shared/unified with haptic-neptune/haptic-triton's `gain`.
const GAINS_DB: [i8; 7] = [-8, -4, 0, 4, 8, 12, 16];

/// Re-assert lizard-off (best-effort; harmless on Gordon, needed if a unit creeps back mid-run).
fn keep_lizard_off(dev: &mut Device) {
    if let Err(e) = dev.set_lizard_mode(false) {
        eprintln!("warning: re-assert lizard-off failed: {e}");
    }
}

/// Fire one `0x8F` pulse at gain 0 (Gordon's model — amplitude is the duty cycle). `wire_pad`:
/// 0=RIGHT, 1=LEFT (Gordon no-ops any other value, so don't pass 2 — use `both`).
fn pulse(dev: &mut Device, wire_pad: u8, dur: u16, interval: u16, count: u16) -> Result<()> {
    let position = if wire_pad == 1 { HapticPosition::Left } else { HapticPosition::Right };
    dev.haptic_pulse(position, HapticPulse { duration: dur, interval, count, gain: 0 })
}

/// Drive both actuators — Gordon ignores `pad=2`, so fire wire 0 and wire 1 separately (they run
/// concurrently).
fn both(dev: &mut Device, dur: u16, interval: u16, count: u16) -> Result<()> {
    pulse(dev, 0, dur, interval, count)?;
    pulse(dev, 1, dur, interval, count)
}

/// Sustain a rumble the *engine's* way on the selected pad(s): re-fire the (short) `count`-cycle
/// train every `REFIRE_MS` for `HOLD_MS`, so the actuator rings up exactly as in-game. `left`/`right`
/// select LEFT (wire 1) / RIGHT (wire 0).
fn sustain_side(dev: &mut Device, running: &common::Running, ms: u64, left: bool, right: bool, p: HapticPulse) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(ms) && running.alive() {
        if left {
            dev.haptic_pulse(HapticPosition::Left, p.clone())?;
        }
        if right {
            dev.haptic_pulse(HapticPosition::Right, p.clone())?;
        }
        sleep(Duration::from_millis(REFIRE_MS));
    }
    Ok(())
}

fn main() -> Result<()> {
    let positional: Vec<String> =
        std::env::args().skip(1).filter(|a| !a.starts_with("--")).collect();

    let mut manager = steam_hid::Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");
    match device.set_lizard_mode(false) {
        Ok(()) => println!("lizard disabled for the test"),
        Err(e) => eprintln!("warning: could not disable lizard: {e}"),
    }
    let running = common::install_ctrlc();

    let mode = positional.first().map(String::as_str);
    // Real test modes get a 1s prep delay; help/unknown does not.
    if matches!(mode, None | Some("rumble" | "clicks" | "duty" | "freq" | "gain" | "longstop" | "custom")) {
        println!("Grip both pads — starting in 1s…");
        sleep(Duration::from_secs(1));
    }

    match mode {
        // === common functional modes (mirror haptic-neptune/haptic-triton) ===

        // A plain sustained rumble — the "it works" check. ~80 Hz at 15% duty on both pads, engine
        // cadence, for HOLD_MS. (The finer amplitude ladder is `duty`.)
        None | Some("rumble") => {
            let hz = 80u32;
            let period = (1_000_000 / hz) as u16;
            let dur = (period as u32 * 15 / 100) as u16;
            let interval = period - dur;
            let count = ((hz * TRAIN_MS) / 1000).max(1) as u16;
            println!("sustained rumble ~{hz}Hz 15% duty (LEFT, RIGHT, BOTH; ~{RUMBLE_HOLD_MS}ms each)…");
            for (label, left, right) in [("LEFT ", true, false), ("RIGHT", false, true), ("BOTH ", true, true)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                keep_lizard_off(&mut device);
                let p = HapticPulse { duration: dur, interval, count, gain: 0 };
                sustain_side(&mut device, &running, RUMBLE_HOLD_MS, left, right, p)?;
                sleep(Duration::from_millis(450));
            }
        }
        // Singular command-haptic clicks: the per-action pulse a `Command`'s Haptics fires on
        // press/release — ONE pulse (count=1). Strength is the single pulse's *duration*. Low/Med/High,
        // each pad then BOTH (both is artificial on Gordon — wire 0 + 1 fired together — for feel).
        Some("clicks") => {
            println!("command-haptic clicks (singular Low/Med/High): LEFT, RIGHT, then BOTH:");
            let clicks = [("Low ", 500u16, 1000u16, 1u16), ("Med ", 1000, 1000, 1), ("High", 2000, 1000, 1)];
            // wire: Some(1)=LEFT, Some(0)=RIGHT, None=BOTH (fire wire 0+1).
            for (side, wire) in [("LEFT", Some(1u8)), ("RIGHT", Some(0u8)), ("BOTH", None)] {
                if !running.alive() {
                    break;
                }
                println!("  {side}:");
                for (name, dur, interval, count) in clicks {
                    if !running.alive() {
                        break;
                    }
                    println!("    {name} (dur={dur}µs interval={interval}µs count={count})");
                    keep_lizard_off(&mut device);
                    match wire {
                        Some(w) => pulse(&mut device, w, dur, interval, count)?,
                        None => both(&mut device, dur, interval, count)?,
                    }
                    sleep(Duration::from_millis(800));
                }
            }
        }

        // === Gordon levers (device-specific primaries) ===

        // Amplitude via DUTY CYCLE (Gordon's lever) at ~80 Hz, engine cadence — what a strength knob
        // maps onto. Fired exactly like the engine (short train re-fired) so the saturation point you
        // feel here is the one the game will have (useful ~1–25% then it saturates).
        Some("duty") => {
            let hz = 80u32;
            let period = (1_000_000 / hz) as u16;
            let count = ((hz * TRAIN_MS) / 1000).max(1) as u16;
            println!("amplitude via DUTY CYCLE at ~{hz}Hz (engine cadence, per pad, ~{HOLD_MS}ms each):");
            for (label, left, right) in [("LEFT ", true, false), ("RIGHT", false, true), ("BOTH ", true, true)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}:");
                for pct in [1u32, 2, 3, 5, 8, 12, 16, 20, 25, 30, 50, 100] {
                    if !running.alive() {
                        break;
                    }
                    let dur = ((period as u32 * pct) / 100).clamp(1, period as u32 - 1) as u16;
                    let interval = period - dur;
                    println!("    {pct:>3}%  (dur={dur}µs interval={interval}µs count={count})");
                    keep_lizard_off(&mut device);
                    let p = HapticPulse { duration: dur, interval, count, gain: 0 };
                    sustain_side(&mut device, &running, HOLD_MS, left, right, p)?;
                    sleep(Duration::from_millis(450));
                }
            }
        }
        // Frequency sweep at 50% duty, engine cadence — low (rumble) to high (tone).
        Some("freq") => {
            println!("frequency sweep (engine cadence, both pads, ~{HOLD_MS}ms each) — rumble → tone:");
            for hz in [25u32, 40, 60, 90, 130, 200, 350, 600, 1000] {
                if !running.alive() {
                    break;
                }
                let period = 1_000_000 / hz;
                let half = (period / 2) as u16;
                let count = ((hz * TRAIN_MS) / 1000).max(1) as u16;
                println!("  {hz:>4} Hz  (dur={half}µs interval={half}µs count={count})");
                keep_lizard_off(&mut device);
                let p = HapticPulse { duration: half, interval: half, count, gain: 0 };
                sustain_side(&mut device, &running, HOLD_MS, true, true, p)?;
                sleep(Duration::from_millis(600));
            }
        }

        // === further Gordon-only modes ===

        // PULSE GAIN sweep (Deck): the `0x8F` `gain` byte is HONORED on the Deck (ignored on Gordon).
        // Unlike DUTY (which fakes amplitude via duty and saturates) this holds a fixed smooth 150 Hz
        // + 50% duty and varies GAIN — ONE long train (no re-fire) so the actuator rings up and plays
        // continuously, testing whether Deck rumble can be *constant* yet *monotonic in strength*.
        Some("gain") => {
            let hz = 150u32;
            let period = (1_000_000 / hz) as u16;
            let half = period / 2;
            let count = ((hz * HOLD_MS as u32) / 1000).max(1) as u16;
            println!("0x8F pulse GAIN sweep (Deck): {hz}Hz 50% duty, one ~{HOLD_MS}ms train, vary gain (dB):");
            for gain in GAINS_DB {
                if !running.alive() {
                    break;
                }
                println!("  gain={gain:>3} dB");
                keep_lizard_off(&mut device);
                let p = HapticPulse { duration: half, interval: half, count, gain };
                device.haptic_pulse(HapticPosition::Left, p.clone())?;
                device.haptic_pulse(HapticPosition::Right, p)?;
                sleep(Duration::from_millis(HOLD_MS)); // let the train play out
                sleep(Duration::from_millis(450)); // inter-step interval (unified w/ neptune/triton)
            }
        }
        // LONG-TRAIN STOP probe: a long train is smoother (no re-fire seam) but keeps playing, so to
        // *use* long trains the engine must STOP one early. Gordon is latest-wins per actuator, so a
        // new pulse REPLACES the running train. Fire an ~8s train, run ~2s, then try a stop candidate:
        //   A) count=1 (dur=1) — replace with a single ~imperceptible tick that ends → should stop.
        //   B) count=0 — replace with a zero-pulse train (may be a no-op the firmware ignores).
        Some("longstop") => {
            let hz = 150u32;
            let period = (1_000_000 / hz) as u16;
            let half = period / 2;
            let long = ((hz * 8_000) / 1000) as u16;
            let fire_long = |dev: &mut Device| -> Result<()> {
                let p = HapticPulse { duration: half, interval: half, count: long, gain: 0 };
                dev.haptic_pulse(HapticPosition::Left, p.clone())?;
                dev.haptic_pulse(HapticPosition::Right, p)
            };
            let stop_with = |dev: &mut Device, count: u16| -> Result<()> {
                let p = HapticPulse { duration: 1, interval: 1, count, gain: 0 };
                dev.haptic_pulse(HapticPosition::Left, p.clone())?;
                dev.haptic_pulse(HapticPosition::Right, p)
            };
            println!("LONG-TRAIN STOP probe (both pads, ~8s train, stop after ~2s):");
            for (label, stop_count) in [("A) count=1", 1u16), ("B) count=0", 0u16)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}: rumbling ~2s…");
                keep_lizard_off(&mut device);
                fire_long(&mut device)?;
                sleep(Duration::from_millis(2000));
                if !running.alive() {
                    break;
                }
                stop_with(&mut device, stop_count)?;
                println!("    STOP sent — SILENT now? (listening ~3s)");
                sleep(Duration::from_millis(3000));
            }
        }
        // Fire one CUSTOM pulse: `custom <dur_us> <interval_us> <count> [pad]` (pad 0=R/1=L/2=both,
        // default both). Frequency ≈ 1e6/(dur+interval); length ≈ count·(dur+interval).
        Some("custom") => {
            let dur: u16 = positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
            let interval: u16 = positional.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
            let count: u16 = positional.get(3).and_then(|s| s.parse().ok()).unwrap_or(50);
            let pad: u8 = positional.get(4).and_then(|s| s.parse().ok()).unwrap_or(2);
            let freq = 1_000_000u32 / (dur as u32 + interval as u32).max(1);
            println!(
                "custom: pad={pad} dur={dur}µs interval={interval}µs count={count} (~{freq} Hz, ~{}ms)",
                count as u32 * (dur as u32 + interval as u32) / 1000
            );
            keep_lizard_off(&mut device);
            if pad >= 2 {
                both(&mut device, dur, interval, count)?;
            } else {
                pulse(&mut device, pad, dur, interval, count)?;
            }
            sleep(Duration::from_millis(1500));
        }
        Some(other) => {
            println!(
                "unknown mode {other:?} — use: rumble | clicks | duty | freq | gain | longstop | \
                 custom <dur> <interval> <count> [pad]"
            );
        }
    }
    Ok(())
}
