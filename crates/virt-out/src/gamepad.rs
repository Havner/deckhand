//! Virtual-gamepad types that are `virt-out`'s own (everything here is gamepad-specific; kb/mouse
//! touch none of it). The emit vocabulary itself (`OutputEvent` + its leaf target enums) lives in
//! the shared `vocab-out` crate; this module holds: [`Rumble`] - what a game sends *back* through
//! the virtual pad (PLAN 2.1 FF back-channel) - and the per-backend [`Dpad`]/[`AxisButtons`] helpers
//! that fold the dpad/trigger/stick pseudo-buttons into the pad's hat and axes.

// Only the hat/axis-pseudo-button helpers (`Dpad`/`AxisButtons`) use these; a kb/mouse-only
// Windows build (no `vigem`/`viiper`) compiles neither, so gate the import with the same cfg.
#[cfg(any(target_os = "linux", all(target_os = "windows", any(feature = "vigem", feature = "viiper"))))]
use vocab_out::{GamepadAxis, GamepadButton};

/// A rumble command received *from* a consumer of our virtual gamepad (game -> pad),
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
///
/// Compiled only when a hat-based gamepad backend is present: the Linux backend (always) or,
/// on Windows, the ViGEm or VIIPER backend (`vigem` / `viiper` features). A kb/mouse-only
/// Windows build has no gamepad output and wouldn't use it. (Add future hat backends here.)
#[cfg(any(target_os = "linux", all(target_os = "windows", any(feature = "vigem", feature = "viiper"))))]
#[derive(Default)]
pub(crate) struct Dpad {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

#[cfg(any(target_os = "linux", all(target_os = "windows", any(feature = "vigem", feature = "viiper"))))]
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

    /// Hat X: right - left (opposing cancels to 0).
    pub(crate) fn x(&self) -> i32 {
        self.right as i32 - self.left as i32
    }

    /// Hat Y: down - up (opposing cancels to 0).
    pub(crate) fn y(&self) -> i32 {
        self.down as i32 - self.up as i32
    }
}

/// Axis pseudo-button realization state. Vocab exposes full-trigger pulls and stick-direction
/// pushes as `GamepadButton`s (like the dpad), but they drive **axes**: a press sets the axis to
/// its extreme, opposing stick directions cancel to neutral, and a full-trigger button pushes the
/// trigger to max. A held direction/trigger **overrides** the analog value on that axis; on release
/// the axis falls back to the last analog value (cached here), so an axis driven by both an analog
/// behaviour and a direction button behaves sensibly. Sticks use the evdev sign (Y +down), matching
/// [`Dpad::y`] and the `GamepadAxis` convention. Same precedent + cfg as [`Dpad`].
#[cfg(any(target_os = "linux", all(target_os = "windows", any(feature = "vigem", feature = "viiper"))))]
#[derive(Default)]
pub(crate) struct AxisButtons {
    ls_up: bool,
    ls_down: bool,
    ls_left: bool,
    ls_right: bool,
    rs_up: bool,
    rs_down: bool,
    rs_left: bool,
    rs_right: bool,
    lt: bool,
    rt: bool,
    /// Last analog value per axis (indexed by [`Self::idx`]).
    analog: [f32; 6],
}

#[cfg(any(target_os = "linux", all(target_os = "windows", any(feature = "vigem", feature = "viiper"))))]
impl AxisButtons {
    /// Record a pseudo-button press; returns the axis it drives (so the caller emits that axis's
    /// combined [`value`](Self::value)), or `None` if `b` isn't one of the ten.
    pub(crate) fn set_button(&mut self, b: &GamepadButton, down: bool) -> Option<GamepadAxis> {
        use GamepadAxis as A;
        use GamepadButton as B;
        Some(match b {
            B::LeftStickUp => {
                self.ls_up = down;
                A::LeftStickY
            }
            B::LeftStickDown => {
                self.ls_down = down;
                A::LeftStickY
            }
            B::LeftStickLeft => {
                self.ls_left = down;
                A::LeftStickX
            }
            B::LeftStickRight => {
                self.ls_right = down;
                A::LeftStickX
            }
            B::RightStickUp => {
                self.rs_up = down;
                A::RightStickY
            }
            B::RightStickDown => {
                self.rs_down = down;
                A::RightStickY
            }
            B::RightStickLeft => {
                self.rs_left = down;
                A::RightStickX
            }
            B::RightStickRight => {
                self.rs_right = down;
                A::RightStickX
            }
            B::LeftTriggerFull => {
                self.lt = down;
                A::LeftTrigger
            }
            B::RightTriggerFull => {
                self.rt = down;
                A::RightTrigger
            }
            _ => return None,
        })
    }

    /// Cache an analog value from a `GamepadAxis` event, so a released direction/trigger falls back
    /// to it rather than to zero.
    pub(crate) fn set_analog(&mut self, a: &GamepadAxis, v: f32) {
        self.analog[Self::idx(a)] = v;
    }

    /// The value to drive an axis with: the digital extreme when a direction/trigger on that axis is
    /// held (opposing stick directions cancel to 0), else the cached analog value. Sticks are in
    /// `-1..=1` (evdev sign), triggers `0..=1`.
    pub(crate) fn value(&self, a: &GamepadAxis) -> f32 {
        use GamepadAxis as A;
        let digital = match a {
            A::LeftStickX => self.ls_right as i32 - self.ls_left as i32,
            A::LeftStickY => self.ls_down as i32 - self.ls_up as i32,
            A::RightStickX => self.rs_right as i32 - self.rs_left as i32,
            A::RightStickY => self.rs_down as i32 - self.rs_up as i32,
            A::LeftTrigger => self.lt as i32,
            A::RightTrigger => self.rt as i32,
        };
        if digital != 0 { digital as f32 } else { self.analog[Self::idx(a)] }
    }

    fn idx(a: &GamepadAxis) -> usize {
        use GamepadAxis as A;
        match a {
            A::LeftStickX => 0,
            A::LeftStickY => 1,
            A::RightStickX => 2,
            A::RightStickY => 3,
            A::LeftTrigger => 4,
            A::RightTrigger => 5,
        }
    }
}
