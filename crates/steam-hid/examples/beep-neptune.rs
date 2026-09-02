//! `beep-neptune` - audible beeps/tones on the Steam Deck via `0xEA` `SET_HAPTIC2`
//! (`MsgTriggerHaptic`), the Deck-specific counterpart to `beep` (Gordon's `0x8f` pulse).
//!
//! **Why a separate example.** On the Deck, `beep`'s `0x8f` route sounds *worse* than on Gordon: the
//! Deck's actuators are **resonant LRAs (~1.5-2 kHz)**, so a hand-timed square wave collapses toward
//! resonance and rings down - only ~5-6 frequencies come through. `0x8f` there is us **manually
//! toggling** an actuator with no firmware help.
//!
//! `0xEA` is a completely different, **firmware-synthesized** engine (Valve's `MsgTriggerHaptic`, SDL
//! `controller_structs.h`). Its `cmd` selector (`haptic_type_t`) carries - besides the click types we
//! already ship - a **`Tone`** (a real `freq` + `dur_ms`) and a **`LogSweep`** (a chirp
//! `lss_start_freq`->`lss_end_freq` over `dur_ms`), scaled by a `dbgain`.
//! **HW-verified on the Deck (this example):** `Tone` gives clean, *tracking* pitch across the whole
//! ~200-2200 Hz band (no `0x8f` resonance collapse); `LogSweep` chirps beautifully; `dbgain` is the
//! working volume lever (`0` is too quiet - the tone modes fire at ~`8` dB). **`ui_intensity` does
//! nothing to a Tone** (levered by `dbgain` instead - that's why there's no `intensity` mode here,
//! unlike the click-only `haptic --ea`). (`cmd = Noise` also fires but is just a rumble - barely more
//! than `Click`/`Insane`, not a sound, and only `dbgain` moves it - so it's dropped.)
//!
//! It drives three `0xEA` entry points on [`steam_hid::Device`]: `haptic_cmd` (the click types
//! Tick/Click), `haptic_tone` (`freq` + `dur_ms`), and `haptic_logsweep` (`lss_start`/`lss_end`).
//! `ui_intensity` (a click lever) is HW-inert for a Tone, so `dbgain` is the tone amplitude lever.
//!
//! The command set is **kept parallel with `beep`**: the shared tone modes come first, same order and
//! names; the `0xEA`-only modes come at the end. **Deck-only** (no-ops on Gordon). Run:
//!   `cargo run -p steam-hid --example beep-neptune -- [MODE]`
//! Shared modes (same in `beep`; the first three are the practical feedback uses):
//!   kernel            replay the kernel's mode-switch notes, via Tone (default)
//!   pattern           candidate feedback patterns (pitch x count x length x gap)
//!   feedback          candidate single feedback beeps (short/distinct)
//!   note <hz> [ms]    one Tone (default 1500 Hz, 200 ms)
//!   sweep             frequency sweep - pitch tracks the number (unlike 0x8f)
//!   fine              fine sweep 500..3000 Hz
//!   gain              volume via dBgain (the working amplitude lever)
//!   melody            a short tune (C-E-G-C) - proof of pitch
//!   vader             the Imperial March
//! `0xEA`-only modes (firmware synth):
//!   chirp             LogSweep glides (rise / fall) - the nicest feedback cue
//!   lfo               LFO-modulated tone (tremolo/texture) - UNTESTED on the Deck
//!   dur               Tone length sweep (0xEA has a real dur_ms; 0x8f has no length field)
//!   clicks            the reference-proven Tick/Click haptic (the baseline 0xEA path)
//! `--wired`/`--bt` pick the transport. Ctrl-C to stop a sweep early.

mod common;

use std::thread::sleep;
use std::time::Duration;

use steam_hid::{Device, HapticIntensity, HapticSide, HapticType, Result};

const BOTH: HapticSide = HapticSide::Both;
/// Frequency-sweep points shared with `beep`'s `sweep` (kept identical for A/B).
const SWEEP_FREQS: [u16; 9] = [400, 600, 800, 1000, 1250, 1500, 1800, 2200, 3000];
/// dB-gain sweep points shared with `beep`'s `gain`.
const GAINS: [i8; 9] = [-16, -12, -8, -4, 0, 4, 8, 12, 16];

/// Play a plain (unmodulated) Deck Tone via `0xEA` - `haptic_tone` with the LFO off. The `lfo` mode
/// drives the `lfo_freq`/`lfo_depth` fields directly instead.
fn tone(dev: &mut Device, side: HapticSide, freq: u16, dur_ms: i16, gain: i8) -> Result<()> {
    dev.haptic_tone(side, freq, dur_ms, gain, 0, 0)
}

/// Re-assert lizard-off before firing (the Deck reverts to lizard ~10 s after lizard-off, which would
/// fire mid-run). Best-effort - a transient write hiccup shouldn't abort the probe.
fn keep_lizard_off(dev: &mut Device) {
    if let Err(e) = dev.set_lizard_mode(false) {
        eprintln!("warning: re-assert lizard-off failed: {e}");
    }
}

/// Play a `(freq_hz, ms)` note table as Tones on both pads, separated by `gap_ms` of silence (so the
/// notes don't overlap). Shared tables live in `common` so `melody`/`vader` match `beep` exactly.
fn play_tune(dev: &mut Device, running: &common::Running, steps: &[(u32, u32)], gap_ms: u64) -> Result<()> {
    for &(hz, ms) in steps {
        if !running.alive() {
            break;
        }
        keep_lizard_off(dev);
        tone(dev, BOTH, hz as u16, ms as i16, 8)?;
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
    println!("NOTE: 0xEA is Deck-only; on Gordon every mode below is silent (that's expected).");
    keep_lizard_off(&mut device);

    let running = common::install_ctrlc();

    match positional.first().map(String::as_str) {
        // === shared modes (mirror beep, same order/names) ===

        // The Linux kernel's mode-switch notes (1502 Hz "on", 1000 Hz "off"), here via Tone instead of
        // `0x8f` - a direct A/B against `beep`'s `kernel` mode on the same pitches.
        None | Some("kernel") => {
            println!("kernel mode-switch notes via Tone: [1502Hz 'on']  ...  [1000Hz 'off']");
            tone(&mut device, BOTH, 1502, 30, 8)?;
            sleep(Duration::from_millis(700));
            tone(&mut device, BOTH, 1000, 30, 8)?;
            sleep(Duration::from_millis(400));
        }
        // Candidate feedback PATTERNS built from tones (pitch x count x length x gap). The real
        // question isn't "can I hear a pitch" but "can I tell these apart"; judge mutual
        // distinguishability. Same set as `beep`'s `pattern` (the labels are a strawman mapping).
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
            let candidates: [(&str, u16, i16); 3] = [
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
        // Single Tone: `note <freq_hz> [ms]` (default 1500 Hz, 200 ms).
        Some("note") => {
            let hz: u16 = positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(1500);
            let ms: i16 = positional.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
            println!("Tone {hz} Hz for {ms} ms");
            tone(&mut device, BOTH, hz, ms, 8)?;
            sleep(Duration::from_millis(ms as u64 + 300));
        }
        // Frequency sweep - the headline result: pitch TRACKS the number across the whole band, no
        // `0x8f` resonance collapse (HW-verified usable up to ~2200 Hz).
        Some("sweep") => {
            println!("Tone freq sweep (~150ms each). Pitch tracks the frequency (unlike 0x8f):");
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
        // Fine sweep across the LRA resonance band - the analog of `beep`'s `fine`, through the
        // firmware synth instead of `0x8f`.
        Some("fine") => {
            println!("Tone fine sweep 500..=3000 Hz, 100 Hz steps (~120ms each):");
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
        // Volume via dBgain on a fixed Tone - the working amplitude lever (i8 dB; ui_intensity is
        // inert for Tone). Mirrors `beep`'s `gain`.
        Some("gain") => {
            const HZ: u16 = 1500;
            println!("volume via dBgain on a {HZ}Hz Tone (the working amplitude lever):");
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
        // A short tune via Tone - proof of pitch (four distinct rising notes, not four buzzes).
        Some("melody") => {
            println!("melody via Tone (C5 E5 G5 C6)...");
            play_tune(&mut device, &running, common::MELODY, 60)?;
        }
        // The Imperial March - a longer recognizable tune (shared note table with beep).
        Some("vader") => {
            println!("the Imperial March, via Tone...");
            play_tune(&mut device, &running, common::VADER, 60)?;
        }

        // === 0xEA-only modes (firmware synth) ===

        // LogSweep glides - a rising then a falling sweep (candidate enter/exit cues), plus a couple
        // of narrow-band ones. The nicest-feeling `0xEA` feedback (HW-verified).
        Some("chirp") => {
            let chirps: [(&str, u16, u16, i16); 12] = [
                ("rise  400 -> 2000  (full)", 400, 2000, 250),
                ("fall 2000 ->  400  (full)", 2000, 400, 250),
                ("rise 1000 -> 1800  (near resonance)", 1000, 1800, 200),
                ("fall 1800 -> 1000  (near resonance)", 1800, 1000, 200),
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
            println!("LogSweep glides (cmd=LogSweep):");
            for (label, a, b, ms) in chirps {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                keep_lizard_off(&mut device);
                device.haptic_logsweep(BOTH, a, b, ms, 8)?;
                sleep(Duration::from_millis(900));
            }
        }
        // LFO-modulated tone - `0xEA` carries `lfo_freq`/`lfo_depth` alongside the tone (Triton splits
        // this out as its own `0x83 LfoTone` report). A low-freq oscillator on top of a fixed carrier
        // should read as tremolo/texture (a "throbbing" beep) vs the flat `sweep` tone. UNTESTED on the
        // Deck - the noise probe found these inert *for Noise*, never on a Tone. Fixed 1500 Hz carrier;
        // part 1 sweeps depth at a fixed rate, part 2 sweeps rate at a fixed depth.
        Some("lfo") => {
            const HZ: u16 = 1500;
            const MS: i16 = 400; // long enough to feel a few LFO cycles
            println!("LFO-tone probe on a {HZ}Hz carrier (does it throb / add texture?):");
            println!(" part 1: depth sweep @ lfo_freq=8 Hz");
            for depth in [0u8, 32, 64, 128, 200, 255] {
                if !running.alive() {
                    break;
                }
                println!("  lfo_depth {depth:>3} (0 = plain tone)");
                keep_lizard_off(&mut device);
                device.haptic_tone(BOTH, HZ, MS, 8, 8, depth)?;
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
                device.haptic_tone(BOTH, HZ, MS, 8, rate, 200)?;
                sleep(Duration::from_millis(700));
            }
        }
        // Tone length sweep - `0xEA` has a real `dur_ms` field (0x8f has no length field at all; there
        // length is countxperiod). Confirm playback time tracks the number.
        Some("dur") => {
            const HZ: u16 = 1500;
            println!("dur_ms sweep on a {HZ}Hz Tone (0xEA has a real length field):");
            for ms in [20i16, 50, 100, 200, 400, 800] {
                if !running.alive() {
                    break;
                }
                println!("  {ms:>3} ms");
                keep_lizard_off(&mut device);
                tone(&mut device, BOTH, HZ, ms, 8)?;
                sleep(Duration::from_millis(ms as u64 + 700));
            }
        }
        // The reference-proven click types (`Tick`/`Click`) - the baseline `0xEA` path any reference
        // actually sends, and what `Device::haptic_cmd` ships. Tick vs Click x a few ui_intensities.
        Some("clicks") => {
            println!("0xEA click baseline (proven Tick/Click) - the reference-sent path:");
            for (tname, ty) in [("Tick ", HapticType::Tick), ("Click", HapticType::Click)] {
                for (iname, int) in [
                    ("System", HapticIntensity::System),
                    ("Long  ", HapticIntensity::Long),
                    ("Insane", HapticIntensity::Insane),
                ] {
                    if !running.alive() {
                        return Ok(());
                    }
                    println!("  {tname} intensity={iname}");
                    keep_lizard_off(&mut device);
                    device.haptic_cmd(BOTH, ty, int, 0)?;
                    sleep(Duration::from_millis(800));
                }
            }
        }
        Some(other) => {
            println!(
                "unknown mode {other:?} - use: kernel | pattern | feedback | note <hz> [ms] | \
                 sweep | fine | gain | melody | vader | chirp | lfo | dur | clicks"
            );
        }
    }
    Ok(())
}
