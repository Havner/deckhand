//! `beep` — audible feedback tones on the **haptic actuator** (`TRIGGER_HAPTIC_PULSE`, `0x8f`).
//!
//! The Gordon-side counterpart to `beep-neptune` (which drives the Deck's firmware-synthesized
//! `0xEA`). The original Steam Controller (Gordon) has **no speaker** — its startup jingle, pairing
//! chirps and mode-switch beeps are all played on the trackpad voice coils. Driven at an audio
//! frequency the actuator *is* the speaker, and Gordon's broadband voice coils reproduce arbitrary
//! tones faithfully. This route also runs on the Deck, but there the **LRAs are resonant (~1.5–2 kHz)**
//! so off-resonance pitch collapses/rings down (that's the whole reason `beep-neptune`/`0xEA` exists).
//!
//! **Mechanism:** with `duration == interval` the pulse is a 50%-duty square wave at
//! `f = 1_000_000 / (duration + interval)` Hz, lasting `count` cycles (so `count ≈ f·ms/1000`).
//! Amplitude is the **duty cycle** on Gordon (`gain` ignored there; honored on the Deck). Every tone
//! fires **both** actuators (Gordon no-ops `pad=2`, so we fire left and right separately).
//!
//! The command set is **kept parallel with `beep-neptune`**: the shared tone modes come first in the
//! same order and with the same names; the `0x8f`-only probes/levers come at the end. Run:
//!   `cargo run -p steam-hid --example beep -- [--dongle] [MODE]`
//! Shared modes (same in `beep-neptune`; the first three are the practical feedback uses):
//!   kernel            replay the kernel's mode-switch notes (default)
//!   pattern           candidate feedback patterns (pitch × count × length × gap)
//!   feedback          candidate single feedback beeps (short/distinct)
//!   note <hz> [ms]    one tone (default 1500 Hz, 200 ms)
//!   sweep             frequency sweep — hear which tones come through
//!   fine              fine sweep 500..3000 Hz (find the LRA resonance on the Deck)
//!   gain              volume via gain dB (Deck's lever; Gordon ignores it)
//!   melody            a short tune (C-E-G-C) — proof of pitch
//!   vader             the Imperial March
//! `0x8f`-only modes (Gordon's route):
//!   duty              volume via duty cycle (Gordon's amplitude lever)
//!   ringdown          fixed pitch × rising duration, and above-resonance ring-down tail
//! `--wired`/`--dongle` pick the transport. Ctrl-C to stop a sweep early.

mod common;

use std::thread::sleep;
use std::time::Duration;

use steam_hid::{Device, HapticPosition, HapticPulse, Result};

/// Frequency-sweep points shared with `beep-neptune`'s `sweep` (kept identical for A/B). Spans the
/// Deck LRA band and beyond so you hear where each device falls off.
const SWEEP_FREQS: [u32; 9] = [400, 600, 800, 1000, 1250, 1500, 1800, 2200, 3000];
/// dB-gain sweep points shared with `beep-neptune`'s `gain`.
const GAINS: [i8; 9] = [-16, -12, -8, -4, 0, 4, 8, 12, 16];

/// Play a square-wave tone of `freq_hz` for `ms` on one actuator (`duration == interval` = 50 % duty;
/// `count` = number of cycles ≈ `freq·ms/1000`). Gordon ignores gain, so it stays 0.
fn tone(dev: &mut Device, motor: HapticPosition, freq_hz: u32, ms: u32) -> Result<()> {
    if freq_hz == 0 {
        return Ok(());
    }
    let half = (500_000 / freq_hz).clamp(1, u16::MAX as u32) as u16; // half-period µs
    let count = ((freq_hz * ms) / 1000).clamp(1, u16::MAX as u32) as u16;
    dev.haptic_pulse(motor, HapticPulse { duration: half, interval: half, count, gain: 0 })
}

/// Same tone on both actuators (Gordon no-ops `pad=2`, so fire left and right separately — they
/// run concurrently and it's simply louder).
fn tone_both(dev: &mut Device, freq_hz: u32, ms: u32) -> Result<()> {
    tone(dev, HapticPosition::Left, freq_hz, ms)?;
    tone(dev, HapticPosition::Right, freq_hz, ms)
}

/// Re-assert lizard-off before firing (best-effort; harmless on Gordon, needed if a unit ever
/// creeps back to lizard mid-run).
fn keep_lizard_off(dev: &mut Device) {
    if let Err(e) = dev.set_lizard_mode(false) {
        eprintln!("warning: re-assert lizard-off failed: {e}");
    }
}

/// Fire a tone with explicit **duty cycle** and **gain** on both actuators — the low-level form the
/// volume probes need. Frequency (period) and total length stay fixed; `duty_pct` is the high-time
/// fraction of the period, `gain` the `0x8f` gain byte (dB). `tone_both` is just this at 50 % / 0 dB.
fn pulse_both(dev: &mut Device, freq_hz: u32, ms: u32, duty_pct: u32, gain: i8) -> Result<()> {
    let period = (1_000_000 / freq_hz).clamp(2, u16::MAX as u32);
    let duration = (period * duty_pct / 100).clamp(1, period - 1) as u16;
    let interval = period as u16 - duration;
    let count = ((freq_hz * ms) / 1000).clamp(1, u16::MAX as u32) as u16;
    let pulse = HapticPulse { duration, interval, count, gain };
    dev.haptic_pulse(HapticPosition::Left, pulse.clone())?;
    dev.haptic_pulse(HapticPosition::Right, pulse)
}

/// Play a sequence of `(freq_hz, ms)` notes on both actuators, separated by `gap_ms` of silence.
/// Since `tone_both` fires the pulse and returns (it plays out on the device), we sleep each note's
/// own duration before the next so the notes don't overlap.
fn play_tune(dev: &mut Device, running: &common::Running, steps: &[(u32, u32)], gap_ms: u64) -> Result<()> {
    for &(hz, ms) in steps {
        if !running.alive() {
            break;
        }
        keep_lizard_off(dev);
        tone_both(dev, hz, ms)?;
        sleep(Duration::from_millis(ms as u64 + gap_ms));
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
    keep_lizard_off(&mut device);

    let running = common::install_ctrlc();

    match positional.first().map(String::as_str) {
        // === shared modes (mirror beep-neptune, same order/names) ===

        // Replay the LINUX `hid-steam` KERNEL driver's mode-switch beep as pure tones: its "on" note
        // (1502 Hz) then its "off" note (1000 Hz), straight from `steam_do_deck_input`. A known-exact,
        // reference-backed example of the haptic-tone mechanism (the kernel's sound when it toggles
        // gamepad/desktop mode). NOT necessarily what Steam plays on a layer change — we have no
        // reference for that, only that any such click must ride the haptics (the SC has no speaker).
        None | Some("kernel") => {
            println!("kernel mode-switch notes: [1502Hz 'on']  …  [1000Hz 'off']");
            tone_both(&mut device, 1502, 30)?;
            sleep(Duration::from_millis(700));
            tone_both(&mut device, 1000, 30)?;
            sleep(Duration::from_millis(400));
        }
        // Candidate feedback PATTERNS built from the portable levers (pitch level × count × length ×
        // gap). The real question isn't "can I hear a pitch" but "can I tell these apart" — on the
        // Deck especially, where pitch is coarse. Judge mutual distinguishability; the labels are just
        // a strawman command mapping.
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
            println!("candidate feedback patterns (~1s apart) — judge whether they're distinguishable:");
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
            let candidates: [(&str, u32, u32); 3] = [
                ("A low  beep  (1000Hz, 25ms)", 1000, 25),
                ("B mid  beep  (1500Hz, 25ms)", 1500, 25),
                ("C high beep  (2200Hz, 20ms)", 2200, 20),
            ];
            println!("feedback-beep candidates (~0.8s apart) — note which reads best as a 'click':");
            for (label, hz, ms) in candidates {
                if !running.alive() {
                    break;
                }
                println!("  {label}");
                keep_lizard_off(&mut device);
                tone_both(&mut device, hz, ms)?;
                sleep(Duration::from_millis(800));
            }
        }
        // Play a single tone: `note <freq_hz> [ms]` (default 1500 Hz, 200 ms).
        Some("note") => {
            let hz: u32 = positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(1500);
            let ms: u32 = positional.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
            println!("note {hz} Hz for {ms} ms");
            tone_both(&mut device, hz, ms)?;
            sleep(Duration::from_millis(ms as u64 + 200));
        }
        // Frequency sweep so you can hear which tones come through cleanly (and which just buzz).
        Some("sweep") => {
            println!("frequency sweep (~40ms each, ~0.8s apart). Ctrl-C to stop.");
            for hz in SWEEP_FREQS {
                if !running.alive() {
                    break;
                }
                println!("  {hz:>4} Hz");
                keep_lizard_off(&mut device);
                tone_both(&mut device, hz, 40)?;
                sleep(Duration::from_millis(800));
            }
        }
        // Fine sweep to locate the Deck LRA's resonance — the band where it's loudest/cleanest and
        // the only frequencies worth using there. 800..=2200 Hz in 100 Hz steps. On Gordon this is
        // just a smooth chromatic rise (broadband voice coil); on the Deck listen for where it peaks.
        Some("fine") => {
            println!("fine sweep 500..=3000 Hz, 100 Hz steps (~40ms each). Find the resonance peak.");
            for hz in (500..=3000).step_by(100) {
                if !running.alive() {
                    break;
                }
                println!("  {hz:>4} Hz");
                keep_lizard_off(&mut device);
                tone_both(&mut device, hz as u32, 40)?;
                sleep(Duration::from_millis(700));
            }
        }
        // VOLUME via GAIN (dB). Fixed 1500 Hz + 50% duty, vary the `0x8f` gain byte. Honored on the
        // Deck (louder as it rises), ignored on Gordon (no change — that's the expected result there).
        Some("gain") => {
            const HZ: u32 = 1500;
            const MS: u32 = 150;
            println!("volume via GAIN dB @ {HZ}Hz, {MS}ms, 50% duty (Deck's lever; Gordon ignores it):");
            for gain in GAINS {
                if !running.alive() {
                    break;
                }
                println!("  gain {gain:>3} dB");
                keep_lizard_off(&mut device);
                pulse_both(&mut device, HZ, MS, 50, gain)?;
                sleep(Duration::from_millis(800));
            }
        }
        // A short tune — proof the actuator renders arbitrary notes, not just a buzz. C-E-G-C octave.
        Some("melody") => {
            println!("melody (C5 E5 G5 C6)…");
            play_tune(&mut device, &running, common::MELODY, 40)?;
        }
        // The Imperial March — a longer recognizable tune (shared note table with beep-neptune).
        Some("vader") => {
            println!("the Imperial March…");
            play_tune(&mut device, &running, common::VADER, 40)?;
        }

        // === 0x8f-only modes (Gordon's amplitude/ring-down levers) ===

        // VOLUME via DUTY CYCLE (Gordon's lever). Fixed 1500 Hz + fixed length, vary the high-time
        // fraction of the period. On Gordon amplitude ≈ duty (gain is ignored), useful ~1–25% then it
        // saturates; here we also see whether duty changes anything on the Deck's LRA.
        Some("duty") => {
            const HZ: u32 = 1500;
            const MS: u32 = 150;
            println!("volume via DUTY CYCLE @ {HZ}Hz, {MS}ms (Gordon's lever; gain=0):");
            for duty in [2u32, 5, 10, 15, 20, 25, 35, 50] {
                if !running.alive() {
                    break;
                }
                println!("  duty {duty:>2}%");
                keep_lizard_off(&mut device);
                pulse_both(&mut device, HZ, MS, duty, 0)?;
                sleep(Duration::from_millis(800));
            }
        }
        // Ring-down test. Part 1: a fixed near-resonance pitch at rising durations — hear whether
        // length scales cleanly. Part 2: fixed durations driven WELL ABOVE resonance — the actuator
        // can't track, so it rings down and the tone sounds longer/muddier than commanded (the "twice
        // as long" effect). Pick durations/pitches for feedback that don't smear.
        Some("ringdown") => {
            const NEAR: u32 = 1500; // near the Deck resonance
            println!("ring-down, part 1: {NEAR} Hz at rising durations (should scale cleanly):");
            for ms in [10u32, 20, 40, 80, 160] {
                if !running.alive() {
                    break;
                }
                println!("  {ms:>3} ms");
                keep_lizard_off(&mut device);
                tone_both(&mut device, NEAR, ms)?;
                sleep(Duration::from_millis(700));
            }
            println!("part 2: above resonance at a fixed 20ms — listen for the ring-down tail:");
            for hz in [2000u32, 2500, 3000, 3500] {
                if !running.alive() {
                    break;
                }
                println!("  {hz:>4} Hz (commanded 20ms)");
                keep_lizard_off(&mut device);
                tone_both(&mut device, hz, 20)?;
                sleep(Duration::from_millis(700));
            }
        }
        Some(other) => {
            println!(
                "unknown mode {other:?} — use: kernel | pattern | feedback | note <hz> [ms] | \
                 sweep | fine | gain | melody | vader | duty | ringdown"
            );
        }
    }
    Ok(())
}
