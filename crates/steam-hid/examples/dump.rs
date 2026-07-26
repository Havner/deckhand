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
    let manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&manager)? else {
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
                    "seq={:<6} L2={:.2} R2={:.2} lstick=({:+.2},{:+.2}) gyro=({},{},{}) buttons={:?}",
                    s.seq,
                    s.left_trigger,
                    s.right_trigger,
                    s.left_stick.x,
                    s.left_stick.y,
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
