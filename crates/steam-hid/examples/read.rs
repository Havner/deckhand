//! `read` — open the target controller and log input **changes** (events).
//!
//! `--raw` disables lizard first (pad mode NONE); `--wired`/`--dongle` pick the
//! transport. Run: `cargo run -p steam-hid --example read -- [--raw] [--wired|--dongle]`.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};

use steam_hid::Manager;

fn main() -> steam_hid::Result<()> {
    let raw = std::env::args().any(|a| a == "--raw");

    let manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    if raw {
        match device.set_lizard_mode(false) {
            Ok(()) => println!("raw mode: lizard disabled (pad mode NONE)"),
            Err(e) => eprintln!("warning: could not disable lizard mode: {e}"),
        }
    }

    let log_path = std::env::temp_dir().join("steam-hid-read.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    writeln!(log, "# selected {desc}").ok();
    println!("Logging changes to {}. Ctrl-C to stop.\n", log_path.display());

    for event in device.events() {
        let line = format!("{event:?}");
        println!("{line}");
        writeln!(log, "{line}").ok();
        log.flush().ok();
    }
    Ok(())
}
