//! Wire decoding: raw HID bytes -> a [`Report`] (PLAN 1.5). One physical read yields exactly one
//! frame of *some* type, so this module decodes straight from the wire packet ([`crate::protocol`])
//! into a `Report`: an input frame becomes [`Report::State`] (a normalized
//! [`ControllerState`], the shared snapshot from `vocab-hid`), a lifecycle frame becomes
//! [`Report::Connected`]/[`Report::Disconnected`]/[`Report::Battery`]. There is no decoded-report
//! intermediate - the wire struct converts directly here. Also folds each device's raw button
//! bitfield into the unified [`Buttons`] (the `map_*` fns, next to their `from_*`).

use crate::protocol::{
    ControllerStatus, GordonButtons, GordonState, NeptuneButtons, NeptuneState,
    TritonBatteryStatus, TritonButtons, TritonStateNoQuat, TritonWirelessStatus, Wire, WireQuat,
    WireVec2, WireVec3, WirelessEvent, ble, event_type, triton, wireless,
};
use vocab_hid::{Buttons, ControllerState, Quati, TrackPad, Timestamp, Vec2, Vec2i, Vec3i};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A high-level frame: a unified input snapshot, or a lifecycle signal (PLAN 1.5).
///
/// `read`/`poll` return this. `State` is the normalized input snapshot; the other
/// variants are the lifecycle frames.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Report {
    State(ControllerState),
    Connected,
    Disconnected,
    Battery(Battery),
}

/// Battery status (wireless controllers only; PLAN 1.5). Offsets per the kernel: voltage at
/// `0x0C`, charge at `0x0E` (Gordon/Neptune `0x04`); Triton reports its own `0x43` layout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Battery {
    pub voltage_mv: u16,
    /// Battery charge, in percent (0..=100).
    pub charge_percent: u8,
}

// --- USB Gordon / Neptune dispatch (the `0x01`-framed report) ---

/// Decode one 64-byte USB Gordon/Neptune report, dispatching on the event byte. `None` for a
/// known-but-undecoded event or a too-short buffer - the caller just reads again, like the Triton
/// path (a malformed frame must not tear down the session). A `State` snapshot is stamped with
/// `timestamp` (ignored for lifecycle frames).
pub(crate) fn parse(buf: &[u8], timestamp: Timestamp) -> Option<Report> {
    // buf[0..2] == 0x01, 0x00; buf[2] == event type. Length-safe access throughout (`get`/`from_bytes`),
    // so a short buffer folds to `None` rather than panicking or erroring.
    match *buf.get(2)? {
        event_type::STATE => Some(Report::State(from_gordon(GordonState::from_bytes(buf)?, timestamp))),
        event_type::DECK_STATE => {
            Some(Report::State(from_neptune(NeptuneState::from_bytes(buf)?, timestamp)))
        }
        event_type::WIRELESS => Some(match WirelessEvent::from_bytes(buf)?.event {
            wireless::DISCONNECTED => Report::Disconnected,
            _ => Report::Connected, // CONNECTED (0x02) and any other -> treat as connect
        }),
        event_type::STATUS => {
            let p = ControllerStatus::from_bytes(buf)?;
            Some(Report::Battery(Battery {
                voltage_mv: p.voltage_mv,
                charge_percent: p.charge_percent,
            }))
        }
        // Unknown event byte: log it (so a new type is visible, not silently dropped) and skip it -
        // the caller reads again (matches the Triton path; PLAN 1.4/1.9).
        id => {
            log::trace!("steam-hid: unknown Gordon/Neptune report event 0x{id:02x}");
            None
        }
    }
}

// --- Gordon (USB + BLE share this fold) ---

/// Fold Gordon's per-device button bits into the unified [`Buttons`] superset.
///
/// Serves **both** USB and Bluetooth Gordon (they share [`GordonButtons`]). A plain 1:1 fold: the USB
/// left-click multiplex is already resolved in [`from_gordon`], and BLE has none, so no touch-gating
/// happens here.
fn map_gordon(g: &GordonButtons) -> Buttons {
    let mut out = Buttons::empty();
    let mut set = |cond: bool, flag: Buttons| {
        if cond {
            out |= flag;
        }
    };
    set(g.contains(GordonButtons::A), Buttons::A);
    set(g.contains(GordonButtons::B), Buttons::B);
    set(g.contains(GordonButtons::X), Buttons::X);
    set(g.contains(GordonButtons::Y), Buttons::Y);
    set(g.contains(GordonButtons::DPAD_UP), Buttons::DPAD_UP);
    set(g.contains(GordonButtons::DPAD_DOWN), Buttons::DPAD_DOWN);
    set(g.contains(GordonButtons::DPAD_LEFT), Buttons::DPAD_LEFT);
    set(g.contains(GordonButtons::DPAD_RIGHT), Buttons::DPAD_RIGHT);
    set(g.contains(GordonButtons::LB), Buttons::LB);
    set(g.contains(GordonButtons::RB), Buttons::RB);
    set(g.contains(GordonButtons::LT), Buttons::LT);
    set(g.contains(GordonButtons::RT), Buttons::RT);
    set(g.contains(GordonButtons::LGRIP), Buttons::LGRIP);
    set(g.contains(GordonButtons::RGRIP), Buttons::RGRIP);
    set(g.contains(GordonButtons::VIEW), Buttons::VIEW);
    set(g.contains(GordonButtons::MENU), Buttons::MENU);
    set(g.contains(GordonButtons::STEAM), Buttons::STEAM);
    // Pad/stick clicks arrive already de-multiplexed (USB in `from_gordon`; BLE has no multiplex).
    set(g.contains(GordonButtons::LPAD_PRESS), Buttons::LPAD_PRESS);
    set(g.contains(GordonButtons::RPAD_PRESS), Buttons::RPAD_PRESS);
    set(g.contains(GordonButtons::LPAD_TOUCH), Buttons::LPAD_TOUCH);
    set(g.contains(GordonButtons::RPAD_TOUCH), Buttons::RPAD_TOUCH);
    set(g.contains(GordonButtons::LSTICK_PRESS), Buttons::LSTICK_PRESS);
    out
}

/// Decode a Gordon **USB** input frame into a unified snapshot (PLAN 1.4 offsets).
///
/// The USB wire multiplexes the left pad and analog stick two ways; both are fully resolved here so
/// the snapshot is clean (matching the BLE path, which has no multiplex - see [`apply_gordon_ble`]):
/// - **coordinates:** pad and stick share `left` (`0x10`), disambiguated by `LPAD_TOUCH` - the pad
///   when touched, the stick when not. Verified on **both** the wireless dongle and wired (`0x36` is
///   *not* the stick - 0 on wireless, small noise on wired). When pad + stick are used **together**
///   the firmware sets `LPAD_AND_JOY` and *flickers* `LPAD_TOUCH` frame-to-frame to tag which the
///   coord belongs to (the coord split still keys on `LPAD_TOUCH` alone). PLAN 1.9.
/// - **click bit:** `LPAD_PRESS` fires for both a pad click *and* a stick click (HW-verified). It's a
///   real pad click only when the pad is *engaged* - touched, **or** `LPAD_AND_JOY` set (so a pad
///   click survives the `LPAD_TOUCH` flicker during simultaneous use). Otherwise it's a stick click
///   (which already sets `LSTICK_PRESS`), so the spurious `LPAD_PRESS` is dropped.
/// - **touch bit:** reported as *engaged* too (`LPAD_TOUCH || LPAD_AND_JOY`) so the touch button
///   stays steady through the axis-tag flicker. (The pad *position* still can't be sampled every
///   frame - a single-field wire limit, PLAN 1.9 - but the buttons are clean.)
fn from_gordon(p: GordonState, timestamp: Timestamp) -> ControllerState {
    // Copy packed fields into aligned locals before use (can't reference a packed field).
    let (seq, btn, left_trigger, right_trigger) = (p.seq, p.buttons, p.left_trigger, p.right_trigger);
    let (left, right_pad, accel, gyro, orientation) =
        (p.left, p.right_pad, p.accel, p.gyro, p.orientation);
    let mut raw = GordonButtons::from_bits_truncate(
        btn[0] as u32 | (btn[1] as u32) << 8 | (btn[2] as u32) << 16,
    );
    let left_touched = raw.contains(GordonButtons::LPAD_TOUCH);
    let left_engaged = left_touched || raw.contains(GordonButtons::LPAD_AND_JOY);
    if raw.contains(GordonButtons::LPAD_PRESS) && !left_engaged {
        raw.remove(GordonButtons::LPAD_PRESS); // was a stick click, not a pad click
    }
    let left_raw: Vec2i = to_vec2i(left);
    let (left_pad_pos, left_stick) = if left_touched {
        (left_raw, Vec2i::default())
    } else {
        (Vec2i::default(), left_raw)
    };
    // Touch *button*: steady while the pad is engaged (matches kernel `BTN_THUMB`).
    raw.set(GordonButtons::LPAD_TOUCH, left_engaged);
    let right_pad: Vec2i = to_vec2i(right_pad);
    ControllerState {
        seq,
        timestamp,
        buttons: map_gordon(&raw),
        left_trigger: norm_u8(left_trigger),
        right_trigger: norm_u8(right_trigger),
        left_stick: norm_stick(&left_stick),
        right_stick: Vec2::default(), // Gordon has no right stick
        left_pad: TrackPad {
            pos: norm_stick(&left_pad_pos),
            pressure: 0.0, // Gordon pads report no pressure
            touched: raw.contains(GordonButtons::LPAD_TOUCH),
        },
        right_pad: TrackPad {
            pos: norm_stick(&right_pad),
            pressure: 0.0,
            touched: raw.contains(GordonButtons::RPAD_TOUCH),
        },
        accel: to_vec3i(accel),
        gyro: gordon_gyro(&to_vec3i(gyro)),
        orientation: to_quati(orientation),
    }
}

// --- Neptune (Steam Deck) ---

/// Fold Neptune's per-device button bits into the unified [`Buttons`] superset (1:1 - the Deck has
/// dedicated press/touch bits and its raw layout already matches the unified naming).
fn map_neptune(n: &NeptuneButtons) -> Buttons {
    let mut out = Buttons::empty();
    let mut set = |cond: bool, flag: Buttons| {
        if cond {
            out |= flag;
        }
    };
    set(n.contains(NeptuneButtons::A), Buttons::A);
    set(n.contains(NeptuneButtons::B), Buttons::B);
    set(n.contains(NeptuneButtons::X), Buttons::X);
    set(n.contains(NeptuneButtons::Y), Buttons::Y);
    set(n.contains(NeptuneButtons::DPAD_UP), Buttons::DPAD_UP);
    set(n.contains(NeptuneButtons::DPAD_DOWN), Buttons::DPAD_DOWN);
    set(n.contains(NeptuneButtons::DPAD_LEFT), Buttons::DPAD_LEFT);
    set(n.contains(NeptuneButtons::DPAD_RIGHT), Buttons::DPAD_RIGHT);
    set(n.contains(NeptuneButtons::LB), Buttons::LB);
    set(n.contains(NeptuneButtons::RB), Buttons::RB);
    set(n.contains(NeptuneButtons::LT), Buttons::LT);
    set(n.contains(NeptuneButtons::RT), Buttons::RT);
    set(n.contains(NeptuneButtons::LGRIP), Buttons::LGRIP);
    set(n.contains(NeptuneButtons::RGRIP), Buttons::RGRIP);
    set(n.contains(NeptuneButtons::LGRIP2), Buttons::LGRIP2);
    set(n.contains(NeptuneButtons::RGRIP2), Buttons::RGRIP2);
    set(n.contains(NeptuneButtons::VIEW), Buttons::VIEW);
    set(n.contains(NeptuneButtons::MENU), Buttons::MENU);
    set(n.contains(NeptuneButtons::STEAM), Buttons::STEAM);
    set(n.contains(NeptuneButtons::QUICK_ACCESS), Buttons::QUICK_ACCESS);
    set(n.contains(NeptuneButtons::LPAD_PRESS), Buttons::LPAD_PRESS);
    set(n.contains(NeptuneButtons::RPAD_PRESS), Buttons::RPAD_PRESS);
    set(n.contains(NeptuneButtons::LPAD_TOUCH), Buttons::LPAD_TOUCH);
    set(n.contains(NeptuneButtons::RPAD_TOUCH), Buttons::RPAD_TOUCH);
    set(n.contains(NeptuneButtons::LSTICK_PRESS), Buttons::LSTICK_PRESS);
    set(n.contains(NeptuneButtons::RSTICK_PRESS), Buttons::RSTICK_PRESS);
    set(n.contains(NeptuneButtons::LSTICK_TOUCH), Buttons::LSTICK_TOUCH);
    set(n.contains(NeptuneButtons::RSTICK_TOUCH), Buttons::RSTICK_TOUCH);
    out
}

/// Decode a Neptune (Steam Deck) input frame into a unified snapshot.
///
/// Offsets are cross-checked against the kernel `hid-steam.c` and the C# `NCInput` struct (they
/// agree). Unlike Gordon there is **no multiplex** - sticks and pads are separate fields, direct
/// press/touch bits, a 1:1 button fold. Triggers are `i16` (`0..=32767`); the trigger **full-pull**
/// is a firmware-synthesized button bit. IMU passes through **raw** - HW-verified (PLAN 1.9):
/// Neptune's accel and gyro already sit in the same unified right-handed frame Gordon reaches *after*
/// its `gordon_gyro` y-negation, so Neptune needs **no** correction. The `*_stick_force` capacitive
/// fields (InputPlumber-only, beyond SDL) are present in the wire struct but deliberately not
/// surfaced (PLAN 1.4).
fn from_neptune(p: NeptuneState, timestamp: Timestamp) -> ControllerState {
    // Copy packed fields into aligned locals before use.
    let btn = p.buttons; // [u8; 8] (SDL 8-byte button union)
    let (seq, left_trigger, right_trigger) = (p.seq, p.left_trigger, p.right_trigger);
    let (left_stick, right_stick, left_pad, right_pad, accel, gyro, orientation) =
        (p.left_stick, p.right_stick, p.left_pad, p.right_pad, p.accel, p.gyro, p.orientation);
    let (left_pad_pressure, right_pad_pressure) = (p.left_pad_pressure, p.right_pad_pressure);
    let buttons = NeptuneButtons::from_bits_truncate(u64::from_le_bytes(btn));
    ControllerState {
        seq,
        timestamp,
        buttons: map_neptune(&buttons),
        left_trigger: norm_i16(left_trigger),
        right_trigger: norm_i16(right_trigger),
        left_stick: norm_stick(&to_vec2i(left_stick)),
        right_stick: norm_stick(&to_vec2i(right_stick)),
        left_pad: TrackPad {
            pos: norm_stick(&to_vec2i(left_pad)),
            pressure: norm_i16(left_pad_pressure),
            touched: buttons.contains(NeptuneButtons::LPAD_TOUCH),
        },
        right_pad: TrackPad {
            pos: norm_stick(&to_vec2i(right_pad)),
            pressure: norm_i16(right_pad_pressure),
            touched: buttons.contains(NeptuneButtons::RPAD_TOUCH),
        },
        accel: to_vec3i(accel),
        gyro: to_vec3i(gyro),
        orientation: to_quati(orientation),
    }
}

// --- Triton dispatch (report id in byte 0; not the `0x01`-framed protocol) ---

/// Decode one Triton (new Steam Controller) report, dispatching on the **report id in byte 0**.
/// `buf` is the raw read (length already sliced by the caller); a `State` snapshot is stamped with
/// `timestamp`.
///
/// Returns `None` for a report we don't decode as a frame yet - the timestamped `0x47` "Ibex" body
/// (added only if a unit streams it, PLAN 1.9) and any unknown id - so the caller keeps reading.
pub(crate) fn parse_triton(buf: &[u8], timestamp: Timestamp) -> Option<Report> {
    match *buf.first()? {
        triton::report::CONTROLLER_STATE | triton::report::CONTROLLER_STATE_BLE => {
            Some(Report::State(from_triton(TritonStateNoQuat::from_bytes(buf)?, timestamp)))
        }
        triton::report::BATTERY_STATUS => {
            let p = TritonBatteryStatus::from_bytes(buf)?; // None if the read is too short
            Some(Report::Battery(Battery {
                voltage_mv: p.voltage_mv,
                charge_percent: p.battery_level,
            }))
        }
        triton::report::WIRELESS_STATUS | triton::report::WIRELESS_STATUS_X => {
            match TritonWirelessStatus::from_bytes(buf)?.state {
                triton::wireless::DISCONNECT => Some(Report::Disconnected),
                triton::wireless::CONNECT => Some(Report::Connected),
                _ => None,
            }
        }
        // Known reports we deliberately don't decode (layouts in `protocol.rs`): the lizard-mode
        // mouse/keyboard and the wireless link telemetry. Skip silently - no point logging them.
        triton::report::LIZARD_MOUSE
        | triton::report::LIZARD_KEYBOARD
        | triton::report::LINK_STATUS => None,
        // Anything else (incl. the 0x47 Ibex timestamped body we don't decode yet, PLAN 1.9): an
        // unknown id - log it at trace so it's visible if a unit streams it, then skip (keep reading).
        id => {
            log::trace!("steam-hid: unknown Triton report id 0x{id:02x}");
            None
        }
    }
}

// --- Triton (new Steam Controller) ---

/// Fold Triton's per-device button bits into the unified [`Buttons`] superset (1:1). The two
/// capacitive **grip-touch** sensors fold into the `L/RGRIP_TOUCH` bits (a Triton-only input).
fn map_triton(t: &TritonButtons) -> Buttons {
    let mut out = Buttons::empty();
    let mut set = |cond: bool, flag: Buttons| {
        if cond {
            out |= flag;
        }
    };
    set(t.contains(TritonButtons::A), Buttons::A);
    set(t.contains(TritonButtons::B), Buttons::B);
    set(t.contains(TritonButtons::X), Buttons::X);
    set(t.contains(TritonButtons::Y), Buttons::Y);
    set(t.contains(TritonButtons::DPAD_UP), Buttons::DPAD_UP);
    set(t.contains(TritonButtons::DPAD_DOWN), Buttons::DPAD_DOWN);
    set(t.contains(TritonButtons::DPAD_LEFT), Buttons::DPAD_LEFT);
    set(t.contains(TritonButtons::DPAD_RIGHT), Buttons::DPAD_RIGHT);
    set(t.contains(TritonButtons::LB), Buttons::LB);
    set(t.contains(TritonButtons::RB), Buttons::RB);
    set(t.contains(TritonButtons::LT), Buttons::LT);
    set(t.contains(TritonButtons::RT), Buttons::RT);
    set(t.contains(TritonButtons::LGRIP), Buttons::LGRIP);
    set(t.contains(TritonButtons::RGRIP), Buttons::RGRIP);
    set(t.contains(TritonButtons::LGRIP2), Buttons::LGRIP2);
    set(t.contains(TritonButtons::RGRIP2), Buttons::RGRIP2);
    set(t.contains(TritonButtons::LGRIP_TOUCH), Buttons::LGRIP_TOUCH);
    set(t.contains(TritonButtons::RGRIP_TOUCH), Buttons::RGRIP_TOUCH);
    set(t.contains(TritonButtons::VIEW), Buttons::VIEW);
    set(t.contains(TritonButtons::MENU), Buttons::MENU);
    set(t.contains(TritonButtons::STEAM), Buttons::STEAM);
    set(t.contains(TritonButtons::QUICK_ACCESS), Buttons::QUICK_ACCESS);
    set(t.contains(TritonButtons::LPAD_PRESS), Buttons::LPAD_PRESS);
    set(t.contains(TritonButtons::RPAD_PRESS), Buttons::RPAD_PRESS);
    set(t.contains(TritonButtons::LPAD_TOUCH), Buttons::LPAD_TOUCH);
    set(t.contains(TritonButtons::RPAD_TOUCH), Buttons::RPAD_TOUCH);
    set(t.contains(TritonButtons::LSTICK_PRESS), Buttons::LSTICK_PRESS);
    set(t.contains(TritonButtons::RSTICK_PRESS), Buttons::RSTICK_PRESS);
    set(t.contains(TritonButtons::LSTICK_TOUCH), Buttons::LSTICK_TOUCH);
    set(t.contains(TritonButtons::RSTICK_TOUCH), Buttons::RSTICK_TOUCH);
    out
}

/// Decode the Triton report `0x42`/`0x45` "NoQuat" body into a unified snapshot.
///
/// Like the Deck: separate stick/pad fields (no Gordon multiplex), direct press/touch bits, a 1:1
/// button fold. Triggers/pad pressure are the analog `i16` (`0..=32767`). The two capacitive
/// **grip-touch** bits fold into the unified `L/RGRIP_TOUCH`. IMU passes through **raw** - HW-verified
/// on a real Triton (PLAN 1.9): accel/gyro already sit in the unified right-handed frame (like
/// Neptune, no correction). Triton's gyro full-scale is 2000 dps (res ~16.384 LSB/dps) vs the
/// canonical `GYRO_RES_PER_DPS = 16` (2048 dps) - a ~2.3 % difference **accepted un-rescaled**.
/// Orientation is not decoded (unused; the NoQuat body carries none). The `0x42` (Full) body is
/// handled here too - NoQuat is its 46-byte prefix, the quaternion is trailing bytes we skip.
fn from_triton(p: TritonStateNoQuat, timestamp: Timestamp) -> ControllerState {
    // Copy packed fields into aligned locals before use.
    let (seq_num, buttons_raw, left_trigger, right_trigger) =
        (p.seq_num, p.buttons, p.left_trigger, p.right_trigger);
    let (left_stick, right_stick, left_pad, right_pad, accel, gyro) =
        (p.left_stick, p.right_stick, p.left_pad, p.right_pad, p.accel, p.gyro);
    let (left_pad_pressure, right_pad_pressure) = (p.left_pad_pressure, p.right_pad_pressure);
    let buttons = TritonButtons::from_bits_truncate(buttons_raw);
    ControllerState {
        seq: seq_num as u32,
        timestamp,
        buttons: map_triton(&buttons),
        left_trigger: norm_i16(left_trigger),
        right_trigger: norm_i16(right_trigger),
        left_stick: norm_stick(&to_vec2i(left_stick)),
        right_stick: norm_stick(&to_vec2i(right_stick)),
        left_pad: TrackPad {
            pos: norm_stick(&to_vec2i(left_pad)),
            pressure: norm_i16(left_pad_pressure),
            touched: buttons.contains(TritonButtons::LPAD_TOUCH),
        },
        right_pad: TrackPad {
            pos: norm_stick(&to_vec2i(right_pad)),
            pressure: norm_i16(right_pad_pressure),
            touched: buttons.contains(TritonButtons::RPAD_TOUCH),
        },
        accel: to_vec3i(accel),
        gyro: to_vec3i(gyro),
        orientation: Quati::default(),
    }
}

// --- Gordon BLE (segmented delta stream accumulated into the snapshot) ---

/// Apply one reassembled **BLE** input payload to the accumulated snapshot `acc`, in place.
///
/// `payload` is a fully-reassembled packet: `byte0` low nibble = report type,
/// `(byte0 & 0xF0) | (byte1 << 8)` = the chunk mask, then each present chunk's bytes in ascending
/// bit order. Chunks decode and normalize straight into `acc` (the same [`ControllerState`] the USB
/// path builds), so BLE needs no decoded-report intermediate. Only `State` reports carry input; on
/// anything else `acc` is left untouched. Returns `true` if an input state was applied (caller stamps
/// `seq`/`timestamp` and emits the snapshot). BLE has no left multiplex (stick/pad are separate
/// chunks), so the button fold is a plain 1:1 map. `seq`/`timestamp` are set by the caller.
pub(crate) fn apply_gordon_ble(acc: &mut ControllerState, payload: &[u8]) -> bool {
    if payload.len() < 2 || (payload[0] & 0x0F) != ble::report_type::STATE {
        return false;
    }
    let mask = ((payload[0] & 0xF0) as u16) | ((payload[1] as u16) << 8);
    let mut p = 2usize;
    // Read a chunk of `n` bytes at the cursor, advancing it; None if the payload is short
    // (defensive - a well-formed packet always fits).
    let mut take = |n: usize| -> Option<usize> {
        if p + n <= payload.len() {
            let at = p;
            p += n;
            Some(at)
        } else {
            None
        }
    };

    use ble::chunk;
    if mask & chunk::BUTTON1 != 0 && let Some(o) = take(3) {
        let raw = GordonButtons::from_bits_truncate(
            payload[o] as u32 | (payload[o + 1] as u32) << 8 | (payload[o + 2] as u32) << 16,
        );
        acc.buttons = map_gordon(&raw);
    }
    if mask & chunk::TRIGGERS != 0 && let Some(o) = take(2) {
        acc.left_trigger = norm_u8(payload[o]);
        acc.right_trigger = norm_u8(payload[o + 1]);
    }
    if mask & chunk::BUTTON3 != 0 {
        take(3); // high button bytes - unused on the original SC
    }
    // `take(n)` guarantees `payload[o..]` has >= n bytes, so each chunk cast below is infallible.
    if mask & chunk::LSTICK != 0 && let Some(o) = take(4) {
        acc.left_stick = norm_stick(&to_vec2i(WireVec2::from_bytes(&payload[o..]).unwrap()));
    }
    if mask & chunk::LPAD != 0 && let Some(o) = take(4) {
        acc.left_pad.pos = norm_stick(&to_vec2i(WireVec2::from_bytes(&payload[o..]).unwrap()));
    }
    if mask & chunk::RPAD != 0 && let Some(o) = take(4) {
        acc.right_pad.pos = norm_stick(&to_vec2i(WireVec2::from_bytes(&payload[o..]).unwrap()));
    }
    if mask & chunk::ACCEL != 0 && let Some(o) = take(6) {
        acc.accel = to_vec3i(WireVec3::from_bytes(&payload[o..]).unwrap());
    }
    if mask & chunk::GYRO != 0 && let Some(o) = take(6) {
        acc.gyro = gordon_gyro(&to_vec3i(WireVec3::from_bytes(&payload[o..]).unwrap()));
    }
    if mask & chunk::QUAT != 0 && let Some(o) = take(8) {
        acc.orientation = to_quati(WireQuat::from_bytes(&payload[o..]).unwrap());
    }
    // Touch buttons live in both `buttons` and the per-pad `touched`; sync the pads from the folded
    // buttons (Gordon pads report no pressure, so `.pos`/`.touched` are the only live pad fields).
    acc.left_pad.touched = acc.buttons.contains(Buttons::LPAD_TOUCH);
    acc.right_pad.touched = acc.buttons.contains(Buttons::RPAD_TOUCH);
    true
}

// --- helpers: gyro frame, normalization, wire -> value converters ---

/// Normalize Gordon's raw gyro into the unified right-handed IMU frame.
///
/// HW-verified (PLAN 1.9): the raw channels are axis-aligned with the accel frame `X=right,
/// Y=forward, Z=up` - `x`=pitch, `y`=roll, `z`=yaw rate - but the device's `y` (roll) channel is
/// mounted **inverted**, giving a left-handed triple `(wx, -wy, wz)`. Negating `y` yields a proper
/// right-handed angular velocity (pitch-up, yaw-left, roll-right all positive).
fn gordon_gyro(raw: &Vec3i) -> Vec3i {
    Vec3i {
        x: raw.x,
        y: raw.y.saturating_neg(),
        z: raw.z,
    }
}

fn norm_u8(v: u8) -> f32 {
    v as f32 / 255.0
}
/// Normalize an unsigned-range `i16` (`0..=32767`) to `0.0..=1.0` - Deck triggers and trackpad
/// pressure. (Bipolar sticks/pads use [`norm_axis`]. Pad-pressure full-scale is provisional.)
fn norm_i16(v: i16) -> f32 {
    (v as f32 / 32767.0).clamp(0.0, 1.0)
}
fn norm_axis(v: i16) -> f32 {
    (v as f32 / 32768.0).clamp(-1.0, 1.0)
}
fn norm_stick(v: &Vec2i) -> Vec2 {
    Vec2 { x: norm_axis(v.x), y: norm_axis(v.y) }
}

// The value types live in `vocab-hid`; a `From` impl here would be an orphan (both `From` and the
// target type are foreign), so these are plain free-fn field copies.
fn to_vec2i(w: WireVec2) -> Vec2i {
    Vec2i { x: w.x, y: w.y }
}
fn to_vec3i(w: WireVec3) -> Vec3i {
    Vec3i { x: w.x, y: w.y, z: w.z }
}
fn to_quati(w: WireQuat) -> Quati {
    Quati { x: w.x, y: w.y, z: w.z, w: w.w }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::REPORT_LEN;

    /// Build a 64-byte USB `0x01`-framed report with the given event byte.
    fn frame_buf(event: u8) -> [u8; REPORT_LEN] {
        let mut b = [0u8; REPORT_LEN];
        b[0] = 0x01;
        b[2] = event;
        b
    }

    /// Build a STATE frame carrying the given raw Gordon button word (low 24 bits at 0x08).
    fn gordon_state(word: u32) -> [u8; REPORT_LEN] {
        let mut b = frame_buf(event_type::STATE);
        b[0x08..0x0B].copy_from_slice(&word.to_le_bytes()[..3]);
        b
    }

    fn buttons_of(buf: &[u8]) -> Buttons {
        match parse(buf, Timestamp::default()) {
            Some(Report::State(s)) => s.buttons,
            other => panic!("expected State, got {other:?}"),
        }
    }

    #[test]
    fn parse_dispatches_lifecycle_frames() {
        let mut b = frame_buf(event_type::WIRELESS);
        b[4] = wireless::CONNECTED;
        assert_eq!(parse(&b, Timestamp::default()), Some(Report::Connected));
        b[4] = wireless::DISCONNECTED;
        assert_eq!(parse(&b, Timestamp::default()), Some(Report::Disconnected));

        let b = frame_buf(event_type::STATUS);
        assert!(matches!(parse(&b, Timestamp::default()), Some(Report::Battery(_))));
    }

    /// The USB left-click multiplex resolves: a stick click (shares the `LPAD_PRESS` bit, no
    /// `LPAD_TOUCH`) must not surface as a pad press.
    #[test]
    fn from_gordon_demuxes_stick_click() {
        let b = buttons_of(&gordon_state(
            (GordonButtons::LPAD_PRESS | GordonButtons::LSTICK_PRESS).bits(),
        ));
        assert!(b.contains(Buttons::LSTICK_PRESS));
        assert!(!b.contains(Buttons::LPAD_PRESS));
    }

    /// A genuine pad click (`LPAD_PRESS` with `LPAD_TOUCH`) survives de-multiplexing.
    #[test]
    fn from_gordon_keeps_touched_pad_click() {
        let b = buttons_of(&gordon_state(
            (GordonButtons::LPAD_PRESS | GordonButtons::LPAD_TOUCH).bits(),
        ));
        assert!(b.contains(Buttons::LPAD_PRESS));
        assert!(b.contains(Buttons::LPAD_TOUCH));
    }

    /// Simultaneous pad+stick use: the firmware sets `LPAD_AND_JOY` and flickers `LPAD_TOUCH`; a pad
    /// click must survive an off-flicker frame (gated on AND_JOY, not touch alone).
    #[test]
    fn from_gordon_keeps_pad_click_via_and_joy() {
        let b = buttons_of(&gordon_state(
            (GordonButtons::LPAD_PRESS | GordonButtons::LSTICK_PRESS | GordonButtons::LPAD_AND_JOY)
                .bits(),
        ));
        assert!(b.contains(Buttons::LPAD_PRESS)); // kept via AND_JOY
        assert!(b.contains(Buttons::LSTICK_PRESS));
    }

    /// Touch *button* stays steady during simultaneous use: on a stick-tag frame the raw `LPAD_TOUCH`
    /// is 0, but `LPAD_AND_JOY` is set, so the reported touch is true (no flicker).
    #[test]
    fn from_gordon_touch_steady_via_and_joy() {
        let b = buttons_of(&gordon_state(
            (GordonButtons::LSTICK_PRESS | GordonButtons::LPAD_AND_JOY).bits(),
        ));
        assert!(b.contains(Buttons::LPAD_TOUCH)); // steady via AND_JOY
    }

    #[test]
    fn parse_skips_short_report() {
        // A known event byte but a truncated buffer: `from_bytes` fails, so parse skips it (`None`)
        // rather than panicking or erroring - the reader reads again instead of tearing down.
        let mut short = [0u8; 10];
        short[2] = event_type::STATE;
        assert!(parse(&short, Timestamp::default()).is_none());
    }
}
