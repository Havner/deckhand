//! `imu` — enable gyro and sample raw accel/gyro to verify the IMU parse.
//!
//! Enables the IMU (raw accel + raw gyro) and prints throttled samples with the
//! raw i16 values and their conversion to g / deg·s⁻¹ (PLAN §1.4 scale constants,
//! provisional). Also disables lizard mode so handling the controller doesn't move
//! the host cursor / fire pad click-haptics while you sample; both the IMU setting
//! and lizard mode are restored on clean exit (Drop).
//!
//! `--wired`/`--dongle` pick the transport.
//! Run: `cargo run -p steam-hid --example imu -- [--wired|--dongle]`.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{ACCEL_RES_PER_G, GYRO_RES_PER_DPS, Manager, Report};

/// Which of the three channels dominates, with its sign — e.g. `+Z`. Returns `~0`
/// when the largest channel is below `noise` (nothing meaningfully happening).
fn dominant(vals: [(i32, &'static str); 3], noise: i32) -> String {
    let (v, label) = vals.iter().copied().max_by_key(|(v, _)| v.abs()).unwrap();
    if v.abs() < noise {
        return "~0".to_string();
    }
    format!("{}{label}", if v >= 0 { '+' } else { '-' })
}

fn main() -> steam_hid::Result<()> {
    let manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    if let Err(e) = device.set_lizard_mode(false) {
        eprintln!("warning: could not disable lizard mode: {e}");
    }
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

    let running = common::install_ctrlc();
    let start = Instant::now();
    let mut last = Instant::now();
    while running.alive() {
        if let Some(Report::State(s)) = device.poll(Duration::from_millis(200))?
            && last.elapsed() >= Duration::from_millis(200)
        {
            last = Instant::now();
            let t = start.elapsed().as_secs_f32();
            let (a, g) = (&s.accel, &s.gyro);
            // Dominant-axis hints: gravity always drives one accel axis (noise ≈ 0.5g raw);
            // gyro only when actually turning (noise ≈ 30°/s raw). Frame is the verified
            // right-handed X=right, Y=forward, Z=up (PLAN §1.9); gyro is pitch/roll/yaw.
            let accel_dom = dominant(
                [(a.x as i32, "X"), (a.y as i32, "Y"), (a.z as i32, "Z")],
                (0.5 * ACCEL_RES_PER_G) as i32,
            );
            let gyro_dom = dominant(
                [(g.x as i32, "X=pitch"), (g.y as i32, "Y=roll"), (g.z as i32, "Z=yaw")],
                (30.0 * GYRO_RES_PER_DPS) as i32,
            );
            let line = format!(
                "t={t:>5.1}  accel raw=({:>6},{:>6},{:>6}) g=({:+.2},{:+.2},{:+.2}) [{:>2}]  |  \
                     gyro raw=({:>6},{:>6},{:>6}) dps=({:+.0},{:+.0},{:+.0}) [{:>7}]",
                a.x,
                a.y,
                a.z,
                a.x as f32 / ACCEL_RES_PER_G,
                a.y as f32 / ACCEL_RES_PER_G,
                a.z as f32 / ACCEL_RES_PER_G,
                accel_dom,
                g.x,
                g.y,
                g.z,
                g.x as f32 / GYRO_RES_PER_DPS,
                g.y as f32 / GYRO_RES_PER_DPS,
                g.z as f32 / GYRO_RES_PER_DPS,
                gyro_dom,
            );
            println!("{line}");
            writeln!(log, "{line}").ok();
            log.flush().ok();
        }
    }
    Ok(())
}
