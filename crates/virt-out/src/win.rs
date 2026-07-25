//! Windows backend: keyboard/mouse via `SendInput`, a virtual Xbox 360 pad via ViGEm
//! (the ViGEmBus driver, through the `vigem-client` crate). The gamepad advertises the
//! standard X360 identity so games see a normal Xbox pad, and we receive game rumble
//! back over ViGEm's notification channel (PLAN §2.1 FF back-channel).
//!
//! Mirrors the Linux backend's [`Sink`] API exactly (`new` / `emit` / `poll_rumble`) so
//! the engine and the `bridge` example are platform-agnostic.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread::JoinHandle;

use vigem_client::{Client, TargetId, XButtons, XGamepad, XTarget};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    MAPVK_VK_TO_VSC, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MapVirtualKeyW, SendInput, VIRTUAL_KEY, VK_0, VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7,
    VK_8, VK_9, VK_A, VK_B, VK_BACK, VK_C, VK_D, VK_DOWN, VK_E, VK_ESCAPE, VK_F, VK_G, VK_H,
    VK_I, VK_J, VK_K, VK_L, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_M, VK_N,
    VK_O, VK_P, VK_Q, VK_R, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_S,
    VK_SPACE, VK_T, VK_TAB, VK_U, VK_UP, VK_V, VK_W, VK_X, VK_Y, VK_Z,
};

use crate::event::{GamepadAxis, GamepadButton, Key, MouseButton, OutputEvent, Rumble};

/// `mouseData` values for the extra mouse buttons (X1 = back, X2 = forward).
const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;
/// One wheel notch (`WHEEL_DELTA`).
const WHEEL_DELTA: i32 = 120;

/// Latest rumble from the virtual pad, written by the ViGEm notification thread and read
/// by [`Sink::poll_rumble`]. ViGEm reports motor speeds as `u8` (the high byte of the
/// XInput `u16`); we widen back on read.
struct RumbleState {
    strong: AtomicU8, // large / low-frequency motor
    weak: AtomicU8,   // small / high-frequency motor
}

/// The output sink: owns the virtual Xbox 360 pad and realizes [`OutputEvent`]s
/// (keyboard/mouse via `SendInput`, gamepad via ViGEm). Sync — call [`Sink::emit`] from
/// the engine's mapping loop; [`Sink::poll_rumble`] returns the pad's current rumble.
/// Dropping the `Sink` unplugs the virtual pad and stops the notification thread.
pub struct Sink {
    target: XTarget,
    // The full gamepad report we resubmit whenever any gamepad input changes. `buttons`
    // holds only the non-dpad bits; the dpad hat is kept separately and OR'd in at submit
    // (both live in the same XInput button word).
    gamepad: XGamepad,
    dpad: (i32, i32),
    // Rumble back-channel: a notification thread stores the latest motor speeds here.
    rumble: Arc<RumbleState>,
    notif: Option<JoinHandle<()>>,
}

impl Sink {
    /// Connect to ViGEmBus, plug in a virtual Xbox 360 pad, and start the rumble
    /// notification thread. Fails if the ViGEmBus driver isn't installed/running.
    pub fn new() -> crate::Result<Self> {
        let client = Client::connect()?;
        let mut target = XTarget::new(client, TargetId::XBOX360_WIRED);
        target.plugin()?;
        target.wait_ready()?;

        let rumble = Arc::new(RumbleState {
            strong: AtomicU8::new(0),
            weak: AtomicU8::new(0),
        });
        // The notification thread blocks on ViGEm and stores each rumble update. It
        // exits when the target is unplugged (drop), which aborts its pending request.
        let sink_rumble = Arc::clone(&rumble);
        let notif = target.request_notification()?.spawn_thread(move |_, data| {
            sink_rumble.strong.store(data.large_motor, Ordering::Relaxed);
            sink_rumble.weak.store(data.small_motor, Ordering::Relaxed);
        });

        Ok(Self {
            target,
            gamepad: XGamepad::default(),
            dpad: (0, 0),
            rumble,
            notif: Some(notif),
        })
    }

    /// Realize a batch of output events. Keyboard/mouse events are injected together via
    /// one `SendInput` call; gamepad changes are accumulated into the pad report and
    /// submitted once (a full XInput report) if anything changed.
    pub fn emit(&mut self, events: &[OutputEvent]) -> crate::Result<()> {
        let mut inputs: Vec<INPUT> = Vec::new();
        let mut gp_dirty = false;

        for ev in events {
            match ev {
                OutputEvent::Key(k, down) => inputs.push(key_input(k, *down)),
                OutputEvent::MouseButton(b, down) => inputs.push(mouse_button_input(b, *down)),
                OutputEvent::MouseMove { dx, dy } => inputs.push(mouse_move_input(*dx, *dy)),
                OutputEvent::Scroll { dx, dy } => {
                    if *dy != 0 {
                        inputs.push(wheel_input(*dy, false));
                    }
                    if *dx != 0 {
                        inputs.push(wheel_input(*dx, true));
                    }
                }
                OutputEvent::GamepadButton(b, down) => {
                    set_button(&mut self.gamepad.buttons, b, *down);
                    gp_dirty = true;
                }
                OutputEvent::GamepadAxis(a, v) => {
                    self.set_axis(a, *v);
                    gp_dirty = true;
                }
            }
        }

        if !inputs.is_empty() {
            // Safety: `inputs` is a valid slice of correctly-initialized INPUTs; cbsize is
            // the size of one INPUT, as the API requires.
            let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
            if sent as usize != inputs.len() {
                // Synthetic input is best-effort: the OS refuses injection (UIPI) while a
                // higher-integrity or *switching* input desktop owns the foreground — e.g.
                // a fullscreen/elevated game exiting, or a UAC/secure-desktop prompt. That
                // shows up as ERROR_ACCESS_DENIED (or, per the SendInput docs, no error set
                // at all for UIPI). It's transient and environmental, not a fault we can act
                // on, so drop the frame and carry on rather than tearing down the mapper.
                // Any other failure is unexpected (likely a malformed INPUT = our bug) and
                // stays fatal. (A persistent denial — deckhand not elevated vs. an elevated
                // game — is left for the engine/UI to detect and advise on; PLAN §2.1.)
                let err = std::io::Error::last_os_error();
                const ERROR_ACCESS_DENIED: i32 = 5;
                const ERROR_SUCCESS: i32 = 0;
                match err.raw_os_error() {
                    Some(ERROR_ACCESS_DENIED) | Some(ERROR_SUCCESS) => {}
                    _ => return Err(err.into()),
                }
            }
        }
        if gp_dirty {
            // Fold the dpad hat into the button word alongside the face/shoulder bits.
            let raw = (self.gamepad.buttons.raw & !DPAD_MASK) | dpad_bits(self.dpad);
            self.gamepad.buttons = XButtons { raw };
            self.target.update(&self.gamepad)?;
        }
        Ok(())
    }

    /// Return the virtual pad's current rumble (zero if nothing is playing). Non-blocking
    /// — reads the latest motor speeds captured by the notification thread. Route the
    /// result onward to real-controller haptics (PLAN §2.1 / §6).
    pub fn poll_rumble(&mut self) -> crate::Result<Rumble> {
        // ViGEm delivers each motor as the high byte of the XInput u16; widen by ×257 so
        // 0xFF maps to 0xFFFF (full scale) rather than 0xFF00.
        let widen = |v: u8| (v as u16) * 257;
        Ok(Rumble {
            strong: widen(self.rumble.strong.load(Ordering::Relaxed)),
            weak: widen(self.rumble.weak.load(Ordering::Relaxed)),
        })
    }

    fn set_axis(&mut self, a: &GamepadAxis, v: f32) {
        match a {
            GamepadAxis::LeftStickX => self.gamepad.thumb_lx = stick(v),
            // XInput sticks are +up; the shared vocabulary uses the evdev sign (+down, see
            // the Linux backend / the bridge's `-s.left_stick.y`), so negate Y here.
            GamepadAxis::LeftStickY => self.gamepad.thumb_ly = stick(-v),
            GamepadAxis::RightStickX => self.gamepad.thumb_rx = stick(v),
            GamepadAxis::RightStickY => self.gamepad.thumb_ry = stick(-v),
            GamepadAxis::LeftTrigger => self.gamepad.left_trigger = trigger(v),
            GamepadAxis::RightTrigger => self.gamepad.right_trigger = trigger(v),
            GamepadAxis::DpadX => self.dpad.0 = v.round().clamp(-1.0, 1.0) as i32,
            GamepadAxis::DpadY => self.dpad.1 = v.round().clamp(-1.0, 1.0) as i32,
        }
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        // Unplug first: this aborts the notification thread's pending request so its
        // blocking poll returns and the thread exits; then join it. (The target's own
        // Drop would unplug too, but we need the unplug *before* the join.)
        let _ = self.target.unplug();
        if let Some(handle) = self.notif.take() {
            let _ = handle.join();
        }
    }
}

// --- gamepad vocabulary → XInput mapping ---

/// The four dpad-hat bits within the XInput button word.
const DPAD_MASK: u16 = XButtons::UP | XButtons::DOWN | XButtons::LEFT | XButtons::RIGHT;

fn set_button(buttons: &mut XButtons, b: &GamepadButton, down: bool) {
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
    };
    if down {
        buttons.raw |= bit;
    } else {
        buttons.raw &= !bit;
    }
}

/// Dpad hat state (`+1`/`-1` per axis) → XInput dpad bits. `DpadX +1 = right`,
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

/// Normalized stick `-1.0..=1.0` → XInput `i16`.
fn stick(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Normalized trigger `0.0..=1.0` → XInput `u8`.
fn trigger(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * u8::MAX as f32) as u8
}

// --- keyboard/mouse → SendInput ---

/// Build a keyboard `INPUT` using scancode injection (games often read scancodes, not
/// virtual keys). The scancode comes from the virtual key via `MapVirtualKeyW`; extended
/// keys (arrows, right ctrl/alt, meta) get the extended-key flag so the E0 prefix is set.
fn key_input(k: &Key, down: bool) -> INPUT {
    let vk = key_vk(k);
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYEVENTF_SCANCODE;
    if is_extended(k) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0), // ignored when KEYEVENTF_SCANCODE is set
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_move_input(dx: i32, dy: i32) -> INPUT {
    mouse_input(dx, dy, 0, MOUSEEVENTF_MOVE)
}

fn mouse_button_input(b: &MouseButton, down: bool) -> INPUT {
    let (flags, data) = match (b, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
    };
    mouse_input(0, 0, data, flags)
}

/// Wheel `INPUT`. `mouseData` is a signed notch count × `WHEEL_DELTA`, passed as the
/// two's-complement `u32` the API expects.
fn wheel_input(ticks: i32, horizontal: bool) -> INPUT {
    let flags = if horizontal {
        MOUSEEVENTF_HWHEEL
    } else {
        MOUSEEVENTF_WHEEL
    };
    mouse_input(0, 0, (ticks * WHEEL_DELTA) as u32, flags)
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Keys needing the extended-key flag (E0-prefixed scancodes): arrows, right ctrl/alt,
/// and the left meta/Windows key. (Right shift is *not* extended.)
fn is_extended(k: &Key) -> bool {
    matches!(
        k,
        Key::Up | Key::Down | Key::Left | Key::Right | Key::RightCtrl | Key::RightAlt | Key::LeftMeta
    )
}

fn key_vk(k: &Key) -> VIRTUAL_KEY {
    match k {
        Key::A => VK_A,
        Key::B => VK_B,
        Key::C => VK_C,
        Key::D => VK_D,
        Key::E => VK_E,
        Key::F => VK_F,
        Key::G => VK_G,
        Key::H => VK_H,
        Key::I => VK_I,
        Key::J => VK_J,
        Key::K => VK_K,
        Key::L => VK_L,
        Key::M => VK_M,
        Key::N => VK_N,
        Key::O => VK_O,
        Key::P => VK_P,
        Key::Q => VK_Q,
        Key::R => VK_R,
        Key::S => VK_S,
        Key::T => VK_T,
        Key::U => VK_U,
        Key::V => VK_V,
        Key::W => VK_W,
        Key::X => VK_X,
        Key::Y => VK_Y,
        Key::Z => VK_Z,
        Key::Num0 => VK_0,
        Key::Num1 => VK_1,
        Key::Num2 => VK_2,
        Key::Num3 => VK_3,
        Key::Num4 => VK_4,
        Key::Num5 => VK_5,
        Key::Num6 => VK_6,
        Key::Num7 => VK_7,
        Key::Num8 => VK_8,
        Key::Num9 => VK_9,
        Key::Space => VK_SPACE,
        Key::Enter => VK_RETURN,
        Key::Escape => VK_ESCAPE,
        Key::Tab => VK_TAB,
        Key::Backspace => VK_BACK,
        Key::Up => VK_UP,
        Key::Down => VK_DOWN,
        Key::Left => VK_LEFT,
        Key::Right => VK_RIGHT,
        Key::LeftShift => VK_LSHIFT,
        Key::RightShift => VK_RSHIFT,
        Key::LeftCtrl => VK_LCONTROL,
        Key::RightCtrl => VK_RCONTROL,
        Key::LeftAlt => VK_LMENU,
        Key::RightAlt => VK_RMENU,
        Key::LeftMeta => VK_LWIN,
    }
}
