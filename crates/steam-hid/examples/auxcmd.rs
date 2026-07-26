//! `auxcmd` — smoke-test the auxiliary output commands (LED, idle timeout, power off).
//! (Named `auxcmd`, not `aux`: `aux` is a reserved device name on Windows.)
//!
//! These have no readable response — verification is **behavioral** (watch the LED,
//! watch it auto-power-off, watch it power off). Each subcommand fires the command
//! then keeps the device open (Drop restores lizard/defaults, which reverts LED &
//! idle, so we must stay alive to observe) and logs lifecycle frames with timestamps.
//!
//! `--wired`/`--dongle` pick the transport. Run:
//!   `cargo run -p steam-hid --example auxcmd -- [--dongle] led 0`     (0..=100 %)
//!   `cargo run -p steam-hid --example auxcmd -- [--dongle] idle 30`   (seconds; 0 disables)
//!   `cargo run -p steam-hid --example auxcmd -- [--dongle] off`
//! Ctrl-C to stop.

mod common;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{Manager, Report};

fn main() -> steam_hid::Result<()> {
    // Positional (non-flag) args: subcommand + optional value.
    let positional: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let Some(sub) = positional.first().map(String::as_str) else {
        eprintln!("usage: auxcmd [--wired|--dongle] <led N | idle SECS | off>");
        return Ok(());
    };

    let manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    let value = positional.get(1).map(String::as_str);
    let banner = match sub {
        "led" => {
            let percent: u8 = value.and_then(|v| v.parse().ok()).unwrap_or(100);
            device.set_led_intensity(percent)?;
            format!("LED set to {percent}% — watch the Steam-button LED (reverts on exit)")
        }
        "idle" => {
            let secs: u16 = value.and_then(|v| v.parse().ok()).unwrap_or(30);
            device.set_idle_timeout(secs)?;
            format!(
                "idle timeout set to {secs}s — leave the controller UNTOUCHED; it should \
                 power off (look for a [disconnected] line ~{secs}s from now)"
            )
        }
        "off" => {
            device.power_off()?;
            "power-off sent — expect an immediate [disconnected]".to_string()
        }
        other => {
            eprintln!("unknown subcommand {other:?}; use: led N | idle SECS | off");
            return Ok(());
        }
    };

    let log_path = std::env::temp_dir().join("steam-hid-auxcmd.log");
    let mut log = BufWriter::new(File::create(&log_path).expect("create log file"));
    writeln!(log, "# selected {desc}").ok();
    writeln!(log, "# cmd: {sub} {}", value.unwrap_or("")).ok();
    println!("{banner}\nLogging lifecycle to {}. Ctrl-C to stop.\n", log_path.display());

    // Keep the device open (so Drop's revert doesn't fire) and log lifecycle frames.
    let running = common::install_ctrlc();
    let start = Instant::now();
    while running.alive() {
        let Some(report) = device.poll(Duration::from_millis(500))? else {
            continue; // timeout — dongle alive, nothing this interval
        };
        let signal = match &report {
            Report::Connected => "[connected]",
            Report::Disconnected => "[disconnected]",
            Report::Battery(_) => "[battery]",
            _ => continue, // input state — ignore
        };
        let line = format!("t={:>6.1}  {signal}", start.elapsed().as_secs_f32());
        println!("{line}");
        writeln!(log, "{line}").ok();
        log.flush().ok();

        // For `off`/`idle`, the controller powering off (a Disconnected *value* — the
        // dongle transport is still alive, so this is not a read error) is the terminal
        // success signal, so stop instead of lingering. `led` keeps running (visual).
        if matches!(report, Report::Disconnected) && matches!(sub, "off" | "idle") {
            println!("controller powered off — done.");
            return Ok(());
        }
    }
    Ok(())
}
