//! `bridge` — hardcoded Gordon → virtual devices, with rumble looped back to the
//! controller's trackpad haptics (PLAN §2.1 Phase B). Proves the full
//! `steam-hid` → `virt-out` path *and* the FF back-channel, end to end, with no
//! engine/config.
//!
//! Mapping (per the user's spec):
//! - A/B/X/Y, L1/R1→LB/RB, View→Back, Menu→Start, Steam→Guide  (1:1)
//! - L4 (left back grip)  → left-stick click
//! - R4 (right back grip) → right-stick click
//! - left-stick click     → keyboard `L`
//! - left pad             → dpad hat (quadrant bits only; pad x/y ignored)
//! - triggers             → LT/RT
//! - left stick           → left stick (but → RIGHT stick while the right pad is clicked;
//!   mode-shift, left stick centres)
//! - right pad            → relative mouse (suppressed during the mode-shift; the
//!   right-pad click itself emits nothing)
//! - rumble back: strong/heavy → left pad, weak/light → right pad (duty-scaled)
//!
//! Disables lizard so the controller feeds raw input only (no doubled mouse/keyboard).
//! Run: `cargo run -p virt-out --example bridge`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use steam_hid::{
    Buttons, ControllerState, Device, HapticPulse as HidRumble, Manager, HapticPosition, Report,
};
use virt_out::{GamepadAxis, GamepadButton, Key, OutputEvent, Rumble, Sink};

/// Pad-units (−1..1) → pixels per frame of relative mouse motion.
const MOUSE_SCALE: f32 = 300.0;
/// Simplest possible acceleration: output is scaled by `1 + speed·MOUSE_ACCEL`, where
/// `speed` is the per-frame pad-delta magnitude — slow moves stay ~1:1, fast swipes go
/// proportionally farther. (A proper time-based curve belongs in the engine.)
const MOUSE_ACCEL: f32 = 12.0;

/// Gyro-aim: raw gyro units → pixels (small), the left-trigger threshold that enables it,
/// and a small **radial** deadzone (px) on the per-frame gyro delta. Gordon's gyro has a
/// small DC bias (stationary raw isn't zero-mean, §1.9) that integrates into a slow cursor
/// drift; a tiny deadzone (~0.1 px) removes it completely while sitting far below real aiming
/// motion, so fine movement is unaffected. (The accumulator integrates faithfully — it does
/// not filter — so this deadzone, not the accumulator, is what suppresses the drift.)
const GYRO_SENS: f32 = 0.007;
const GYRO_TRIGGER: f32 = 0.90;
const GYRO_DEADZONE_PX: f32 = 0.1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut manager = Manager::new()?;
    let mut device =
        open_active(&mut manager)?.ok_or("no Steam controller found — connected and powered on?")?;
    println!("controller: {:?} / {:?}", device.info().kind, device.info().transport);
    if let Err(e) = device.set_lizard_mode(false) {
        eprintln!("warning: couldn't disable lizard mode: {e}");
    }
    if let Err(e) = device.set_gyro(true) {
        eprintln!("warning: couldn't enable gyro: {e}");
    }

    let mut sink = Sink::new()?;
    println!(
        "virtual Xbox 360 pad + keyboard + mouse live. Drive the controller; rumble the pad \
         (a game or `fftest`) to feel Gordon buzz. Ctrl-C to stop."
    );

    // Catch Ctrl-C so we break the loop and return — running Drop, which restores Gordon's
    // lizard mode and unplugs the virtual pad. On Windows, Ctrl-C otherwise aborts the
    // process without running destructors, leaving the controller stuck in lizard-off.
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || r.store(false, Ordering::Relaxed))?;

    let mut bridge = Bridge::default();
    let mut last_haptic = Instant::now();
    while running.load(Ordering::Relaxed) {
        // Gordon input → virtual devices (emit full state each frame; the kernel input
        // core drops unchanged key/abs values, so consumers see only real changes).
        match device.poll(Duration::from_millis(4))? {
            Some(Report::State(s)) => {
                let events = bridge.frame(&s);
                sink.emit(&events)?;
            }
            // The controller resets its config when it re-joins the dongle, so re-apply
            // on every (re)connect. (In the real product this lives in the engine's
            // connect-handling — steam-hid stays config-agnostic; PLAN §1.9/§4.)
            Some(Report::Connected) => {
                let r = device.set_lizard_mode(false).and_then(|()| device.set_gyro(true));
                match r {
                    Ok(()) => println!("controller reconnected — config re-applied"),
                    Err(e) => eprintln!("warning: couldn't re-apply config on reconnect: {e}"),
                }
            }
            _ => {}
        }
        // Virtual-pad rumble → Gordon trackpad haptics.
        let rumble = sink.poll_rumble()?;
        apply_haptics(&mut device, &rumble, &mut last_haptic)?;
    }

    // Ctrl-C: fall out of the loop so `device` and `sink` drop here — restoring Gordon's
    // lizard mode and unplugging the virtual pad before we exit.
    println!("\nshutting down — restoring controller and unplugging virtual pad.");
    Ok(())
}

#[derive(Default)]
struct Bridge {
    /// Last right-pad position while touched, for computing relative mouse deltas.
    /// `None` when the finger is up (or mode-shift active), so re-touch doesn't jump.
    prev_rpad: Option<(f32, f32)>,
    /// Sub-pixel remainder carried between frames: we emit only whole pixels and keep
    /// the fraction, so slow/small movement isn't truncated away (feels much better).
    mouse_acc: (f32, f32),
}

impl Bridge {
    fn frame(&mut self, s: &ControllerState) -> Vec<OutputEvent> {
        let b = &s.buttons;
        let held = |f: Buttons| b.contains(f);
        let modeshift = held(Buttons::RPAD_PRESS); // right-pad click → left→right-stick

        let mut ev = vec![
            // buttons (1:1 + remaps)
            gp(GamepadButton::A, held(Buttons::A)),
            gp(GamepadButton::B, held(Buttons::B)),
            gp(GamepadButton::X, held(Buttons::X)),
            gp(GamepadButton::Y, held(Buttons::Y)),
            gp(GamepadButton::LeftBumper, held(Buttons::LB)),
            gp(GamepadButton::RightBumper, held(Buttons::RB)),
            gp(GamepadButton::Back, held(Buttons::VIEW)),
            gp(GamepadButton::Start, held(Buttons::MENU)),
            gp(GamepadButton::Guide, held(Buttons::STEAM)),
            gp(GamepadButton::LeftStick, held(Buttons::LGRIP)), // left back grip
            gp(GamepadButton::RightStick, held(Buttons::RGRIP)), // right back grip
            OutputEvent::Key(Key::L, held(Buttons::LSTICK_PRESS)), // stick click → key L
            // triggers (1:1)
            ax(GamepadAxis::LeftTrigger, s.left_trigger),
            ax(GamepadAxis::RightTrigger, s.right_trigger),
            // left pad → dpad buttons (quadrant bits only; virt-out folds them into the hat)
            gp(GamepadButton::DpadUp, held(Buttons::DPAD_UP)),
            gp(GamepadButton::DpadDown, held(Buttons::DPAD_DOWN)),
            gp(GamepadButton::DpadLeft, held(Buttons::DPAD_LEFT)),
            gp(GamepadButton::DpadRight, held(Buttons::DPAD_RIGHT)),
        ];

        // --- left stick → left stick, or (mode-shift) → right stick ---
        // Y is inverted vs. the Xbox convention (up = negative), so negate it (covers
        // both the left-stick and mode-shift right-stick paths, which share `ly`).
        let (lx, ly) = (s.left_stick.x, -s.left_stick.y);
        if modeshift {
            ev.push(ax(GamepadAxis::RightStickX, lx));
            ev.push(ax(GamepadAxis::RightStickY, ly));
            ev.push(ax(GamepadAxis::LeftStickX, 0.0));
            ev.push(ax(GamepadAxis::LeftStickY, 0.0));
        } else {
            ev.push(ax(GamepadAxis::LeftStickX, lx));
            ev.push(ax(GamepadAxis::LeftStickY, ly));
            ev.push(ax(GamepadAxis::RightStickX, 0.0));
            ev.push(ax(GamepadAxis::RightStickY, 0.0));
        }

        // --- mouse: right pad (relative) + gyro (rate) → shared sub-pixel accumulator ---
        // Right pad → relative mouse (suppressed during mode-shift).
        if !modeshift && s.right_pad.touched {
            let (x, y) = (s.right_pad.pos.x, s.right_pad.pos.y);
            if let Some((px, py)) = self.prev_rpad {
                let (rx, ry) = (x - px, y - py);
                let accel = 1.0 + rx.hypot(ry) * MOUSE_ACCEL; // fast swipes travel farther
                self.mouse_acc.0 += rx * MOUSE_SCALE * accel;
                self.mouse_acc.1 += ry * -MOUSE_SCALE * accel; // pad up → cursor up
            }
            self.prev_rpad = Some((x, y));
        } else {
            self.prev_rpad = None; // finger up / mode-shift → no jump on next touch
        }

        // Gyro → mouse, only while the left trigger is held ≥ 90%: yaw → horizontal,
        // pitch → vertical. The radial deadzone gate skips a gyro frame whose delta is
        // smaller than `GYRO_DEADZONE_PX` (it never reaches the accumulator) — a tiny value
        // kills the small resting-bias drift without touching real aiming motion.
        // NOTE(sign): flip either term if the axis feels inverted.
        if s.left_trigger >= GYRO_TRIGGER {
            let gdx = -(s.gyro.z as f32) * GYRO_SENS; // yaw-left → cursor left
            let gdy = (s.gyro.x as f32) * GYRO_SENS; // pitch-up → cursor down (inverted)
            if gdx.hypot(gdy) >= GYRO_DEADZONE_PX {
                self.mouse_acc.0 += gdx;
                self.mouse_acc.1 += gdy;
            }
        }

        // Emit whole pixels; carry the sub-pixel remainder forward (small moves preserved).
        let dx = self.mouse_acc.0.trunc() as i32;
        let dy = self.mouse_acc.1.trunc() as i32;
        self.mouse_acc.0 -= dx as f32;
        self.mouse_acc.1 -= dy as f32;
        if dx != 0 || dy != 0 {
            ev.push(OutputEvent::MouseMove { dx, dy });
        }

        ev
    }
}

fn gp(b: GamepadButton, down: bool) -> OutputEvent {
    OutputEvent::GamepadButton(b, down)
}
fn ax(a: GamepadAxis, v: f32) -> OutputEvent {
    OutputEvent::GamepadAxis(a, v)
}

/// Route received rumble to Gordon's trackpad actuators (throttled re-fire while the
/// game commands rumble). ~60 Hz (in Gordon's rumble range, PLAN §1.9) with magnitude
/// mapped onto the duty cycle as a crude amplitude (non-linear — a response curve is a
/// possible later tweak).
fn apply_haptics(
    device: &mut Device,
    rumble: &Rumble,
    last: &mut Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    if rumble.is_zero() || last.elapsed() < Duration::from_millis(120) {
        return Ok(());
    }
    *last = Instant::now();
    if rumble.strong > 0 {
        device.haptic_pulse(HapticPosition::Left, train(rumble.strong))?;
    }
    if rumble.weak > 0 {
        device.haptic_pulse(HapticPosition::Right, train(rumble.weak))?;
    }
    Ok(())
}

fn train(magnitude: u16) -> HidRumble {
    const PERIOD_US: u32 = 16_667; // ~60 Hz
    const STRENGTH: f32 = 0.5; // scale amplitude (duty cycle) to 50%
    // Magnitude 0..65535 → duty cycle (our amplitude lever), linear × STRENGTH.
    let full = magnitude as f32 / u16::MAX as f32;
    let duty = ((full * STRENGTH * PERIOD_US as f32) as u32).clamp(600, PERIOD_US - 600);
    HidRumble {
        duration: duty as u16,
        interval: (PERIOD_US - duty) as u16,
        count: 12, // ~200 ms train, re-fired every 120 ms → continuous while held
        gain: 0,
    }
}

/// Open the first controller slot that actually streams (single/wired → open directly;
/// multiple dongle slots → poll each for a frame). Mirrors the examples' selection.
fn open_active(manager: &mut Manager) -> steam_hid::Result<Option<Device>> {
    let infos = manager.enumerate()?;
    if infos.len() == 1 {
        return Ok(Some(manager.open(&infos[0])?));
    }
    for info in &infos {
        let mut device = manager.open(info)?;
        for _ in 0..8 {
            if matches!(
                device.poll(Duration::from_millis(200))?,
                Some(Report::State(_)) | Some(Report::Connected)
            ) {
                return Ok(Some(device));
            }
        }
    }
    Ok(None)
}
