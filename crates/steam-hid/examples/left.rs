//! `left` — dump the raw left-side fields to pin down the pad/stick multiplex.
//!
//! Prints (throttled) the raw `lpad_x/y` (`0x10`) and `w_joy` (`0x36`) fields plus
//! the `LPAD_TOUCH` / `LPAD_AND_JOY` / `LSTICK_PRESS` flags. On the wired branch the
//! parser reads `0x10`/`0x36` straight, so those columns are the raw offsets — use
//! this to check whether the wired stick really lives at `0x36` (PLAN §1.4/§1.9).
//!
//! Flags: `--raw` disables lizard first; `--wired` / `--dongle` restrict to that
//! transport (useful when both are plugged in).
//! Run: `cargo run -p steam-hid --example left -- [--raw] [--wired|--dongle]`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use steam_hid::{GordonButtons, Manager, RawReport, Transport};

fn main() -> steam_hid::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let raw = args.iter().any(|a| a == "--raw");
    let want = if args.iter().any(|a| a == "--wired") {
        Some(Transport::UsbWired)
    } else if args.iter().any(|a| a == "--dongle") {
        Some(Transport::UsbDongle)
    } else {
        None
    };

    let manager = Manager::new()?;
    let devices = manager.enumerate()?;
    if devices.is_empty() {
        println!("No Steam controller gamepad interfaces found.");
        return Ok(());
    }

    let candidates: Vec<&_> =
        devices.iter().filter(|i| want.as_ref().is_none_or(|t| i.transport == *t)).collect();
    if candidates.is_empty() {
        println!("No matching device for the requested transport.");
        return Ok(());
    }

    let describe = |i: &steam_hid::DeviceInfo| {
        format!("{:?} / {:?} {:04x}:{:04x} iface={}", i.kind, i.transport, i.vid, i.pid, i.interface)
    };

    // One candidate (e.g. wired, or a filtered dongle): use it directly — a wired
    // controller is idle until you move it, so don't gate on a frame. Multiple
    // candidates (dongle slots): pick the one that actually streams.
    let (desc, mut device) = if candidates.len() == 1 {
        let info = candidates[0];
        (describe(info), manager.open(info)?)
    } else {
        let mut chosen = None;
        for info in candidates {
            let mut device = manager.open(info)?;
            for _ in 0..8 {
                if matches!(
                    device.poll_raw(Duration::from_millis(200))?,
                    Some(RawReport::Gordon(_)) | Some(RawReport::Connected)
                ) {
                    chosen = Some((describe(info), device));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        match chosen {
            Some(v) => v,
            None => {
                println!("No active slot streamed input — is the controller on?");
                return Ok(());
            }
        }
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
