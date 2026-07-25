//! The output vocabulary (PLAN §2.1) — platform-agnostic. What the engine can emit;
//! the backend maps these to OS codes. Deliberately minimal for the Phase A slice —
//! the API is not stable (PLAN §0), grow the enums as the mapper needs them.

/// A batch item handed to [`crate::Sink::emit`]. **Levels** (`Key`/button/axis) carry
/// the desired state; **deltas** (`MouseMove`/`Scroll`) are relative. The engine sends
/// only changes — `virt-out` just realizes them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OutputEvent {
    /// Keyboard key down (`true`) / up (`false`).
    Key(Key, bool),
    /// Mouse button down / up.
    MouseButton(MouseButton, bool),
    /// Relative pointer motion.
    MouseMove { dx: i32, dy: i32 },
    /// Wheel ticks (`dy` vertical, `dx` horizontal).
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

/// A keyboard key (subset — grow as needed).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    Space, Enter, Escape, Tab, Backspace,
    Up, Down, Left, Right,
    LeftShift, RightShift, LeftCtrl, RightCtrl, LeftAlt, RightAlt, LeftMeta,
}

impl Key {
    /// Every variant — used to declare the virtual keyboard's key set.
    pub const ALL: &'static [Key] = &[
        Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H, Key::I, Key::J,
        Key::K, Key::L, Key::M, Key::N, Key::O, Key::P, Key::Q, Key::R, Key::S, Key::T,
        Key::U, Key::V, Key::W, Key::X, Key::Y, Key::Z,
        Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4,
        Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
        Key::Space, Key::Enter, Key::Escape, Key::Tab, Key::Backspace,
        Key::Up, Key::Down, Key::Left, Key::Right,
        Key::LeftShift, Key::RightShift, Key::LeftCtrl, Key::RightCtrl,
        Key::LeftAlt, Key::RightAlt, Key::LeftMeta,
    ];
}

/// A mouse button.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseButton {
    pub const ALL: &'static [MouseButton] = &[
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Back,
        MouseButton::Forward,
    ];
}

/// A virtual-gamepad button (Xbox 360 layout).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
    ];
}

/// A virtual-gamepad axis. Dpad is modelled as a hat axis (Xbox/xpad reality), not
/// buttons; the engine translates dpad presses to `Dpad{X,Y}` values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
    DpadX,
    DpadY,
}

impl GamepadAxis {
    pub const ALL: &'static [GamepadAxis] = &[
        GamepadAxis::LeftStickX,
        GamepadAxis::LeftStickY,
        GamepadAxis::RightStickX,
        GamepadAxis::RightStickY,
        GamepadAxis::LeftTrigger,
        GamepadAxis::RightTrigger,
        GamepadAxis::DpadX,
        GamepadAxis::DpadY,
    ];
}
