//! Layer 1: the decoded wire frame (PLAN §1.5).
//!
//! One physical read yields exactly one frame of *some* type — an input frame or
//! a lifecycle frame — so [`RawReport`] carries both. Field offsets follow PLAN
//! §1.4 and are **unverified on hardware** (PLAN §1.9).

use crate::buttons::{GordonBleButtons, GordonButtons, NeptuneButtons};
use crate::error::{Error, Result};
use crate::protocol::{REPORT_LEN, ble, event_type, wireless};
use crate::value::{Quati, Vec2i, Vec3i};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A decoded wire frame: a per-device input report, or a lifecycle signal.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum RawReport {
    /// Original Steam Controller input (`0x01`).
    Gordon(GordonReport),
    /// Original Steam Controller input over **Bluetooth** — a full snapshot
    /// accumulated from the BLE delta stream (see [`GordonBleReport`]).
    GordonBle(GordonBleReport),
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

/// Original Steam Controller (Gordon) input fields (PLAN §1.4).
///
/// Battery is not here — it arrives out-of-band as [`RawReport::Battery`] (`0x04`).
/// (The inline `GCInput @0x3E` field the C# reads was verified vestigial — always
/// `0` on the dongle — so it is not decoded; PLAN §1.9.)
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GordonReport {
    pub seq: u32,
    pub buttons: GordonButtons,
    pub left_trigger: u8,
    pub right_trigger: u8,
    /// Left stick position (shares `0x10` with the pad — see `parse`).
    pub left_stick: Vec2i,
    /// Left pad position (shares `0x10` with the stick — see `parse`).
    pub left_pad: Vec2i,
    pub right_pad: Vec2i,
    pub accel: Vec3i,
    /// Raw gyro, **device order** `x=0x22, y=0x24, z=0x26` (angular velocity, axes
    /// aligned with `accel`: pitch/roll/yaw). Left as the device sends it — the raw
    /// `y` (roll) channel is inverted vs. a right-handed frame; the sign is corrected
    /// only in [`ControllerState`] (`gordon_gyro`, PLAN §1.9). The C# `gyaw`/`groll`
    /// field names are transposed; the offsets here are correct.
    pub gyro: Vec3i,
    pub orientation: Quati,
}

/// Original Steam Controller (Gordon) input over **Bluetooth** (PLAN §1.4).
///
/// The BLE wire is a *delta* stream — each packet carries only the chunks that
/// changed — so this struct is the **accumulated** state, updated in place by
/// [`apply_gordon_ble`] and emitted as a full snapshot per input packet. Unlike
/// USB Gordon there is **no left multiplex**: `left_stick` and `left_pad` are
/// distinct, always-current fields. IMU raw values match USB Gordon exactly
/// (HW-verified, PLAN §1.9), so the same `gordon_gyro` correction applies during
/// conversion. `seq` is synthesized (the wire carries no sequence number). The
/// `orientation` quaternion chunk is only present if `SEND_ORIENTATION` is enabled;
/// we don't enable it, so it stays default (unused downstream — PLAN §1.9).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GordonBleReport {
    pub seq: u32,
    pub buttons: GordonBleButtons,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_stick: Vec2i,
    pub left_pad: Vec2i,
    pub right_pad: Vec2i,
    pub accel: Vec3i,
    pub gyro: Vec3i,
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

/// Decode a Gordon input frame (PLAN §1.4 offsets).
///
/// The left pad and analog stick share `lpad_x/y` (`0x10`), disambiguated by
/// `LPAD_TOUCH`: the pad when touched, the stick when not. Verified on **both** the
/// wireless dongle and wired — `0x36` is *not* the stick (0 on wireless, small noise
/// on wired) and `LPAD_AND_JOY` is unused. (PLAN §1.4.)
fn parse_gordon(b: &[u8]) -> GordonReport {
    let buttons = GordonButtons::from_bits_truncate(
        b[0x08] as u32 | (b[0x09] as u32) << 8 | (b[0x0A] as u32) << 16,
    );
    let left_raw = vec2i_at(b, 0x10);
    let (left_pad, left_stick) = if buttons.contains(GordonButtons::LPAD_TOUCH) {
        (left_raw, Vec2i::default())
    } else {
        (Vec2i::default(), left_raw)
    };
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
pub(crate) fn apply_gordon_ble(acc: &mut GordonBleReport, payload: &[u8]) -> bool {
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
        acc.buttons = GordonBleButtons::from_bits_truncate(
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

    #[test]
    fn parse_rejects_short_report() {
        let short = [0u8; 10];
        assert!(matches!(parse(&short), Err(Error::ShortReport { .. })));
    }
}
