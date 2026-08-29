//! `stickforce` — print ONLY the Deck's thumbstick **capacitive force** (raw `NeptuneReport` fields
//! at 0x3C/0x3E; InputPlumber-only, Neptune-only). Press the stick tops to see it move (~0..112).
//! Ctrl-C to stop. Run: `cargo run -p steam-hid --example stickforce -- [--wired|--dongle]`.

mod common;

use std::time::{Duration, Instant};

use steam_hid::{Manager, RawReport};

fn main() -> steam_hid::Result<()> {
    let mut manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}. Press the stick tops (Ctrl-C to stop)…");
    let _ = device.set_lizard_mode(false); // Deck streams raw input only with lizard off
    let running = common::install_ctrlc();
    let mut last = Instant::now();
    while running.alive() {
        if last.elapsed() >= Duration::from_secs(2) {
            let _ = device.set_lizard_mode(false); // re-assert (Deck reverts ~10 s)
            last = Instant::now();
        }
        if let Some(RawReport::Neptune(r)) = device.poll_raw(Duration::from_millis(500))? {
            println!("L={:>6} R={:>6}", r.left_stick_force, r.right_stick_force);
        }
    }
    Ok(())
}
