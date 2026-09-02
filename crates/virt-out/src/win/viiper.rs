//! VIIPER controller backend: a virtual Xbox 360 pad via VIIPER (Virtual Input over IP
//! EmulatoR). Gated behind the `viiper` feature. **Experimental** - VIIPER is new and the
//! deployment is heavier than ViGEm.
//!
//! Unlike ViGEm (a local kernel driver we IOCTL directly), VIIPER is a **USB/IP** system with
//! three parts, none of which we link:
//!   1. `usbip-win2` - a USB/IP vhci kernel driver (the ViGEmBus-equivalent), installed
//!      separately: <https://github.com/vadimgrn/usbip-win2>.
//!   2. `viiper.exe server` - a server process hosting a USB/IP endpoint (`:3241`) plus a
//!      management/streaming API (`:3242`), which auto-attaches created devices to the local
//!      `usbip-win2` driver so the OS sees a real pad.
//!   3. `viiper-client` (this crate's dep) - a **pure-Rust TCP client** to that API. No driver,
//!      no DLL, no FFI. So this file compiles and links anywhere Windows does; it only *works*
//!      at runtime once the driver is installed and the server is running.
//!
//! Config: we connect to `127.0.0.1:3242` by default (override `DECKHAND_VIIPER_ADDR`) and, by
//! default, **unauthenticated (plain TCP)**. Localhost needs no auth unless the server is run with
//! `--api.require-local-host-auth`, and `viiper-client` 0.7's *encrypted* path **deadlocks on
//! teardown** (see `resolve_password` / `Drop`), so plain is both sufficient and the only path that
//! shuts down cleanly. Set `DECKHAND_VIIPER_PASSWORD` to force the encrypted path if your server
//! requires auth (accepting that Ctrl-C will then hang until the crate bug is fixed upstream).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use viiper_client::devices::xbox360::{OUTPUT_SIZE, Xbox360Input};
use viiper_client::{DeviceCreateRequest, DeviceStream, ViiperClient};

use crate::gamepad::{AxisButtons, Dpad, Rumble};
use vocab_out::{GamepadAxis, GamepadButton};

/// Default VIIPER API address (the `--api.addr` port, not the USB/IP `:3241` port).
const DEFAULT_ADDR: &str = "127.0.0.1:3242";
/// The device `type` string the server expects for an Xbox 360 pad.
const DEVICE_TYPE_XBOX360: &str = "xbox360";

/// Latest rumble from the virtual pad, written by the device stream's output thread and read
/// by [`ViiperController::poll_rumble`]. VIIPER reports the two motors as `u8` (`left` = large /
/// low-frequency, `right` = small / high-frequency); we widen back on read.
struct RumbleState {
    strong: AtomicU8, // left / large / low-frequency motor
    weak: AtomicU8,   // right / small / high-frequency motor
}

/// A virtual Xbox 360 pad backed by a VIIPER server. Owns the management client (for teardown),
/// the streaming connection, the pending input snapshot, dpad-hat state, and the rumble
/// back-channel. Dropping it closes the stream and removes the bus (and its device) from the
/// server.
pub(crate) struct ViiperController {
    client: ViiperClient,
    bus_id: u32,
    // The streaming connection we push full input snapshots down. `Option` only so `Drop` can
    // close it *before* the server-side bus removal; always `Some` during normal operation.
    stream: Option<DeviceStream>,
    // Full input state resent on any change (VIIPER wants a full snapshot per send). `buttons`
    // holds the non-dpad bits; the dpad hat is kept separately and OR'd in at flush.
    input: Xbox360Input,
    dpad: Dpad,
    // Full-trigger / stick-direction pseudo-buttons, folded into the stick/trigger axes.
    axis_buttons: AxisButtons,
    // Set by set_button/set_axis; cleared on flush - avoids resending an unchanged snapshot.
    dirty: bool,
    rumble: Arc<RumbleState>,
}

impl ViiperController {
    /// Write a (combined) axis value into the snapshot, translating the shared evdev-signed vocab to
    /// XInput (sticks are +up -> negate Y; triggers `0..1`). Shared by `set_axis` and the axis
    /// pseudo-buttons (`AxisButtons`).
    fn write_axis(&mut self, a: &GamepadAxis, v: f32) {
        match a {
            GamepadAxis::LeftStickX => self.input.lx = stick(v),
            GamepadAxis::LeftStickY => self.input.ly = stick(-v),
            GamepadAxis::RightStickX => self.input.rx = stick(v),
            GamepadAxis::RightStickY => self.input.ry = stick(-v),
            GamepadAxis::LeftTrigger => self.input.lt = trigger(v),
            GamepadAxis::RightTrigger => self.input.rt = trigger(v),
        }
    }

    /// Connect to the VIIPER server, create a bus + Xbox 360 device, open the device stream, and
    /// start receiving rumble. Fails if the server isn't reachable/authenticated; note the
    /// device only appears to the OS if the `usbip-win2` driver is installed and the server's
    /// auto-attach is enabled.
    fn new() -> crate::Result<Self> {
        let addr = resolve_addr();
        let client = match resolve_password() {
            Some(pw) => ViiperClient::new_with_password(addr, pw),
            None => ViiperClient::new(addr),
        };

        // Verify connectivity up front so a misconfigured server fails here (at start()), not
        // silently later.
        let ping = client.ping()?;
        let bus_id = client.bus_create(None)?.bus_id;
        let req = DeviceCreateRequest {
            r#type: Some(DEVICE_TYPE_XBOX360.to_string()),
            id_vendor: None,
            id_product: None,
            device_specific: None, // default gamepad subtype
        };
        let dev_id = client.bus_device_add(bus_id, &req)?.dev_id;
        let mut stream = client.connect_device(bus_id, &dev_id)?;

        // Rumble back-channel: the stream's output thread delivers 2-byte {left,right} packets.
        let rumble = Arc::new(RumbleState {
            strong: AtomicU8::new(0),
            weak: AtomicU8::new(0),
        });
        let cb_rumble = Arc::clone(&rumble);
        stream.on_output(move |reader| {
            let mut buf = [0u8; OUTPUT_SIZE]; // {left, right}
            reader.read_exact(&mut buf)?;
            cb_rumble.strong.store(buf[0], Ordering::Relaxed);
            cb_rumble.weak.store(buf[1], Ordering::Relaxed);
            Ok(())
        })?;

        log::info!(
            "virt-out(win): controller backend = viiper (server {} v{}, {}, bus {} dev {})",
            ping.server,
            ping.version,
            addr,
            bus_id,
            dev_id
        );
        Ok(Self {
            client,
            bus_id,
            stream: Some(stream),
            input: Xbox360Input::default(),
            dpad: Dpad::default(),
            axis_buttons: AxisButtons::default(),
            dirty: false,
            rumble,
        })
    }

    fn set_button(&mut self, b: &GamepadButton, down: bool) {
        if self.dpad.set(b, down) {
            // dpad -> hat bits, folded in at flush
        } else if let Some(axis) = self.axis_buttons.set_button(b, down) {
            // Full-trigger / stick-direction pseudo-button -> drive its axis to the combined value.
            let vc = self.axis_buttons.value(&axis);
            self.write_axis(&axis, vc);
        } else {
            set_button_bit(&mut self.input.buttons, b, down);
        }
        self.dirty = true;
    }

    fn set_axis(&mut self, a: &GamepadAxis, v: f32) {
        // Cache the analog value and write it combined with any held axis-button (which overrides).
        self.axis_buttons.set_analog(a, v);
        let vc = self.axis_buttons.value(a);
        self.write_axis(a, vc);
        self.dirty = true;
    }

    fn flush(&mut self) -> crate::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        // Fold the dpad hat into the button word alongside the face/shoulder bits.
        self.input.buttons =
            (self.input.buttons & !DPAD_MASK) | dpad_bits((self.dpad.x(), self.dpad.y()));
        if let Some(stream) = self.stream.as_mut() {
            stream.send(&self.input)?;
        }
        self.dirty = false;
        Ok(())
    }

    fn poll_rumble(&mut self) -> crate::Result<Rumble> {
        // VIIPER delivers each motor as a u8; widen by x257 so 0xFF maps to 0xFFFF (full scale).
        let widen = |v: u8| (v as u16) * 257;
        Ok(Rumble {
            strong: widen(self.rumble.strong.load(Ordering::Relaxed)),
            weak: widen(self.rumble.weak.load(Ordering::Relaxed)),
        })
    }
}

impl Drop for ViiperController {
    fn drop(&mut self) {
        // Drop the stream first - its `Drop` shuts the socket (unblocking the rumble reader thread)
        // and joins it - then remove the bus (cascades to the device) so we don't leak one per run.
        // Clean teardown relies on the **plain-TCP** path: `viiper-client` 0.7's `EncryptedStream`
        // deadlocks here (its `read` holds the mutex `shutdown` needs), so we default to an
        // unauthenticated connection (see `resolve_password`). Note `bus_remove` does *not* close
        // our API device-stream socket - only the server's URB stream - so it cannot rescue the
        // encrypted path; avoiding encryption is the actual fix.
        drop(self.stream.take());
        let _ = self.client.bus_remove(Some(self.bus_id));
    }
}

// --- config resolution ---

/// The VIIPER API address: `DECKHAND_VIIPER_ADDR` if set and parseable, else `127.0.0.1:3242`.
fn resolve_addr() -> SocketAddr {
    std::env::var("DECKHAND_VIIPER_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| DEFAULT_ADDR.parse().expect("valid default addr"))
}

/// The API password, or `None` to connect **unauthenticated (plain TCP)** - the default. Only
/// `DECKHAND_VIIPER_PASSWORD` (if non-empty) opts into the encrypted path.
///
/// We do **not** auto-read the server's key file (`%APPDATA%\VIIPER\viiper.key.txt`) because that
/// would force the encrypted path, which **deadlocks on shutdown** in `viiper-client` 0.7: its
/// `EncryptedStream::read` holds the shared read mutex across the blocking `recv`, while
/// `DeviceStream::drop` -> `shutdown` needs that same mutex - so with a concurrent rumble reader
/// (`on_output`) any clean teardown hangs until the socket is closed by the server (which
/// `bus_remove` does *not* do for the API stream). Plain TCP has no such lock, so it shuts down
/// cleanly, and localhost needs no auth by default. Set the env var only if the server enforces
/// `--api.require-local-host-auth` (and expect the shutdown hang until the crate is fixed).
fn resolve_password() -> Option<String> {
    let pw = std::env::var("DECKHAND_VIIPER_PASSWORD").ok()?;
    let pw = pw.trim().to_string();
    if pw.is_empty() { None } else { Some(pw) }
}

// --- gamepad vocabulary -> XInput (VIIPER Xbox360) mapping ---

// Standard XInput button bits, as the VIIPER Xbox360 wire format uses them (dpad lives in the
// same button word - no separate hat). Kept as clean `u32`s (the crate's generated constants
// are a mix of `u8`/`i32`).
const BTN_DPAD_UP: u32 = 0x0001;
const BTN_DPAD_DOWN: u32 = 0x0002;
const BTN_DPAD_LEFT: u32 = 0x0004;
const BTN_DPAD_RIGHT: u32 = 0x0008;
const BTN_START: u32 = 0x0010;
const BTN_BACK: u32 = 0x0020;
const BTN_LTHUMB: u32 = 0x0040;
const BTN_RTHUMB: u32 = 0x0080;
const BTN_LSHOULDER: u32 = 0x0100;
const BTN_RSHOULDER: u32 = 0x0200;
const BTN_GUIDE: u32 = 0x0400;
const BTN_A: u32 = 0x1000;
const BTN_B: u32 = 0x2000;
const BTN_X: u32 = 0x4000;
const BTN_Y: u32 = 0x8000;

/// The four dpad bits within the button word.
const DPAD_MASK: u32 = BTN_DPAD_UP | BTN_DPAD_DOWN | BTN_DPAD_LEFT | BTN_DPAD_RIGHT;

fn set_button_bit(buttons: &mut u32, b: &GamepadButton, down: bool) {
    let bit = match b {
        GamepadButton::A => BTN_A,
        GamepadButton::B => BTN_B,
        GamepadButton::X => BTN_X,
        GamepadButton::Y => BTN_Y,
        GamepadButton::LeftBumper => BTN_LSHOULDER,
        GamepadButton::RightBumper => BTN_RSHOULDER,
        GamepadButton::Back => BTN_BACK,
        GamepadButton::Start => BTN_START,
        GamepadButton::Guide => BTN_GUIDE,
        GamepadButton::LeftStick => BTN_LTHUMB,
        GamepadButton::RightStick => BTN_RTHUMB,
        GamepadButton::DpadUp
        | GamepadButton::DpadDown
        | GamepadButton::DpadLeft
        | GamepadButton::DpadRight => {
            unreachable!("dpad directions fold into the hat - see Dpad / set_button")
        }
        b if b.is_axis_button() => {
            unreachable!("axis pseudo-buttons fold into the stick/trigger axes - see AxisButtons")
        }
        _ => unreachable!("set_button_bit covers every non-hat, non-axis button"),
    };
    if down {
        *buttons |= bit;
    } else {
        *buttons &= !bit;
    }
}

/// Dpad hat state (`+1`/`-1` per axis) -> XInput dpad bits. `DpadX +1 = right`,
/// `DpadY +1 = down` (matches the vocabulary / evdev hat convention).
fn dpad_bits((x, y): (i32, i32)) -> u32 {
    let mut bits = 0;
    if x > 0 {
        bits |= BTN_DPAD_RIGHT;
    } else if x < 0 {
        bits |= BTN_DPAD_LEFT;
    }
    if y > 0 {
        bits |= BTN_DPAD_DOWN;
    } else if y < 0 {
        bits |= BTN_DPAD_UP;
    }
    bits
}

/// Normalized stick `-1.0..=1.0` -> XInput `i16`.
fn stick(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Normalized trigger `0.0..=1.0` -> XInput `u8`.
fn trigger(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * u8::MAX as f32) as u8
}
