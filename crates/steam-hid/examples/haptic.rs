//! `haptic` — verify Gordon's trackpad haptics (`0x8F` ID_TRIGGER_HAPTIC_PULSE).
//!
//! Per the kernel, Gordon has ONE haptic path: `0x8F` driving the two **trackpad**
//! actuators (`0xEB` rumble + `0xEA` haptic2 are Deck-only — the original SC has no
//! rumble motors; `0xEB` confirmed to do nothing on Gordon). Pad mapping is verified:
//! **wire byte 0 = RIGHT pad, 1 = LEFT** (the kernel's "legacy swap").
//!
//! This run isolates the **gain byte** (the kernel's 8-byte variant vs C#'s 7-byte
//! form): it alternates gain 0 dB / +6 dB on the RIGHT pad back-to-back, then fires the
//! −24 dB minimum. **Verified outcome (Gordon): gain is ignored** — no audible difference
//! across −24..+6, so the two packet variants are functionally identical here (the gain
//! byte is honored only on the Deck per the kernel). Fired raw via the escape hatch.
//!
//! Disables lizard mode first so pad-touch doesn't trigger lizard's own click-haptics
//! (Drop restores lizard on exit). `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example haptic -- [--wired|--dongle]`.

mod common;

use std::thread::sleep;
use std::time::Duration;

use steam_hid::{Device, Manager};

/// Fire a `0x8F` trackpad haptic pulse (kernel form: 8-byte payload + `gain`).
/// `wire_pad`: 0 = RIGHT, 1 = LEFT (verified). `dur`/`interval` are microseconds
/// (pulse on-time / gap), `count` = number of pulses, `gain` in dB (−24..+6, as u8).
fn pulse(
    dev: &mut Device,
    wire_pad: u8,
    dur: u16,
    interval: u16,
    count: u16,
    gain: u8,
) -> steam_hid::Result<()> {
    let [d0, d1] = dur.to_le_bytes();
    let [i0, i1] = interval.to_le_bytes();
    let [c0, c1] = count.to_le_bytes();
    dev.send_feature_report(&[0x8F, 8, wire_pad, d0, d1, i0, i1, c0, c1, gain])
}

fn main() -> steam_hid::Result<()> {
    let manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    // Disable lizard so pad-touch doesn't fire lizard's own click-haptics while you
    // feel for the test buzz (Drop restores lizard on exit).
    match device.set_lizard_mode(false) {
        Ok(()) => println!("lizard disabled for the test\n"),
        Err(e) => eprintln!("warning: could not disable lizard: {e}\n"),
    }

    let pause = Duration::from_millis(1800);
    // A clearly-perceptible ~1 kHz tone: 500 µs on / 500 µs off, 400 pulses (~400 ms).
    let (dur, intv, cnt) = (500u16, 500u16, 400u16);
    const RIGHT: u8 = 0;

    // Give yourself a moment to grip the pads before the first buzz.
    println!("starting in 2s…");
    sleep(Duration::from_secs(2));

    // Alternate low/high on the RIGHT pad, back-to-back, so the gain difference is easy
    // to compare (−24 dB is encoded as a u8: 256 − 24 = 232).
    let steps: [(&str, u8); 5] = [
        ("gain  0 dB (baseline)", 0),
        ("gain +6 dB (max) — louder?", 6),
        ("gain  0 dB (baseline)", 0),
        ("gain +6 dB (max) — louder?", 6),
        ("gain -24 dB (min) — quietest?", 232),
    ];
    for (label, gain) in steps {
        println!("RIGHT pad: {label}");
        pulse(&mut device, RIGHT, dur, intv, cnt, gain)?;
        sleep(pause);
    }

    println!("\nDone. Does +6 dB read louder than 0 dB, and -24 dB quieter (i.e. is gain honored)?");
    Ok(())
}
