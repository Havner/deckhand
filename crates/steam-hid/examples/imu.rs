//! `imu` — enable gyro and sample raw accel/gyro to verify the IMU parse.
//!
//! Enables the IMU (raw accel + raw gyro) and prints throttled samples with the
//! raw i16 values and their conversion to g / deg·s⁻¹ (PLAN §1.4 scale constants,
//! provisional). Only sends `set_gyro` (no lizard-off) so it can't leave the
//! controller dead; the IMU setting is restored on clean exit.
//! Run: `cargo run -p steam-hid --example imu`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{Manager, Report};

// PLAN §1.4 scale constants (provisional — this is what we're checking).
const ACCEL_RES_PER_G: f32 = 16384.0;
const GYRO_RES_PER_DPS: f32 = 16.0;

fn main() -> steam_hid::Result<()> {
    let manager = Manager::new()?;
    let devices = manager.enumerate()?;
    if devices.is_empty() {
        println!("No Steam controller gamepad interfaces found.");
        return Ok(());
    }

    // Find the active slot.
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
    }
    let Some((iface, mut device)) = active else {
        println!("No active slot — is the controller powered on?");
        return Ok(());
    };

    let log_path = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::temp_dir()
            .join("steam-hid-imu.log")
            .to_string_lossy()
            .into_owned()
    });
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));

    if let Err(e) = device.set_gyro(true) {
        eprintln!("warning: could not enable gyro: {e}");
    }
    println!(
        "iface {iface}: gyro enabled. Logging to {log_path}\nHold still (accel ~1g on \
         one axis, gyro ~0), then rotate/tilt one axis at a time. Ctrl-C to stop.\n"
    );

    let mut last = Instant::now();
    loop {
        if let Some(Report::State(s)) = device.poll(Duration::from_millis(200))?
            && last.elapsed() >= Duration::from_millis(200)
        {
            last = Instant::now();
            let (a, g) = (&s.accel, &s.gyro);
            let line = format!(
                "accel raw=({:>6},{:>6},{:>6}) g=({:+.2},{:+.2},{:+.2})  |  \
                     gyro raw=({:>6},{:>6},{:>6}) dps=({:+.0},{:+.0},{:+.0})",
                a.x,
                a.y,
                a.z,
                a.x as f32 / ACCEL_RES_PER_G,
                a.y as f32 / ACCEL_RES_PER_G,
                a.z as f32 / ACCEL_RES_PER_G,
                g.x,
                g.y,
                g.z,
                g.x as f32 / GYRO_RES_PER_DPS,
                g.y as f32 / GYRO_RES_PER_DPS,
                g.z as f32 / GYRO_RES_PER_DPS,
            );
            println!("{line}");
            writeln!(log, "{line}").ok();
            log.flush().ok();
        }
    }
}
