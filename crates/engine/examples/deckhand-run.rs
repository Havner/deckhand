//! `deckhand-run` — the engine's headless test harness (PLAN §4.2 S10).
//!
//! Loads a profile (and optional fallback + globals) from RON, compiles them to `Program`s,
//! and drives the real controller through the full engine: input → Mapper → virtual devices,
//! with rumble looped back. This is the first thing that runs the whole stack end to end; it
//! HW-validates the engine against the `virt-out` bridge.
//!
//! Generate profiles with `cargo run -p config --example example_profiles <dir>`, then e.g.:
//!   cargo run -p engine --example deckhand-run -- --dongle <dir>/game_profile.ron \
//!       --fallback <dir>/desktop_profile.ron --globals <dir>/globals.ron
//!
//! With the example globals, holding **Steam + RightGrip** toggles main↔fallback. Ctrl-C
//! shuts down cleanly (device → lizard restored, virtual pad unplugged). `-v`/`-vv`/`-vvv`
//! (or `RUST_LOG`) raise log verbosity from the default (warn) to info/debug/trace.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::Parser;
use config::{ConfigDoc, GlobalConfig};
use engine::{DeviceSelect, Engine, Input, Role, compile};
use steam_hid::Transport;

/// Drive the deckhand engine from RON profiles against a real Steam controller.
#[derive(Parser)]
#[command(name = "deckhand-run", version, about)]
struct Args {
    /// Main profile (RON) — the mapping that runs on start.
    profile: PathBuf,
    /// Optional fallback profile (RON) — swapped to by a SwitchFallback chord.
    #[arg(short, long, value_name = "RON")]
    fallback: Option<PathBuf>,
    /// Optional global config (RON) — master rumble + chords.
    #[arg(short, long, value_name = "RON")]
    globals: Option<PathBuf>,
    /// Restrict to the wired controller.
    #[arg(short, long, conflicts_with = "dongle")]
    wired: bool,
    /// Restrict to the wireless dongle.
    #[arg(short, long)]
    dongle: bool,
    /// Increase log verbosity: -v info, -vv debug, -vvv trace (default: warn). `RUST_LOG` overrides.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

impl Args {
    /// The default log filter for the `-v` count (RUST_LOG, if set, takes precedence).
    fn log_level(&self) -> &'static str {
        match self.verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }
    }
}

impl Args {
    fn device_select(&self) -> DeviceSelect {
        if self.wired {
            DeviceSelect::Transport(Transport::UsbWired)
        } else if self.dongle {
            DeviceSelect::Transport(Transport::UsbDongle)
        } else {
            DeviceSelect::Auto
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(args.log_level()))
        .init();

    let mut engine = Engine::new();
    engine.set_input(Input::Local(args.device_select()));

    // Main profile (required).
    engine.apply(load_program(&args.profile)?, Role::Main);
    println!("main profile: {}", args.profile.display());

    // Optional fallback profile + globals (the SwitchFallback chord in globals swaps to it).
    if let Some(path) = &args.fallback {
        engine.apply(load_program(path)?, Role::Fallback);
        println!("fallback profile: {}", path.display());
    }
    if let Some(path) = &args.globals {
        let globals: GlobalConfig = ron::from_str(&std::fs::read_to_string(path)?)?;
        engine.set_globals(globals);
        println!("globals: {}", path.display());
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
fn load_program(path: &Path) -> Result<engine::Program, Box<dyn Error>> {
    let doc: ConfigDoc = ron::from_str(&std::fs::read_to_string(path)?)?;
    Ok(compile(&doc).map_err(engine::Error::Compile)?)
}
