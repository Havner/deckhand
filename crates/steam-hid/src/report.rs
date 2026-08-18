//! Layer 1: the decoded wire frame (PLAN §1.5).
//!
//! One physical read yields exactly one frame of *some* type — an input frame or
//! a lifecycle frame — so [`RawReport`] carries both. Field offsets follow PLAN
//! §1.4 and are **unverified on hardware** (PLAN §1.9).

use crate::buttons::{GordonButtons, NeptuneButtons};
use crate::error::{Error, Result};
use crate::protocol::{REPORT_LEN, ble, event_type, wireless};
use crate::value::{Quati, Vec2i, Vec3i};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A decoded wire frame: a per-device input report, or a lifecycle signal.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum RawReport {
    /// Original Steam Controller input — USB (`0x01`) or, accumulated from the
    /// Bluetooth delta stream, a full [`GordonReport`] snapshot (both transports
    /// converge on the same report; see `parse_gordon` / `apply_gordon_ble`).
    Gordon(GordonReport),
    /// Steam Deck input (`0x09`).
    Neptune(NeptuneReport),
    /// A wireless controller connected (`0x03`, payload `0x02`).
    Connected,
    /// A wireless controller disconnected (`0x03`, payload `0x01`).
    Disconnected,
    /// A battery-status frame (`0x04`, wireless only).
    Battery(BatteryRaw),
}

/// Raw battery status from the `0x04` frame (offsets per the kernel: voltage at
/// `0x0C`, charge at `0x0E`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BatteryRaw {
    /// Battery voltage in millivolts.
    pub voltage_mv: u16,
    /// Battery charge, in percent (0..=100).
    pub charge_percent: u8,
}

/// Original Steam Controller (Gordon) input fields — **USB and Bluetooth** (PLAN §1.4).
///
/// Both transports converge on this one report: `parse_gordon` decodes the USB
/// 64-byte frame, `apply_gordon_ble` accumulates the BLE delta stream, and both
/// emit a clean snapshot with `buttons` already de-multiplexed (see `parse_gordon`).
/// Battery is not here — it arrives out-of-band as [`RawReport::Battery`] (`0x04`).
/// (The inline `GCInput @0x3E` field the C# reads was verified vestigial — always
/// `0` on the dongle — so it is not decoded; PLAN §1.9.)
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GordonReport {
    /// Frame sequence; wire-provided on USB, synthesized on BLE (which carries none).
    pub seq: u32,
    pub buttons: GordonButtons,
    pub left_trigger: u8,
    pub right_trigger: u8,
    /// Left stick position. On **USB** the stick and pad multiplex onto `0x10`, so
    /// only the *untouched-pad* one is live per frame (resolved in `parse_gordon`);
    /// on **BLE** stick and pad are separate chunks, both always live.
    pub left_stick: Vec2i,
    /// Left pad position (see [`left_stick`](Self::left_stick) for the USB multiplex).
    pub left_pad: Vec2i,
    pub right_pad: Vec2i,
    pub accel: Vec3i,
    /// Raw gyro, **device order** `x=0x22, y=0x24, z=0x26` (angular velocity, axes
    /// aligned with `accel`: pitch/roll/yaw). Left as the device sends it — the raw
    /// `y` (roll) channel is inverted vs. a right-handed frame; the sign is corrected
    /// only in [`ControllerState`] (`gordon_gyro`, PLAN §1.9). The C# `gyaw`/`groll`
    /// field names are transposed; the offsets here are correct. BLE raw IMU is
    /// identical to USB (HW-verified), so the same correction applies.
    pub gyro: Vec3i,
    /// Fused orientation quaternion. USB sends it every frame; on BLE it's an optional
    /// chunk we don't enable (`SEND_ORIENTATION` off), so it stays default there.
    pub orientation: Quati,
}

/// Steam Deck (Neptune) input fields (PLAN §1.4).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NeptuneReport {
    pub seq: u32,
    pub buttons: NeptuneButtons,
    pub left_trigger: i16,
    pub right_trigger: i16,
    pub left_stick: Vec2i,
    pub right_stick: Vec2i,
    pub left_pad: Vec2i,
    pub right_pad: Vec2i,
    pub left_pad_pressure: i16,
    pub right_pad_pressure: i16,
    pub accel: Vec3i,
    pub gyro: Vec3i,
    pub orientation: Quati,
}

// --- little-endian field readers (offsets are absolute into the 64-byte report) ---

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn i16_at(b: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([b[off], b[off + 1]])
}
fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn vec2i_at(b: &[u8], off: usize) -> Vec2i {
    Vec2i {
        x: i16_at(b, off),
        y: i16_at(b, off + 2),
    }
}
fn vec3i_at(b: &[u8], off: usize) -> Vec3i {
    Vec3i {
        x: i16_at(b, off),
        y: i16_at(b, off + 2),
        z: i16_at(b, off + 4),
    }
}

/// Parse one 64-byte report into a [`RawReport`], dispatching on the event byte.
///
/// Returns [`Error::ShortReport`] if `buf` is too small.
pub(crate) fn parse(buf: &[u8]) -> Result<RawReport> {
    if buf.len() < REPORT_LEN {
        return Err(Error::ShortReport {
            expected: REPORT_LEN,
            got: buf.len(),
        });
    }
    // buf[0..2] == 0x01, 0x00; buf[2] == event type.
    match buf[2] {
        event_type::INPUT_DATA => Ok(RawReport::Gordon(parse_gordon(buf))),
        event_type::DECK_INPUT_DATA => Ok(RawReport::Neptune(parse_neptune(buf))),
        event_type::CONNECT => Ok(match buf[4] {
            wireless::DISCONNECTED => RawReport::Disconnected,
            _ => RawReport::Connected, // CONNECTED (0x02) and any other → treat as connect
        }),
        event_type::BATTERY => Ok(RawReport::Battery(BatteryRaw {
            voltage_mv: u16_at(buf, 0x0C),
            charge_percent: buf[0x0E],
        })),
        // Unknown event byte: model as a benign connect ping for now (PLAN §1.4/§1.9).
        _ => Ok(RawReport::Connected),
    }
}

/// Decode a Gordon **USB** input frame (PLAN §1.4 offsets).
///
/// The USB wire multiplexes the left pad and analog stick two ways; both are fully
/// resolved **here** so the emitted [`GordonReport`] is clean (matching the BLE path,
/// which has no multiplex — see `apply_gordon_ble`):
/// - **coordinates:** pad and stick share `lpad_x/y` (`0x10`), disambiguated by
///   `LPAD_TOUCH` — the pad when touched, the stick when not. Verified on **both** the
///   wireless dongle and wired (`0x36` is *not* the stick — 0 on wireless, small noise
///   on wired). When pad + stick are used **together** the firmware sets `LPAD_AND_JOY`
///   and *flickers* `LPAD_TOUCH` frame-to-frame to tag which the coord belongs to (the
///   per-frame tag is why the coord split still keys on `LPAD_TOUCH` alone). PLAN §1.9.
/// - **click bit:** `LPAD_PRESS` fires for both a pad click *and* a stick click (HW-
///   verified). It's a real pad click only when the pad is *engaged* — touched, **or**
///   `LPAD_AND_JOY` set (so a pad click survives the `LPAD_TOUCH` flicker during
///   simultaneous use). Otherwise it's a stick click (which already sets `LSTICK_PRESS`),
///   so drop the spurious `LPAD_PRESS`. After this, `LPAD_PRESS` means a pad click and
///   `LSTICK_PRESS` a stick click, unambiguously.
/// - **touch bit:** reported as *engaged* too (`LPAD_TOUCH || LPAD_AND_JOY`) so the touch
///   *button* stays steady through the axis-tag flicker during simultaneous use. (The
///   pad *position* still can't be sampled every frame — that's a single-field wire
///   limit, PLAN §1.9 — but the buttons are clean.)
fn parse_gordon(b: &[u8]) -> GordonReport {
    let mut buttons = GordonButtons::from_bits_truncate(
        b[0x08] as u32 | (b[0x09] as u32) << 8 | (b[0x0A] as u32) << 16,
    );
    let left_touched = buttons.contains(GordonButtons::LPAD_TOUCH);
    let left_engaged = left_touched || buttons.contains(GordonButtons::LPAD_AND_JOY);
    if buttons.contains(GordonButtons::LPAD_PRESS) && !left_engaged {
        buttons.remove(GordonButtons::LPAD_PRESS); // was a stick click, not a pad click
    }
    let left_raw = vec2i_at(b, 0x10);
    let (left_pad, left_stick) = if left_touched {
        (left_raw, Vec2i::default())
    } else {
        (Vec2i::default(), left_raw)
    };
    // Touch *button*: report it steady while the pad is engaged. In simultaneous
    // pad+stick use the wire flickers LPAD_TOUCH as the per-frame axis tag (with
    // LPAD_AND_JOY held set), so the raw bit toggles though the finger never leaves.
    // The axis split above already consumed the raw per-frame tag; only the emitted
    // button is normalized. Matches kernel `BTN_THUMB = lpad_touched || lpad_and_joy`.
    buttons.set(GordonButtons::LPAD_TOUCH, left_engaged);
    GordonReport {
        seq: u32_at(b, 0x04),
        buttons,
        left_trigger: b[0x0B],
        right_trigger: b[0x0C],
        left_stick,
        left_pad,
        right_pad: vec2i_at(b, 0x14),
        accel: vec3i_at(b, 0x1C),
        gyro: vec3i_at(b, 0x22),
        orientation: Quati {
            x: i16_at(b, 0x28),
            y: i16_at(b, 0x2A),
            z: i16_at(b, 0x2C),
            w: i16_at(b, 0x2E),
        },
    }
}

/// Apply one reassembled **BLE** input payload to the accumulated `acc`, in place.
///
/// `payload` is a fully-reassembled packet: `byte0` low nibble = report type,
/// `(byte0 & 0xF0) | (byte1 << 8)` = the chunk mask, then each present chunk's
/// bytes in ascending bit order. Only `State` reports carry input; on anything
/// else (e.g. a `Status`/battery report) `acc` is left untouched. Returns `true`
/// if an input state was applied (caller bumps `seq` and emits a snapshot).
pub(crate) fn apply_gordon_ble(acc: &mut GordonReport, payload: &[u8]) -> bool {
    if payload.len() < 2 || (payload[0] & 0x0F) != ble::report_type::STATE {
        return false;
    }
    let mask = ((payload[0] & 0xF0) as u16) | ((payload[1] as u16) << 8);
    let mut p = 2usize;
    // Read a chunk of `n` bytes starting at the cursor, advancing it; None if the
    // payload is short (defensive — a well-formed packet always fits).
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
        acc.buttons = GordonButtons::from_bits_truncate(
            payload[o] as u32 | (payload[o + 1] as u32) << 8 | (payload[o + 2] as u32) << 16,
        );
    }
    if mask & chunk::TRIGGERS != 0 && let Some(o) = take(2) {
        acc.left_trigger = payload[o];
        acc.right_trigger = payload[o + 1];
    }
    if mask & chunk::BUTTON3 != 0 {
        take(3); // high button bytes — unused on the original SC
    }
    if mask & chunk::LSTICK != 0 && let Some(o) = take(4) {
        acc.left_stick = vec2i_at(payload, o);
    }
    if mask & chunk::LPAD != 0 && let Some(o) = take(4) {
        acc.left_pad = vec2i_at(payload, o);
    }
    if mask & chunk::RPAD != 0 && let Some(o) = take(4) {
        acc.right_pad = vec2i_at(payload, o);
    }
    if mask & chunk::ACCEL != 0 && let Some(o) = take(6) {
        acc.accel = vec3i_at(payload, o);
    }
    if mask & chunk::GYRO != 0 && let Some(o) = take(6) {
        acc.gyro = vec3i_at(payload, o);
    }
    if mask & chunk::QUAT != 0 && let Some(o) = take(8) {
        // quat wire order is w,x,y,z (SDL); Quati stores x,y,z,w.
        acc.orientation = Quati {
            w: i16_at(payload, o),
            x: i16_at(payload, o + 2),
            y: i16_at(payload, o + 4),
            z: i16_at(payload, o + 6),
        };
    }
    true
}

/// Decode a Neptune (Steam Deck) input frame.
///
/// Offsets are cross-checked against the kernel `hid-steam.c`
/// (`steam_do_deck_input_event` / `_sensors_event`) and the C# `NCInput` struct —
/// they agree. Unlike Gordon there is **no multiplex**: sticks and pads are
/// separate fields, both stored raw (the mapper gates the pad on touch). Triggers
/// are `i16` (`0..=32767`); the trigger **full-pull** is a firmware-synthesized
/// button bit (`buttons0` bit 0/1 = R2/L2), verified present on the Deck despite
/// no microswitch. IMU (accel/gyro/orientation) passes through **raw in device
/// order** — Neptune axis/sign are **unverified** (different sensor; corrected later
/// in `ControllerState`, PLAN §1.9).
fn parse_neptune(b: &[u8]) -> NeptuneReport {
    // buttons0..6 = bytes 0x08..0x0E; byte N at bits 8*N (byte 0x0C is unused).
    let buttons = NeptuneButtons::from_bits_truncate(
        b[0x08] as u64
            | (b[0x09] as u64) << 8
            | (b[0x0A] as u64) << 16
            | (b[0x0B] as u64) << 24
            | (b[0x0C] as u64) << 32
            | (b[0x0D] as u64) << 40
            | (b[0x0E] as u64) << 48,
    );
    NeptuneReport {
        seq: u32_at(b, 0x04),
        buttons,
        left_trigger: i16_at(b, 0x2C),
        right_trigger: i16_at(b, 0x2E),
        left_stick: vec2i_at(b, 0x30),
        right_stick: vec2i_at(b, 0x34),
        left_pad: vec2i_at(b, 0x10),
        right_pad: vec2i_at(b, 0x14),
        left_pad_pressure: i16_at(b, 0x38),
        right_pad_pressure: i16_at(b, 0x3A),
        accel: vec3i_at(b, 0x18),
        gyro: vec3i_at(b, 0x1E),
        orientation: Quati {
            x: i16_at(b, 0x24),
            y: i16_at(b, 0x26),
            z: i16_at(b, 0x28),
            w: i16_at(b, 0x2A),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buttons::GordonButtons;
    use crate::protocol::{event_type, wireless};

    fn frame_buf(event: u8) -> [u8; REPORT_LEN] {
        let mut b = [0u8; REPORT_LEN];
        b[0] = 0x01;
        b[2] = event;
        b
    }

    #[test]
    fn parse_dispatches_lifecycle_frames() {
        let mut b = frame_buf(event_type::CONNECT);
        b[4] = wireless::CONNECTED;
        assert_eq!(parse(&b).unwrap(), RawReport::Connected);
        b[4] = wireless::DISCONNECTED;
        assert_eq!(parse(&b).unwrap(), RawReport::Disconnected);

        let b = frame_buf(event_type::BATTERY);
        assert!(matches!(parse(&b).unwrap(), RawReport::Battery(_)));
    }

    /// parse_gordon resolves the USB left-click multiplex: a stick click (shares the
    /// `LPAD_PRESS` bit, no `LPAD_TOUCH`) must not surface as a pad press.
    #[test]
    fn parse_gordon_demuxes_stick_click() {
        let mut b = frame_buf(event_type::INPUT_DATA);
        let word = (GordonButtons::LPAD_PRESS | GordonButtons::LSTICK_PRESS).bits();
        b[0x08..0x0B].copy_from_slice(&word.to_le_bytes()[..3]);
        let RawReport::Gordon(g) = parse(&b).unwrap() else { panic!("expected Gordon") };
        assert!(g.buttons.contains(GordonButtons::LSTICK_PRESS));
        assert!(!g.buttons.contains(GordonButtons::LPAD_PRESS));
    }

    /// A genuine pad click (`LPAD_PRESS` with `LPAD_TOUCH`) survives de-multiplexing.
    #[test]
    fn parse_gordon_keeps_touched_pad_click() {
        let mut b = frame_buf(event_type::INPUT_DATA);
        let word = (GordonButtons::LPAD_PRESS | GordonButtons::LPAD_TOUCH).bits();
        b[0x08..0x0B].copy_from_slice(&word.to_le_bytes()[..3]);
        let RawReport::Gordon(g) = parse(&b).unwrap() else { panic!("expected Gordon") };
        assert!(g.buttons.contains(GordonButtons::LPAD_PRESS));
        assert!(g.buttons.contains(GordonButtons::LPAD_TOUCH));
    }

    /// Simultaneous pad+stick use: the firmware sets `LPAD_AND_JOY` and flickers
    /// `LPAD_TOUCH`; a pad click must survive an off-flicker frame (gated on AND_JOY,
    /// not touch alone) — HW-verified on the dongle.
    #[test]
    fn parse_gordon_keeps_pad_click_via_and_joy() {
        let mut b = frame_buf(event_type::INPUT_DATA);
        // pad click + stick click, LPAD_TOUCH momentarily 0 but LPAD_AND_JOY set.
        let word = (GordonButtons::LPAD_PRESS
            | GordonButtons::LSTICK_PRESS
            | GordonButtons::LPAD_AND_JOY)
            .bits();
        b[0x08..0x0B].copy_from_slice(&word.to_le_bytes()[..3]);
        let RawReport::Gordon(g) = parse(&b).unwrap() else { panic!("expected Gordon") };
        assert!(g.buttons.contains(GordonButtons::LPAD_PRESS)); // kept via AND_JOY
        assert!(g.buttons.contains(GordonButtons::LSTICK_PRESS));
    }

    /// Touch *button* stays steady during simultaneous use: on a stick-tag frame the raw
    /// LPAD_TOUCH is 0, but LPAD_AND_JOY is set, so the reported touch is true (no flicker).
    #[test]
    fn parse_gordon_touch_steady_via_and_joy() {
        let mut b = frame_buf(event_type::INPUT_DATA);
        let word = (GordonButtons::LSTICK_PRESS | GordonButtons::LPAD_AND_JOY).bits();
        b[0x08..0x0B].copy_from_slice(&word.to_le_bytes()[..3]);
        let RawReport::Gordon(g) = parse(&b).unwrap() else { panic!("expected Gordon") };
        assert!(g.buttons.contains(GordonButtons::LPAD_TOUCH)); // steady via AND_JOY
    }

    #[test]
    fn parse_rejects_short_report() {
        let short = [0u8; 10];
        assert!(matches!(parse(&short), Err(Error::ShortReport { .. })));
    }
}
