//! Linux backend: three uinput virtual devices (keyboard / mouse / gamepad) via
//! `evdev`. The gamepad emulates an Xbox 360 pad (`input_id` 045e:028e + xpad code
//! set) and advertises `FF_RUMBLE` so we can receive game rumble (PLAN §2.1).

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AbsoluteAxisEvent, AttributeSet, BusType, EventSummary,
    FFEffectCode, FFEffectKind, InputEvent, InputId, KeyCode, KeyEvent, RelativeAxisCode,
    RelativeAxisEvent, UInputCode, UinputAbsSetup,
};

use crate::event::{GamepadAxis, GamepadButton, Key, MouseButton, OutputEvent, Rumble};

const FF_MAX_EFFECTS: u32 = 16;

/// A stored force-feedback effect: its rumble magnitudes plus the replay timing we
/// must honor ourselves (the uinput-userspace FF model makes us stop playback when
/// `length` elapses — the kernel doesn't do it for us).
struct FfEffect {
    rumble: Rumble,
    length_ms: u32, // 0 = play until an explicit stop
    delay_ms: u32,
}

/// The output sink: owns the virtual devices and realizes [`OutputEvent`]s.
///
/// Sync — call [`Sink::emit`] from the engine's mapping loop. [`Sink::poll_rumble`]
/// drains rumble uploaded by a consumer of the virtual gamepad. Dropping the `Sink`
/// destroys the uinput devices.
pub struct Sink {
    keyboard: VirtualDevice,
    mouse: VirtualDevice,
    gamepad: VirtualDevice,
    // Force-feedback state: a small pool of effect ids, the effects uploaded for each,
    // the one currently playing, and when it should auto-stop (per its replay length).
    ff_free_ids: Vec<i16>,
    ff_effects: HashMap<i16, FfEffect>,
    ff_playing: Option<i16>,
    ff_until: Option<Instant>,
}

impl Sink {
    /// Create the three virtual devices. Needs write access to `/dev/uinput`
    /// (see `udev/70-deckhand-uinput.rules`).
    pub fn new() -> crate::Result<Self> {
        Ok(Self {
            keyboard: build_keyboard()?,
            mouse: build_mouse()?,
            gamepad: build_gamepad()?,
            ff_free_ids: (0..FF_MAX_EFFECTS as i16).rev().collect(),
            ff_effects: HashMap::new(),
            ff_playing: None,
            ff_until: None,
        })
    }

    /// Realize a batch of output events. Events are grouped per device and each device
    /// is emitted once (evdev appends a `SYN_REPORT` per `emit`).
    pub fn emit(&mut self, events: &[OutputEvent]) -> crate::Result<()> {
        let mut kb: Vec<InputEvent> = Vec::new();
        let mut mouse: Vec<InputEvent> = Vec::new();
        let mut gp: Vec<InputEvent> = Vec::new();

        for ev in events {
            match ev {
                OutputEvent::Key(k, down) => kb.push(*KeyEvent::new(key_code(k), *down as i32)),
                OutputEvent::MouseButton(b, down) => {
                    mouse.push(*KeyEvent::new(mouse_code(b), *down as i32))
                }
                OutputEvent::MouseMove { dx, dy } => {
                    mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_X, *dx));
                    mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_Y, *dy));
                }
                OutputEvent::Scroll { dx, dy } => {
                    if *dy != 0 {
                        mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_WHEEL, *dy));
                    }
                    if *dx != 0 {
                        mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_HWHEEL, *dx));
                    }
                }
                OutputEvent::GamepadButton(b, down) => {
                    gp.push(*KeyEvent::new(gamepad_code(b), *down as i32))
                }
                OutputEvent::GamepadAxis(a, v) => {
                    gp.push(*AbsoluteAxisEvent::new(abs_code(a), abs_value(a, *v)))
                }
            }
        }

        if !kb.is_empty() {
            self.keyboard.emit(&kb)?;
        }
        if !mouse.is_empty() {
            self.mouse.emit(&mouse)?;
        }
        if !gp.is_empty() {
            self.gamepad.emit(&gp)?;
        }
        Ok(())
    }

    /// Drain any force-feedback traffic from the virtual gamepad and return the
    /// currently-commanded rumble (zero if nothing is playing). Non-blocking. Route
    /// the result onward to real-controller haptics (PLAN §2.1 / §6).
    pub fn poll_rumble(&mut self) -> crate::Result<Rumble> {
        // The gamepad fd is non-blocking (set in `build_gamepad`), so with no FF traffic
        // this returns `WouldBlock` — treat that as "nothing this cycle".
        let events: Vec<InputEvent> = match self.gamepad.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        for event in events {
            match event.destructure() {
                // Upload an effect. New effects (id `-1`) get a pooled id; updates (a game
                // changing an effect in place — e.g. dropping rumble to 0) keep their id.
                // `FFUploadEvent` owns its fd, so it doesn't borrow `self.gamepad`.
                EventSummary::UInput(ev, UInputCode::UI_FF_UPLOAD, _) => {
                    let mut up = self.gamepad.process_ff_upload(ev)?;
                    let existing = up.effect_id();
                    let id = if existing >= 0 { Some(existing) } else { self.ff_free_ids.pop() };
                    match id {
                        Some(id) => {
                            up.set_effect_id(id);
                            up.set_retval(0);
                        }
                        None => up.set_retval(-1), // pool exhausted
                    }
                    let data = up.effect();
                    if let (Some(id), FFEffectKind::Rumble { strong_magnitude, weak_magnitude }) =
                        (id, data.kind)
                    {
                        self.ff_effects.insert(
                            id,
                            FfEffect {
                                rumble: Rumble { strong: strong_magnitude, weak: weak_magnitude },
                                length_ms: data.replay.length as u32,
                                delay_ms: data.replay.delay as u32,
                            },
                        );
                    }
                }
                // An effect is erased — reclaim its id.
                EventSummary::UInput(ev, UInputCode::UI_FF_ERASE, _) => {
                    let id = self.gamepad.process_ff_erase(ev)?.effect_id() as i16;
                    self.ff_effects.remove(&id);
                    if self.ff_playing == Some(id) {
                        self.ff_playing = None;
                        self.ff_until = None;
                    }
                    self.ff_free_ids.push(id);
                }
                // Play (value = repeat count ≥ 1) / stop (0) of an effect id. On play we
                // honor the effect's replay `length` by scheduling an auto-stop — in the
                // uinput-userspace FF model nothing stops it for us. `length == 0` means
                // play until an explicit stop. (Single-slot: last-played effect wins.)
                EventSummary::ForceFeedback(_, effect, value) => {
                    let id = effect.0 as i16;
                    if value != 0 {
                        self.ff_playing = Some(id);
                        let (length, delay) = self
                            .ff_effects
                            .get(&id)
                            .map(|e| (e.length_ms, e.delay_ms))
                            .unwrap_or((0, 0));
                        self.ff_until = (length > 0).then(|| {
                            let total = delay + length * value.max(1) as u32;
                            Instant::now() + Duration::from_millis(total as u64)
                        });
                    } else if self.ff_playing == Some(id) {
                        self.ff_playing = None;
                        self.ff_until = None;
                    }
                }
                _ => {}
            }
        }
        // Auto-stop once the replay length has elapsed.
        if self.ff_until.is_some_and(|t| Instant::now() >= t) {
            self.ff_playing = None;
            self.ff_until = None;
        }
        Ok(self
            .ff_playing
            .and_then(|id| self.ff_effects.get(&id))
            .map(|e| e.rumble.clone())
            .unwrap_or_default())
    }
}

// --- device construction ---

fn build_keyboard() -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for k in Key::ALL {
        keys.insert(key_code(k));
    }
    VirtualDevice::builder()?
        .name("deckhand virtual keyboard")
        .with_keys(&keys)?
        .build()
}

fn build_mouse() -> io::Result<VirtualDevice> {
    let mut buttons = AttributeSet::<KeyCode>::new();
    for b in MouseButton::ALL {
        buttons.insert(mouse_code(b));
    }
    let mut rel = AttributeSet::<RelativeAxisCode>::new();
    rel.insert(RelativeAxisCode::REL_X);
    rel.insert(RelativeAxisCode::REL_Y);
    rel.insert(RelativeAxisCode::REL_WHEEL);
    rel.insert(RelativeAxisCode::REL_HWHEEL);
    VirtualDevice::builder()?
        .name("deckhand virtual mouse")
        .with_keys(&buttons)?
        .with_relative_axes(&rel)?
        .build()
}

fn build_gamepad() -> io::Result<VirtualDevice> {
    let mut buttons = AttributeSet::<KeyCode>::new();
    for b in GamepadButton::ALL {
        buttons.insert(gamepad_code(b));
    }
    let mut ff = AttributeSet::<FFEffectCode>::new();
    ff.insert(FFEffectCode::FF_RUMBLE);

    // Xbox 360 identity: bus USB, Microsoft vendor, X360 product. SDL keys its built-in
    // mapping off this + the xpad code set below → games see a standard Xbox pad.
    let mut builder = VirtualDevice::builder()?
        .name("Microsoft X-Box 360 pad")
        .input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0114))
        .with_keys(&buttons)?
        .with_ff(&ff)?
        .with_ff_effects_max(FF_MAX_EFFECTS);
    for axis in GamepadAxis::ALL {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(abs_code(axis), abs_info(axis)))?;
    }
    let device = builder.build()?;
    set_nonblocking(&device)?; // so poll_rumble's fetch_events doesn't block on no FF
    Ok(device)
}

/// Put a device's fd in non-blocking mode (evdev's sync `VirtualDevice` reads block
/// otherwise — only its tokio path sets `O_NONBLOCK`).
fn set_nonblocking(device: &VirtualDevice) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = device.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// --- vocabulary → evdev code mapping ---

fn key_code(k: &Key) -> KeyCode {
    match k {
        Key::A => KeyCode::KEY_A,
        Key::B => KeyCode::KEY_B,
        Key::C => KeyCode::KEY_C,
        Key::D => KeyCode::KEY_D,
        Key::E => KeyCode::KEY_E,
        Key::F => KeyCode::KEY_F,
        Key::G => KeyCode::KEY_G,
        Key::H => KeyCode::KEY_H,
        Key::I => KeyCode::KEY_I,
        Key::J => KeyCode::KEY_J,
        Key::K => KeyCode::KEY_K,
        Key::L => KeyCode::KEY_L,
        Key::M => KeyCode::KEY_M,
        Key::N => KeyCode::KEY_N,
        Key::O => KeyCode::KEY_O,
        Key::P => KeyCode::KEY_P,
        Key::Q => KeyCode::KEY_Q,
        Key::R => KeyCode::KEY_R,
        Key::S => KeyCode::KEY_S,
        Key::T => KeyCode::KEY_T,
        Key::U => KeyCode::KEY_U,
        Key::V => KeyCode::KEY_V,
        Key::W => KeyCode::KEY_W,
        Key::X => KeyCode::KEY_X,
        Key::Y => KeyCode::KEY_Y,
        Key::Z => KeyCode::KEY_Z,
        Key::Num0 => KeyCode::KEY_0,
        Key::Num1 => KeyCode::KEY_1,
        Key::Num2 => KeyCode::KEY_2,
        Key::Num3 => KeyCode::KEY_3,
        Key::Num4 => KeyCode::KEY_4,
        Key::Num5 => KeyCode::KEY_5,
        Key::Num6 => KeyCode::KEY_6,
        Key::Num7 => KeyCode::KEY_7,
        Key::Num8 => KeyCode::KEY_8,
        Key::Num9 => KeyCode::KEY_9,
        Key::Space => KeyCode::KEY_SPACE,
        Key::Enter => KeyCode::KEY_ENTER,
        Key::Escape => KeyCode::KEY_ESC,
        Key::Tab => KeyCode::KEY_TAB,
        Key::Backspace => KeyCode::KEY_BACKSPACE,
        Key::Up => KeyCode::KEY_UP,
        Key::Down => KeyCode::KEY_DOWN,
        Key::Left => KeyCode::KEY_LEFT,
        Key::Right => KeyCode::KEY_RIGHT,
        Key::LeftShift => KeyCode::KEY_LEFTSHIFT,
        Key::RightShift => KeyCode::KEY_RIGHTSHIFT,
        Key::LeftCtrl => KeyCode::KEY_LEFTCTRL,
        Key::RightCtrl => KeyCode::KEY_RIGHTCTRL,
        Key::LeftAlt => KeyCode::KEY_LEFTALT,
        Key::RightAlt => KeyCode::KEY_RIGHTALT,
        Key::LeftMeta => KeyCode::KEY_LEFTMETA,
    }
}

fn mouse_code(b: &MouseButton) -> KeyCode {
    match b {
        MouseButton::Left => KeyCode::BTN_LEFT,
        MouseButton::Right => KeyCode::BTN_RIGHT,
        MouseButton::Middle => KeyCode::BTN_MIDDLE,
        MouseButton::Back => KeyCode::BTN_SIDE,
        MouseButton::Forward => KeyCode::BTN_EXTRA,
    }
}

// A=SOUTH, B=EAST, X=NORTH, Y=WEST — matches xpad/X360 (SDL maps these to A/B/X/Y).
fn gamepad_code(b: &GamepadButton) -> KeyCode {
    match b {
        GamepadButton::A => KeyCode::BTN_SOUTH,
        GamepadButton::B => KeyCode::BTN_EAST,
        GamepadButton::X => KeyCode::BTN_NORTH,
        GamepadButton::Y => KeyCode::BTN_WEST,
        GamepadButton::LeftBumper => KeyCode::BTN_TL,
        GamepadButton::RightBumper => KeyCode::BTN_TR,
        GamepadButton::Back => KeyCode::BTN_SELECT,
        GamepadButton::Start => KeyCode::BTN_START,
        GamepadButton::Guide => KeyCode::BTN_MODE,
        GamepadButton::LeftStick => KeyCode::BTN_THUMBL,
        GamepadButton::RightStick => KeyCode::BTN_THUMBR,
    }
}

fn abs_code(a: &GamepadAxis) -> AbsoluteAxisCode {
    match a {
        GamepadAxis::LeftStickX => AbsoluteAxisCode::ABS_X,
        GamepadAxis::LeftStickY => AbsoluteAxisCode::ABS_Y,
        GamepadAxis::RightStickX => AbsoluteAxisCode::ABS_RX,
        GamepadAxis::RightStickY => AbsoluteAxisCode::ABS_RY,
        GamepadAxis::LeftTrigger => AbsoluteAxisCode::ABS_Z,
        GamepadAxis::RightTrigger => AbsoluteAxisCode::ABS_RZ,
        GamepadAxis::DpadX => AbsoluteAxisCode::ABS_HAT0X,
        GamepadAxis::DpadY => AbsoluteAxisCode::ABS_HAT0Y,
    }
}

// xpad ranges: sticks i16, triggers 0..255, dpad hat -1..1.
fn abs_info(a: &GamepadAxis) -> AbsInfo {
    match a {
        GamepadAxis::LeftStickX
        | GamepadAxis::LeftStickY
        | GamepadAxis::RightStickX
        | GamepadAxis::RightStickY => AbsInfo::new(0, -32768, 32767, 16, 128, 0),
        GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger => AbsInfo::new(0, 0, 255, 0, 0, 0),
        GamepadAxis::DpadX | GamepadAxis::DpadY => AbsInfo::new(0, -1, 1, 0, 0, 0),
    }
}

// Normalized f32 → device units: sticks/dpad from -1..1, triggers from 0..1.
fn abs_value(a: &GamepadAxis, v: f32) -> i32 {
    match a {
        GamepadAxis::LeftStickX
        | GamepadAxis::LeftStickY
        | GamepadAxis::RightStickX
        | GamepadAxis::RightStickY => (v.clamp(-1.0, 1.0) * 32767.0) as i32,
        GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger => (v.clamp(0.0, 1.0) * 255.0) as i32,
        GamepadAxis::DpadX | GamepadAxis::DpadY => v.round().clamp(-1.0, 1.0) as i32,
    }
}
