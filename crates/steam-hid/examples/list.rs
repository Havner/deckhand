//! `list` — enumerate Steam controller gamepad interfaces.
//!
//! Safe/read-only: it only enumerates (no `open`, no commands), so it can't
//! affect controller state. Run: `cargo run -p steam-hid --example list`.

use steam_hid::Manager;

fn main() -> steam_hid::Result<()> {
    let manager = Manager::new()?;
    let devices = manager.enumerate()?;

    if devices.is_empty() {
        println!("No Steam controller gamepad interfaces found.");
        return Ok(());
    }

    println!("Found {} gamepad interface(s):", devices.len());
    for d in &devices {
        // `id` is the stable DeviceId string (PLAN §4.3) — usable as `--input <id>`; a `serial`
        // of `None` (empty last field) means single-dongle stays unique via transport+slot.
        println!(
            "  {:?} / {:?}  {:04x}:{:04x} iface={} serial={:?}  id={}",
            d.kind, d.transport, d.vid, d.pid, d.interface, d.serial, d.id()
        );
    }
    Ok(())
}
