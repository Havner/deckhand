//! [`OutputEvent`] - the platform-agnostic emit vocabulary. This is what the mapper produces and
//! hands `virt-out`'s `Sink::emit`; the leaf target enums (`Key` / `MouseButton` / `GamepadButton`
//! / `GamepadAxis`) live alongside it in this crate. The backend maps them to OS codes.

use crate::{GamepadAxis, GamepadButton, Key, MouseButton};

/// A batch item handed to `virt-out`'s `Sink::emit`. **Levels** (`Key`/button/axis) carry the
/// desired state; **deltas** (`MouseMove`/`Scroll`) are relative. The mapper sends only changes -
/// `virt-out` just realizes them.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputEvent {
    /// Keyboard key down (`true`) / up (`false`).
    Key(Key, bool),
    /// Mouse button down / up. The `Scroll*` pseudo-buttons realize as wheel ticks on
    /// `down`; `up` is a no-op (a scroll tick has no held state).
    MouseButton(MouseButton, bool),
    /// Relative pointer motion.
    MouseMove { dx: i32, dy: i32 },
    /// Wheel ticks (`dy` vertical, `dx` horizontal) - discrete continuous scroll from a behavior.
    Scroll { dx: i32, dy: i32 },
    /// High-resolution smooth scroll, in units where **120 = one wheel detent** (evdev
    /// `REL_WHEEL_HI_RES` / Windows `WHEEL_DELTA`). Backends emit the fine-grained value and
    /// synthesize a legacy notch every 120 so non-hi-res consumers still scroll.
    SmoothScroll { dx: i32, dy: i32 },
    /// Virtual-gamepad button down / up.
    GamepadButton(GamepadButton, bool),
    /// Virtual-gamepad axis position - sticks/dpad in `-1.0..=1.0`, triggers `0.0..=1.0`.
    GamepadAxis(GamepadAxis, f32),
}
