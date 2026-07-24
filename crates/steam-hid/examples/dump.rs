//! `dump` — enumerate Steam controllers, open the first, and print state changes.
//!
//! Skeleton per PLAN §1.8. Run with: `cargo run -p steam-hid --example dump`.
//! (On Linux the in-kernel `hid-steam` driver claims the device; raw access needs
//! the udev rule in `crates/steam-hid/udev/` — PLAN §1.6.)

use std::time::Duration;

use steam_hid::{Manager, Report};

fn main() -> steam_hid::Result<()> {
    let manager = Manager::new()?;

    let devices = manager.enumerate()?;
    if devices.is_empty() {
        println!("No Steam controller gamepad interfaces found.");
        return Ok(());
    }
    println!("Found {} device(s):", devices.len());
    for d in &devices {
        println!(
            "  {:?} / {:?}  {:04x}:{:04x} iface={} serial={:?}",
            d.kind, d.transport, d.vid, d.pid, d.interface, d.serial
        );
    }

    let mut device = manager.open(&devices[0])?;
    println!("\nOpened {:?}. Enabling raw mode…", device.info().kind);
    if let Err(e) = device.set_lizard_mode(false) {
        eprintln!("warning: could not disable lizard mode: {e}");
    }
    if let Err(e) = device.set_gyro(true) {
        eprintln!("warning: could not enable gyro: {e}");
    }

    println!("Reading (Ctrl-C to stop)…");
    loop {
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
}
