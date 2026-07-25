//! `imu` — enable gyro and sample raw accel/gyro to verify the IMU parse.
//!
//! Enables the IMU (raw accel + raw gyro) and prints throttled samples with the
//! raw i16 values and their conversion to g / deg·s⁻¹ (PLAN §1.4 scale constants,
//! provisional). Only sends `set_gyro` (no lizard-off) so it can't leave the
//! controller dead; the IMU setting is restored on clean exit.
//!
//! `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example imu -- [--wired|--dongle]`.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{Manager, Report};

// PLAN §1.4 scale constants (provisional — this is what we're checking).
const ACCEL_RES_PER_G: f32 = 16384.0;
const GYRO_RES_PER_DPS: f32 = 16.0;

fn main() -> steam_hid::Result<()> {
    let manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    if let Err(e) = device.set_gyro(true) {
        eprintln!("warning: could not enable gyro: {e}");
    }

    let log_path = std::env::temp_dir().join("steam-hid-imu.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    writeln!(log, "# selected {desc}").ok();
    println!(
        "gyro enabled. Logging to {}\nHold still (accel ~1g on one axis, gyro ~0), then \
         rotate/tilt one axis at a time. Ctrl-C to stop.\n",
        log_path.display()
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
