//! Linux backend: three uinput virtual devices (keyboard / mouse / gamepad) via
//! `evdev`. The gamepad emulates an Xbox 360 pad (`input_id` 045e:028e + xpad code
//! set) and advertises `FF_RUMBLE` so we can receive game rumble (PLAN 2.1).

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AbsoluteAxisEvent, AttributeSet, BusType, EventSummary,
    FFEffectCode, FFEffectKind, InputEvent, InputId, KeyCode, KeyEvent, RelativeAxisCode,
    RelativeAxisEvent, UInputCode, UinputAbsSetup,
};

use crate::event::{AxisButtons, Dpad, OutputEvent, Rumble};
use vocab_out::{GamepadAxis, GamepadButton, Key, MouseButton};

const FF_MAX_EFFECTS: u32 = 16;

/// A stored force-feedback effect: its rumble magnitudes plus the replay timing we
/// must honor ourselves (the uinput-userspace FF model makes us stop playback when
/// `length` elapses - the kernel doesn't do it for us).
struct FfEffect {
    rumble: Rumble,
    length_ms: u32, // 0 = play until an explicit stop
    delay_ms: u32,
}

/// The output sink: owns the virtual devices and realizes [`OutputEvent`]s.
///
/// Sync - call [`Sink::emit`] from the engine's mapping loop. [`Sink::poll_rumble`]
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
    // Dpad direction state, folded into the ABS_HAT0X/Y hat on change.
    dpad: Dpad,
    // Full-trigger / stick-direction pseudo-buttons, folded into the stick/trigger axes.
    axis_buttons: AxisButtons,
    // High-res scroll: accumulated hi-res units (120 = one detent) per axis, so a legacy
    // REL_WHEEL/REL_HWHEEL notch is synthesized every 120 for non-hi-res consumers.
    hi_res_wheel: i32,
    hi_res_hwheel: i32,
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
            dpad: Dpad::default(),
            axis_buttons: AxisButtons::default(),
            hi_res_wheel: 0,
            hi_res_hwheel: 0,
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
                OutputEvent::MouseButton(b, down) => match scroll_delta(b) {
                    // Scroll pseudo-button: one wheel notch on press; release is a no-op.
                    Some((horizontal, dir)) => {
                        if *down {
                            push_wheel(&mut mouse, horizontal, dir);
                        }
                    }
                    None => mouse.push(*KeyEvent::new(mouse_code(b), *down as i32)),
                },
                OutputEvent::MouseMove { dx, dy } => {
                    mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_X, *dx));
                    mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_Y, *dy));
                }
                OutputEvent::Scroll { dx, dy } => {
                    push_wheel(&mut mouse, false, *dy);
                    push_wheel(&mut mouse, true, *dx);
                }
                OutputEvent::SmoothScroll { dx, dy } => {
                    // Emit the fine-grained hi-res value, and synthesize a legacy notch each time
                    // 120 units (one detent) accumulate so non-hi-res apps still scroll.
                    if *dy != 0 {
                        mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_WHEEL_HI_RES, *dy));
                        if let Some(notch) = accumulate_notch(&mut self.hi_res_wheel, *dy) {
                            mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_WHEEL, notch));
                        }
                    }
                    if *dx != 0 {
                        mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_HWHEEL_HI_RES, *dx));
                        if let Some(notch) = accumulate_notch(&mut self.hi_res_hwheel, *dx) {
                            mouse.push(*RelativeAxisEvent::new(RelativeAxisCode::REL_HWHEEL, notch));
                        }
                    }
                }
                OutputEvent::GamepadButton(b, down) => {
                    if self.dpad.set(b, *down) {
                        // Dpad direction -> fold into the hat (both axes; the kernel drops
                        // the unchanged one).
                        gp.push(*AbsoluteAxisEvent::new(
                            AbsoluteAxisCode::ABS_HAT0X,
                            self.dpad.x(),
                        ));
                        gp.push(*AbsoluteAxisEvent::new(
                            AbsoluteAxisCode::ABS_HAT0Y,
                            self.dpad.y(),
                        ));
                    } else if let Some(axis) = self.axis_buttons.set_button(b, *down) {
                        // Full-trigger / stick-direction pseudo-button -> drive its axis to the
                        // combined value (digital extreme while held, else the cached analog).
                        let vc = self.axis_buttons.value(&axis);
                        gp.push(*AbsoluteAxisEvent::new(abs_code(&axis), abs_value(&axis, vc)));
                    } else {
                        gp.push(*KeyEvent::new(gamepad_code(b), *down as i32));
                    }
                }
                OutputEvent::GamepadAxis(a, v) => {
                    // Cache the analog value and emit it combined with any held axis-button (which
                    // overrides it) - so an analog stick and a stick-direction button coexist.
                    self.axis_buttons.set_analog(a, *v);
                    let vc = self.axis_buttons.value(a);
                    gp.push(*AbsoluteAxisEvent::new(abs_code(a), abs_value(a, vc)))
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
    /// the result onward to real-controller haptics (PLAN 2.1 / 6).
    pub fn poll_rumble(&mut self) -> crate::Result<Rumble> {
        // The gamepad fd is non-blocking (set in `build_gamepad`), so with no FF traffic
        // this returns `WouldBlock` - treat that as "nothing this cycle".
        let events: Vec<InputEvent> = match self.gamepad.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        for event in events {
            match event.destructure() {
                // Upload an effect. New effects (id `-1`) get a pooled id; updates (a game
                // changing an effect in place - e.g. dropping rumble to 0) keep their id.
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
                // An effect is erased - reclaim its id.
                EventSummary::UInput(ev, UInputCode::UI_FF_ERASE, _) => {
                    let id = self.gamepad.process_ff_erase(ev)?.effect_id() as i16;
                    self.ff_effects.remove(&id);
                    if self.ff_playing == Some(id) {
                        self.ff_playing = None;
                        self.ff_until = None;
                    }
                    self.ff_free_ids.push(id);
                }
                // Play (value = repeat count >= 1) / stop (0) of an effect id. On play we
                // honor the effect's replay `length` by scheduling an auto-stop - in the
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
        if !b.is_scroll() {
            buttons.insert(mouse_code(b)); // scroll pseudo-buttons use REL_WHEEL, not BTN_*
        }
    }
    let mut rel = AttributeSet::<RelativeAxisCode>::new();
    rel.insert(RelativeAxisCode::REL_X);
    rel.insert(RelativeAxisCode::REL_Y);
    rel.insert(RelativeAxisCode::REL_WHEEL);
    rel.insert(RelativeAxisCode::REL_HWHEEL);
    rel.insert(RelativeAxisCode::REL_WHEEL_HI_RES);
    rel.insert(RelativeAxisCode::REL_HWHEEL_HI_RES);
    VirtualDevice::builder()?
        .name("deckhand virtual mouse")
        .with_keys(&buttons)?
        .with_relative_axes(&rel)?
        .build()
}

fn build_gamepad() -> io::Result<VirtualDevice> {
    let mut buttons = AttributeSet::<KeyCode>::new();
    for b in GamepadButton::ALL {
        // Dpad directions fold into the hat, and the axis pseudo-buttons into the stick/trigger
        // axes - neither is advertised as a `BTN_*` key.
        if !b.is_dpad() && !b.is_axis_button() {
            buttons.insert(gamepad_code(b));
        }
    }
    let mut ff = AttributeSet::<FFEffectCode>::new();
    ff.insert(FFEffectCode::FF_RUMBLE);

    // Xbox 360 identity: bus USB, Microsoft vendor, X360 product. SDL keys its built-in
    // mapping off this + the xpad code set below -> games see a standard Xbox pad.
    let mut builder = VirtualDevice::builder()?
        .name("Microsoft X-Box 360 pad")
        .input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0114))
        .with_keys(&buttons)?
        .with_ff(&ff)?
        .with_ff_effects_max(FF_MAX_EFFECTS);
    for axis in GamepadAxis::ALL {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(abs_code(axis), abs_info(axis)))?;
    }
    // Dpad hat (the four dpad `GamepadButton`s fold into these, -1..1 per axis).
    let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
    for code in [AbsoluteAxisCode::ABS_HAT0X, AbsoluteAxisCode::ABS_HAT0Y] {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, hat))?;
    }
    let device = builder.build()?;
    set_nonblocking(&device)?; // so poll_rumble's fetch_events doesn't block on no FF
    Ok(device)
}

/// Put a device's fd in non-blocking mode (evdev's sync `VirtualDevice` reads block
/// otherwise - only its tokio path sets `O_NONBLOCK`).
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

// --- vocabulary -> evdev code mapping ---

fn key_code(k: &Key) -> KeyCode {
    match k {
        Key::LeftShift => KeyCode::KEY_LEFTSHIFT,
        Key::RightShift => KeyCode::KEY_RIGHTSHIFT,
        Key::LeftCtrl => KeyCode::KEY_LEFTCTRL,
        Key::RightCtrl => KeyCode::KEY_RIGHTCTRL,
        Key::LeftAlt => KeyCode::KEY_LEFTALT,
        Key::RightAlt => KeyCode::KEY_RIGHTALT,
        Key::LeftMeta => KeyCode::KEY_LEFTMETA,
        Key::RightMeta => KeyCode::KEY_RIGHTMETA,
        Key::Esc => KeyCode::KEY_ESC,
        Key::Tab => KeyCode::KEY_TAB,
        Key::CapsLock => KeyCode::KEY_CAPSLOCK,
        Key::Backspace => KeyCode::KEY_BACKSPACE,
        Key::Enter => KeyCode::KEY_ENTER,
        Key::Space => KeyCode::KEY_SPACE,
        Key::Compose => KeyCode::KEY_COMPOSE,
        Key::Menu => KeyCode::KEY_MENU,
        Key::Up => KeyCode::KEY_UP,
        Key::Down => KeyCode::KEY_DOWN,
        Key::Left => KeyCode::KEY_LEFT,
        Key::Right => KeyCode::KEY_RIGHT,
        Key::Insert => KeyCode::KEY_INSERT,
        Key::Delete => KeyCode::KEY_DELETE,
        Key::Home => KeyCode::KEY_HOME,
        Key::End => KeyCode::KEY_END,
        Key::PageUp => KeyCode::KEY_PAGEUP,
        Key::PageDown => KeyCode::KEY_PAGEDOWN,
        Key::Grave => KeyCode::KEY_GRAVE,
        Key::K102nd => KeyCode::KEY_102ND,
        Key::Minus => KeyCode::KEY_MINUS,
        Key::Equal => KeyCode::KEY_EQUAL,
        Key::LeftBrace => KeyCode::KEY_LEFTBRACE,
        Key::RightBrace => KeyCode::KEY_RIGHTBRACE,
        Key::Backslash => KeyCode::KEY_BACKSLASH,
        Key::Semicolon => KeyCode::KEY_SEMICOLON,
        Key::Apostrophe => KeyCode::KEY_APOSTROPHE,
        Key::Comma => KeyCode::KEY_COMMA,
        Key::Dot => KeyCode::KEY_DOT,
        Key::Slash => KeyCode::KEY_SLASH,
        Key::D1 => KeyCode::KEY_1,
        Key::D2 => KeyCode::KEY_2,
        Key::D3 => KeyCode::KEY_3,
        Key::D4 => KeyCode::KEY_4,
        Key::D5 => KeyCode::KEY_5,
        Key::D6 => KeyCode::KEY_6,
        Key::D7 => KeyCode::KEY_7,
        Key::D8 => KeyCode::KEY_8,
        Key::D9 => KeyCode::KEY_9,
        Key::D0 => KeyCode::KEY_0,
        Key::Q => KeyCode::KEY_Q,
        Key::W => KeyCode::KEY_W,
        Key::E => KeyCode::KEY_E,
        Key::R => KeyCode::KEY_R,
        Key::T => KeyCode::KEY_T,
        Key::Y => KeyCode::KEY_Y,
        Key::U => KeyCode::KEY_U,
        Key::I => KeyCode::KEY_I,
        Key::O => KeyCode::KEY_O,
        Key::P => KeyCode::KEY_P,
        Key::A => KeyCode::KEY_A,
        Key::S => KeyCode::KEY_S,
        Key::D => KeyCode::KEY_D,
        Key::F => KeyCode::KEY_F,
        Key::G => KeyCode::KEY_G,
        Key::H => KeyCode::KEY_H,
        Key::J => KeyCode::KEY_J,
        Key::K => KeyCode::KEY_K,
        Key::L => KeyCode::KEY_L,
        Key::Z => KeyCode::KEY_Z,
        Key::X => KeyCode::KEY_X,
        Key::C => KeyCode::KEY_C,
        Key::V => KeyCode::KEY_V,
        Key::B => KeyCode::KEY_B,
        Key::N => KeyCode::KEY_N,
        Key::M => KeyCode::KEY_M,
        Key::F1 => KeyCode::KEY_F1,
        Key::F2 => KeyCode::KEY_F2,
        Key::F3 => KeyCode::KEY_F3,
        Key::F4 => KeyCode::KEY_F4,
        Key::F5 => KeyCode::KEY_F5,
        Key::F6 => KeyCode::KEY_F6,
        Key::F7 => KeyCode::KEY_F7,
        Key::F8 => KeyCode::KEY_F8,
        Key::F9 => KeyCode::KEY_F9,
        Key::F10 => KeyCode::KEY_F10,
        Key::F11 => KeyCode::KEY_F11,
        Key::F12 => KeyCode::KEY_F12,
        Key::Print => KeyCode::KEY_PRINT,
        Key::SysRq => KeyCode::KEY_SYSRQ,
        Key::ScrollLock => KeyCode::KEY_SCROLLLOCK,
        Key::Pause => KeyCode::KEY_PAUSE,
        Key::NumLock => KeyCode::KEY_NUMLOCK,
        Key::KpSlash => KeyCode::KEY_KPSLASH,
        Key::KpAsterisk => KeyCode::KEY_KPASTERISK,
        Key::KpMinus => KeyCode::KEY_KPMINUS,
        Key::KpPlus => KeyCode::KEY_KPPLUS,
        Key::KpEnter => KeyCode::KEY_KPENTER,
        Key::Kp7 => KeyCode::KEY_KP7,
        Key::Kp8 => KeyCode::KEY_KP8,
        Key::Kp9 => KeyCode::KEY_KP9,
        Key::Kp4 => KeyCode::KEY_KP4,
        Key::Kp5 => KeyCode::KEY_KP5,
        Key::Kp6 => KeyCode::KEY_KP6,
        Key::Kp1 => KeyCode::KEY_KP1,
        Key::Kp2 => KeyCode::KEY_KP2,
        Key::Kp3 => KeyCode::KEY_KP3,
        Key::Kp0 => KeyCode::KEY_KP0,
        Key::KpDot => KeyCode::KEY_KPDOT,
        Key::Mute => KeyCode::KEY_MUTE,
        Key::VolumeDown => KeyCode::KEY_VOLUMEDOWN,
        Key::VolumeUp => KeyCode::KEY_VOLUMEUP,
        Key::MicMute => KeyCode::KEY_MICMUTE,
        Key::PlayPause => KeyCode::KEY_PLAYPAUSE,
        Key::Play => KeyCode::KEY_PLAY,
        Key::PreviousSong => KeyCode::KEY_PREVIOUSSONG,
        Key::NextSong => KeyCode::KEY_NEXTSONG,
        Key::Rewind => KeyCode::KEY_REWIND,
        Key::FastForward => KeyCode::KEY_FASTFORWARD,
        Key::StopCd => KeyCode::KEY_STOPCD,
        Key::Back => KeyCode::KEY_BACK,
        Key::Forward => KeyCode::KEY_FORWARD,
        Key::BrightnessDown => KeyCode::KEY_BRIGHTNESSDOWN,
        Key::BrightnessUp => KeyCode::KEY_BRIGHTNESSUP,
        Key::BrightnessCycle => KeyCode::KEY_BRIGHTNESS_CYCLE,
        Key::BrightnessAuto => KeyCode::KEY_BRIGHTNESS_AUTO,
        Key::KbdIllumToggle => KeyCode::KEY_KBDILLUMTOGGLE,
        Key::KbdIllumDown => KeyCode::KEY_KBDILLUMDOWN,
        Key::KbdIllumUp => KeyCode::KEY_KBDILLUMUP,
    }
}

fn mouse_code(b: &MouseButton) -> KeyCode {
    match b {
        MouseButton::Left => KeyCode::BTN_LEFT,
        MouseButton::Right => KeyCode::BTN_RIGHT,
        MouseButton::Middle => KeyCode::BTN_MIDDLE,
        MouseButton::Back => KeyCode::BTN_SIDE,
        MouseButton::Forward => KeyCode::BTN_EXTRA,
        MouseButton::ScrollUp
        | MouseButton::ScrollDown
        | MouseButton::ScrollLeft
        | MouseButton::ScrollRight => {
            unreachable!("scroll pseudo-buttons are realized as wheel ticks - see scroll_delta")
        }
    }
}

/// The wheel direction for a scroll pseudo-button: `(horizontal, +/-1 notch)`; `None` for real
/// buttons. Signs: up/right = `+1` (`REL_WHEEL`/`REL_HWHEEL` convention).
fn scroll_delta(b: &MouseButton) -> Option<(bool, i32)> {
    match b {
        MouseButton::ScrollUp => Some((false, 1)),
        MouseButton::ScrollDown => Some((false, -1)),
        MouseButton::ScrollRight => Some((true, 1)),
        MouseButton::ScrollLeft => Some((true, -1)),
        _ => None,
    }
}

/// Emit `notches` discrete wheel notches as BOTH the hi-res value (120 per notch) and the legacy
/// notch. A device that advertises `REL_WHEEL_HI_RES` (ours does, for smooth scroll) MUST send the
/// hi-res event - libinput uses it and **ignores a lone legacy `REL_WHEEL`**, so discrete scroll is
/// silent under Wayland without this. (Legacy stays for non-hi-res X consumers.)
fn push_wheel(mouse: &mut Vec<InputEvent>, horizontal: bool, notches: i32) {
    if notches == 0 {
        return;
    }
    let (legacy, hi_res) = if horizontal {
        (RelativeAxisCode::REL_HWHEEL, RelativeAxisCode::REL_HWHEEL_HI_RES)
    } else {
        (RelativeAxisCode::REL_WHEEL, RelativeAxisCode::REL_WHEEL_HI_RES)
    };
    mouse.push(*RelativeAxisEvent::new(hi_res, notches * SCROLL_HI_RES_PER_NOTCH));
    mouse.push(*RelativeAxisEvent::new(legacy, notches));
}

/// Hi-res wheel units per detent (kernel/libinput convention; matches Windows `WHEEL_DELTA`).
const SCROLL_HI_RES_PER_NOTCH: i32 = vocab_out::SCROLL_HI_RES_PER_DETENT;

/// Add a hi-res scroll `delta` (units of 1/120 detent) to `accum`, returning the number of full
/// legacy wheel notches that accumulated (with sign), keeping the sub-notch remainder. `None` when
/// no full notch crossed this event.
fn accumulate_notch(accum: &mut i32, delta: i32) -> Option<i32> {
    *accum += delta;
    let notches = *accum / SCROLL_HI_RES_PER_NOTCH; // truncates toward zero
    if notches != 0 {
        *accum -= notches * SCROLL_HI_RES_PER_NOTCH;
        Some(notches)
    } else {
        None
    }
}

// A=SOUTH, B=EAST, X=NORTH, Y=WEST - matches xpad/X360 (SDL maps these to A/B/X/Y).
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
        GamepadButton::DpadUp
        | GamepadButton::DpadDown
        | GamepadButton::DpadLeft
        | GamepadButton::DpadRight => {
            unreachable!("dpad directions fold into the hat - see Dpad / emit")
        }
        b if b.is_axis_button() => {
            unreachable!("axis pseudo-buttons fold into stick/trigger axes - see AxisButtons / emit")
        }
        _ => unreachable!("gamepad_code covers every non-hat, non-axis button"),
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
    }
}

// xpad ranges: sticks i16, triggers 0..255. (Dpad is a hat, set directly in emit.)
fn abs_info(a: &GamepadAxis) -> AbsInfo {
    match a {
        GamepadAxis::LeftStickX
        | GamepadAxis::LeftStickY
        | GamepadAxis::RightStickX
        | GamepadAxis::RightStickY => AbsInfo::new(0, -32768, 32767, 16, 128, 0),
        GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger => AbsInfo::new(0, 0, 255, 0, 0, 0),
    }
}

// Normalized f32 -> device units: sticks from -1..1, triggers from 0..1.
fn abs_value(a: &GamepadAxis, v: f32) -> i32 {
    match a {
        GamepadAxis::LeftStickX
        | GamepadAxis::LeftStickY
        | GamepadAxis::RightStickX
        | GamepadAxis::RightStickY => (v.clamp(-1.0, 1.0) * 32767.0) as i32,
        GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger => (v.clamp(0.0, 1.0) * 255.0) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::accumulate_notch;

    #[test]
    fn accumulate_notch_synthesizes_legacy_ticks_per_120() {
        let mut acc = 0;
        assert_eq!(accumulate_notch(&mut acc, 80), None); // below a detent
        assert_eq!(acc, 80);
        assert_eq!(accumulate_notch(&mut acc, 50), Some(1)); // 130 -> one notch, 10 remainder
        assert_eq!(acc, 10);
        // A big fast scroll crosses several notches at once, sign preserved.
        let mut acc = 0;
        assert_eq!(accumulate_notch(&mut acc, -250), Some(-2));
        assert_eq!(acc, -10);
    }
}
