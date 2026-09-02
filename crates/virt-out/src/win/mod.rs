//! Windows backend: keyboard/mouse via `SendInput`, and a virtual gamepad via a
//! compile-time-selected **controller backend** ([`vigem`] - a virtual Xbox 360 pad through
//! the ViGEmBus driver / the `vigem-client` crate; later `viiper`; or [`none`] when neither
//! feature is enabled). The gamepad advertises a standard Xbox 360 identity so games see a
//! normal pad, and game rumble flows back over the backend's notification channel
//! (PLAN 2.1 FF back-channel).
//!
//! Mirrors the Linux backend's [`Sink`] API exactly (`new` / `emit` / `poll_rumble`) so the
//! engine and the `bridge` example stay platform-agnostic. Only the gamepad path is
//! backend-specific; keyboard/mouse is always `SendInput` regardless of the controller
//! backend (so a `--no-default-features` build still maps kb/mouse fully).

use std::mem::size_of;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    MAPVK_VK_TO_VSC, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MapVirtualKeyW, SendInput, VIRTUAL_KEY,
    // Media / volume / browser keys - a VK but no keyboard scancode (injected by virtual key).
    VK_BROWSER_BACK, VK_BROWSER_FORWARD, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE,
    VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
    VK_0, VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7, VK_8, VK_9,
    VK_A, VK_B, VK_C, VK_D, VK_E, VK_F, VK_G, VK_H, VK_I, VK_J, VK_K, VK_L, VK_M, VK_N, VK_O,
    VK_P, VK_Q, VK_R, VK_S, VK_T, VK_U, VK_V, VK_W, VK_X, VK_Y, VK_Z,
    VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_F10, VK_F11, VK_F12,
    VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6,
    VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9,
    VK_ADD, VK_APPS, VK_BACK, VK_CAPITAL, VK_DECIMAL, VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END,
    VK_ESCAPE, VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_MULTIPLY, VK_NEXT, VK_NUMLOCK, VK_OEM_1, VK_OEM_102, VK_OEM_2, VK_OEM_3, VK_OEM_4,
    VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS,
    VK_PAUSE, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN,
    VK_SCROLL, VK_SNAPSHOT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
};

use crate::event::{OutputEvent, Rumble};
use vocab_out::{GamepadAxis, GamepadButton, Key, MouseButton};

// --- controller backend selection (compile-time, mutually exclusive) ---
//
// Exactly one backend type is compiled in, aliased to `Backend`: `vigem` (ViGEmBus pad),
// `viiper` (VIIPER USB/IP pad), or - with neither feature - the `none` stub, which drops
// gamepad output with a warning while kb/mouse keep working. The two real backends are
// mutually exclusive; enabling both is a build error rather than a silent pick.
//
// `Sink` holds the selected `Backend` directly and drives only the gamepad path through it:
// state is accumulated via `set_button`/`set_axis` and pushed to the OS by `flush` (once per
// `emit`, only if something changed); `poll_rumble` reads the pad's current rumble. Keyboard/
// mouse never touch the backend (always `SendInput`). The three backends share that method
// set by convention - the identical shape is what lets `Backend` be a drop-in alias.

#[cfg(all(feature = "vigem", feature = "viiper"))]
compile_error!(
    "features `vigem` and `viiper` are mutually exclusive - enable at most one controller backend"
);

#[cfg(feature = "vigem")]
mod vigem;
#[cfg(feature = "vigem")]
use vigem::VigemController as Backend;

// `not(vigem)` so that if both features are (mistakenly) on, only one `Backend` is defined and
// the `compile_error!` above is the sole error rather than being buried under a name clash.
#[cfg(all(feature = "viiper", not(feature = "vigem")))]
mod viiper;
#[cfg(all(feature = "viiper", not(feature = "vigem")))]
use viiper::ViiperController as Backend;

#[cfg(not(any(feature = "vigem", feature = "viiper")))]
mod none;
#[cfg(not(any(feature = "vigem", feature = "viiper")))]
use none::NoController as Backend;

/// The output sink: realizes [`OutputEvent`]s - keyboard/mouse via `SendInput`, gamepad via
/// the compile-time-selected controller backend. Sync - call [`Sink::emit`] from the engine's
/// mapping loop; [`Sink::poll_rumble`] returns the pad's current rumble. Dropping the `Sink`
/// drops the backend (unplugging the virtual pad, if any).
pub struct Sink {
    controller: Backend,
}

impl Sink {
    /// Create the sink: initialize the controller backend (plug in the virtual pad, if the
    /// selected backend has one) and prepare the kb/mouse path. Fails if the backend's driver
    /// isn't available.
    pub fn new() -> crate::Result<Self> {
        Ok(Self {
            controller: Backend::new()?,
        })
    }

    /// Realize a batch of output events. Keyboard/mouse events are injected together via one
    /// `SendInput` call; gamepad changes are accumulated in the backend and flushed once (a
    /// full report) if anything changed.
    pub fn emit(&mut self, events: &[OutputEvent]) -> crate::Result<()> {
        let mut inputs: Vec<INPUT> = Vec::new();

        for ev in events {
            match ev {
                OutputEvent::Key(k, down) => {
                    // Warn once per press for keys this backend can't realize (no Windows VK), so
                    // a mis-bound key isn't silently dropped. Release edge stays quiet.
                    if *down && is_unsupported(k) {
                        log::warn!(
                            "virt-out(win): key {k:?} has no Windows key mapping; output dropped"
                        );
                    }
                    if let Some(inp) = key_input(k, *down) {
                        inputs.push(inp);
                    }
                }
                OutputEvent::MouseButton(b, down) => match scroll_of(b) {
                    // Scroll pseudo-button: one wheel tick on press; release is a no-op.
                    Some((ticks, horizontal)) => {
                        if *down {
                            inputs.push(wheel_input(ticks, horizontal));
                        }
                    }
                    None => inputs.push(mouse_button_input(b, *down)),
                },
                OutputEvent::MouseMove { dx, dy } => inputs.push(mouse_move_input(*dx, *dy)),
                OutputEvent::Scroll { dx, dy } => {
                    if *dy != 0 {
                        inputs.push(wheel_input(*dy, false));
                    }
                    if *dx != 0 {
                        inputs.push(wheel_input(*dx, true));
                    }
                }
                OutputEvent::SmoothScroll { dx, dy } => {
                    // Hi-res units are already in WHEEL_DELTA scale (120 = one notch); pass them
                    // straight through as sub-notch mouseData. Windows accumulates for apps that
                    // don't do smooth scroll, so no legacy fallback is needed here.
                    if *dy != 0 {
                        inputs.push(wheel_hires_input(*dy, false));
                    }
                    if *dx != 0 {
                        inputs.push(wheel_hires_input(*dx, true));
                    }
                }
                OutputEvent::GamepadButton(b, down) => self.controller.set_button(b, *down),
                OutputEvent::GamepadAxis(a, v) => self.controller.set_axis(a, *v),
            }
        }

        if !inputs.is_empty() {
            // Safety: `inputs` is a valid slice of correctly-initialized INPUTs; cbsize is
            // the size of one INPUT, as the API requires.
            let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
            if sent as usize != inputs.len() {
                // Synthetic input is best-effort: the OS refuses injection (UIPI) while a
                // higher-integrity or *switching* input desktop owns the foreground - e.g.
                // a fullscreen/elevated game exiting, or a UAC/secure-desktop prompt. That
                // shows up as ERROR_ACCESS_DENIED (or, per the SendInput docs, no error set
                // at all for UIPI). It's transient and environmental, not a fault we can act
                // on, so drop the frame and carry on rather than tearing down the mapper.
                // Any other failure is unexpected (likely a malformed INPUT = our bug) and
                // stays fatal. (A persistent denial - deckhand not elevated vs. an elevated
                // game - is left for the engine/UI to detect and advise on; PLAN 2.1.)
                let err = std::io::Error::last_os_error();
                const ERROR_ACCESS_DENIED: i32 = 5;
                const ERROR_SUCCESS: i32 = 0;
                match err.raw_os_error() {
                    Some(ERROR_ACCESS_DENIED) | Some(ERROR_SUCCESS) => {}
                    _ => return Err(err.into()),
                }
            }
        }

        self.controller.flush()
    }

    /// Return the virtual pad's current rumble (zero if nothing is playing, or if no
    /// controller backend is compiled in). Non-blocking. Route the result onward to
    /// real-controller haptics (PLAN 2.1 / 6).
    pub fn poll_rumble(&mut self) -> crate::Result<Rumble> {
        self.controller.poll_rumble()
    }
}

// --- keyboard/mouse -> SendInput ---

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
        (MouseButton::ScrollUp | MouseButton::ScrollDown | MouseButton::ScrollLeft
        | MouseButton::ScrollRight, _) => {
            unreachable!("scroll pseudo-buttons are realized as wheel ticks - see scroll_of")
        }
    };
    mouse_input(0, 0, data, flags)
}

/// Wheel ticks + horizontal flag for a scroll pseudo-button; `None` for real buttons.
fn scroll_of(b: &MouseButton) -> Option<(i32, bool)> {
    match b {
        MouseButton::ScrollUp => Some((1, false)),
        MouseButton::ScrollDown => Some((-1, false)),
        MouseButton::ScrollRight => Some((1, true)),
        MouseButton::ScrollLeft => Some((-1, true)),
        _ => None,
    }
}

/// Wheel `INPUT`. `mouseData` is a signed notch count x `WHEEL_DELTA`, passed as the
/// two's-complement `u32` the API expects.
fn wheel_input(ticks: i32, horizontal: bool) -> INPUT {
    let flags = if horizontal {
        MOUSEEVENTF_HWHEEL
    } else {
        MOUSEEVENTF_WHEEL
    };
    mouse_input(0, 0, (ticks * WHEEL_DELTA) as u32, flags)
}

/// Hi-res wheel `INPUT`: `units` are already in `WHEEL_DELTA` scale (120 = one notch), so they go
/// straight into `mouseData` - sub-`WHEEL_DELTA` values scroll smoothly in apps that support it.
fn wheel_hires_input(units: i32, horizontal: bool) -> INPUT {
    let flags = if horizontal {
        MOUSEEVENTF_HWHEEL
    } else {
        MOUSEEVENTF_WHEEL
    };
    mouse_input(0, 0, units as u32, flags)
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

/// `mouseData` values for the extra mouse buttons (X1 = back, X2 = forward).
const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;
/// One wheel notch (`WHEEL_DELTA`).
const WHEEL_DELTA: i32 = vocab_out::SCROLL_HI_RES_PER_DETENT;

/// Build a keyboard `INPUT`. Normal keys use **scancode injection** (games often read
/// scancodes, not virtual keys): the scancode comes from the VK via `MapVirtualKeyW`, and
/// extended keys (arrows, right ctrl/alt, meta, nav, numpad slash/enter) get the extended-key
/// flag so the E0 prefix is set. The media / volume / browser keys are instead injected by
/// **virtual key** (`is_vk_only`): they *do* have scancodes, but only the **E0-extended** ones -
/// injecting that scancode without the E0 prefix collides with an ordinary letter (volume-up's
/// scancode `0x30` is `B`, volume-down's `0x2E` is `C`), so we bypass the scancode path entirely
/// and let the shell consume the consumer-control VK. Returns `None` only for keys with no VK at
/// all (`Compose`, exotic keypad, and the brightness/keyboard-illumination keys Windows has no VK
/// for). Linux maps everything via evdev.
fn key_input(k: &Key, down: bool) -> Option<INPUT> {
    let vk = key_vk(k)?;
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;

    let (wvk, wscan, mut flags) = if scan != 0 && !is_vk_only(k) {
        // Scancode injection (games read scancodes). Extended keys get the E0 flag.
        let mut f = KEYEVENTF_SCANCODE;
        if is_extended(k) {
            f |= KEYEVENTF_EXTENDEDKEY;
        }
        (VIRTUAL_KEY(0), scan, f) // wVk ignored when KEYEVENTF_SCANCODE is set
    } else {
        // Media / volume / browser (or a key Windows gives no scancode) -> inject by virtual key
        // directly. Plain VK injection (no extended flag) is the proven path for consumer-control
        // VKs; if one doesn't register on some setup, adding `KEYEVENTF_EXTENDEDKEY` here is the
        // first thing to try.
        (vk, 0u16, KEYBD_EVENT_FLAGS(0))
    };
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    Some(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: wvk, wScan: wscan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    })
}

/// Keys needing the extended-key flag (E0-prefixed scancodes): arrows, the navigation
/// cluster, right ctrl/alt, both meta/Windows keys, the application/menu key, numpad
/// divide, and numpad enter. (Right shift is *not* extended.)
fn is_extended(k: &Key) -> bool {
    matches!(
        k,
        Key::Up | Key::Down | Key::Left | Key::Right
            | Key::Insert | Key::Delete | Key::Home | Key::End | Key::PageUp | Key::PageDown
            | Key::RightCtrl | Key::RightAlt | Key::LeftMeta | Key::RightMeta | Key::Menu
            | Key::KpSlash | Key::KpEnter
    )
}

/// Keys that must be injected by **virtual key**, never by scancode. The consumer-control keys
/// (volume, media transport, browser navigation) have only E0-extended scancodes, so the plain
/// scancode `MapVirtualKeyW` returns would land on an ordinary letter (volume-up `0x30` -> `B`,
/// volume-down `0x2E` -> `C`). Injecting the VK lets the shell dispatch the real consumer action.
fn is_vk_only(k: &Key) -> bool {
    matches!(
        k,
        Key::Mute
            | Key::VolumeDown
            | Key::VolumeUp
            | Key::PlayPause
            | Key::PreviousSong
            | Key::NextSong
            | Key::StopCd
            | Key::Back
            | Key::Forward
    )
}

/// True for keys this backend can't realize: Windows has no virtual key for them at all
/// (`Compose`, brightness / keyboard-illumination, `MicMute`, and the media-transport keys with
/// no distinct VK - `Play`/`Rewind`/`FastForward`).
/// Defined as "`key_vk` has no mapping", so it can never drift from the actual `None` cases.
/// `key_input` drops these; `emit` logs a warning so a mis-bound key isn't silently swallowed.
fn is_unsupported(k: &Key) -> bool {
    key_vk(k).is_none()
}

/// Map a key to a Windows virtual-key, or `None` if Windows has no VK for it (`Compose`,
/// brightness / keyboard-illumination). The media/volume/browser keys map to a
/// consumer-control VK; `key_input` injects those by virtual key (`is_vk_only`) rather than by
/// their E0-extended scancode. Numpad Enter maps to Return + the extended flag.
fn key_vk(k: &Key) -> Option<VIRTUAL_KEY> {
    Some(match k {
        Key::LeftShift => VK_LSHIFT,
        Key::RightShift => VK_RSHIFT,
        Key::LeftCtrl => VK_LCONTROL,
        Key::RightCtrl => VK_RCONTROL,
        Key::LeftAlt => VK_LMENU,
        Key::RightAlt => VK_RMENU,
        Key::LeftMeta => VK_LWIN,
        Key::RightMeta => VK_RWIN,
        Key::Esc => VK_ESCAPE,
        Key::Tab => VK_TAB,
        Key::CapsLock => VK_CAPITAL,
        Key::Backspace => VK_BACK,
        Key::Enter => VK_RETURN,
        Key::Space => VK_SPACE,
        Key::Menu => VK_APPS,
        Key::Up => VK_UP,
        Key::Down => VK_DOWN,
        Key::Left => VK_LEFT,
        Key::Right => VK_RIGHT,
        Key::Insert => VK_INSERT,
        Key::Delete => VK_DELETE,
        Key::Home => VK_HOME,
        Key::End => VK_END,
        Key::PageUp => VK_PRIOR,
        Key::PageDown => VK_NEXT,
        Key::Grave => VK_OEM_3,
        Key::K102nd => VK_OEM_102,
        Key::Minus => VK_OEM_MINUS,
        Key::Equal => VK_OEM_PLUS,
        Key::LeftBrace => VK_OEM_4,
        Key::RightBrace => VK_OEM_6,
        Key::Backslash => VK_OEM_5,
        Key::Semicolon => VK_OEM_1,
        Key::Apostrophe => VK_OEM_7,
        Key::Comma => VK_OEM_COMMA,
        Key::Dot => VK_OEM_PERIOD,
        Key::Slash => VK_OEM_2,
        Key::D1 => VK_1,
        Key::D2 => VK_2,
        Key::D3 => VK_3,
        Key::D4 => VK_4,
        Key::D5 => VK_5,
        Key::D6 => VK_6,
        Key::D7 => VK_7,
        Key::D8 => VK_8,
        Key::D9 => VK_9,
        Key::D0 => VK_0,
        Key::Q => VK_Q,
        Key::W => VK_W,
        Key::E => VK_E,
        Key::R => VK_R,
        Key::T => VK_T,
        Key::Y => VK_Y,
        Key::U => VK_U,
        Key::I => VK_I,
        Key::O => VK_O,
        Key::P => VK_P,
        Key::A => VK_A,
        Key::S => VK_S,
        Key::D => VK_D,
        Key::F => VK_F,
        Key::G => VK_G,
        Key::H => VK_H,
        Key::J => VK_J,
        Key::K => VK_K,
        Key::L => VK_L,
        Key::Z => VK_Z,
        Key::X => VK_X,
        Key::C => VK_C,
        Key::V => VK_V,
        Key::B => VK_B,
        Key::N => VK_N,
        Key::M => VK_M,
        Key::F1 => VK_F1,
        Key::F2 => VK_F2,
        Key::F3 => VK_F3,
        Key::F4 => VK_F4,
        Key::F5 => VK_F5,
        Key::F6 => VK_F6,
        Key::F7 => VK_F7,
        Key::F8 => VK_F8,
        Key::F9 => VK_F9,
        Key::F10 => VK_F10,
        Key::F11 => VK_F11,
        Key::F12 => VK_F12,
        Key::Print | Key::SysRq => VK_SNAPSHOT,
        Key::ScrollLock => VK_SCROLL,
        Key::Pause => VK_PAUSE,
        Key::NumLock => VK_NUMLOCK,
        Key::KpSlash => VK_DIVIDE,
        Key::KpAsterisk => VK_MULTIPLY,
        Key::KpMinus => VK_SUBTRACT,
        Key::KpPlus => VK_ADD,
        Key::KpEnter => VK_RETURN, // numpad enter -> Return (extended flag set)
        Key::Kp7 => VK_NUMPAD7,
        Key::Kp8 => VK_NUMPAD8,
        Key::Kp9 => VK_NUMPAD9,
        Key::Kp4 => VK_NUMPAD4,
        Key::Kp5 => VK_NUMPAD5,
        Key::Kp6 => VK_NUMPAD6,
        Key::Kp1 => VK_NUMPAD1,
        Key::Kp2 => VK_NUMPAD2,
        Key::Kp3 => VK_NUMPAD3,
        Key::Kp0 => VK_NUMPAD0,
        Key::KpDot => VK_DECIMAL,
        // Media / volume / browser: `key_input` injects these by virtual key (`is_vk_only`) - their
        // only scancodes are E0-extended and would otherwise collide with letter scancodes.
        Key::Mute => VK_VOLUME_MUTE,
        Key::VolumeDown => VK_VOLUME_DOWN,
        Key::VolumeUp => VK_VOLUME_UP,
        Key::PlayPause => VK_MEDIA_PLAY_PAUSE,
        Key::PreviousSong => VK_MEDIA_PREV_TRACK,
        Key::NextSong => VK_MEDIA_NEXT_TRACK,
        Key::StopCd => VK_MEDIA_STOP,
        Key::Back => VK_BROWSER_BACK,
        Key::Forward => VK_BROWSER_FORWARD,
        // No Windows VK at all (or no exact match) -> not injected. `Play`/`Rewind`/`FastForward`
        // and `MicMute` have no distinct VK; brightness / keyboard illumination aren't virtual
        // keys on Windows.
        Key::Compose
        | Key::MicMute
        | Key::Play
        | Key::Rewind
        | Key::FastForward
        | Key::BrightnessDown
        | Key::BrightnessUp
        | Key::BrightnessCycle
        | Key::BrightnessAuto
        | Key::KbdIllumToggle
        | Key::KbdIllumDown
        | Key::KbdIllumUp => return None,
    })
}
