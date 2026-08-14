//! `vocab-out` — the hardware-independent **output vocabulary** shared by `config` (which
//! names the target of a binding) and `virt-out` (which realizes it). Its input counterpart is
//! `vocab-hid` (the raw controller button bits). No platform deps:
//! each backend maps these **bare names** to OS codes (`virt-out` Linux → evdev, Windows
//! → scancode/VK). PLAN §2.1 / §3.
//!
//! The enums are deliberately **not** `#[non_exhaustive]`: adding a variant is a
//! deliberate breaking change that forces every backend's mapping to be updated — a
//! missing mapping should be a compile error, not a silent no-op (PLAN §0, "break the
//! API freely").

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A keyboard key, grouped by function (modifiers → editing → arrows → nav → punctuation
/// → digits → letters → function → print → keypad → media → …). Names are our own;
/// backends map them to OS codes. Adopted from the `uinput-simulation` reference through
/// `KbdIllumUp`; the more obscure codes are added when needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Key {
    // modifiers
    LeftShift, RightShift, LeftCtrl, RightCtrl, LeftAlt, RightAlt, LeftMeta, RightMeta,
    // editing / whitespace
    Esc, Tab, CapsLock, Backspace, Enter, Space,
    // special
    Compose, Menu,
    // arrows
    Up, Down, Left, Right,
    // navigation
    Insert, Delete, Home, End, PageUp, PageDown,
    // punctuation / symbols
    Grave, K102nd, Minus, Equal, LeftBrace, RightBrace, Backslash,
    Semicolon, Apostrophe, Comma, Dot, Slash,
    // digits (keyboard order, 0 last)
    D1, D2, D3, D4, D5, D6, D7, D8, D9, D0,
    // letters (QWERTY rows)
    Q, W, E, R, T, Y, U, I, O, P,
    A, S, D, F, G, H, J, K, L,
    Z, X, C, V, B, N, M,
    // function
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    // print / system cluster
    Print, SysRq, ScrollLock, Pause,
    // keypad (physical order; 0 then dot last)
    NumLock, KpSlash, KpAsterisk, KpMinus, KpPlus, KpEnter,
    Kp7, Kp8, Kp9, Kp4, Kp5, Kp6, Kp1, Kp2, Kp3, Kp0, KpDot,
    // audio
    Mute, VolumeDown, VolumeUp, MicMute,
    // media transport (Stop after Play; backward-first for prev/next)
    PlayPause, Play, StopCd, PreviousSong, NextSong, Rewind, FastForward,
    // browser
    Back, Forward,
    // brightness / keyboard illumination
    BrightnessDown, BrightnessUp, BrightnessCycle, BrightnessAuto,
    KbdIllumToggle, KbdIllumDown, KbdIllumUp,
}

impl Key {
    /// Every variant, in declared (grouped) order — for advertising the virtual keyboard
    /// and for UI listings.
    pub const ALL: &'static [Key] = &[
        Key::LeftShift, Key::RightShift, Key::LeftCtrl, Key::RightCtrl,
        Key::LeftAlt, Key::RightAlt, Key::LeftMeta, Key::RightMeta,
        Key::Esc, Key::Tab, Key::CapsLock, Key::Backspace, Key::Enter, Key::Space,
        Key::Compose, Key::Menu,
        Key::Up, Key::Down, Key::Left, Key::Right,
        Key::Insert, Key::Delete, Key::Home, Key::End, Key::PageUp, Key::PageDown,
        Key::Grave, Key::K102nd, Key::Minus, Key::Equal, Key::LeftBrace, Key::RightBrace,
        Key::Backslash, Key::Semicolon, Key::Apostrophe, Key::Comma, Key::Dot, Key::Slash,
        Key::D1, Key::D2, Key::D3, Key::D4, Key::D5, Key::D6, Key::D7, Key::D8, Key::D9, Key::D0,
        Key::Q, Key::W, Key::E, Key::R, Key::T, Key::Y, Key::U, Key::I, Key::O, Key::P,
        Key::A, Key::S, Key::D, Key::F, Key::G, Key::H, Key::J, Key::K, Key::L,
        Key::Z, Key::X, Key::C, Key::V, Key::B, Key::N, Key::M,
        Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6,
        Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
        Key::Print, Key::SysRq, Key::ScrollLock, Key::Pause,
        Key::NumLock, Key::KpSlash, Key::KpAsterisk, Key::KpMinus, Key::KpPlus, Key::KpEnter,
        Key::Kp7, Key::Kp8, Key::Kp9, Key::Kp4, Key::Kp5, Key::Kp6,
        Key::Kp1, Key::Kp2, Key::Kp3, Key::Kp0, Key::KpDot,
        Key::Mute, Key::VolumeDown, Key::VolumeUp, Key::MicMute,
        Key::PlayPause, Key::Play, Key::StopCd, Key::PreviousSong, Key::NextSong,
        Key::Rewind, Key::FastForward,
        Key::Back, Key::Forward,
        Key::BrightnessDown, Key::BrightnessUp, Key::BrightnessCycle, Key::BrightnessAuto,
        Key::KbdIllumToggle, Key::KbdIllumDown, Key::KbdIllumUp,
    ];
}

/// A mouse button. The four `Scroll*` are **discrete-scroll pseudo-buttons** — a scroll
/// tick is a button-like impulse (fires per activation, `Turbo` repeats), so it's folded
/// in here (Round D); the backend realizes them as wheel ticks, not `BTN_*`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

impl MouseButton {
    pub const ALL: &'static [MouseButton] = &[
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Back,
        MouseButton::Forward,
        MouseButton::ScrollUp,
        MouseButton::ScrollDown,
        MouseButton::ScrollLeft,
        MouseButton::ScrollRight,
    ];

    /// True for the discrete-scroll pseudo-buttons (realized as wheel ticks, not `BTN_*`).
    pub fn is_scroll(&self) -> bool {
        matches!(
            self,
            MouseButton::ScrollUp
                | MouseButton::ScrollDown
                | MouseButton::ScrollLeft
                | MouseButton::ScrollRight
        )
    }
}

/// High-resolution scroll units per wheel detent — the shared kernel/libinput/Windows
/// convention (evdev `REL_WHEEL_HI_RES` and Windows `WHEEL_DELTA` both use 120). One
/// source of truth for the engine (which scales motion into these units) and every
/// `virt-out` backend (which emits them, synthesizing a legacy notch every 120).
pub const SCROLL_HI_RES_PER_DETENT: i32 = 120;

/// A virtual-gamepad button (Xbox 360 layout). Two families are **pseudo-buttons** that the pad
/// realizes as something other than a plain button bit, folded in the backend (opposing directions
/// cancel to neutral): the four `Dpad*` fold into the XInput/evdev **hat**; and the full-trigger
/// (`*TriggerFull`) + eight stick-direction (`*Stick{Up,Down,Left,Right}`) buttons drive an **axis**
/// to its extreme (a full pull / a stick pushed to the edge). All are bindable as discrete actions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GamepadButton {
    A,
    B,
    X,
    Y,
    LeftBumper,
    RightBumper,
    Back,
    Start,
    Guide,
    LeftStick,
    RightStick,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    // Axis pseudo-buttons: a press drives the axis to its extreme (see `is_axis_button`).
    LeftTriggerFull,
    RightTriggerFull,
    LeftStickUp,
    LeftStickDown,
    LeftStickLeft,
    LeftStickRight,
    RightStickUp,
    RightStickDown,
    RightStickLeft,
    RightStickRight,
}

impl GamepadButton {
    pub const ALL: &'static [GamepadButton] = &[
        GamepadButton::A,
        GamepadButton::B,
        GamepadButton::X,
        GamepadButton::Y,
        GamepadButton::LeftBumper,
        GamepadButton::RightBumper,
        GamepadButton::Back,
        GamepadButton::Start,
        GamepadButton::Guide,
        GamepadButton::LeftStick,
        GamepadButton::RightStick,
        GamepadButton::DpadUp,
        GamepadButton::DpadDown,
        GamepadButton::DpadLeft,
        GamepadButton::DpadRight,
        GamepadButton::LeftTriggerFull,
        GamepadButton::RightTriggerFull,
        GamepadButton::LeftStickUp,
        GamepadButton::LeftStickDown,
        GamepadButton::LeftStickLeft,
        GamepadButton::LeftStickRight,
        GamepadButton::RightStickUp,
        GamepadButton::RightStickDown,
        GamepadButton::RightStickLeft,
        GamepadButton::RightStickRight,
    ];

    /// True for the four dpad directions (the backend folds these into the hat, not a
    /// `BTN_*`/XInput button bit).
    pub fn is_dpad(&self) -> bool {
        matches!(
            self,
            GamepadButton::DpadUp
                | GamepadButton::DpadDown
                | GamepadButton::DpadLeft
                | GamepadButton::DpadRight
        )
    }

    /// True for the axis pseudo-buttons — the two full-trigger pulls and the eight stick-direction
    /// pushes. The backend realizes these by driving an **axis** to its extreme (opposing stick
    /// directions cancel), not a button bit, so they carry no `BTN_*`/XInput bit.
    pub fn is_axis_button(&self) -> bool {
        matches!(
            self,
            GamepadButton::LeftTriggerFull
                | GamepadButton::RightTriggerFull
                | GamepadButton::LeftStickUp
                | GamepadButton::LeftStickDown
                | GamepadButton::LeftStickLeft
                | GamepadButton::LeftStickRight
                | GamepadButton::RightStickUp
                | GamepadButton::RightStickDown
                | GamepadButton::RightStickLeft
                | GamepadButton::RightStickRight
        )
    }
}

/// A virtual-gamepad axis — sticks and analog triggers only. The dpad is **not** here:
/// it's four logical [`GamepadButton`]s (the hat is a backend realization detail).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

impl GamepadAxis {
    pub const ALL: &'static [GamepadAxis] = &[
        GamepadAxis::LeftStickX,
        GamepadAxis::LeftStickY,
        GamepadAxis::RightStickX,
        GamepadAxis::RightStickY,
        GamepadAxis::LeftTrigger,
        GamepadAxis::RightTrigger,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_arrays_have_no_dupes_and_cover_scroll() {
        // A light guard that ALL stays in sync (unique + non-empty).
        assert_eq!(Key::ALL.len(), 127);
        let mut seen = std::collections::HashSet::new();
        for k in Key::ALL {
            assert!(seen.insert(k), "duplicate in Key::ALL: {k:?}");
        }
        assert_eq!(MouseButton::ALL.iter().filter(|b| b.is_scroll()).count(), 4);
    }
}
