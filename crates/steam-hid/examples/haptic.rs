//! `haptic` — explore controller haptics: Gordon's trackpad pulse (`0x8F`
//! TRIGGER_HAPTIC_PULSE) and the Deck's dual-motor rumble (`0xEB` TRIGGER_RUMBLE_CMD).
//!
//! The `0x8F` pulse is a square wave on a trackpad actuator: each cycle is `duration` µs
//! ON then `interval` µs OFF, repeated `count` times. So **frequency ≈ 1e6/(duration+
//! interval) Hz**, the on/off ratio is the **duty cycle**, and `count`×(dur+interval) is
//! the total length. Low frequencies (~30–150 Hz) feel like **rumble**; high ones (~1 kHz)
//! are an audible **tone**. `gain` is ignored on Gordon (verified). Pad map (verified):
//! wire 0 = RIGHT, 1 = LEFT; **`pad=2` (BOTH) is NOT honored by Gordon — it no-ops**, so
//! "both" is done by firing wire 0 + wire 1 separately.
//!
//! The `0xEB` rumble (**Deck-only**; no-ops on Gordon) is the firmware-sustained continuous
//! dual-motor rumble the kernel wires `FF_RUMBLE` to: `left`/`right` are raw magnitudes
//! (`0..=65535`) held until the next command (`0,0` stops), plus per-motor dB gains.
//!
//! No args → the Gordon **pulse** suite: a **frequency sweep**, a **duty-cycle** sweep (both
//! fired the way the **engine** does — a short pulse train re-fired contiguously, so the actuator
//! rings up the same as in-game), and **command-haptic clicks** (the singular per-action Low/Med/
//! High pulse). `--freq` / `--duty` / `--cmd` run a subset. **`--rumble`** runs the Deck `0xEB`
//! motor sweep (strength per motor, then a gain sweep). **`--pgain`** runs the Deck `0x8F` **pulse
//! gain** sweep (fixed frequency, amplitude via the Deck-honored `gain` byte). **`--longstop`**
//! probes how to **stop** a long (no-re-fire) train early (count=1 vs count=0). **`--eint`** sweeps
//! the `0xEB` `intensity` word (a finer, **inverted** amplitude lever — 0 = strongest). **`--ea`**
//! probes the unused `0xEA` `SET_HAPTIC2` (position/style/intensity, from the C#). `<dur_us>
//! <interval_us> <count> [pad]` → fire one **custom** pulse (pad 0=R/1=L/2=both, default both).
//!
//! **`--triton`** runs the new Steam Controller's own suite — `0x80` dual-motor rumble (per-motor
//! strength, gain, intensity) then `0x82` clicks (command × gain, per side); it's a self-contained
//! path (Triton haptics are output reports, unrelated to `0x8F`/`0xEB`/`0xEA`).
//!
//! Disables lizard first — and **re-asserts it before every step** (the Deck reverts to lizard
//! ~10 s after lizard-off, which would fire during a long sweep); Drop restores it.
//! `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example haptic -- [--wired|--dongle] [--rumble|--pgain] [dur int count [pad]]`.

mod common;

use std::thread::sleep;
use std::time::{Duration, Instant};

use steam_hid::{Device, HapticPulse, HapticStyle, Manager, Motor};

// Mirror the engine's rumble cadence (`crates/engine/src/runtime.rs` RUMBLE_TRAIN_MS /
// RUMBLE_REFIRE_MS — keep in sync) so what you feel in the sweeps matches the game: a *short*
// pulse train re-fired contiguously (so the actuator rings up), not one long clean blast.
const TRAIN_MS: u32 = 250;
const REFIRE_MS: u64 = 220;
/// How long to hold each sweep step (the engine sustains as long as the game commands rumble).
const HOLD_MS: u64 = 1200;

/// Re-assert lizard-off. The Deck reverts ~10 s after lizard-off, so call this before every
/// sweep step (best-effort — a transient write hiccup shouldn't abort the test).
fn keep_lizard_off(dev: &mut Device) {
    if let Err(e) = dev.set_lizard_mode(false) {
        eprintln!("warning: re-assert lizard-off failed: {e}");
    }
}

/// Fire one `0x8F` pulse at gain 0 (Gordon's model — amplitude is the duty cycle). `wire_pad`:
/// 0=RIGHT, 1=LEFT (Gordon no-ops any other value, so do NOT pass 2 here — use `both`). Maps the
/// wire pad to a `Motor` and delegates to `Device::haptic_pulse` (which applies the same L/R swap),
/// so the bytes are identical to the raw form.
fn pulse(dev: &mut Device, wire_pad: u8, dur: u16, interval: u16, count: u16) -> steam_hid::Result<()> {
    let motor = if wire_pad == 1 { Motor::Left } else { Motor::Right };
    dev.haptic_pulse(motor, HapticPulse { duration: dur, interval, count, gain: 0 })
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

/// Hold a Deck `0xEB` motor rumble at `left`/`right` (dB gains `lg`/`rg`) for `HOLD_MS`, then
/// stop. The firmware sustains it from the single command, so there is no re-fire (unlike the
/// pulse `sustain`) — we just wait, staying Ctrl-C-responsive, and send `(0,0)` to stop.
fn rumble_hold(
    dev: &mut Device,
    running: &common::Running,
    left: u16,
    right: u16,
    lg: i8,
    rg: i8,
) -> steam_hid::Result<()> {
    keep_lizard_off(dev);
    dev.rumble_cmd(0, left, right, lg, rg)?; // intensity 0 = strongest
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(HOLD_MS) && running.alive() {
        sleep(Duration::from_millis(50));
    }
    dev.rumble_cmd(0, 0, 0, lg, rg) // stop
}

/// Hold a Triton `0x80` rumble for `ms`, re-firing periodically (the firmware sustains each command
/// well past a few hundred ms), then stop with `(0,0)`. Re-asserts lizard-off first (Triton reverts
/// ~3 s after lizard-off on ALL transports — not just BT).
fn triton_hold(
    dev: &mut Device,
    running: &common::Running,
    intensity: u16,
    left: u16,
    left_gain: i8,
    right: u16,
    right_gain: i8,
) -> steam_hid::Result<()> {
    keep_lizard_off(dev);
    let end = Instant::now() + Duration::from_millis(HOLD_MS);
    while Instant::now() < end && running.alive() {
        dev.rumble_triton(intensity, left, right, left_gain, right_gain)?;
        sleep(Duration::from_millis(400));
    }
    dev.rumble_triton(0, 0, 0, 0, 0)?; // stop
    Ok(())
}

/// Triton haptic exploration: `0x80` rumble (per-motor strength, gain, intensity) then `0x82`
/// clicks (command × gain, per side). All ranges provisional — this is where to find the real ones.
fn run_triton(dev: &mut Device, running: &common::Running) -> steam_hid::Result<()> {
    let pause = Duration::from_millis(700);
    println!("\n=== TRITON RUMBLE 0x80 — per-motor STRENGTH sweep (~{HOLD_MS}ms each) ===");
    for (label, l, r) in [("LEFT", true, false), ("RIGHT", false, true), ("BOTH", true, true)] {
        for drive in [4000u16, 8000, 16000, 32000, 48000, 65535] {
            if !running.alive() {
                return Ok(());
            }
            let (ld, rd) = (if l { drive } else { 0 }, if r { drive } else { 0 });
            println!("  {label:>5} drive={drive:>5}  (left={ld} right={rd})");
            triton_hold(dev, running, 0, ld, 0, rd, 0)?;
            sleep(pause);
        }
    }

    println!("\n=== TRITON RUMBLE 0x80 — GAIN sweep (both motors, drive=32000, dB) ===");
    for g in [-8i8, -4, 0, 4, 8] {
        if !running.alive() {
            return Ok(());
        }
        println!("  gain={g:>3} dB");
        triton_hold(dev, running, 0, 32000, g, 32000, g)?;
        sleep(pause);
    }

    println!("\n=== TRITON RUMBLE 0x80 — INTENSITY sweep (drive=32000 both, 0=strongest per Deck) ===");
    for int in [0u16, 1000, 4000, 16000, 32000] {
        if !running.alive() {
            return Ok(());
        }
        println!("  intensity={int:>5}");
        triton_hold(dev, running, int, 32000, 0, 32000, 0)?;
        sleep(pause);
    }

    // amplitude is UNSIGNED (0x00=medium … 0xff=strong per sc-controller), swept across the full
    // range so any effect is visible — the earlier signed-dB sweep wrapped -8 → 248 and clustered.
    println!("\n=== TRITON CLICK 0x82 — style × amplitude (0=medium..255=strong), per side ===");
    for (label, motor) in [("LEFT", Motor::Left), ("RIGHT", Motor::Right)] {
        for style in [HapticStyle::Weak, HapticStyle::Strong] {
            for amp in [0u8, 32, 64, 128, 200, 255] {
                if !running.alive() {
                    return Ok(());
                }
                println!("  {label:>5} style={style:?} amp={amp:>3}");
                keep_lizard_off(dev); // Triton reverts to lizard ~3 s after lizard-off (all transports)
                dev.haptic_command_triton(motor.clone(), style.clone(), amp)?;
                sleep(Duration::from_millis(450));
            }
        }
    }
    println!("\nTriton haptic suite done.");
    Ok(())
}

fn main() -> steam_hid::Result<()> {
    let positional: Vec<String> =
        std::env::args().skip(1).filter(|a| !a.starts_with("--")).collect();

    let mut manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
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

    // Triton (new Steam Controller) uses entirely different output-report haptics (`0x80` rumble /
    // `0x82` click), so it gets its own self-contained suite. Provisional — this is the tool to tune
    // the reader's Triton haptic constants.
    if std::env::args().any(|a| a == "--triton") {
        return run_triton(&mut device, &running);
    }

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
        keep_lizard_off(&mut device);
        if pad >= 2 {
            both(&mut device, dur, interval, count)?;
        } else {
            pulse(&mut device, pad, dur, interval, count)?;
        }
        sleep(Duration::from_millis(1500));
        return Ok(());
    }

    // Which groups to run. `--freq`/`--duty`/`--cmd` are the Gordon pulse suite (default when no
    // flag is given); `--rumble` (Deck `0xEB`), `--pgain` (Deck `0x8F`+gain), and `--longstop`
    // (long-train stop probe) are opt-in. Any flag → run only the flagged groups.
    let args: Vec<String> = std::env::args().collect();
    let has = |name: &str| args.iter().any(|a| a == name);
    let default_suite = !(has("--freq")
        || has("--duty")
        || has("--cmd")
        || has("--rumble")
        || has("--pgain")
        || has("--longstop")
        || has("--eint")
        || has("--ea"));
    let run_freq = default_suite || has("--freq");
    let run_duty = default_suite || has("--duty");
    let run_cmd = default_suite || has("--cmd");
    let run_rumble = has("--rumble");
    let run_pgain = has("--pgain");
    let run_longstop = has("--longstop");
    let run_eint = has("--eint");
    let run_ea = has("--ea");

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
            keep_lizard_off(&mut device);
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
            keep_lizard_off(&mut device);
            sustain(&mut device, &running, dur, interval, count)?;
            sleep(pause);
        }
    }

    // --- Command-haptic clicks: the singular per-action pulse a `Command`'s `Haptics` fires on
    // press/release — ONE pulse (count=1), like the lizard-mode trackpad ticks, NOT a burst.
    // Strength is the single pulse's *duration* (interval is trailing-only at count=1). Three
    // strengths Low/Med/High, one side then the other, to feel and then tune. Starting values. ---
    if run_cmd {
        println!("\n=== COMMAND-HAPTIC CLICKS (singular Low/Med/High, left pad then right) ===");
        // (dur_us, interval_us, count) — one tick each; duration is the only strength lever.
        let clicks = [
            ("Low ", 500u16, 1000u16, 1u16),
            ("Med ", 1000, 1000, 1),
            ("High", 2000, 1000, 1),
        ];
        for (side, wire) in [("LEFT", 1u8), ("RIGHT", 0u8)] {
            if !running.alive() {
                break;
            }
            println!("  {side} pad:");
            for (name, dur, interval, count) in clicks {
                if !running.alive() {
                    break;
                }
                println!("    {name} (dur={dur}µs interval={interval}µs count={count})");
                keep_lizard_off(&mut device);
                pulse(&mut device, wire, dur, interval, count)?;
                sleep(pause);
            }
        }
    }

    // --- Deck motor rumble (0xEB): firmware-sustained continuous rumble, the kernel FF path.
    // First a strength sweep per motor (LEFT/RIGHT/BOTH) at the kernel gains, then a gain sweep
    // at mid strength so you can feel what the dB gain does (and whether it fixes the L/R
    // imbalance / weak saturation the 0x8F pulse showed). Deck-only — no-ops on Gordon. ---
    if run_rumble {
        const KERNEL_LG: i8 = 2; // kernel drives FF_RUMBLE with left = +2 dB, right = 0 dB
        const KERNEL_RG: i8 = 0;
        let speed = |pct: u32| ((u16::MAX as u32 * pct) / 100) as u16;

        println!("\n=== MOTOR RUMBLE 0xEB — STRENGTH sweep (kernel gains L={KERNEL_LG} R={KERNEL_RG}dB, ~{HOLD_MS}ms each) ===");
        for (label, left_on, right_on) in [("LEFT ", true, false), ("RIGHT", false, true), ("BOTH ", true, true)] {
            if !running.alive() {
                break;
            }
            println!("  {label}:");
            for pct in [10u32, 25, 50, 75, 90, 100] {
                if !running.alive() {
                    break;
                }
                let (l, r) = (if left_on { speed(pct) } else { 0 }, if right_on { speed(pct) } else { 0 });
                println!("    {pct:>3}%  (left={l} right={r})");
                rumble_hold(&mut device, &running, l, r, KERNEL_LG, KERNEL_RG)?;
                sleep(pause/4);
            }
        }

        println!("\n=== MOTOR RUMBLE 0xEB — GAIN sweep (both motors at 50% strength, ~{HOLD_MS}ms each) ===");
        let mid = speed(50);
        for gain in [-8i8, -4, -2, 0, 2, 4, 6] {
            if !running.alive() {
                break;
            }
            println!("  gain={gain:>3} dB  (left={mid} right={mid})");
            rumble_hold(&mut device, &running, mid, mid, gain, gain)?;
            sleep(pause/4);
        }
    }

    // --- 0x8F PULSE GAIN sweep (Deck): the `0x8F` pulse's `gain` byte is HONORED on the Deck
    // (ignored on Gordon), so unlike the DUTY sweep (which fakes amplitude via duty cycle and
    // saturates/inverts) this holds a fixed smooth frequency + 50% duty and varies GAIN as the
    // amplitude lever. ONE long train (no re-fire) so the actuator rings up and plays continuously
    // — testing whether Deck rumble can be *constant* (like 0x8F) yet *monotonic in strength*
    // (like gain). Uses `Device::haptic_pulse` (Motor::Left = wire 1, Motor::Right = wire 0). ---
    if run_pgain {
        let hz = 150u32; // smooth mid rumble
        let period = (1_000_000 / hz) as u16;
        let half = period / 2; // 50% duty
        let count = ((hz * HOLD_MS as u32) / 1000).max(1) as u16; // one ~HOLD_MS train
        println!("\n=== 0x8F PULSE GAIN sweep (Deck): {hz}Hz 50% duty, one ~{HOLD_MS}ms train, vary gain (dB) ===");
        for gain in [-24i8, -20, -16, -12, -8, -4, -2, 0, 2, 4, 6] {
            if !running.alive() {
                break;
            }
            println!("  gain={gain:>3} dB  (dur={half}µs interval={half}µs count={count})");
            keep_lizard_off(&mut device);
            let p = HapticPulse { duration: half, interval: half, count, gain };
            device.haptic_pulse(Motor::Left, p.clone())?;
            device.haptic_pulse(Motor::Right, p)?;
            sleep(Duration::from_millis(HOLD_MS)); // let the single train play out
            sleep(pause);
        }
    }

    // --- LONG-TRAIN STOP probe: a long train is smoother (no re-fire seam), but a fixed short
    // train self-terminates whereas a long one keeps playing — so to *use* long trains the engine
    // must be able to STOP one early (when the game drops rumble). Gordon is latest-wins per
    // actuator, so a new pulse REPLACES the running train. Fire an ~8s train on both pads, let it
    // run ~2s, then try a stop and listen for silence. Two candidates, each on a fresh train:
    //   A) count=1 (dur=1) — replace with a single ~imperceptible tick that ends → should stop.
    //   B) count=0 — replace with a zero-pulse train (may be a no-op the firmware ignores).
    // Tell me which window actually went silent. ---
    if run_longstop {
        let hz = 150u32;
        let period = (1_000_000 / hz) as u16;
        let half = period / 2;
        let long = ((hz * 8_000) / 1000) as u16; // ~8s train — far longer than the observe window
        let fire_long = |dev: &mut Device| -> steam_hid::Result<()> {
            let p = HapticPulse { duration: half, interval: half, count: long, gain: 0 };
            dev.haptic_pulse(Motor::Left, p.clone())?;
            dev.haptic_pulse(Motor::Right, p)
        };
        let stop_with = |dev: &mut Device, count: u16| -> steam_hid::Result<()> {
            // dur/interval=1 so if `count`>0 the replacing pulse is a single ~imperceptible tick.
            let p = HapticPulse { duration: 1, interval: 1, count, gain: 0 };
            dev.haptic_pulse(Motor::Left, p.clone())?;
            dev.haptic_pulse(Motor::Right, p)
        };

        println!("\n=== LONG-TRAIN STOP probe (both pads, ~8s train, stop after ~2s) ===");
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
            println!("    STOP sent — SILENT now? (listening ~3s; if you still feel it, this stop failed)");
            sleep(Duration::from_millis(3000));
        }
    }

    // --- 0xEB INTENSITY sweep (Deck): the `0xeb` `intensity` word — a FINER amplitude lever than
    // the coarse dB `gain`, but **inverted** (0 = strongest, larger = weaker, ~unfelt near u16::MAX;
    // usable ~0..16k). Fire a fixed 50% strength + 6dB gain and sweep intensity so you can feel the
    // amplitude fall off. One burst per value, no stop — the ~0.5s burst self-expires. ---
    if run_eint {
        let strength = u16::MAX / 2; // ~50%
        let gain = 6i8;
        println!("\n=== 0xEB INTENSITY sweep (Deck): left=right={strength}, gain={gain}dB, sweep intensity (0=strongest) ===");
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

    // --- 0xEA SET_HAPTIC2 (Deck): the C# app's exclusive Deck haptic — a short, finely-tuned
    // trackpad "click" (nicer than 0x8f; the strongest beats a full 0x8f click). All three fields
    // the C# `NCHapticPacket2` exposes — motor (LEFT/RIGHT), style (Disabled/Weak/Strong), gain
    // (i8 dB, C#: −7..5 ⇒ −2..+10 dB). Uses `Device::haptic_cmd` (which maps the motor + fills the
    // rest). Per pad: a Disabled "off" check, then Weak and Strong across a gain sweep. ---
    if run_ea {
        println!("\n=== 0xEA SET_HAPTIC2 (Deck): motor × style × gain ===");
        for (pad, motor) in [("LEFT ", Motor::Left), ("RIGHT", Motor::Right)] {
            if !running.alive() {
                break;
            }
            println!("  {pad}:");
            println!("    style=Disabled (expect OFF)");
            keep_lizard_off(&mut device);
            device.haptic_cmd(motor.clone(), HapticStyle::Disabled, 0)?;
            sleep(Duration::from_millis(1000));
            for (sname, style) in [("Weak  ", HapticStyle::Weak), ("Strong", HapticStyle::Strong)] {
                if !running.alive() {
                    break;
                }
                println!("    style={sname}:");
                for gain in [-7i8, -4, -2, 0, 2, 4, 5] {
                    if !running.alive() {
                        break;
                    }
                    println!("      gain={gain:>3} (~{}dB)", gain as i32 + 5);
                    keep_lizard_off(&mut device);
                    device.haptic_cmd(motor.clone(), style.clone(), gain)?;
                    sleep(Duration::from_millis(1200));
                }
            }
        }
    }

    println!(
        "\nDone. DUTY sweep fires the engine's way (short train re-fired every {REFIRE_MS}ms) — note \
         where strength stops changing. COMMAND clicks are the singular per-action pulse. MOTOR \
         RUMBLE (--rumble) is the Deck 0xEB path (pulsating). PULSE GAIN (--pgain) is 0x8F with \
         amplitude via the Deck-honored gain byte. LONG-TRAIN STOP (--longstop) checks which stop \
         silences a long train. INTENSITY (--eint) is the 0xEB fine amplitude lever (inverted, \
         0=strongest). SET_HAPTIC2 (--ea) probes the unused 0xEA path (position/style/intensity) — \
         tell me if/how it differs from 0xEB."
    );
    Ok(())
}
