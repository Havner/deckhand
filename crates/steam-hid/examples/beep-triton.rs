//! `beep-triton` - audible beeps/tones on the new Steam Controller (Triton, 2026) via its haptic
//! **output reports**, the Triton counterpart to `beep` (Gordon `0x8f`) and `beep-neptune` (Deck
//! `0xEA`).
//!
//! Triton doesn't drive haptics through feature reports at all - it uses **output reports** on the
//! interrupt-OUT endpoint, one report id per primitive (SDL `ValveTritonOutReportMessageIDs`):
//! `0x80` rumble, `0x81` pulse, `0x82` command/click, `0x83` LFO-tone, `0x84` log-sweep, `0x85`
//! script. This example is the **audio/beep probe** for that family - everything here is **HW-UNTESTED**
//! (the reader only ships rumble `0x80` + click `0x82` so far); this is how we find out what beeps.
//!
//! The primitives line up almost 1:1 with the Deck's `0xEA`, so the command set is **kept parallel
//! with `beep`/`beep-neptune`**: the shared tone modes come first, same order and names, and are all
//! driven by **`0x83 LfoTone` with `lfo_depth = 0`** - a plain firmware tone, Triton's analog of the
//! Deck's `cmd = Tone`. The Triton-only modes come at the end: `chirp` (`0x84` log-sweep), `lfo`
//! (`0x83` *with* modulation - the LFO is a first-class report here, the most interesting
//! Triton-original lever), and `clicks` (`0x82`, the proven baseline). (`0x81` pulse, the Gordon-`0x8f`
//! analog, was probed and dropped - erratic HW output; see `protocol::MsgHapticPulse`.) `gain` defaults
//! to ~8 dB (a guess carried from the Deck; the `gain` mode sweeps it).
//!
//! Grip both pads. **Triton-only** (no-ops elsewhere). `--wired`/`--bt`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example beep-triton -- [MODE]`
//! Shared modes (same in `beep`/`beep-neptune`; the first three are the practical feedback uses):
//!   kernel            the two reference pitches (1502/1000 Hz), for cross-device A/B (default)
//!   pattern           candidate feedback patterns (pitch x count x length x gap)
//!   feedback          candidate single feedback beeps (short/distinct)
//!   note <hz> [ms]    one tone (default 1500 Hz, 200 ms)
//!   sweep             frequency sweep - does the tone track pitch? (Triton tops out ~1900 Hz)
//!   fine              fine sweep 500..3000 Hz
//!   gain              volume via dBgain
//!   melody            a short tune (C-E-G-C) - proof of pitch
//!   vader             the Imperial March
//! Triton-only modes (output-report family):
//!   chirp             0x84 log-sweep glides (rise / fall)
//!   lfo               0x83 LFO-modulated tone (tremolo/texture) - the Triton-original lever
//!   clicks            0x82 command/click (Weak/Strong) - the proven baseline
//! Ctrl-C to stop a sweep early.

mod common;

use std::thread::sleep;
use std::time::Duration;

use steam_hid::{Device, HapticSide, HapticStyle, Result};

const BOTH: HapticSide = HapticSide::Both;
/// Frequency-sweep points shared with `beep`/`beep-neptune`'s `sweep` (kept identical for A/B).
const SWEEP_FREQS: [u16; 9] = [400, 600, 800, 1000, 1250, 1500, 1800, 2200, 3000];
/// dB-gain sweep points shared with `beep`/`beep-neptune`'s `gain`.
const GAINS: [i8; 9] = [-16, -12, -8, -4, 0, 4, 8, 12, 16];

/// Play a plain (unmodulated) Triton tone via `0x83 LfoTone` with the LFO off - the shared-mode
/// player, Triton's analog of the Deck's plain `Tone`. The `lfo` mode drives the modulation directly.
fn tone(dev: &mut Device, side: HapticSide, freq: u16, dur_ms: u16, gain: i8) -> Result<()> {
    dev.lfo_tone_triton(side, freq, dur_ms, gain, 0, 0)
}

/// Re-assert lizard-off before firing. Triton reverts to lizard ~3 s after lizard-off on ALL
/// transports, which would fire mid-run. Best-effort - a transient write hiccup shouldn't abort it.
fn keep_lizard_off(dev: &mut Device) {
    if let Err(e) = dev.set_lizard_mode(false) {
        eprintln!("warning: re-assert lizard-off failed: {e}");
    }
}

/// Play a `(freq_hz, ms)` note table as plain tones on both pads, separated by `gap_ms` of silence
/// (so notes don't overlap). Shared tables live in `common` so `melody`/`vader` match the others.
fn play_tune(dev: &mut Device, running: &common::Running, steps: &[(u32, u32)], gap_ms: u64) -> Result<()> {
    for &(hz, ms) in steps {
        if !running.alive() {
            break;
        }
        keep_lizard_off(dev);
        tone(dev, BOTH, hz as u16, ms as u16, 8)?;
        sleep(Duration::from_millis(ms as u64 + gap_ms));
    }
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
    println!("NOTE: this is the Triton haptic family; on Gordon/Deck every mode below is silent.");
    keep_lizard_off(&mut device);

    let running = common::install_ctrlc();

    match positional.first().map(String::as_str) {
        // === shared modes (mirror beep/beep-neptune, same order/names; all via 0x83 plain tone) ===

        // The same two reference pitches beep/beep-neptune play (1502 Hz "on", 1000 Hz "off") - no
        // kernel involvement on Triton, just a fixed A/B pair for cross-device comparison.
        None | Some("kernel") => {
            println!("reference pitches via 0x83 tone: [1502Hz]  ...  [1000Hz]");
            tone(&mut device, BOTH, 1502, 30, 8)?;
            sleep(Duration::from_millis(700));
            tone(&mut device, BOTH, 1000, 30, 8)?;
            sleep(Duration::from_millis(400));
        }
        // Candidate feedback PATTERNS (pitch x count x length x gap) - same set as the others.
        Some("pattern") => {
            type Pat = (&'static str, &'static [(u32, u32)], u64); // label, notes, gap_ms
            let patterns: [Pat; 6] = [
                ("1 short  high        (add-layer?)",    &[(1500, 30)], 0),
                ("1 long   high        (change-set?)",   &[(1500, 160)], 0),
                ("2 short  high        (remove-layer?)", &[(1500, 30), (1500, 30)], 70),
                ("3 short  high",                        &[(1500, 25), (1500, 25), (1500, 25)], 60),
                ("low->high rising     (enter?)",        &[(600, 60), (1500, 60)], 40),
                ("high->low falling    (exit?)",         &[(1500, 60), (600, 60)], 40),
            ];
            println!("candidate feedback patterns (~1s apart) - judge whether they're distinguishable:");
            for (label, steps, gap) in patterns {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                play_tune(&mut device, &running, steps, gap)?;
                sleep(Duration::from_millis(1000));
            }
        }
        // Candidate single feedback beeps to pick from for on-demand audible feedback. Short/distinct.
        Some("feedback") => {
            let candidates: [(&str, u16, u16); 3] = [
                ("A low  beep  (1000Hz, 25ms)", 1000, 25),
                ("B mid  beep  (1500Hz, 25ms)", 1500, 25),
                ("C high beep  (2200Hz, 20ms)", 2200, 20),
            ];
            println!("feedback-beep candidates (~0.8s apart) - note which reads best as a 'click':");
            for (label, hz, ms) in candidates {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                keep_lizard_off(&mut device);
                tone(&mut device, BOTH, hz, ms, 8)?;
                sleep(Duration::from_millis(800));
            }
        }
        // Single tone: `note <freq_hz> [ms]` (default 1500 Hz, 200 ms).
        Some("note") => {
            let hz: u16 = positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(1500);
            let ms: u16 = positional.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
            println!("tone {hz} Hz for {ms} ms");
            tone(&mut device, BOTH, hz, ms, 8)?;
            sleep(Duration::from_millis(ms as u64 + 300));
        }
        // Frequency sweep - does the 0x83 tone track pitch cleanly (like the Deck's 0xEA) across the
        // band, or fall off like an untuned actuator?
        Some("sweep") => {
            println!("tone freq sweep (~150ms each). Does pitch track the frequency?");
            for hz in SWEEP_FREQS {
                if !running.alive() {
                    break;
                }
                println!("  {hz:>4} Hz");
                keep_lizard_off(&mut device);
                tone(&mut device, BOTH, hz, 150, 8)?;
                sleep(Duration::from_millis(800));
            }
        }
        // Fine sweep across the band - Triton's tone tops out ~1900 Hz (2000+ is silent or repeats
        // lower pitches), so the sweep runs 500..3000 to bracket the whole usable range and the edge.
        Some("fine") => {
            println!("tone fine sweep 500..=3000 Hz, 100 Hz steps (~120ms each):");
            for hz in (500u16..=3000).step_by(100) {
                if !running.alive() {
                    break;
                }
                println!("  {hz:>4} Hz");
                keep_lizard_off(&mut device);
                tone(&mut device, BOTH, hz, 120, 8)?;
                sleep(Duration::from_millis(650));
            }
        }
        // Volume via dBgain on a fixed tone - find the usable amplitude range (and whether 8 is a good
        // default, as on the Deck).
        Some("gain") => {
            const HZ: u16 = 1500;
            println!("volume via dBgain on a {HZ}Hz tone:");
            for gain in GAINS {
                if !running.alive() {
                    break;
                }
                println!("  gain {gain:>3} dB");
                keep_lizard_off(&mut device);
                tone(&mut device, BOTH, HZ, 150, gain)?;
                sleep(Duration::from_millis(800));
            }
        }
        // A short tune - proof of pitch (four distinct rising notes, not four buzzes).
        Some("melody") => {
            println!("melody via 0x83 tone (C5 E5 G5 C6)...");
            play_tune(&mut device, &running, common::MELODY, 60)?;
        }
        // The Imperial March - a longer recognizable tune (shared note table with the others).
        Some("vader") => {
            println!("the Imperial March, via 0x83 tone...");
            play_tune(&mut device, &running, common::VADER, 60)?;
        }

        // === Triton-only modes (output-report family) ===

        // NOTE: `0x81` pulse (the Gordon-0x8f analog) was probed and DROPPED - HW output was erratic
        // (unpredictable tones/noises/thumps, some alarming); the clean Triton beep paths are the 0x83
        // tone / 0x84 log-sweep below. See `protocol::MsgHapticPulse`.

        // 0x84 LOG-SWEEP glides - a rising then a falling sweep (candidate enter/exit cues), plus a
        // couple of narrow-band ones. Triton's counterpart to the Deck's `chirp`.
        Some("chirp") => {
            let chirps: [(&str, u16, u16, u16); 12] = [
                ("rise  400 -> 2000  (full)", 400, 2000, 250),
                ("fall 2000 ->  400  (full)", 2000, 400, 250),
                ("rise 1000 -> 1800  (mid)", 1000, 1800, 200),
                ("fall 1800 -> 1000  (mid)", 1800, 1000, 200),
                // narrow ~400 Hz sub-bands, low -> high, each BOTH ways
                ("rise  400 ->  800  (low)", 400, 800, 200),
                ("fall  800 ->  400  (low)", 800, 400, 200),
                ("rise  800 -> 1200  (mid-lo)", 800, 1200, 200),
                ("fall 1200 ->  800  (mid-lo)", 1200, 800, 200),
                ("rise 1200 -> 1600  (mid-hi)", 1200, 1600, 200),
                ("fall 1600 -> 1200  (mid-hi)", 1600, 1200, 200),
                ("rise 1600 -> 2000  (high)", 1600, 2000, 200),
                ("fall 2000 -> 1600  (high)", 2000, 1600, 200),
            ];
            println!("0x84 log-sweep glides:");
            for (label, a, b, ms) in chirps {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                keep_lizard_off(&mut device);
                device.logsweep_triton(BOTH, a, b, ms, 8)?;
                sleep(Duration::from_millis(900));
            }
        }
        // 0x83 LFO-modulated tone - the Triton-original lever: the LFO is its own first-class field
        // pair here (`lfo_freq` rate, `lfo_depth` amount). A low-freq oscillator on a fixed carrier
        // should read as tremolo/texture (a "throbbing" beep) vs the flat `sweep` tone. Fixed 1500 Hz
        // carrier; part 1 sweeps depth at a fixed rate, part 2 sweeps rate at a fixed depth.
        Some("lfo") => {
            const HZ: u16 = 1500;
            const MS: u16 = 400; // long enough to feel a few LFO cycles
            println!("0x83 LFO-tone probe on a {HZ}Hz carrier (does it throb / add texture?):");
            println!(" part 1: depth sweep @ lfo_freq=8 Hz");
            for depth in [0u8, 32, 64, 128, 200, 255] {
                if !running.alive() {
                    break;
                }
                println!("  lfo_depth {depth:>3} (0 = plain tone)");
                keep_lizard_off(&mut device);
                device.lfo_tone_triton(BOTH, HZ, MS, 8, 8, depth)?;
                sleep(Duration::from_millis(700));
            }
            println!(" part 2: rate sweep @ lfo_depth=200 (lfo_freq is a u16 - the character keeps");
            println!("         changing well above 64, so sweep the whole range)");
            for rate in [2u16, 4, 8, 16, 32, 64, 256, 1024, 4096, 16384, 32768, 65535] {
                if !running.alive() {
                    break;
                }
                println!("  lfo_freq {rate:>2} Hz");
                keep_lizard_off(&mut device);
                device.lfo_tone_triton(BOTH, HZ, MS, 8, rate, 200)?;
                sleep(Duration::from_millis(700));
            }
        }
        // 0x82 COMMAND/CLICK - the proven baseline (rumble `0x80` + this are all the reader ships).
        // Style Weak/Strong (the main strength lever) x a few amplitude trims, both pads.
        Some("clicks") => {
            println!("0x82 command/click baseline (proven) - style x amplitude:");
            for style in [HapticStyle::Weak, HapticStyle::Strong] {
                for amp in [0u8, 128, 255] {
                    if !running.alive() {
                        return Ok(());
                    }
                    println!("  style={style:?} amp={amp:>3}");
                    keep_lizard_off(&mut device);
                    device.haptic_command_triton(BOTH, style, amp)?;
                    sleep(Duration::from_millis(700));
                }
            }
        }
        Some(other) => {
            println!(
                "unknown mode {other:?} - use: kernel | pattern | feedback | note <hz> [ms] | \
                 sweep | fine | gain | melody | vader | chirp | lfo | clicks"
            );
        }
    }
    Ok(())
}
