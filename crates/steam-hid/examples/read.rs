//! `read` — open the active slot read-only and log input **changes** (events).
//!
//! Safe: sends no lizard-off, so the kernel resumes on close. Prints only changes
//! (via `events()`) to the terminal and appends them to a log file for later
//! inspection. Run: `cargo run -p steam-hid --example read [LOGFILE]`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Duration;

use steam_hid::{Manager, Report};

fn main() -> steam_hid::Result<()> {
    let log_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| std::env::temp_dir().join("steam-hid-read.log").to_string_lossy().into_owned());

    let manager = Manager::new()?;
    let devices = manager.enumerate()?;
    if devices.is_empty() {
        println!("No Steam controller gamepad interfaces found.");
        return Ok(());
    }

    // Find the slot with a controller (streams input, or reports connected).
    let mut active = None;
    for info in &devices {
        let mut device = manager.open(info)?;
        for _ in 0..8 {
            match device.poll(Duration::from_millis(200))? {
                Some(Report::State(_)) | Some(Report::Connected) => {
                    active = Some((info.interface, device));
                    break;
                }
                _ => {}
            }
        }
        if active.is_some() {
            break;
        }
        println!("iface {}: idle", info.interface);
    }

    let Some((iface, mut device)) = active else {
        println!("No active slot — is the controller powered on?");
        return Ok(());
    };

    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    println!(
        "iface {iface} active. Logging changes to:\n  {log_path}\n\n\
         Wake the controller (press Steam), then do your sequence. Ctrl-C to stop.\n"
    );

    for event in device.events() {
        let line = format!("{event:?}");
        println!("{line}");
        writeln!(log, "{line}").ok();
        log.flush().ok();
    }
    Ok(())
}
