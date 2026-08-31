//! `battery` — report Gordon battery status from the `0x04` frame.
//!
//! The dongle sends battery `0x04` frames periodically; this also actively prompts
//! one with a wireless-state request. Safe (no lizard-off). Wired Gordon is
//! USB-powered and sends no `0x04` frame, so this is a dongle-only diagnostic.
//!
//! `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example battery -- [--wired|--dongle]`.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{Manager, Report};

fn main() -> steam_hid::Result<()> {
    let mut manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    let log_path = std::env::temp_dir().join("steam-hid-battery.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    writeln!(log, "# selected {desc}").ok();
    println!(
        "Battery status from 0x04 frames (logging to {}). Ctrl-C to stop.\n",
        log_path.display()
    );
    let mut last_request = Instant::now();

    let running = common::install_ctrlc();
    while running.alive() {
        // Actively prompt an 0x04 status frame every few seconds.
        if last_request.elapsed() >= Duration::from_secs(3) {
            last_request = Instant::now();
            device.dongle_get_wireless_state().ok();
        }

        let line = match device.poll(Duration::from_millis(500))? {
            Some(Report::Battery(b)) => format!("{} mV, {}%", b.voltage_mv, b.charge_percent),
            Some(Report::Connected) => "[connected]".to_string(),
            Some(Report::Disconnected) => "[disconnected]".to_string(),
            _ => continue,
        };
        println!("{line}");
        writeln!(log, "{line}").ok();
        log.flush().ok();
    }
    Ok(())
}
