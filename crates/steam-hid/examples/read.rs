//! `read` - open the target controller and log input **changes** (events).
//!
//! Disables lizard mode (raw pads) so the controller feeds real input instead of
//! emulating mouse/keyboard, then streams the change-driven `events()` view and logs
//! each event. This is the one example that demonstrates the `Events` API.
//! `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example read -- [--wired|--dongle]`.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};

use steam_hid::Manager;

fn main() -> steam_hid::Result<()> {
    let mut manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found - connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    match device.set_lizard_mode(false) {
        Ok(()) => println!("lizard disabled (raw input)"),
        Err(e) => eprintln!("warning: could not disable lizard mode: {e}"),
    }

    let log_path = std::env::temp_dir().join("steam-hid-read.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    writeln!(log, "# selected {desc}").ok();
    println!("Logging changes to {}. Ctrl-C to stop.\n", log_path.display());

    // Ctrl-C ends the stream. `events()` blocks until the next frame, so the exit fires
    // when the next event arrives - immediate while you're driving the controller, and
    // within the dongle's ~1s battery heartbeat when idle. (A wired Gordon sends nothing
    // while untouched, so there it exits on the next input.)
    let running = common::install_ctrlc();
    for event in device.events() {
        if !running.alive() {
            break;
        }
        let line = format!("{event:?}");
        println!("{line}");
        writeln!(log, "{line}").ok();
        log.flush().ok();
    }
    Ok(())
}
