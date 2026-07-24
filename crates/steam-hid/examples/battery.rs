//! `battery` — report Gordon battery status from the `0x04` frame.
//!
//! The dongle sends battery `0x04` frames periodically; this also actively prompts
//! one with a wireless-state request. Safe (no lizard-off).
//! Run: `cargo run -p steam-hid --example battery`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{Manager, RawReport};

fn main() -> steam_hid::Result<()> {
    let manager = Manager::new()?;
    let devices = manager.enumerate()?;
    if devices.is_empty() {
        println!("No Steam controller gamepad interfaces found.");
        return Ok(());
    }

    // Find the active slot (streams input or reports connected).
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

    let log_path = std::env::temp_dir().join("steam-hid-battery.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    println!(
        "Battery status from 0x04 frames (logging to {}). Ctrl-C to stop.\n",
        log_path.display()
    );
    let mut last_request = Instant::now();

    loop {
        // Actively prompt an 0x04 status frame every few seconds.
        if last_request.elapsed() >= Duration::from_secs(3) {
            last_request = Instant::now();
            device.send_feature_report(&[0xB4, 0x00]).ok(); // DONGLE_GET_WIRELESS_STATE
        }

        let line = match device.poll_raw(Duration::from_millis(500))? {
            Some(RawReport::Battery(b)) => format!("{} mV, {}%", b.voltage_mv, b.charge_percent),
            Some(RawReport::Connected) => "[connected]".to_string(),
            Some(RawReport::Disconnected) => "[disconnected]".to_string(),
            _ => continue,
        };
        println!("{line}");
        writeln!(log, "{line}").ok();
        log.flush().ok();
    }
}
