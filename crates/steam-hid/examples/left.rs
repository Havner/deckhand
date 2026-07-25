//! `left` — dump the resolved left pad/stick + flags (throttled), for debugging.
//!
//! The pad and analog stick share `0x10`, disambiguated by `LPAD_TOUCH` (see
//! `report::parse_gordon`); this prints the parsed result (raw i16, not deadbanded
//! like `read`'s events) so you can watch the multiplex resolve live.
//!
//! `--raw` disables lizard first; `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example left -- [--raw] [--wired|--dongle]`.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{GordonButtons, Manager, RawReport};

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
            Ok(()) => println!("raw mode: lizard disabled"),
            Err(e) => eprintln!("warning: could not disable lizard: {e}"),
        }
    }

    let log_path = std::env::temp_dir().join("steam-hid-left.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    writeln!(log, "# selected {desc}").ok();
    println!("Dumping resolved left pad/stick to {}. Ctrl-C to stop.\n", log_path.display());

    let mut last = Instant::now();
    loop {
        if let Some(RawReport::Gordon(g)) = device.poll_raw(Duration::from_millis(100))?
            && last.elapsed() >= Duration::from_millis(120)
        {
            last = Instant::now();
            let line = format!(
                "pad=({:>6},{:>6})  stick=({:>6},{:>6})  touch={} sclick={}",
                g.left_pad.x,
                g.left_pad.y,
                g.left_stick.x,
                g.left_stick.y,
                g.buttons.contains(GordonButtons::LPAD_TOUCH) as u8,
                g.buttons.contains(GordonButtons::LSTICK_PRESS) as u8,
            );
            println!("{line}");
            writeln!(log, "{line}").ok();
            log.flush().ok();
        }
    }
}
