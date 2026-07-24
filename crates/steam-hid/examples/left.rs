//! `left` — dump the raw left-side fields to pin down the pad/stick multiplex.
//!
//! Prints (throttled) the raw `lpad_x/y` (`0x10`), the `w_joy` field (`0x36`), and
//! the `LPAD_TOUCH` / `LPAD_AND_JOY` / `LSTICK_PRESS` flags, so we can see which
//! field carries the stick and how to disambiguate pad vs stick (PLAN §1.4/§1.9).
//! `--raw` disables lizard first. Run: `cargo run -p steam-hid --example left -- [--raw]`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{GordonButtons, Manager, RawReport};

fn main() -> steam_hid::Result<()> {
    let raw = std::env::args().any(|a| a == "--raw");

    let manager = Manager::new()?;
    let devices = manager.enumerate()?;
    if devices.is_empty() {
        println!("No Steam controller gamepad interfaces found.");
        return Ok(());
    }

    let mut active = None;
    for info in &devices {
        let mut device = manager.open(info)?;
        for _ in 0..8 {
            match device.poll_raw(Duration::from_millis(200))? {
                Some(RawReport::Gordon(_)) | Some(RawReport::Connected) => {
                    active = Some(device);
                    break;
                }
                _ => {}
            }
        }
        if active.is_some() {
            break;
        }
    }
    let Some(mut device) = active else {
        println!("No active slot — is the controller powered on?");
        return Ok(());
    };

    if raw {
        match device.set_lizard_mode(false) {
            Ok(()) => println!("raw mode: lizard disabled"),
            Err(e) => eprintln!("warning: could not disable lizard: {e}"),
        }
    }

    let log_path = std::env::temp_dir().join("steam-hid-left.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    println!("Dumping left-side raw fields to {}. Ctrl-C to stop.\n", log_path.display());

    let mut last = Instant::now();
    loop {
        if let Some(RawReport::Gordon(g)) = device.poll_raw(Duration::from_millis(100))?
            && last.elapsed() >= Duration::from_millis(120)
        {
            last = Instant::now();
            let line = format!(
                "pad@0x10=({:>6},{:>6})  joy@0x36=({:>6},{:>6})  touch={} and_joy={} sclick={}",
                g.left_pad.x,
                g.left_pad.y,
                g.left_stick.x,
                g.left_stick.y,
                g.buttons.contains(GordonButtons::LPAD_TOUCH) as u8,
                g.buttons.contains(GordonButtons::LPAD_AND_JOY) as u8,
                g.buttons.contains(GordonButtons::LSTICK_PRESS) as u8,
            );
            println!("{line}");
            writeln!(log, "{line}").ok();
            log.flush().ok();
        }
    }
}
