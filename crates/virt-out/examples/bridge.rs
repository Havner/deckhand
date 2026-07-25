//! `bridge` — hardcoded Gordon → virtual devices, with rumble looped back to the
//! controller's trackpad haptics (PLAN §2.1 Phase B). Proves the full
//! `steam-hid` → `virt-out` path *and* the FF back-channel, end to end, with no
//! engine/config.
//!
//! Mapping (per the user's spec):
//! - A/B/X/Y, L1/R1→LB/RB, Menu→Back, Options→Start, Steam→Guide  (1:1)
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

use std::time::{Duration, Instant};

use steam_hid::{Buttons, ControllerState, Device, Manager, Motor, Report, Rumble as HidRumble};
use virt_out::{GamepadAxis, GamepadButton, Key, OutputEvent, Rumble, Sink};

/// Pad-units (−1..1) → pixels per frame of relative mouse motion.
const MOUSE_SCALE: f32 = 900.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manager = Manager::new()?;
    let mut device =
        open_active(&manager)?.ok_or("no Steam controller found — connected and powered on?")?;
    println!("controller: {:?} / {:?}", device.info().kind, device.info().transport);
    if let Err(e) = device.set_lizard_mode(false) {
        eprintln!("warning: couldn't disable lizard mode: {e}");
    }

    let mut sink = Sink::new()?;
    println!(
        "virtual Xbox 360 pad + keyboard + mouse live. Drive the controller; rumble the pad \
         (a game or `fftest`) to feel Gordon buzz. Ctrl-C to stop."
    );

    let mut bridge = Bridge::default();
    let mut last_haptic = Instant::now();
    loop {
        // Gordon input → virtual devices (emit full state each frame; the kernel input
        // core drops unchanged key/abs values, so consumers see only real changes).
        if let Some(Report::State(s)) = device.poll(Duration::from_millis(4))? {
            let events = bridge.frame(&s);
            sink.emit(&events)?;
        }
        // Virtual-pad rumble → Gordon trackpad haptics.
        let rumble = sink.poll_rumble()?;
        apply_haptics(&mut device, &rumble, &mut last_haptic)?;
    }
}

#[derive(Default)]
struct Bridge {
    /// Last right-pad position while touched, for computing relative mouse deltas.
    /// `None` when the finger is up (or mode-shift active), so re-touch doesn't jump.
    prev_rpad: Option<(f32, f32)>,
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
            gp(GamepadButton::LeftBumper, held(Buttons::L1)),
            gp(GamepadButton::RightBumper, held(Buttons::R1)),
            gp(GamepadButton::Back, held(Buttons::MENU)),
            gp(GamepadButton::Start, held(Buttons::OPTIONS)),
            gp(GamepadButton::Guide, held(Buttons::STEAM)),
            gp(GamepadButton::LeftStick, held(Buttons::L4)), // left back grip
            gp(GamepadButton::RightStick, held(Buttons::R4)), // right back grip
            OutputEvent::Key(Key::L, held(Buttons::LSTICK_PRESS)), // stick click → key L
            // triggers (1:1)
            ax(GamepadAxis::LeftTrigger, s.left_trigger),
            ax(GamepadAxis::RightTrigger, s.right_trigger),
            // left pad → dpad hat (quadrant bits only)
            ax(GamepadAxis::DpadX, axis_of(held(Buttons::DPAD_RIGHT), held(Buttons::DPAD_LEFT))),
            ax(GamepadAxis::DpadY, axis_of(held(Buttons::DPAD_DOWN), held(Buttons::DPAD_UP))),
        ];

        // --- left stick → left stick, or (mode-shift) → right stick ---
        // NOTE(sign): pass-through Y; flip here if up/down is inverted in-game.
        let (lx, ly) = (s.left_stick.x, s.left_stick.y);
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

        // --- right pad → relative mouse (suppressed during mode-shift) ---
        if !modeshift && s.right_pad.touched {
            let (x, y) = (s.right_pad.pos.x, s.right_pad.pos.y);
            if let Some((px, py)) = self.prev_rpad {
                let dx = ((x - px) * MOUSE_SCALE) as i32;
                let dy = ((y - py) * -MOUSE_SCALE) as i32; // pad up → cursor up
                if dx != 0 || dy != 0 {
                    ev.push(OutputEvent::MouseMove { dx, dy });
                }
            }
            self.prev_rpad = Some((x, y));
        } else {
            self.prev_rpad = None; // finger up / mode-shift → no jump on next touch
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
/// `+1` if `pos` held, `-1` if `neg` held, else `0` — for a dpad hat axis.
fn axis_of(pos: bool, neg: bool) -> f32 {
    match (pos, neg) {
        (true, false) => 1.0,
        (false, true) => -1.0,
        _ => 0.0,
    }
}

/// Route received rumble to Gordon's trackpad actuators (throttled re-fire while the
/// game commands rumble). ~90 Hz (in Gordon's rumble range, PLAN §1.9) with magnitude
/// mapped onto the duty cycle as a crude amplitude (non-linear — fine-tuning deferred).
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
        device.rumble(Motor::Left, train(rumble.strong))?;
    }
    if rumble.weak > 0 {
        device.rumble(Motor::Right, train(rumble.weak))?;
    }
    Ok(())
}

fn train(magnitude: u16) -> HidRumble {
    const PERIOD_US: u32 = 11_000; // ~90 Hz
    let duty = (magnitude as u32 * PERIOD_US / u16::MAX as u32).clamp(600, PERIOD_US - 600);
    HidRumble {
        duration: duty as u16,
        interval: (PERIOD_US - duty) as u16,
        count: 18, // ~200 ms train, re-fired every 120 ms → continuous while held
        gain: 0,
    }
}

/// Open the first controller slot that actually streams (single/wired → open directly;
/// multiple dongle slots → poll each for a frame). Mirrors the examples' selection.
fn open_active(manager: &Manager) -> steam_hid::Result<Option<Device>> {
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
