//! `haptic-triton` - the new Steam Controller's rumble/click haptics: dual-motor **rumble** (`0x80`
//! HapticRumble) and the **command/click** (`0x82` HapticCommand). The Triton counterpart to `haptic`
//! (Gordon `0x8F`) and `haptic-neptune` (Deck `0xEB`/`0xEA`); Triton's audible-tone side lives in
//! `beep-triton`.
//!
//! Triton drives these as **output reports** (interrupt-OUT, report id in byte 0), not feature
//! reports. `0x80` rumble has the same levers as the Deck's `0xEB`: per-motor `left`/`right` drive
//! ("speed"), per-motor dB `gain`, and a finer inverted `intensity` (`0` = strongest); the firmware
//! safety-times out in ~50 ms, so a sustained rumble is re-issued (~40 ms). `0x82` click's main lever
//! is the `HapticStyle` (off/weak/strong), plus an unsigned `amplitude` trim (`0`=medium..`255`=strong,
//! largely inert). **Triton-only.**
//!
//! Command set is **kept parallel with `haptic`/`haptic-neptune`**: the common functional modes first
//! (`rumble`/`clicks`), then the motor levers (`speed`/`gain`), then the rest. `--wired`/`--bt`/
//! `--dongle` pick the transport. Run:
//!   `cargo run -p steam-hid --example haptic-triton -- [MODE]`
//! Common modes (same in haptic/haptic-neptune):
//!   rumble                    sustained mid rumble, both motors (default)
//!   clicks                    0x82 command/click - side x style (Weak/Strong) x amplitude
//! Motor levers (device-specific primaries):
//!   speed                     0x80 per-motor drive sweep (both motors)
//!   gain                      0x80 GAIN sweep (dB) at mid drive
//! Further Triton-only modes:
//!   intensity                 0x80 intensity word sweep (finer, INVERTED amplitude; 0 = strongest)
//!   pulse                     0x81 pulse-train freq sweep (rumble; ~600-700 Hz oscillates, else fine)
//!   clicks-pulse              0x81 single-pulse clicks (repeat_count=1) - Gordon-style, width=strength
//! Ctrl-C to stop a sweep early.

mod common;

use std::thread::sleep;
use std::time::{Duration, Instant};

use steam_hid::{Device, HapticSide, HapticStyle, Result};

/// How long to sustain each sweep step.
const HOLD_MS: u64 = 1200;
/// How long to hold each rumble L/R/BOTH step (a bit longer, so the feel is clear).
const RUMBLE_HOLD_MS: u64 = 1500;
/// Re-fire cadence for a sustained 0x80 rumble - matches engine `TRITON_REFIRE_MS`.
const REFIRE_MS: u64 = 400;
/// Motor-drive sweep points (percent), shared with haptic-neptune's `speed`.
const SPEED_PCTS: [u32; 6] = [10, 25, 50, 75, 90, 100];
/// dB-gain sweep points, shared with haptic-neptune's `gain`.
const GAINS_DB: [i8; 7] = [-8, -4, 0, 4, 8, 12, 16];

/// Re-assert lizard-off (Triton reverts ~3 s after lizard-off on ALL transports).
fn keep_lizard_off(dev: &mut Device) {
    if let Err(e) = dev.set_lizard_mode(false) {
        eprintln!("warning: re-assert lizard-off failed: {e}");
    }
}

/// `pct` (0..=100) of full `u16` motor drive.
fn speed(pct: u32) -> u16 {
    ((u16::MAX as u32 * pct) / 100) as u16
}

/// Hold a `0x80` rumble for `HOLD_MS`, re-firing every ~400 ms (the firmware sustains each command
/// well past a few hundred ms), then stop with `(0,0)`. Re-asserts lizard-off first. `gain` is applied
/// to both motors.
fn triton_hold(dev: &mut Device, running: &common::Running, ms: u64, intensity: u16, left: u16, right: u16, gain: i8) -> Result<()> {
    keep_lizard_off(dev);
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end && running.alive() {
        dev.rumble_triton(intensity, left, right, gain, gain)?;
        sleep(Duration::from_millis(REFIRE_MS));
    }
    dev.rumble_triton(0, 0, 0, 0, 0)?; // stop
    Ok(())
}

fn main() -> Result<()> {
    let positional: Vec<String> =
        std::env::args().skip(1).filter(|a| !a.starts_with("--")).collect();

    let mut manager = steam_hid::Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found - connected/on?");
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
    if matches!(mode, None | Some("rumble" | "clicks" | "speed" | "gain" | "intensity" | "pulse" | "clicks-pulse")) {
        println!("Hold the controller - starting in 1s... (0x80/0x82 are Triton-only.)");
        sleep(Duration::from_secs(1));
    }

    let pause = Duration::from_millis(450);

    match mode {
        // === common functional modes (mirror haptic/haptic-neptune) ===

        // A plain sustained rumble - the "it works" check. Both motors at 50% drive.
        None | Some("rumble") => {
            let mid = speed(50);
            println!("sustained rumble ~50% (LEFT, RIGHT, BOTH; ~{RUMBLE_HOLD_MS}ms each)...");
            for (label, l, r) in [("LEFT ", mid, 0), ("RIGHT", 0, mid), ("BOTH ", mid, mid)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                triton_hold(&mut device, &running, RUMBLE_HOLD_MS, 0, l, r, 0)?;
                sleep(pause);
            }
        }
        // 0x82 command/click - side x style (Weak/Strong = the main strength lever) x amplitude
        // (unsigned trim, 0=medium..255=strong; largely inert). BOTH honored.
        Some("clicks") => {
            println!("0x82 command/click: side x style x amplitude:");
            for (label, side) in [("LEFT ", HapticSide::Left), ("RIGHT", HapticSide::Right), ("BOTH ", HapticSide::Both)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}:");
                for style in [HapticStyle::Weak, HapticStyle::Strong] {
                    for amp in [0u8, 128, 255] {
                        if !running.alive() {
                            break;
                        }
                        println!("    style={style:?} amp={amp:>3}");
                        keep_lizard_off(&mut device);
                        device.haptic_command_triton(side, style, amp)?;
                        sleep(Duration::from_millis(800));
                    }
                }
            }
        }

        // === motor levers (device-specific primaries) ===

        // 0x80 per-motor DRIVE ("speed") sweep, both motors together. Display: percent | device value.
        Some("speed") => {
            println!("0x80 SPEED sweep (per motor, ~{HOLD_MS}ms each):");
            for (label, left_on, right_on) in [("LEFT ", true, false), ("RIGHT", false, true), ("BOTH ", true, true)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}:");
                for pct in SPEED_PCTS {
                    if !running.alive() {
                        break;
                    }
                    let v = speed(pct);
                    let (l, r) = (if left_on { v } else { 0 }, if right_on { v } else { 0 });
                    println!("    {pct:>3}%  (left={l} right={r})");
                    triton_hold(&mut device, &running, HOLD_MS, 0, l, r, 0)?;
                    sleep(pause);
                }
            }
        }
        // 0x80 GAIN sweep: both motors at 50% drive, vary the dB gain (both) up to +16 dB.
        Some("gain") => {
            let mid = speed(50);
            println!("0x80 GAIN sweep (both motors 50% drive, ~{HOLD_MS}ms each):");
            for g in GAINS_DB {
                if !running.alive() {
                    break;
                }
                println!("  gain={g:>3} dB  (left={mid} right={mid})");
                triton_hold(&mut device, &running, HOLD_MS, 0, mid, mid, g)?;
                sleep(pause);
            }
        }

        // === further Triton-only modes ===

        // 0x80 INTENSITY sweep: the finer, INVERTED amplitude lever (0 = strongest per the Deck's
        // 0xEB), both motors at 50% drive.
        Some("intensity") => {
            let mid = speed(50);
            println!("0x80 INTENSITY sweep (both motors 50% drive, 0=strongest, ~{HOLD_MS}ms each):");
            for int in [0u16, 1000, 4000, 16000, 32000] {
                if !running.alive() {
                    break;
                }
                println!("  intensity={int:>5}");
                triton_hold(&mut device, &running, HOLD_MS, int, mid, mid, 0)?;
                sleep(pause);
            }
        }
        // 0x81 pulse TRAIN frequency sweep - a usable pulse/RUMBLE across the range (HW: fine, except a
        // narrow ~600-700 Hz band that oscillates oddly). 50% duty, ~400ms train per step. Per side:
        // L/R are physically SWAPPED (like Gordon's 0x8f), so the label is the felt pad and the sent
        // HapticSide is the opposite; BOTH works.
        Some("pulse") => {
            let train_ms = 400u32;
            println!("0x81 pulse-train freq sweep (rumble probe), per side:");
            for (label, side) in [("LEFT ", HapticSide::Right), ("RIGHT", HapticSide::Left), ("BOTH ", HapticSide::Both)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}:");
                for hz in [20u32, 40, 60, 90, 130, 200, 350, 600, 1000, 1500] {
                    if !running.alive() {
                        break;
                    }
                    let period = (1_000_000 / hz).max(2) as u16;
                    let half = period / 2;
                    let count = ((hz * train_ms) / 1000).max(1) as u16;
                    println!("    {hz:>4} Hz  (on={half}us off={half}us count={count})");
                    keep_lizard_off(&mut device);
                    device.pulse_triton(side, half, half, count)?;
                    sleep(Duration::from_millis(1000));
                }
            }
        }
        // 0x81 SINGLE-PULSE clicks - an alternative to the 2-strength 0x82 click. `repeat_count = 1`
        // (a single impulse - the *repeated* pulse is what was erratic), `on_us` = strength, mirroring
        // Gordon's count=1 0x8f click ladder. Probes whether 0x81 gives more/finer click strengths.
        Some("clicks-pulse") => {
            let clicks = [("Low ", 500u16), ("Med ", 1000), ("High", 2000), ("Max ", 4000)]; // on_us
            println!("0x81 single-pulse clicks (repeat_count=1, on_us=strength): side x strength:");
            for (label, side) in [("LEFT ", HapticSide::Left), ("RIGHT", HapticSide::Right), ("BOTH ", HapticSide::Both)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}:");
                for (name, on) in clicks {
                    if !running.alive() {
                        break;
                    }
                    println!("    {name} (on={on}us)");
                    keep_lizard_off(&mut device);
                    device.pulse_triton(side, on, 1000, 1)?; // off_us trailing-only at count=1
                    sleep(Duration::from_millis(800));
                }
            }
        }
        Some(other) => {
            println!("unknown mode {other:?} - use: rumble | clicks | speed | gain | intensity | pulse | clicks-pulse");
        }
    }
    Ok(())
}
