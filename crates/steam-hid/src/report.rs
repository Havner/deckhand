//! Layer 1: the decoded wire frame (PLAN §1.5).
//!
//! One physical read yields exactly one frame of *some* type — an input frame or
//! a lifecycle frame — so [`RawReport`] carries both. Field offsets follow PLAN
//! §1.4 and are **unverified on hardware** (PLAN §1.9).

use crate::buttons::{GordonButtons, NeptuneButtons, TritonButtons};
use crate::error::{Error, Result};
use crate::protocol::{
    ControllerStatus, GordonState, NeptuneState, REPORT_LEN, TritonBatteryStatus,
    TritonStateNoQuat, TritonWirelessStatus, Wire, WireQuat, WireVec2, WireVec3, WirelessEvent,
    ble, event_type, triton, wireless,
};
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
    /// New Steam Controller (Triton) input — report `0x42`/`0x45`.
    Triton(TritonReport),
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
    /// Thumbstick **capacitive force** (bytes 0x3C/0x3E) — a raw capacitive magnitude for the stick
    /// top, not a clean/normalized force. HW: idles ~`-5..0` and rises to ~`380..450` when pressed,
    /// per-stick (InputPlumber's `STICK_FORCE_MAX = 112` is wrong); most likely the signal the
    /// firmware thresholds into the binary stick-**touch** bit. Kept raw for reference and **not
    /// exposed** as a bindable input. InputPlumber-only: SDL's `SteamDeckStatePacket_t` and the
    /// kernel stop at pad pressure and never read these. **Neptune (Deck) only** — the Triton body
    /// has no equivalent field.
    pub left_stick_force: i16,
    pub right_stick_force: i16,
    pub left_pad: Vec2i,
    pub right_pad: Vec2i,
    pub left_pad_pressure: i16,
    pub right_pad_pressure: i16,
    pub accel: Vec3i,
    pub gyro: Vec3i,
    pub orientation: Quati,
}

/// New Steam Controller (Triton) input fields (PLAN §1.4).
///
/// Decoded from the report `0x42`/`0x45` "NoQuat" body (`parse_triton`). Same *field set* as
/// [`NeptuneReport`] — Triton is a Deck-shaped superset — but a **more compact byte layout** and
/// no left-multiplex (separate stick/pad fields, both always live). The on-controller quaternion
/// (present in the older `0x42` body) is **not decoded** — it is unused downstream, and the NoQuat
/// parse works for both bodies (the quaternion is simply trailing bytes we skip). Triggers are the
/// analog 16-bit values (`0..=32767`); their digital full-pull is a button bit. Battery arrives
/// out-of-band as [`RawReport::Battery`] (report `0x43`).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TritonReport {
    /// Frame sequence — synthesized from the 1-byte wire `seq_num`.
    pub seq: u32,
    pub buttons: TritonButtons,
    pub left_trigger: i16,
    pub right_trigger: i16,
    pub left_stick: Vec2i,
    pub right_stick: Vec2i,
    pub left_pad: Vec2i,
    pub right_pad: Vec2i,
    pub left_pad_pressure: i16,
    pub right_pad_pressure: i16,
    pub accel: Vec3i,
    /// Raw gyro — already in the unified right-handed frame (HW-verified; passed through in
    /// [`ControllerState`] with no correction, like Neptune — PLAN §1.9).
    pub gyro: Vec3i,
}

// --- raw wire chunk → decoded value conversions (the wire structs live in `protocol`) ---

impl From<WireVec2> for Vec2i {
    fn from(v: WireVec2) -> Self {
        Vec2i { x: v.x, y: v.y }
    }
}
impl From<WireVec3> for Vec3i {
    fn from(v: WireVec3) -> Self {
        Vec3i { x: v.x, y: v.y, z: v.z }
    }
}
impl From<WireQuat> for Quati {
    fn from(q: WireQuat) -> Self {
        Quati { x: q.x, y: q.y, z: q.z, w: q.w }
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
    // buf[0..2] == 0x01, 0x00; buf[2] == event type. `buf` is >= REPORT_LEN, so every cast fits.
    match buf[2] {
        event_type::STATE => {
            Ok(RawReport::Gordon(parse_gordon(GordonState::from_bytes(buf).unwrap())))
        }
        event_type::DECK_STATE => {
            Ok(RawReport::Neptune(parse_neptune(NeptuneState::from_bytes(buf).unwrap())))
        }
        event_type::WIRELESS => {
            let event = WirelessEvent::from_bytes(buf).unwrap().event;
            Ok(match event {
                wireless::DISCONNECTED => RawReport::Disconnected,
                _ => RawReport::Connected, // CONNECTED (0x02) and any other → treat as connect
            })
        }
        event_type::STATUS => {
            let p = ControllerStatus::from_bytes(buf).unwrap();
            Ok(RawReport::Battery(BatteryRaw {
                voltage_mv: p.voltage_mv,
                charge_percent: p.charge_percent,
            }))
        }
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
fn parse_gordon(p: GordonState) -> GordonReport {
    // Copy packed fields into aligned locals before use (can't reference a packed field).
    let (seq, btn, left_trigger, right_trigger) = (p.seq, p.buttons, p.left_trigger, p.right_trigger);
    let (left, right_pad, accel, gyro, orientation) =
        (p.left, p.right_pad, p.accel, p.gyro, p.orientation);
    let mut buttons = GordonButtons::from_bits_truncate(
        btn[0] as u32 | (btn[1] as u32) << 8 | (btn[2] as u32) << 16,
    );
    let left_touched = buttons.contains(GordonButtons::LPAD_TOUCH);
    let left_engaged = left_touched || buttons.contains(GordonButtons::LPAD_AND_JOY);
    if buttons.contains(GordonButtons::LPAD_PRESS) && !left_engaged {
        buttons.remove(GordonButtons::LPAD_PRESS); // was a stick click, not a pad click
    }
    let left_raw: Vec2i = left.into();
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
        seq,
        buttons,
        left_trigger,
        right_trigger,
        left_stick,
        left_pad,
        right_pad: right_pad.into(),
        accel: accel.into(),
        gyro: gyro.into(),
        orientation: orientation.into(),
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
    // `take(n)` guarantees `payload[o..]` has >= n bytes, so each chunk cast below is infallible.
    if mask & chunk::LSTICK != 0 && let Some(o) = take(4) {
        acc.left_stick = WireVec2::from_bytes(&payload[o..]).unwrap().into();
    }
    if mask & chunk::LPAD != 0 && let Some(o) = take(4) {
        acc.left_pad = WireVec2::from_bytes(&payload[o..]).unwrap().into();
    }
    if mask & chunk::RPAD != 0 && let Some(o) = take(4) {
        acc.right_pad = WireVec2::from_bytes(&payload[o..]).unwrap().into();
    }
    if mask & chunk::ACCEL != 0 && let Some(o) = take(6) {
        acc.accel = WireVec3::from_bytes(&payload[o..]).unwrap().into();
    }
    if mask & chunk::GYRO != 0 && let Some(o) = take(6) {
        acc.gyro = WireVec3::from_bytes(&payload[o..]).unwrap().into();
    }
    if mask & chunk::QUAT != 0 && let Some(o) = take(8) {
        acc.orientation = WireQuat::from_bytes(&payload[o..]).unwrap().into();
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
fn parse_neptune(p: NeptuneState) -> NeptuneReport {
    // Copy packed fields into aligned locals before use.
    let btn = p.buttons; // [u8; 8] (SDL 8-byte button union)
    let (seq, left_trigger, right_trigger) = (p.seq, p.left_trigger, p.right_trigger);
    let (left_stick, right_stick, left_pad, right_pad, accel, gyro, orientation) =
        (p.left_stick, p.right_stick, p.left_pad, p.right_pad, p.accel, p.gyro, p.orientation);
    let (left_pad_pressure, right_pad_pressure, left_stick_force, right_stick_force) =
        (p.left_pad_pressure, p.right_pad_pressure, p.left_stick_force, p.right_stick_force);
    // 8-byte button union → u64, byte N at bits 8*N (undefined bits are truncated).
    let buttons = NeptuneButtons::from_bits_truncate(u64::from_le_bytes(btn));
    NeptuneReport {
        seq,
        buttons,
        left_trigger,
        right_trigger,
        left_stick: left_stick.into(),
        right_stick: right_stick.into(),
        left_stick_force,
        right_stick_force,
        left_pad: left_pad.into(),
        right_pad: right_pad.into(),
        left_pad_pressure,
        right_pad_pressure,
        accel: accel.into(),
        gyro: gyro.into(),
        orientation: orientation.into(),
    }
}

/// Parse one Triton (new Steam Controller) report, dispatching on the **report id in byte 0**
/// (Triton does not use the `0x01`-framed `ValveInReport_t`). `buf` is the raw read of length `n`.
///
/// Returns `None` for a report we don't decode as a frame yet — the timestamped `0x47` "Ibex"
/// body (added only if a unit streams it, PLAN §1.9) and any unknown id — so the caller keeps
/// reading rather than surfacing a bogus frame.
pub(crate) fn parse_triton(buf: &[u8]) -> Option<RawReport> {
    match *buf.first()? {
        triton::report::CONTROLLER_STATE | triton::report::CONTROLLER_STATE_BLE => {
            parse_triton_state(buf).map(RawReport::Triton)
        }
        triton::report::BATTERY_STATUS => {
            let p = TritonBatteryStatus::from_bytes(buf)?; // None if the read is too short
            let (level, voltage) = (p.battery_level, p.voltage_mv);
            Some(RawReport::Battery(BatteryRaw { voltage_mv: voltage, charge_percent: level }))
        }
        triton::report::WIRELESS_STATUS | triton::report::WIRELESS_STATUS_X => {
            match TritonWirelessStatus::from_bytes(buf)?.state {
                triton::wireless::DISCONNECT => Some(RawReport::Disconnected),
                triton::wireless::CONNECT => Some(RawReport::Connected),
                _ => None,
            }
        }
        // 0x47 (Ibex, timestamped body) and anything else: not decoded — skip.
        _ => None,
    }
}

/// Decode the Triton report `0x42`/`0x45` "NoQuat" body. Offsets are into the raw read (byte 0 =
/// report id); little-endian throughout. Cross-checked against SDL `TritonMTUNoQuat_t` and
/// sc-controller's `docs/steam-controller-v2-protocol.md` (they agree). Returns `None` if the
/// read is too short to contain the IMU block.
fn parse_triton_state(b: &[u8]) -> Option<TritonReport> {
    // `from_bytes` returns None if the read is shorter than the 46-byte NoQuat body (through the gyro
    // block). IMU is 0/constant unless the gyro is enabled, but the fields are always present. The
    // `0x42` (Full) body is parsed here too — NoQuat is its 46-byte prefix; the quat is trailing.
    let p = TritonStateNoQuat::from_bytes(b)?;
    // Copy packed fields into aligned locals before use.
    let (seq_num, buttons_raw, left_trigger, right_trigger) =
        (p.seq_num, p.buttons, p.left_trigger, p.right_trigger);
    let (left_stick, right_stick, left_pad, right_pad, accel, gyro) =
        (p.left_stick, p.right_stick, p.left_pad, p.right_pad, p.accel, p.gyro);
    let (left_pad_pressure, right_pad_pressure) = (p.left_pad_pressure, p.right_pad_pressure);
    Some(TritonReport {
        seq: seq_num as u32,
        buttons: TritonButtons::from_bits_truncate(buttons_raw),
        left_trigger,
        right_trigger,
        left_stick: left_stick.into(),
        right_stick: right_stick.into(),
        left_pad: left_pad.into(),
        left_pad_pressure,
        right_pad: right_pad.into(),
        right_pad_pressure,
        // imu_timestamp (offset 30) unused — we synthesize seq from seq_num.
        accel: accel.into(),
        gyro: gyro.into(),
    })
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
        let mut b = frame_buf(event_type::WIRELESS);
        b[4] = wireless::CONNECTED;
        assert_eq!(parse(&b).unwrap(), RawReport::Connected);
        b[4] = wireless::DISCONNECTED;
        assert_eq!(parse(&b).unwrap(), RawReport::Disconnected);

        let b = frame_buf(event_type::STATUS);
        assert!(matches!(parse(&b).unwrap(), RawReport::Battery(_)));
    }

    /// parse_gordon resolves the USB left-click multiplex: a stick click (shares the
    /// `LPAD_PRESS` bit, no `LPAD_TOUCH`) must not surface as a pad press.
    #[test]
    fn parse_gordon_demuxes_stick_click() {
        let mut b = frame_buf(event_type::STATE);
        let word = (GordonButtons::LPAD_PRESS | GordonButtons::LSTICK_PRESS).bits();
        b[0x08..0x0B].copy_from_slice(&word.to_le_bytes()[..3]);
        let RawReport::Gordon(g) = parse(&b).unwrap() else { panic!("expected Gordon") };
        assert!(g.buttons.contains(GordonButtons::LSTICK_PRESS));
        assert!(!g.buttons.contains(GordonButtons::LPAD_PRESS));
    }

    /// A genuine pad click (`LPAD_PRESS` with `LPAD_TOUCH`) survives de-multiplexing.
    #[test]
    fn parse_gordon_keeps_touched_pad_click() {
        let mut b = frame_buf(event_type::STATE);
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
        let mut b = frame_buf(event_type::STATE);
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
        let mut b = frame_buf(event_type::STATE);
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
