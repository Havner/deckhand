//! `haptic` — explore Gordon's trackpad haptic parameters (`0x8F` TRIGGER_HAPTIC_PULSE).
//!
//! The `0x8F` pulse is a square wave on a trackpad actuator: each cycle is `duration` µs
//! ON then `interval` µs OFF, repeated `count` times. So **frequency ≈ 1e6/(duration+
//! interval) Hz**, the on/off ratio is the **duty cycle**, and `count`×(dur+interval) is
//! the total length. Low frequencies (~30–150 Hz) feel like **rumble**; high ones (~1 kHz)
//! are an audible **tone**. `gain` is ignored on Gordon (verified). Pad map (verified):
//! wire 0 = RIGHT, 1 = LEFT; **`pad=2` (BOTH) is NOT honored by Gordon — it no-ops**, so
//! "both" is done by firing wire 0 + wire 1 separately. `0xeb`/`0xea` are Deck-only.
//!
//! No args → runs a **frequency sweep** then a **duty-cycle** sweep on both pads, each fired the
//! way the **engine** does (a short pulse train re-fired contiguously, so the actuator rings up the
//! same as in-game — tuning here transfers). `--freq` / `--duty` run just one (default both).
//! `<dur_us> <interval_us> <count> [pad]` → fire one **custom** pulse (pad 0=R/1=L/2=both, default
//! both).
//!
//! Disables lizard first (so pad-touch doesn't fire lizard click-haptics; Drop restores).
//! `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example haptic -- [--wired|--dongle] [dur int count [pad]]`.

mod common;

use std::thread::sleep;
use std::time::{Duration, Instant};

use steam_hid::{Device, Manager};

// Mirror the engine's rumble cadence (`crates/engine/src/runtime.rs` RUMBLE_TRAIN_MS /
// RUMBLE_REFIRE_MS — keep in sync) so what you feel in the sweeps matches the game: a *short*
// pulse train re-fired contiguously (so the actuator rings up), not one long clean blast.
const TRAIN_MS: u32 = 250;
const REFIRE_MS: u64 = 220;
/// How long to hold each sweep step (the engine sustains as long as the game commands rumble).
const HOLD_MS: u64 = 1200;

/// Fire one `0x8F` pulse (kernel 8-byte form). `wire_pad`: 0=RIGHT, 1=LEFT (Gordon
/// no-ops any other value, so do NOT pass 2 here — use `both`).
fn pulse(dev: &mut Device, wire_pad: u8, dur: u16, interval: u16, count: u16) -> steam_hid::Result<()> {
    let [d0, d1] = dur.to_le_bytes();
    let [i0, i1] = interval.to_le_bytes();
    let [c0, c1] = count.to_le_bytes();
    dev.send_feature_report(&[0x8F, 8, wire_pad, d0, d1, i0, i1, c0, c1, 0])
}

/// Drive both actuators — Gordon ignores `pad=2`, so fire wire 0 and wire 1 separately
/// (back-to-back; they run concurrently).
fn both(dev: &mut Device, dur: u16, interval: u16, count: u16) -> steam_hid::Result<()> {
    pulse(dev, 0, dur, interval, count)?;
    pulse(dev, 1, dur, interval, count)
}

/// Sustain a rumble the *engine's* way: re-fire the (short) `count`-cycle train on both pads every
/// `REFIRE_MS` for `HOLD_MS`, so the actuator rings up exactly as it does in-game.
fn sustain(
    dev: &mut Device,
    running: &common::Running,
    dur: u16,
    interval: u16,
    count: u16,
) -> steam_hid::Result<()> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(HOLD_MS) && running.alive() {
        both(dev, dur, interval, count)?;
        sleep(Duration::from_millis(REFIRE_MS));
    }
    Ok(())
}

fn main() -> steam_hid::Result<()> {
    let positional: Vec<String> =
        std::env::args().skip(1).filter(|a| !a.starts_with("--")).collect();

    let manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");
    match device.set_lizard_mode(false) {
        Ok(()) => println!("lizard disabled for the test"),
        Err(e) => eprintln!("warning: could not disable lizard: {e}"),
    }
    // Ctrl-C ends the sweep between pulses (and, on Windows, keeps the process alive so
    // Drop restores lizard instead of the console handler aborting it mid-test).
    let running = common::install_ctrlc();

    // Custom single-pulse mode: `haptic <dur_us> <interval_us> <count> [pad]`.
    if positional.len() >= 3 {
        let dur: u16 = positional[0].parse().expect("dur_us");
        let interval: u16 = positional[1].parse().expect("interval_us");
        let count: u16 = positional[2].parse().expect("count");
        let pad: u8 = positional.get(3).and_then(|p| p.parse().ok()).unwrap_or(2);
        let freq = 1_000_000u32 / (dur as u32 + interval as u32).max(1);
        println!(
            "\ncustom: pad={pad} dur={dur}µs interval={interval}µs count={count} (~{freq} Hz, \
             ~{}ms)",
            count as u32 * (dur as u32 + interval as u32) / 1000
        );
        if pad >= 2 {
            both(&mut device, dur, interval, count)?;
        } else {
            pulse(&mut device, pad, dur, interval, count)?;
        }
        sleep(Duration::from_millis(1500));
        return Ok(());
    }

    // Which sweeps to run — `--freq` / `--duty` pick one; neither = both.
    let args: Vec<String> = std::env::args().collect();
    let (run_freq, run_duty) = match (
        args.iter().any(|a| a == "--freq"),
        args.iter().any(|a| a == "--duty"),
    ) {
        (false, false) => (true, true),
        (f, d) => (f, d),
    };

    let pause = Duration::from_millis(1800);
    println!("\nGrip BOTH pads. Starting in 2s…");
    sleep(Duration::from_secs(2));

    // --- Frequency sweep at 50% duty, engine cadence — low (rumble) → high (tone) ---
    if run_freq {
        println!("\n=== FREQUENCY SWEEP (engine cadence, both pads, ~{HOLD_MS}ms each) — rumble → tone ===");
        for freq in [25u32, 40, 60, 90, 130, 200, 350, 600, 1000] {
            if !running.alive() {
                break;
            }
            let period = 1_000_000 / freq; // µs
            let half = (period / 2) as u16;
            let count = ((freq * TRAIN_MS) / 1000).max(1) as u16;
            println!("  {freq:>4} Hz  (dur={half}µs interval={half}µs count={count})");
            sustain(&mut device, &running, half, half, count)?;
            sleep(pause);
        }
    }

    // --- Duty-cycle sweep at ~80 Hz, engine cadence — the amplitude lever is duty only (`gain`
    // ignored), so this is what a strength knob maps onto. Firing exactly like the engine (short
    // train re-fired) so the saturation point you feel here is the one the game will have. ---
    if run_duty {
        println!("\n=== DUTY SWEEP at ~80 Hz (engine cadence, ~{HOLD_MS}ms each) — find where strength saturates ===");
        let hz = 80u32;
        let period = (1_000_000 / hz) as u16; // 12500 µs
        let count = ((hz * TRAIN_MS) / 1000).max(1) as u16;
        for pct in [1u32, 2, 3, 5, 8, 12, 20, 30, 40, 50, 60, 70, 80, 90, 100] {
            if !running.alive() {
                break;
            }
            let dur = ((period as u32 * pct) / 100).clamp(1, period as u32 - 1) as u16;
            let interval = period - dur;
            println!("  {pct:>3}% on  (dur={dur}µs interval={interval}µs count={count})");
            sustain(&mut device, &running, dur, interval, count)?;
            sleep(pause);
        }
    }

    println!(
        "\nDone — these fired the same way the engine does (short train re-fired every {REFIRE_MS}ms, \
         both pads). On the DUTY sweep: which %% still changed, and roughly where did it stop getting \
         stronger? That's the band `RUMBLE_MAX_DUTY` should map full strength onto."
    );
    Ok(())
}
