//! `dump` — open the target controller in full raw mode and print every frame.
//!
//! Disables lizard (raw pads) and enables gyro, then polls and prints each frame
//! (state / connect / battery) — the broad "show me everything" diagnostic, vs
//! `read` which logs only changes. `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example dump -- [--wired|--dongle]`.
//! (On Linux the in-kernel `hid-steam` driver claims the device; raw access needs
//! the udev rule in `crates/steam-hid/udev/` — PLAN §1.6.)

mod common;

use std::time::Duration;

use steam_hid::{Manager, Report};

fn main() -> steam_hid::Result<()> {
    let mut manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}. Enabling raw mode…");

    if let Err(e) = device.set_lizard_mode(false) {
        eprintln!("warning: could not disable lizard mode: {e}");
    }
    if let Err(e) = device.set_gyro(true) {
        eprintln!("warning: could not enable gyro: {e}");
    }

    println!("Reading (Ctrl-C to stop)…");
    let running = common::install_ctrlc();
    while running.alive() {
        match device.poll(Duration::from_millis(1000))? {
            None => {} // timeout — nothing this interval
            Some(report) => match report {
                Report::State(s) => println!(
                    // Fixed-width fields (widths = each value's max: seq u32 = 10, accel/gyro
                    // i16 = 6; the {:.2}/{:+.2} floats are already 4/5 for normalized ranges) so
                    // columns don't flow — only the trailing `buttons` is variable.
                    "seq={:<10} L2={:.2} R2={:.2} \
                     lstick=({:+.2},{:+.2}) rstick=({:+.2},{:+.2}) \
                     lpad=({:+.2},{:+.2}) rpad=({:+.2},{:+.2}) lpad_p={:.2} rpad_p={:.2} \
                     accel=({:>6},{:>6},{:>6}) gyro=({:>6},{:>6},{:>6}) buttons={:?}",
                    s.seq,
                    s.left_trigger,
                    s.right_trigger,
                    s.left_stick.x,
                    s.left_stick.y,
                    s.right_stick.x,
                    s.right_stick.y,
                    s.left_pad.pos.x,
                    s.left_pad.pos.y,
                    s.right_pad.pos.x,
                    s.right_pad.pos.y,
                    s.left_pad.pressure,
                    s.right_pad.pressure,
                    s.accel.x,
                    s.accel.y,
                    s.accel.z,
                    s.gyro.x,
                    s.gyro.y,
                    s.gyro.z,
                    s.buttons,
                ),
                Report::Connected => println!("[connected]"),
                Report::Disconnected => println!("[disconnected]"),
                Report::Battery(b) => println!("[battery {} mV]", b.voltage_mv),
                _ => {}
            },
        }
    }
    Ok(())
}
