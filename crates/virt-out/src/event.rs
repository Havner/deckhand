//! The emit vocabulary (PLAN §2.1) — platform-agnostic. [`OutputEvent`] is what the
//! engine hands [`crate::Sink::emit`]; the leaf target enums (`Key` / `MouseButton` /
//! `GamepadButton` / `GamepadAxis`) live in the shared [`vocab`] crate and are re-exported
//! from the crate root. The backend maps them to OS codes.

use vocab::{GamepadAxis, GamepadButton, Key, MouseButton};

/// A batch item handed to [`crate::Sink::emit`]. **Levels** (`Key`/button/axis) carry
/// the desired state; **deltas** (`MouseMove`/`Scroll`) are relative. The engine sends
/// only changes — `virt-out` just realizes them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OutputEvent {
    /// Keyboard key down (`true`) / up (`false`).
    Key(Key, bool),
    /// Mouse button down / up. The `Scroll*` pseudo-buttons realize as wheel ticks on
    /// `down`; `up` is a no-op (a scroll tick has no held state).
    MouseButton(MouseButton, bool),
    /// Relative pointer motion.
    MouseMove { dx: i32, dy: i32 },
    /// Wheel ticks (`dy` vertical, `dx` horizontal) — continuous scroll from a behavior.
    Scroll { dx: i32, dy: i32 },
    /// Virtual-gamepad button down / up.
    GamepadButton(GamepadButton, bool),
    /// Virtual-gamepad axis position — sticks/dpad in `-1.0..=1.0`, triggers `0.0..=1.0`.
    GamepadAxis(GamepadAxis, f32),
}

/// A rumble command received *from* a consumer of our virtual gamepad (game → pad),
/// to route onward to real-controller haptics. Magnitudes are `0..=u16::MAX`
/// (heavy/low-frequency and light/high-frequency motors, the Xbox model).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rumble {
    pub strong: u16,
    pub weak: u16,
}

impl Rumble {
    pub fn is_zero(&self) -> bool {
        self.strong == 0 && self.weak == 0
    }
}

/// Dpad realization state. The vocab exposes the dpad as four logical `GamepadButton`s,
/// but the XInput/evdev virtual pad models it as a **hat**; each backend tracks the four
/// directions here and folds them to a hat value (`-1/0/+1` per axis), **cancelling
/// opposing directions to neutral**.
#[derive(Default)]
pub(crate) struct Dpad {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

impl Dpad {
    /// Apply a button event; returns `true` if `b` was a dpad direction (so the caller
    /// folds to the hat instead of emitting a normal button).
    pub(crate) fn set(&mut self, b: &GamepadButton, down: bool) -> bool {
        match b {
            GamepadButton::DpadUp => self.up = down,
            GamepadButton::DpadDown => self.down = down,
            GamepadButton::DpadLeft => self.left = down,
            GamepadButton::DpadRight => self.right = down,
            _ => return false,
        }
        true
    }

    /// Hat X: right − left (opposing cancels to 0).
    pub(crate) fn x(&self) -> i32 {
        self.right as i32 - self.left as i32
    }

    /// Hat Y: down − up (opposing cancels to 0).
    pub(crate) fn y(&self) -> i32 {
        self.down as i32 - self.up as i32
    }
}
