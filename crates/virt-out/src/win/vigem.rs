//! ViGEm controller backend: a virtual Xbox 360 pad through the ViGEmBus driver (the
//! `vigem-client` crate). Gated behind the `vigem` feature (on by default). Advertises the
//! standard X360 identity so games see a normal Xbox pad, and receives game rumble back over
//! ViGEm's notification channel (PLAN 2.1 FF back-channel).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread::JoinHandle;

use vigem_client::{Client, TargetId, XButtons, XGamepad, XTarget};

use super::ControllerBackend;
use crate::event::{AxisButtons, Dpad, Rumble};
use vocab_out::{GamepadAxis, GamepadButton};

/// Latest rumble from the virtual pad, written by the ViGEm notification thread and read by
/// [`VigemController::poll_rumble`]. ViGEm reports motor speeds as `u8` (the high byte of the
/// XInput `u16`); we widen back on read.
struct RumbleState {
    strong: AtomicU8, // large / low-frequency motor
    weak: AtomicU8,   // small / high-frequency motor
}

/// A virtual Xbox 360 pad backed by ViGEmBus. Owns the target, the report we resubmit on
/// change, dpad-hat state, and the rumble back-channel. Dropping it unplugs the pad and stops
/// the notification thread.
pub(crate) struct VigemController {
    target: XTarget,
    // The full gamepad report we resubmit whenever any gamepad input changes. `buttons` holds
    // only the non-dpad bits; the dpad hat is kept separately and OR'd in at flush (both live
    // in the same XInput button word).
    gamepad: XGamepad,
    // Dpad direction state, folded into the XInput hat bits at flush.
    dpad: Dpad,
    // Full-trigger / stick-direction pseudo-buttons, folded into the stick/trigger axes.
    axis_buttons: AxisButtons,
    // Set by set_button/set_axis; cleared on flush - avoids resubmitting an unchanged report.
    dirty: bool,
    // Rumble back-channel: a notification thread stores the latest motor speeds here.
    rumble: Arc<RumbleState>,
    notif: Option<JoinHandle<()>>,
}

impl VigemController {
    /// Write a (combined) axis value into the report, translating the shared evdev-signed vocab to
    /// XInput (sticks are +up -> negate Y; triggers `0..1`). Shared by `set_axis` and the axis
    /// pseudo-buttons (`AxisButtons`), so both go through the identical conversion.
    fn write_axis(&mut self, a: &GamepadAxis, v: f32) {
        match a {
            GamepadAxis::LeftStickX => self.gamepad.thumb_lx = stick(v),
            GamepadAxis::LeftStickY => self.gamepad.thumb_ly = stick(-v),
            GamepadAxis::RightStickX => self.gamepad.thumb_rx = stick(v),
            GamepadAxis::RightStickY => self.gamepad.thumb_ry = stick(-v),
            GamepadAxis::LeftTrigger => self.gamepad.left_trigger = trigger(v),
            GamepadAxis::RightTrigger => self.gamepad.right_trigger = trigger(v),
        }
    }
}

impl ControllerBackend for VigemController {
    /// Connect to ViGEmBus, plug in a virtual Xbox 360 pad, and start the rumble notification
    /// thread. Fails if the ViGEmBus driver isn't installed/running.
    fn new() -> crate::Result<Self> {
        let client = Client::connect()?;
        let mut target = XTarget::new(client, TargetId::XBOX360_WIRED);
        target.plugin()?;
        target.wait_ready()?;

        let rumble = Arc::new(RumbleState {
            strong: AtomicU8::new(0),
            weak: AtomicU8::new(0),
        });
        // The notification thread blocks on ViGEm and stores each rumble update. It exits when
        // the target is unplugged (drop), which aborts its pending request.
        let sink_rumble = Arc::clone(&rumble);
        let notif = target.request_notification()?.spawn_thread(move |_, data| {
            sink_rumble.strong.store(data.large_motor, Ordering::Relaxed);
            sink_rumble.weak.store(data.small_motor, Ordering::Relaxed);
        });

        log::info!("virt-out(win): controller backend = vigem (virtual Xbox 360 pad)");
        Ok(Self {
            target,
            gamepad: XGamepad::default(),
            dpad: Dpad::default(),
            axis_buttons: AxisButtons::default(),
            dirty: false,
            rumble,
            notif: Some(notif),
        })
    }

    fn set_button(&mut self, b: &GamepadButton, down: bool) {
        if self.dpad.set(b, down) {
            // dpad -> hat bits, folded in at flush
        } else if let Some(axis) = self.axis_buttons.set_button(b, down) {
            // Full-trigger / stick-direction pseudo-button -> drive its axis to the combined value.
            let vc = self.axis_buttons.value(&axis);
            self.write_axis(&axis, vc);
        } else {
            set_button_bit(&mut self.gamepad.buttons, b, down);
        }
        self.dirty = true;
    }

    fn set_axis(&mut self, a: &GamepadAxis, v: f32) {
        // Cache the analog value and write it combined with any held axis-button (which overrides).
        self.axis_buttons.set_analog(a, v);
        let vc = self.axis_buttons.value(a);
        self.write_axis(a, vc);
        self.dirty = true;
    }

    fn flush(&mut self) -> crate::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        // Fold the dpad hat into the button word alongside the face/shoulder bits.
        let raw =
            (self.gamepad.buttons.raw & !DPAD_MASK) | dpad_bits((self.dpad.x(), self.dpad.y()));
        self.gamepad.buttons = XButtons { raw };
        self.target.update(&self.gamepad)?;
        self.dirty = false;
        Ok(())
    }

    fn poll_rumble(&mut self) -> crate::Result<Rumble> {
        // ViGEm delivers each motor as the high byte of the XInput u16; widen by x257 so 0xFF
        // maps to 0xFFFF (full scale) rather than 0xFF00.
        let widen = |v: u8| (v as u16) * 257;
        Ok(Rumble {
            strong: widen(self.rumble.strong.load(Ordering::Relaxed)),
            weak: widen(self.rumble.weak.load(Ordering::Relaxed)),
        })
    }
}

impl Drop for VigemController {
    fn drop(&mut self) {
        // Unplug first: this aborts the notification thread's pending request so its blocking
        // poll returns and the thread exits; then join it. (The target's own Drop would unplug
        // too, but we need the unplug *before* the join.)
        let _ = self.target.unplug();
        if let Some(handle) = self.notif.take() {
            let _ = handle.join();
        }
    }
}

// --- gamepad vocabulary -> XInput mapping ---

/// The four dpad-hat bits within the XInput button word.
const DPAD_MASK: u16 = XButtons::UP | XButtons::DOWN | XButtons::LEFT | XButtons::RIGHT;

fn set_button_bit(buttons: &mut XButtons, b: &GamepadButton, down: bool) {
    let bit = match b {
        GamepadButton::A => XButtons::A,
        GamepadButton::B => XButtons::B,
        GamepadButton::X => XButtons::X,
        GamepadButton::Y => XButtons::Y,
        GamepadButton::LeftBumper => XButtons::LB,
        GamepadButton::RightBumper => XButtons::RB,
        GamepadButton::Back => XButtons::BACK,
        GamepadButton::Start => XButtons::START,
        GamepadButton::Guide => XButtons::GUIDE,
        GamepadButton::LeftStick => XButtons::LTHUMB,
        GamepadButton::RightStick => XButtons::RTHUMB,
        GamepadButton::DpadUp
        | GamepadButton::DpadDown
        | GamepadButton::DpadLeft
        | GamepadButton::DpadRight => {
            unreachable!("dpad directions fold into the hat - see Dpad / set_button")
        }
        b if b.is_axis_button() => {
            unreachable!("axis pseudo-buttons fold into the stick/trigger axes - see AxisButtons")
        }
        _ => unreachable!("set_button_bit covers every non-hat, non-axis button"),
    };
    if down {
        buttons.raw |= bit;
    } else {
        buttons.raw &= !bit;
    }
}

/// Dpad hat state (`+1`/`-1` per axis) -> XInput dpad bits. `DpadX +1 = right`,
/// `DpadY +1 = down` (matches the vocabulary / evdev hat convention).
fn dpad_bits((x, y): (i32, i32)) -> u16 {
    let mut bits = 0;
    if x > 0 {
        bits |= XButtons::RIGHT;
    } else if x < 0 {
        bits |= XButtons::LEFT;
    }
    if y > 0 {
        bits |= XButtons::DOWN;
    } else if y < 0 {
        bits |= XButtons::UP;
    }
    bits
}

/// Normalized stick `-1.0..=1.0` -> XInput `i16`.
fn stick(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Normalized trigger `0.0..=1.0` -> XInput `u8`.
fn trigger(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * u8::MAX as f32) as u8
}
