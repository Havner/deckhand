//! `haptic-neptune` — the Steam Deck's rumble/click haptics: the dual-motor **rumble** (`0xEB`
//! TRIGGER_RUMBLE_CMD) and the finely-tuned trackpad **click** (`0xEA` SET_HAPTIC2). The Deck
//! counterpart to `haptic` (Gordon `0x8F`) and `haptic-triton` (Triton `0x80`/`0x82`); the Deck's
//! audible-tone side lives in `beep-neptune`.
//!
//! `0xEB` (`rumble_cmd`) is the kernel's `FF_RUMBLE` path: firmware-sustained **pulsating** dual-motor
//! rumble — `left`/`right` = per-motor magnitude ("speed"), `left_gain`/`right_gain` (dB) = amplitude,
//! and `intensity` a finer **inverted** amplitude lever (`0` = strongest). Each command is a fixed
//! short burst; a sustained rumble is re-issued. `0xEA` (`haptic_cmd`) is a short trackpad **click**
//! (strongest beats a full `0x8F`): levers are the haptic `type` (Tick/Click), `ui_intensity` (0..4),
//! and `dBgain`. **Deck-only** (`0xEB`/`0xEA` no-op on Gordon).
//!
//! Command set is **kept parallel with `haptic`/`haptic-triton`**: the common functional modes first
//! (`rumble`/`clicks`), then the Deck's motor levers (`speed`/`gain`), then the rest. Run:
//!   `cargo run -p steam-hid --example haptic-neptune -- [--wired|--dongle] [MODE]`
//! Common modes (same in haptic/haptic-triton):
//!   rumble                    sustained mid rumble, both motors (default)
//!   clicks                    0xEA clicks — side × type (Tick/Click) × ui_intensity
//! Motor levers (device-specific primaries):
//!   speed                     0xEB per-motor magnitude sweep (both motors)
//!   gain                      0xEB GAIN sweep (dB) at mid strength
//! Further Deck-only modes:
//!   intensity                 0xEB intensity word sweep (finer, INVERTED amplitude; 0 = strongest)
//!   clicks-gain               0xEA Click: side × ui_intensity(System/Long) × gain (-8,0,8,16 dB)
//! Ctrl-C to stop a sweep early.

mod common;

use std::thread::sleep;
use std::time::{Duration, Instant};

use steam_hid::{Device, HapticIntensity, HapticSide, HapticType, Result};

/// How long to sustain each rumble step.
const HOLD_MS: u64 = 1200;
/// 0xEB plays a fixed ~0.5 s burst, so re-fire it to sustain — matches engine `NEPTUNE_REFIRE_MS`.
const REFIRE_MS: u64 = 500;
/// How long to hold each rumble L/R/BOTH step (a bit longer, so the feel is clear).
const RUMBLE_HOLD_MS: u64 = 1500;
/// Fixed rumble gain (dB), both motors — same on left and right (no per-motor split).
const RUMBLE_GAIN_DB: i8 = 4;
/// Motor-magnitude sweep points (percent), shared with haptic-triton's `speed`.
const SPEED_PCTS: [u32; 6] = [10, 25, 50, 75, 90, 100];
/// dB-gain sweep points, shared with haptic-triton's `gain`.
const GAINS_DB: [i8; 7] = [-8, -4, 0, 4, 8, 12, 16];

/// Re-assert lizard-off (the Deck reverts ~10 s after lizard-off, which would fire mid-run).
fn keep_lizard_off(dev: &mut Device) {
    if let Err(e) = dev.set_lizard_mode(false) {
        eprintln!("warning: re-assert lizard-off failed: {e}");
    }
}

/// `pct` (0..=100) of full `u16` motor magnitude.
fn speed(pct: u32) -> u16 {
    ((u16::MAX as u32 * pct) / 100) as u16
}

/// Hold a `0xEB` motor rumble at `left`/`right` (dB gain `gain`, both motors) for `HOLD_MS`, then
/// stop. Each `0xEB` command is a fixed ~0.5 s burst, so re-fire every `REFIRE_MS` (like the engine)
/// to sustain the full duration — otherwise it dies after ~0.5 s. `intensity` 0 = strongest.
fn rumble_hold(dev: &mut Device, running: &common::Running, ms: u64, left: u16, right: u16, gain: i8) -> Result<()> {
    keep_lizard_off(dev);
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end && running.alive() {
        dev.rumble_cmd(0, left, right, gain, gain)?;
        sleep(Duration::from_millis(REFIRE_MS));
    }
    dev.rumble_cmd(0, 0, 0, gain, gain) // stop
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
    if matches!(mode, None | Some("rumble" | "clicks" | "speed" | "gain" | "intensity" | "clicks-gain")) {
        println!("Hold the controller — starting in 1s… (0xEB/0xEA are Deck-only, silent on Gordon.)");
        sleep(Duration::from_secs(1));
    }

    match mode {
        // === common functional modes (mirror haptic/haptic-triton) ===

        // A plain sustained rumble — the "it works" check. Both motors at 50%, gain 4 dB (both).
        None | Some("rumble") => {
            let mid = speed(50);
            println!("sustained rumble ~50% (gain {RUMBLE_GAIN_DB}dB; LEFT, RIGHT, BOTH; ~{RUMBLE_HOLD_MS}ms each)…");
            for (label, l, r) in [("LEFT ", mid, 0), ("RIGHT", 0, mid), ("BOTH ", mid, mid)] {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                rumble_hold(&mut device, &running, RUMBLE_HOLD_MS, l, r, RUMBLE_GAIN_DB)?;
                sleep(Duration::from_millis(450));
            }
        }
        // 0xEA SET_HAPTIC2 clicks — the finely-tuned trackpad click (nicer than 0x8F; the strongest
        // beats a full 0x8F click). side (LEFT/RIGHT/BOTH — 0xEA honors a native BOTH) × type
        // (Tick, Click — Off skipped, it's silent) × every ui_intensity (System..Insane; HW: 0..2
        // identical, 3 stronger, 4 stronger/other). Gain fixed at 0.
        Some("clicks") => {
            let ints = [
                ("System", HapticIntensity::System),
                ("Short ", HapticIntensity::Short),
                ("Medium", HapticIntensity::Medium),
                ("Long  ", HapticIntensity::Long),
                ("Insane", HapticIntensity::Insane),
            ];
            println!("0xEA clicks: side × type × ui_intensity (gain=0):");
            for (pad, side) in [("LEFT ", HapticSide::Left), ("RIGHT", HapticSide::Right), ("BOTH ", HapticSide::Both)] {
                if !running.alive() {
                    break;
                }
                println!("  {pad}:");
                for (tname, htype) in [("Tick ", HapticType::Tick), ("Click", HapticType::Click)] {
                    if !running.alive() {
                        break;
                    }
                    println!("    type={tname}:");
                    for (iname, intensity) in &ints {
                        if !running.alive() {
                            break;
                        }
                        println!("      intensity={iname}");
                        keep_lizard_off(&mut device);
                        device.haptic_cmd(side, htype, *intensity, 0)?;
                        sleep(Duration::from_millis(800));
                    }
                }
            }
        }

        // === motor levers (device-specific primaries) ===

        // 0xEB per-motor MAGNITUDE ("speed") sweep, both motors together, at the fixed rumble gain.
        // Display: percent | device value (left/right magnitude).
        Some("speed") => {
            println!("0xEB SPEED sweep (per motor, gain {RUMBLE_GAIN_DB}dB, ~{HOLD_MS}ms each):");
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
                    rumble_hold(&mut device, &running, HOLD_MS, l, r, RUMBLE_GAIN_DB)?;
                    sleep(Duration::from_millis(450));
                }
            }
        }
        // 0xEB GAIN sweep: both motors at 50% strength, vary the dB gain (both) up to +16 dB.
        Some("gain") => {
            let mid = speed(50);
            println!("0xEB GAIN sweep (both motors 50% strength, ~{HOLD_MS}ms each):");
            for gain in GAINS_DB {
                if !running.alive() {
                    break;
                }
                println!("  gain={gain:>3} dB  (left={mid} right={mid})");
                rumble_hold(&mut device, &running, HOLD_MS, mid, mid, gain)?;
                sleep(Duration::from_millis(450));
            }
        }

        // === further Deck-only modes ===

        // 0xEB INTENSITY sweep: the `intensity` word — a FINER amplitude lever than the coarse dB gain,
        // **inverted** (0 = strongest, larger = weaker, ~unfelt near u16::MAX; usable ~0..16k). A LE
        // u16: the low byte alone is imperceptible (it's the LSB), so sweep across the whole word.
        // Fixed 50% strength + 6 dB gain; one burst per value, self-expires.
        Some("intensity") => {
            let strength = speed(50);
            let gain = 6i8;
            println!("0xEB INTENSITY sweep (left=right={strength}, gain={gain}dB, 0=strongest):");
            for intensity in [0u16, 1, 16, 256, 1024, 2048, 4096, 8192, 16384, 32768, 65535] {
                if !running.alive() {
                    break;
                }
                println!("  intensity={intensity:>5}  (burst self-expires ~0.5s)");
                keep_lizard_off(&mut device);
                device.rumble_cmd(intensity, strength, strength, gain, gain)?;
                sleep(Duration::from_millis(1500));
            }
            let _ = device.rumble_cmd(0, 0, 0, gain, gain); // ensure silence at the end
        }
        // 0xEA CLICK gain sweep — isolate `dbgain` as the click strength lever: `cmd=Click` only,
        // ui_intensity System/Long, gain -8/0/8/16 dB. side (LEFT/RIGHT/BOTH). Same timing as `clicks`.
        Some("clicks-gain") => {
            println!("0xEA CLICK gain sweep: side × ui_intensity(System/Long) × gain (dB):");
            for (pad, side) in [("LEFT ", HapticSide::Left), ("RIGHT", HapticSide::Right), ("BOTH ", HapticSide::Both)] {
                if !running.alive() {
                    break;
                }
                println!("  {pad}:");
                for (iname, intensity) in [("System", HapticIntensity::System), ("Long  ", HapticIntensity::Long)] {
                    if !running.alive() {
                        break;
                    }
                    println!("    intensity={iname}:");
                    for gain in [-8i8, 0, 8, 16] {
                        if !running.alive() {
                            break;
                        }
                        println!("      gain={gain:>3} dB");
                        keep_lizard_off(&mut device);
                        device.haptic_cmd(side, HapticType::Click, intensity, gain)?;
                        sleep(Duration::from_millis(800));
                    }
                }
            }
        }
        Some(other) => {
            println!("unknown mode {other:?} — use: rumble | clicks | speed | gain | intensity | clicks-gain");
        }
    }
    Ok(())
}
