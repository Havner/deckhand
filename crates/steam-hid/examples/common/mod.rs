//! Shared helpers for the steam-hid examples.
//!
//! Each example includes this module and uses a subset of it (e.g. one-shot examples skip the
//! Ctrl-C helper), so unused-per-example items are expected.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use steam_hid::{Device, DeviceInfo, Manager, Report, Result, Transport};

/// Shared note tables for the `beep` / `beep-neptune` tune modes as `(freq_hz, ms)`. The notes are
/// device-independent (only the *player* differs: Gordon's `0x8f` pulse vs the Deck's `0xEA` Tone),
/// so both examples read the same tables — keeping `melody`/`vader` identical across them.
///
/// `MELODY`: a plain C-major arpeggio (C5 E5 G5 C6) — the proof-of-pitch tune.
pub const MELODY: &[(u32, u32)] = &[(523, 180), (659, 180), (784, 180), (1047, 300)];

/// `VADER`: the *Imperial March* opening (two phrases). All notes sit in ~310–620 Hz, well inside
/// both the Gordon voice coil's range and the Deck LRA's usable Tone band.
pub const VADER: &[(u32, u32)] = &[
    // G4 G4 G4  Eb4. Bb4  G4  Eb4. Bb4  G4~
    (392, 350), (392, 350), (392, 350), (311, 250), (466, 120), (392, 350), (311, 250), (466, 120), (392, 700),
    // D5 D5 D5  Eb5. Bb4  Gb4 Eb4. Bb4  G4~
    (587, 350), (587, 350), (587, 350), (622, 250), (466, 120), (370, 350), (311, 250), (466, 120), (392, 700),
];

/// Ctrl-C run flag returned by [`install_ctrlc`]. [`alive`](Running::alive) is `true`
/// until the first Ctrl-C; loop examples run `while running.alive()`.
pub struct Running(Arc<AtomicBool>);

impl Running {
    /// `true` until Ctrl-C has been pressed.
    pub fn alive(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Install a Ctrl-C handler and return a [`Running`] flag that flips to `false` on the
/// first Ctrl-C.
///
/// Loop examples run `while running.alive()` so they fall out of the loop and drop their
/// `Device` — running `Drop`, which restores lizard mode (and reverts LED/idle). This
/// matters most on **Windows**, where Ctrl-C otherwise aborts the process *without*
/// running destructors, leaving the controller stuck in lizard-off. (On Linux the kernel
/// `hid-steam` driver re-asserts lizard on close, but relying on that is a Linux-only
/// crutch.) See CLAUDE.md "Clean shutdown".
pub fn install_ctrlc() -> Running {
    let flag = Arc::new(AtomicBool::new(true));
    let f = flag.clone();
    if let Err(e) = ctrlc::set_handler(move || f.store(false, Ordering::Relaxed)) {
        eprintln!("warning: couldn't install Ctrl-C handler: {e}");
    }
    Running(flag)
}

/// Enumerate and open the target controller — the one selection path all examples
/// share.
///
/// Honors `--wired` / `--dongle` / `--bt` args to restrict transport. A single (or
/// explicitly-filtered) candidate is opened **directly**: a wired/Bluetooth
/// controller is idle until you move it, so we must *not* gate on receiving a frame
/// (that gate works on the dongle only because the receiver sends periodic
/// connect/battery frames). With multiple candidates (dongle slots) it polls to pick
/// the one that actually streams. Returns the device and a human description, or `None`.
pub fn select_device(manager: &mut Manager) -> Result<Option<(String, Device)>> {
    let args: Vec<String> = std::env::args().collect();
    let want = if args.iter().any(|a| a == "--wired") {
        Some(Transport::UsbWired)
    } else if args.iter().any(|a| a == "--dongle") {
        Some(Transport::UsbDongle)
    } else if args.iter().any(|a| a == "--bt") {
        Some(Transport::Bluetooth)
    } else {
        None
    };

    let devices = manager.enumerate()?;
    let candidates: Vec<&DeviceInfo> =
        devices.iter().filter(|i| want.as_ref().is_none_or(|t| i.transport == *t)).collect();
    if candidates.is_empty() {
        return Ok(None);
    }

    let describe = |i: &DeviceInfo| {
        format!("{:?} / {:?} {:04x}:{:04x} iface={}", i.kind, i.transport, i.vid, i.pid, i.interface)
    };

    // Unambiguous: open it directly (don't wait for a frame — wired is idle until moved).
    if candidates.len() == 1 {
        let info = candidates[0];
        return Ok(Some((describe(info), manager.open(info)?)));
    }

    // Multiple candidates (dongle slots): pick the one that streams / reports connected.
    for info in candidates {
        let mut device = manager.open(info)?;
        for _ in 0..8 {
            if matches!(
                device.poll(Duration::from_millis(200))?,
                Some(Report::State(_) | Report::Connected)
            ) {
                return Ok(Some((describe(info), device)));
            }
        }
    }
    Ok(None)
}
