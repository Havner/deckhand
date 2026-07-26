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
