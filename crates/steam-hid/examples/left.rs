//! `left` - dump the resolved left pad/stick + flags (throttled), for debugging.
//!
//! The pad and analog stick share `0x10`, disambiguated by `LPAD_TOUCH` (resolved in
//! `state::from_gordon`); this prints the normalized snapshot so you can watch the multiplex
//! resolve live - pad and stick land in separate fields (`left_pad.pos` vs `left_stick`).
//!
//! Disables lizard mode (raw pads); `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example left -- [--wired|--dongle]`.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{Buttons, Manager, Report};

fn main() -> steam_hid::Result<()> {
    let mut manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found - connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    match device.set_lizard_mode(false) {
        Ok(()) => println!("lizard disabled (raw input)"),
        Err(e) => eprintln!("warning: could not disable lizard: {e}"),
    }

    let log_path = std::env::temp_dir().join("steam-hid-left.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    writeln!(log, "# selected {desc}").ok();
    println!("Dumping resolved left pad/stick to {}. Ctrl-C to stop.\n", log_path.display());

    let running = common::install_ctrlc();
    let mut last = Instant::now();
    while running.alive() {
        if let Some(Report::State(s)) = device.poll(Duration::from_millis(100))?
            && last.elapsed() >= Duration::from_millis(120)
        {
            last = Instant::now();
            let line = format!(
                "pad=({:+.3},{:+.3})  stick=({:+.3},{:+.3})  touch={} sclick={}",
                s.left_pad.pos.x,
                s.left_pad.pos.y,
                s.left_stick.x,
                s.left_stick.y,
                s.left_pad.touched as u8,
                s.buttons.contains(Buttons::LSTICK_PRESS) as u8,
            );
            println!("{line}");
            writeln!(log, "{line}").ok();
            log.flush().ok();
        }
    }
    Ok(())
}
