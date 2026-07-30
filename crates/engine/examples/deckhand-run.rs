//! `deckhand-run` — the engine's headless test harness (PLAN §4.2 S10).
//!
//! Loads a profile (and optional fallback + globals) from RON, compiles them to `Program`s,
//! and drives the real controller through the full engine: input → Mapper → virtual devices,
//! with rumble looped back. This is the first thing that runs the whole stack end to end; it
//! HW-validates the engine against the `virt-out` bridge.
//!
//! Usage:
//!   cargo run -p engine --example deckhand-run -- [--wired|--dongle] \
//!       PROFILE.ron [--fallback FALLBACK.ron] [--globals GLOBALS.ron]
//!
//! Ctrl-C shuts down cleanly (device → lizard restored, virtual pad unplugged). Set
//! `RUST_LOG=info` for logs.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use config::{ConfigDoc, GlobalConfig};
use engine::{DeviceSelect, Engine, Input, Role, compile};
use steam_hid::Transport;

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    let select = if args.iter().any(|a| a == "--wired") {
        DeviceSelect::Transport(Transport::UsbWired)
    } else if args.iter().any(|a| a == "--dongle") {
        DeviceSelect::Transport(Transport::UsbDongle)
    } else {
        DeviceSelect::Auto
    };
    let profile = positional(&args).ok_or("usage: deckhand-run [--wired|--dongle] PROFILE.ron \
        [--fallback FB.ron] [--globals G.ron]")?;

    let mut engine = Engine::new();
    engine.set_input(Input::Local(select));

    // Active profile (required).
    engine.apply(load_program(&profile)?, Role::Active);
    println!("active profile: {profile}");

    // Optional fallback profile + globals.
    if let Some(path) = flag_value(&args, "--fallback") {
        engine.apply(load_program(&path)?, Role::Fallback);
        println!("fallback profile: {path}");
    }
    if let Some(path) = flag_value(&args, "--globals") {
        let globals: GlobalConfig = ron::from_str(&std::fs::read_to_string(&path)?)?;
        engine.set_globals(globals);
        println!("globals: {path}");
    }

    // Show what's attached, then start.
    for info in engine.devices()? {
        println!("device: {:?} / {:?} {:04x}:{:04x}", info.kind, info.transport, info.vid, info.pid);
    }
    engine.start()?;
    println!("engine running ({:?}). Ctrl-C to stop.", engine.status());

    // Ctrl-C / SIGTERM → fall out and shut down cleanly (runs the runtime teardown).
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || r.store(false, Ordering::Relaxed))?;
    while running.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("\nshutting down — restoring controller and unplugging virtual pad.");
    engine.shutdown()?;
    Ok(())
}

/// Read a RON profile and compile it to a `Program`.
fn load_program(path: &str) -> Result<engine::Program, Box<dyn Error>> {
    let doc: ConfigDoc = ron::from_str(&std::fs::read_to_string(path)?)?;
    Ok(compile(&doc).map_err(engine::Error::Compile)?)
}

/// The first bare positional argument (not a flag, not a flag's value).
fn positional(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--fallback" | "--globals" => i += 2, // skip flag + its value
            a if a.starts_with("--") => i += 1,   // bare flag
            a => return Some(a.to_string()),
        }
    }
    None
}

/// The value following `flag`, if present.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}
