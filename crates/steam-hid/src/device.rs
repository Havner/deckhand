//! The opened device: per-device I/O and the command/haptic surface (PLAN 1.5, 1.6).
//! Discovery and opening live in `backend` (`Manager`); the device identity vocabulary in `info`.

use std::time::{Duration, Instant};

use crate::backend::HidapiDevice;
use crate::error::{Error, Result};
use crate::event::Events;
use crate::info::{DeviceInfo, Transport};
use crate::protocol::{
    self, ControllerStringAttributes, GyroMode, HapticIntensity, HapticPosition, HapticSide,
    HapticStyle, HapticType, MsgId, TrackpadDPadMode, TritonOutReport, Wire, setting,
};
use crate::state::{self, Battery, ControllerState, Report};
use crate::value::Timestamp;

/// Internal read timeout for the "blocking" `read_*`; looped so it can later be
/// made cancellable for cooperative shutdown (PLAN 1.6).
const READ_TIMEOUT_MS: i32 = 1000;

/// Parameters for a `TRIGGER_HAPTIC_PULSE` (`0x8f`) trackpad haptic pulse ([`Device::haptic_pulse`]):
/// the actuator plays `count` pulses, each `duration` us on then `interval` us off, so
/// `duration`/`interval` set the tone and `count` the length. `gain` (dB) is honored on the Deck but
/// **ignored on Gordon** (amplitude there = duty cycle). This is a *pulse* on the trackpad actuator -
/// Gordon's only haptic, usable on the Deck too; the Deck's native dual-motor rumble is a different
/// command ([`Device::rumble_cmd`], `0xeb`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HapticPulse {
    pub duration: u16,
    pub interval: u16,
    pub count: u16,
    pub gain: i8,
}

/// An open device: a transport endpoint that outlives connect/disconnect (PLAN 1.5).
///
/// Driven in **one** mode at a time - snapshots (`read`/`poll`) or events
/// (`events`) - since both consume the single frame stream.
pub struct Device {
    backend: HidapiDevice,
    info: DeviceInfo,
    start: Instant,
    connected: bool,
    battery: Option<Battery>,
    buf: [u8; protocol::REPORT_LEN],
    /// BLE reassembly + accumulation state; `Some` only on the Bluetooth transport.
    ble: Option<BleState>,
}

/// Per-device state for the Bluetooth transport (PLAN 1.4).
///
/// BLE input is a segmented *delta* stream, so we reassemble 20-byte segments into
/// a full packet and accumulate chunk updates into `acc`, emitting a snapshot per
/// completed input packet. `seq` is synthesized (the wire has none).
struct BleState {
    /// Reassembly buffer for the current multi-segment packet.
    assembled: [u8; protocol::ble::SEGMENT_PAYLOAD * protocol::ble::MAX_SEGMENTS],
    /// Next segment number expected (resets to 0 on a completed/!ordered packet).
    expected_seg: usize,
    /// Accumulated snapshot - BLE chunks decode straight into it (only-changed chunks arrive per
    /// packet), so there is no decoded-report intermediate (see `state::apply_gordon_ble`).
    acc: ControllerState,
    /// Synthesized sequence counter, bumped per input snapshot.
    seq: u32,
}

impl BleState {
    fn new() -> Self {
        BleState {
            assembled: [0u8; protocol::ble::SEGMENT_PAYLOAD * protocol::ble::MAX_SEGMENTS],
            expected_seg: 0,
            acc: ControllerState::default(),
            seq: 0,
        }
    }
}

impl Device {
    pub(crate) fn new(backend: HidapiDevice, info: DeviceInfo) -> Result<Self> {
        // Wired USB and Bluetooth are point-to-point (connected the moment the
        // endpoint opens); only the dongle multiplexes an absent controller.
        let connected = matches!(info.transport, Transport::UsbWired | Transport::Bluetooth);
        // Gordon's BLE segmented-delta reassembly. Triton over BLE is a *different* framing (its own
        // report ids, no segmentation) handled by the Triton path - so this state is Gordon-only.
        let ble = (info.transport.is_bluetooth() && !info.kind.is_triton()).then(BleState::new);
        let mut dev = Device {
            backend,
            info,
            start: Instant::now(),
            connected,
            battery: None,
            buf: [0u8; protocol::REPORT_LEN],
            ble,
        };
        // On a wireless endpoint, prompt the current connection status so an
        // already-connected controller surfaces without waiting (PLAN 1.6).
        // Gordon dongle only - this is the original receiver's wireless-state command; the Triton
        // puck streams state by default when a controller is present, so it needs no prompt.
        if matches!(dev.info.transport, Transport::UsbDongle) && !dev.info.kind.is_triton() {
            let _ = dev.dongle_get_wireless_state();
        }
        Ok(dev)
    }

    /// The device this was opened from.
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Whether a controller is currently connected on this endpoint (cached from
    /// `0x03` frames; convenience, not load-bearing - PLAN 1.5).
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Last-known battery status, if any (wireless only).
    pub fn battery(&self) -> Option<Battery> {
        self.battery.clone()
    }

    // --- input: one physical read == one frame of some type ---

    /// Read one high-level frame (normalized snapshot or lifecycle), blocking (PLAN 1.5).
    pub fn read(&mut self) -> Result<Report> {
        loop {
            if let Some(report) = self.next_frame(READ_TIMEOUT_MS)? {
                return Ok(report);
            }
        }
    }

    /// Read one high-level frame, waiting up to `timeout`. `None` on timeout.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Report>> {
        self.next_frame(clamp_timeout(timeout))
    }

    /// The change-driven [`Events`] view over this device (PLAN 1.5).
    pub fn events(&mut self) -> Events<'_> {
        Events::new(self)
    }

    /// Update the cached connection/battery state from a decoded frame.
    fn update_cache(&mut self, report: &Report) {
        match report {
            Report::Connected => self.connected = true,
            Report::Disconnected => self.connected = false,
            Report::Battery(b) => self.battery = Some(b.clone()),
            Report::State(_) => {}
        }
    }

    /// Read one frame, decode it straight to a [`Report`], and update cached state.
    fn next_frame(&mut self, timeout_ms: i32) -> Result<Option<Report>> {
        if self.info.kind.is_triton() {
            return self.next_frame_triton(timeout_ms);
        }
        if self.ble.is_some() {
            return self.next_frame_ble(timeout_ms);
        }
        // The default path: **USB Gordon (wired + dongle) and Neptune** - one physical read is one
        // 64-byte `0x01`-framed report (`[0x01, 0x00, <event>, ...]`), decoded by `state::parse`.
        let n = self.backend.read_timeout(&mut self.buf, timeout_ms)?;
        if n == 0 {
            return Ok(None);
        }
        let report = state::parse(&self.buf, self.now())?;
        self.update_cache(&report);
        Ok(Some(report))
    }

    /// Triton read path: one physical read == one report, dispatched by the **report id in byte 0**
    /// (not the `0x01`-framed event byte Gordon/Neptune use - see `state::parse_triton`). Works for
    /// every Triton transport: the puck and wire stream state as `0x42`, **Bluetooth streams `0x45`**
    /// (both the same "NoQuat" body). On Linux/Windows the OS HID-over-GATT stack reassembles BLE and
    /// prepends the report id, so BT reports arrive here exactly like USB (no segmentation, unlike
    /// Gordon BLE). Reports we don't decode as a frame (the `0x47` timestamped body, unknown ids) are
    /// skipped by reading again within the timeout budget; a read timeout returns `None`.
    fn next_frame_triton(&mut self, timeout_ms: i32) -> Result<Option<Report>> {
        loop {
            let n = self.backend.read_timeout(&mut self.buf, timeout_ms)?;
            if n == 0 {
                return Ok(None);
            }
            let Some(report) = state::parse_triton(&self.buf[..n], self.now()) else {
                continue; // undecoded report - keep reading
            };
            self.update_cache(&report);
            return Ok(Some(report));
        }
    }

    /// BLE read path: reassemble 20-byte segments into a packet, accumulate its chunks, and emit a
    /// full snapshot per completed **input** packet (PLAN 1.4).
    ///
    /// Loops within one call so a multi-segment frame returns as one [`Report`]; a read timeout
    /// returns `None` with partial reassembly state preserved for the next call. Non-input (status)
    /// packets are skipped (read again).
    fn next_frame_ble(&mut self, timeout_ms: i32) -> Result<Option<Report>> {
        use protocol::ble;
        let start = self.start; // Copy - stamp the snapshot without re-borrowing self
        loop {
            let mut seg = [0u8; ble::SEGMENT_SIZE];
            let n = self.backend.read_timeout(&mut seg, timeout_ms)?;
            if n == 0 {
                return Ok(None); // timeout - partial reassembly (if any) is retained
            }
            if n < ble::SEGMENT_SIZE || seg[0] != ble::REPORT_ID {
                continue;
            }
            let hdr = seg[1];
            if hdr & ble::SEG_DATA_FLAG == 0 {
                continue; // empty segment
            }
            let ble_state = self.ble.as_mut().expect("ble state present on BT transport");
            let segnum = (hdr & ble::SEG_NUM_MASK) as usize;
            if segnum != ble_state.expected_seg {
                // Out of order: resync only on a fresh packet (segment 0).
                ble_state.expected_seg = 0;
                if segnum != 0 {
                    continue;
                }
            }
            if segnum >= ble::MAX_SEGMENTS {
                ble_state.expected_seg = 0;
                continue;
            }
            let at = segnum * ble::SEGMENT_PAYLOAD;
            ble_state.assembled[at..at + ble::SEGMENT_PAYLOAD]
                .copy_from_slice(&seg[2..ble::SEGMENT_SIZE]);

            if hdr & ble::SEG_LAST_FLAG == 0 {
                ble_state.expected_seg += 1;
                continue; // need more segments
            }

            // Packet complete: decode its chunks into the accumulated snapshot.
            ble_state.expected_seg = 0;
            let len = (segnum + 1) * ble::SEGMENT_PAYLOAD;
            let assembled = ble_state.assembled;
            if state::apply_gordon_ble(&mut ble_state.acc, &assembled[..len]) {
                ble_state.seq = ble_state.seq.wrapping_add(1);
                ble_state.acc.seq = ble_state.seq;
                ble_state.acc.timestamp = Timestamp(start.elapsed());
                return Ok(Some(Report::State(ble_state.acc.clone())));
            }
            // Non-input (status) packet - keep reading within the timeout budget.
        }
    }

    fn now(&self) -> Timestamp {
        Timestamp(self.start.elapsed())
    }

    // --- commands: raw transport primitives ---

    /// Send a raw feature report. Takes the **logical** command
    /// `[cmd_id, len, payload...]`; the transport framing is applied here, not by the
    /// caller (PLAN 1.4): USB prepends report id 0 and pads to 64; Bluetooth splits
    /// the command into Report-ID-3 segments (`[0x03][0x80|seg|(0x40 if last)]
    /// [<=18 data]`, zero-padded to 20 - the command bytes are identical to USB).
    fn send_feature_report(&mut self, cmd: &[u8]) -> Result<()> {
        if self.info.transport.is_bluetooth() && !self.info.kind.is_triton() {
            for seg in frame_ble(cmd) {
                self.backend.send_feature_report(&seg)?;
            }
            Ok(())
        } else if self.info.kind.is_triton() {
            // Triton's command channel rides feature report **0x01** and the whole HID report is
            // **exactly 64 bytes** (report-id byte + 63 payload) - the device stalls a SET_REPORT of
            // any other length (Broken pipe otherwise). Gordon/Neptune use report id 0x00 with 64
            // *data* bytes (65-byte buffer; report 0 is unnumbered so nothing extra goes on the wire).
            self.backend
                .send_feature_report(&frame(cmd, protocol::REPORT_ID_TRITON, protocol::REPORT_LEN))
        } else {
            self.backend
                .send_feature_report(&frame(cmd, protocol::REPORT_ID, 1 + protocol::REPORT_LEN))
        }
    }

    /// Get a raw feature report, sizing the buffer to the device's report shape - the symmetric
    /// counterpart to [`Self::send_feature_report`] (PLAN 1.4): Triton reads report id 0x01 into an
    /// **exactly 64-byte** buffer (a longer GetFeature stalls the ioctl with Broken pipe, mirroring
    /// the SET side), Gordon/Neptune read report id 0x00 into a 65-byte buffer. Returns the reply
    /// bytes truncated to the count read, with the report-id byte still at `[0]`.
    ///
    /// NOTE: the Triton branch is a best effort that does NOT actually work - Triton stalls a
    /// GET_FEATURE on report 0x01 regardless of buffer size, so the GET round-trips (serials/
    /// attributes/settings) all Broken-pipe on it. Its feature channel is effectively write-only;
    /// command replies come back over the interrupt-IN input stream instead (SDL's Triton driver
    /// never issues a feature GET, and sc-controller leaves the read-back a TODO). Making getters
    /// work on Triton would need reverse-engineering that reply framing - out of scope; getters is a
    /// Gordon/Neptune tool. The kept 64-byte shape is correct-if-it-ever-answers, and harmless.
    fn get_feature_report(&mut self) -> Result<Vec<u8>> {
        let (report_id, buf_len) = if self.info.kind.is_triton() {
            (protocol::REPORT_ID_TRITON, protocol::REPORT_LEN)
        } else {
            (protocol::REPORT_ID, 1 + protocol::REPORT_LEN)
        };
        let mut buf = vec![0u8; buf_len];
        buf[0] = report_id;
        let n = self.backend.get_feature_report(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Write a GET request (`request[0]` = command id) and read the reply, retrying a few times (the
    /// device may return other reports first - mirrors SDL's `ReadResponse`). Locates the echoed
    /// command id (at offset 0 or 1, absorbing hidapi's report-id-byte ambiguity for report 0),
    /// strips the `[cmd, len]` header, and returns `(body, len)`: `body` = the reply payload ready to
    /// cast (the bytes after the header), `len` = the reply's length byte clamped to the bytes
    /// available. `valid(len, body)` gates acceptance (rejects stale/other-report replies).
    fn get_roundtrip(
        &mut self,
        cmd: MsgId,
        length: u8,
        payload: &[u8],
        valid: impl Fn(usize, &[u8]) -> bool,
    ) -> Result<(Vec<u8>, usize)> {
        // Build the request through the shared `FeatureReportHeader` (same header as `feature()`) -
        // GET commands vary the header's `length` field: payload length for SETTINGS, `0` for
        // ATTRIBUTES, and the requested max response length for the STRING getter, so it's passed in
        // rather than derived from `payload.len()`.
        let echo = cmd as u8;
        let mut request = protocol::FeatureReportHeader { cmd: echo, length }.as_bytes().to_vec();
        request.extend_from_slice(payload);
        // The dongle's feature endpoint is flaky under back-to-back I/O: a `SetFeature` too soon after
        // a prior `GetFeature` EPIPEs, and a `GetFeature` can race the device mid-updating its reply
        // buffer (stale/half-written). So re-send the whole exchange each attempt, settling around
        // both halves, and only accept a reply whose `cmd` echo matches AND passes `valid`.
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(10));
            if self.send_feature_report(&request).is_err() {
                continue; // busy -> retry the SetFeature
            }
            std::thread::sleep(Duration::from_millis(20)); // let the reply compute
            let buf = self.get_feature_report()?;
            // The reply is `[report-id, cmd, len, body...]` - hidapi keeps the report-id byte at [0]
            // (report 0 on Gordon/Neptune, 0x01 on Triton; matches SDL's `ReadResponse`, HW-confirmed
            // on Gordon). Reject a stale / other-report reply whose echoed cmd at [1] doesn't match
            // (the device replies out of order under back-to-back GETs).
            if buf.len() < 3 || buf[1] != echo {
                continue;
            }
            let body = buf[3..].to_vec();
            let len = (buf[2] as usize).min(body.len());
            if valid(len, &body) {
                return Ok((body, len));
            }
        }
        Err(Error::Unsupported("GET response not received/validated"))
    }

    /// Build `[FeatureReportHeader, payload...]` and send it framed.
    fn feature(&mut self, cmd: MsgId, payload: &[u8]) -> Result<()> {
        let header = protocol::FeatureReportHeader { cmd: cmd as u8, length: payload.len() as u8 };
        let mut bytes = Vec::with_capacity(2 + payload.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(payload);
        self.send_feature_report(&bytes)
    }

    /// Build `[report_id, payload...]` and send it as a Triton haptic **output** report. The output-
    /// report analog of [`Self::feature`], but the framing is a single report-id byte (no length
    /// field) and it rides the interrupt-OUT endpoint. Output reports carry their own fixed lengths,
    /// so no `frame`-style padding.
    fn output(&mut self, cmd: TritonOutReport, payload: &[u8]) -> Result<()> {
        let mut report = Vec::with_capacity(1 + payload.len());
        report.push(cmd as u8);
        report.extend_from_slice(payload);
        self.backend.send_output_report(&report)
    }

    // --- commands: simple generic (most apply to every device) ---

    /// Write settings via `SET_SETTINGS_VALUES` - a concatenation of [`protocol::ControllerSetting`]
    /// `(id, value-le)` triples. Takes `(id, value)` pairs for call-site ergonomics.
    fn set_settings(&mut self, pairs: &[(u8, u16)]) -> Result<()> {
        let mut payload = Vec::with_capacity(pairs.len() * 3);
        for &(setting_num, value) in pairs {
            payload.extend_from_slice(protocol::ControllerSetting { setting_num, value }.as_bytes());
        }
        self.feature(MsgId::SetSettingsValues, &payload)
    }

    /// Enable ("lizard") or disable raw mode.
    ///
    /// Lizard-off clears digital mappings and sets both trackpads to `NONE`
    /// (raw). The Deck (Neptune) reverts to lizard ~10 s after lizard-off unless
    /// it is re-asserted, so a long-lived consumer must call this periodically -
    /// the engine reader does (~2 s, Neptune-gated; HW-verified holding a real
    /// Deck alive across a multi-minute session). steam-hid spawns NO keep-alive
    /// thread by design: `Device` is the single writer (Send, not Sync), so the
    /// cadence lives in the owner's read loop, alongside its other device writes.
    pub fn set_lizard_mode(&mut self, on: bool) -> Result<()> {
        if on {
            self.feature(MsgId::SetDefaultDigitalMappings, &[])?;
            self.feature(MsgId::LoadDefaultSettings, &[])?;
        } else {
            self.feature(MsgId::ClearDigitalMappings, &[])?;
            self.set_settings(&[
                (setting::LEFT_TRACKPAD_MODE, TrackpadDPadMode::None as u16),
                (setting::RIGHT_TRACKPAD_MODE, TrackpadDPadMode::None as u16),
            ])?;
        }
        Ok(())
    }

    /// Set the IMU mode bits (gyro/accel/orientation), PLAN 1.4.
    pub fn set_imu_mode(&mut self, mode: GyroMode) -> Result<()> {
        self.set_settings(&[(setting::IMU_MODE, mode.bits())])
    }

    /// Convenience: enable raw accel + raw gyro, or turn the IMU off.
    pub fn set_gyro(&mut self, on: bool) -> Result<()> {
        let mode = if on {
            GyroMode::raw_motion()
        } else {
            GyroMode::empty()
        };
        self.set_imu_mode(mode)
    }

    /// Set the LED brightness (0..=100 %).
    pub fn set_led_intensity(&mut self, percent: u8) -> Result<()> {
        self.set_settings(&[(setting::LED_USER_BRIGHTNESS, percent.min(100) as u16)])
    }

    /// Set the sleep/idle inactivity timeout, in seconds.
    pub fn set_idle_timeout(&mut self, secs: u16) -> Result<()> {
        self.set_settings(&[(setting::SLEEP_INACTIVITY_TIMEOUT, secs)])
    }

    /// Prompt the dongle for a wireless-state / battery status frame (`DONGLE_GET_WIRELESS_STATE`,
    /// `0xB4`, no payload). The Gordon receiver answers with a `0x04` status frame carrying the
    /// connection state and battery charge, so this both surfaces an already-connected controller and
    /// refreshes battery: [`Device::new`] sends it once on open, and a battery poller re-sends it
    /// periodically (see the `battery` example). Dongle-only - a harmless no-op prompt elsewhere.
    pub fn dongle_get_wireless_state(&mut self) -> Result<()> {
        self.feature(MsgId::DongleGetWirelessState, &[])
    }

    /// Power the controller off.
    pub fn power_off(&mut self) -> Result<()> {
        self.feature(MsgId::TurnOffController, b"off!")
    }

    // --- commands: GET round-trip queries (probe with the `getters` example) ---

    /// Read a string attribute (e.g. the unit serial) via `GET_STRING_ATTRIBUTE` (`0xAE`). Request is
    /// `[0xAE, max_len, tag]` (kernel `steam_get_serial` uses `max_len` = 0x16); the reply is
    /// `[cmd, str_len, echoed_tag, string...]`. **USB only** (BLE feature-report segmentation not
    /// handled here). **HW-UNTESTED.**
    pub fn get_string_attribute(&mut self, tag: ControllerStringAttributes) -> Result<String> {
        let tag = tag as u8;
        // Accept only a reply for *this* tag whose string is non-empty (rejects a stale other-tag
        // reply and the mid-update empty-buffer race). `body` = `MsgGetStringAttribute { tag,
        // value[20] }`; `len` = the reply header's body length (tag + value = 21, HW-observed), an
        // upper bound only - the string itself is NUL-terminated, so we cut `value` at the first NUL.
        let (body, len) = self.get_roundtrip(
            MsgId::GetStringAttribute,
            0x16, // requested max response length (kernel `steam_get_serial`)
            &[tag],
            |_len, body| body.first() == Some(&tag) && body.get(1).is_some_and(|&b| b != 0),
        )?;
        let msg = protocol::MsgGetStringAttribute::from_bytes(&body)
            .ok_or(Error::ShortReport { expected: 21, got: body.len() })?;
        let value = msg.value; // copy the packed array out by value before slicing
        let raw = &value[..len.min(value.len())];
        let raw = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        Ok(String::from_utf8_lossy(raw).into_owned())
    }

    /// Read the controller's read-only attributes via `GET_ATTRIBUTES_VALUES` (`0x83`) - the **full**
    /// list of `(tag, value)` (SDL sends a bare `[0x83]`; the firmware returns all - there's no
    /// per-tag request). `tag` is a raw byte (name it against the `protocol::attribute` const ids),
    /// `value` a `u32`. **USB only.**
    pub fn get_attributes(&mut self) -> Result<Vec<(u8, u32)>> {
        let (data, len) = self.get_roundtrip(
            MsgId::GetAttributesValues,
            0, // no request payload - returns the full set
            &[],
            |len, _| len > 0, // non-empty attribute list
        )?;
        Ok(data[..len]
            .as_chunks::<5>() // ControllerAttribute = {tag:u8, value:u32}
            .0
            .iter()
            .filter_map(|c| protocol::ControllerAttribute::from_bytes(c))
            .map(|a| (a.tag, a.value))
            .collect())
    }

    /// Read specific settings via `GET_SETTINGS_VALUES` (`0x89`) - returns `(id, value)` pairs. The
    /// **request uses the same `ControllerSetting[]` shape as `SET_SETTINGS_VALUES`** - one `(id,
    /// value)` triple per wanted setting (the value is ignored on a read; HW-confirmed on Gordon).
    /// **USB only.**
    pub fn get_settings(&mut self, ids: &[u8]) -> Result<Vec<(u8, u16)>> {
        let mut entries = Vec::with_capacity(ids.len() * 3);
        for &id in ids {
            entries.extend_from_slice(
                protocol::ControllerSetting { setting_num: id, value: 0 }.as_bytes(),
            );
        }
        let (data, len) =
            self.get_roundtrip(MsgId::GetSettingsValues, entries.len() as u8, &entries, |_, _| true)?;
        Ok(data[..len]
            .as_chunks::<3>() // ControllerSetting = {id:u8, value:u16}
            .0
            .iter()
            .filter_map(|c| protocol::ControllerSetting::from_bytes(c))
            .map(|s| (s.setting_num, s.value))
            .collect())
    }

    // --- commands: device-specific (haptics / rumble) ---

    /// Trigger a trackpad haptic **pulse** (`0x8f`), kernel 8-byte form.
    ///
    /// **Verified on Gordon** (PLAN 1.9): [`HapticPosition`] is the trackpad actuator (Gordon's wire
    /// values are swapped: `Right = 0`, `Left = 1`). There is no "both" (pad 2 no-ops on Gordon), so
    /// the caller fires the two pads separately. `params.gain` is honored on the Deck but ignored on
    /// Gordon. This drives the *trackpad* actuator (Gordon's only haptic; works on the Deck too). For
    /// the Deck's native continuous rumble use [`Self::rumble_cmd`].
    pub fn haptic_pulse(&mut self, position: HapticPosition, params: HapticPulse) -> Result<()> {
        let msg = protocol::MsgFireHapticPulse {
            which_pad: position as u8,
            duration: params.duration,
            interval: params.interval,
            count: params.count,
            gain: params.gain,
        };
        self.feature(MsgId::TriggerHapticPulse, msg.as_bytes())
    }

    /// Fire the Deck's `0xEA` `SET_HAPTIC2` - a short, finely-tuned trackpad **click** haptic (much
    /// better than `0x8f` for command clicks; the strongest setting beats a full `0x8f` click). `cmd`
    /// picks the haptic type (we use [`HapticType::Tick`]/[`HapticType::Click`]), `ui_intensity` is a
    /// second HW-confirmed lever (see [`HapticIntensity`]), and `gain` (dB) scales it - together a
    /// wide range of click strengths.
    ///
    /// **Deck-only** (no-ops on Gordon). Payload is the full [`protocol::MsgTriggerHaptic`] (SDL);
    /// only `side`/`cmd`/`ui_intensity`/`dbgain` are set, the tone/lfo/sweep fields stay zero.
    /// [`HapticSide`] is the `0/1/2` Left/Right/Both convention (the **reverse** of `0x8f`'s pads).
    pub fn haptic_cmd(
        &mut self,
        side: HapticSide,
        cmd: HapticType,
        intensity: HapticIntensity,
        gain: i8,
    ) -> Result<()> {
        let msg = protocol::MsgTriggerHaptic {
            side: side as u8,
            cmd: cmd as u8,
            ui_intensity: intensity as u8,
            dbgain: gain,
            ..Default::default()
        };
        self.feature(MsgId::TriggerHapticCmd, msg.as_bytes())
    }

    /// Fire a firmware-synthesized **tone** via `0xEA` (`cmd = Tone`): a clean `freq`-Hz tone held for
    /// `dur_ms`, scaled by `gain` (dB). The Deck's audible-beep path - unlike the `0x8f` pulse (a
    /// hand-timed square wave that collapses toward the LRA resonance so only ~5-6 pitches come
    /// through), the firmware tracks pitch cleanly from ~200 Hz up to a ~2 kHz ceiling (higher is
    /// silent). `ui_intensity` is HW-inert for a tone, so `gain` is the only amplitude lever (`0` is
    /// very quiet; ~8 dB is a usable level).
    ///
    /// `lfo_freq`/`lfo_depth` add a low-frequency-oscillator modulation on top of the carrier
    /// (`lfo_depth = 0` = a plain unmodulated tone; nonzero = a tremolo/textured tone). These are the
    /// same fields Triton exposes as its own [`Self::lfo_tone_triton`] (`0x83`) report. **HW-tested:**
    /// the LFO does make an audible difference on a Tone, but subtly and not yet as a predictable lever
    /// (how to drive it consistently is unclear); `lfo_freq` is a `u16` whose character keeps shifting
    /// above ~64.
    ///
    /// **Deck-only** (`0xEA` no-ops on Gordon - use [`Self::haptic_pulse`] there). Same
    /// [`HapticSide`] `0/1/2` convention as [`Self::haptic_cmd`]; see also [`Self::haptic_logsweep`].
    pub fn haptic_tone(
        &mut self,
        side: HapticSide,
        freq: u16,
        dur_ms: i16,
        gain: i8,
        lfo_freq: u16,
        lfo_depth: u8,
    ) -> Result<()> {
        let msg = protocol::MsgTriggerHaptic {
            side: side as u8,
            cmd: HapticType::Tone as u8,
            dbgain: gain,
            freq,
            dur_ms,
            lfo_freq,
            lfo_depth,
            ..Default::default()
        };
        self.feature(MsgId::TriggerHapticCmd, msg.as_bytes())
    }

    /// Fire a firmware **log-frequency sweep** (chirp) via `0xEA` (`cmd = LogSweep`): glide from
    /// `start` to `end` Hz over `dur_ms`, scaled by `gain` (dB). A rising/falling glide reads as a
    /// distinct "enter"/"exit" cue and is the nicest-feeling `0xEA` feedback (HW-verified on the Deck).
    ///
    /// **Deck-only** (no-ops on Gordon). Same [`HapticSide`] convention as [`Self::haptic_cmd`]; the
    /// single-pitch counterpart is [`Self::haptic_tone`].
    pub fn haptic_logsweep(
        &mut self,
        side: HapticSide,
        start: u16,
        end: u16,
        dur_ms: i16,
        gain: i8,
    ) -> Result<()> {
        let msg = protocol::MsgTriggerHaptic {
            side: side as u8,
            cmd: HapticType::LogSweep as u8,
            dbgain: gain,
            dur_ms,
            lss_start_freq: start,
            lss_end_freq: end,
            ..Default::default()
        };
        self.feature(MsgId::TriggerHapticCmd, msg.as_bytes())
    }

    /// Drive the Deck's dual haptic motors - **rumble** (`0xeb` `TRIGGER_RUMBLE_CMD`), kernel
    /// 9-byte form.
    ///
    /// The Deck's native rumble, what the Linux `hid-steam` driver wires `FF_RUMBLE` to. Each
    /// command plays a **fixed short burst** (~0.5 s, HW-measured - the packet has no length field),
    /// so a sustained rumble must be **re-issued** periodically; `(0, 0)` stops it. Character is
    /// **pulsating**: `left`/`right` set the **pulse rate** (higher = faster; *not* a rumble
    /// frequency), while amplitude has two levers - `left_gain`/`right_gain` (dB, coarse) and
    /// **`intensity`** (a finer amplitude control gain lacks, but **inverted**: `0` = strongest,
    /// larger = weaker, ~unfelt near `u16::MAX`; usable ~`0..16k`). We currently pass `intensity = 0`
    /// (strongest) everywhere - plumbed but not yet used as a mapping lever.
    ///
    /// **`intensity` is a `u16` (LE), kernel- and SDL-confirmed** (`report[3]` = LSB, `report[4]` =
    /// MSB - kernel `steam_haptic_rumble`; SDL `MsgSimpleRumbleCmd.unIntensity`). A HW sweep of the
    /// low byte *alone* feels like it does nothing, but that's only because it's the least-significant
    /// byte (0..255 of a 0..65535 range) - it's fine resolution, not a dead field (the low bytes of
    /// `left`/`right` behave the same). InputPlumber's "single-byte intensity + event_type" split is
    /// wrong. The leading `report[2]` (kernel 0 / SDL `unRumbleType`) is a rumble-type selector we
    /// leave at 0.
    ///
    /// **Deck-only:** Gordon has no motors, so `0xeb` no-ops there - use [`Self::haptic_pulse`] for
    /// Gordon. (`left` = strong/large motor, `right` = weak/small, matching the kernel's
    /// `rumble_left`/`rumble_right` <- FF strong/weak.)
    pub fn rumble_cmd(
        &mut self,
        intensity: u16,
        left: u16,
        right: u16,
        left_gain: i8,
        right_gain: i8,
    ) -> Result<()> {
        let msg = protocol::MsgSimpleRumbleCmd {
            rumble_type: 0,
            intensity,
            left_speed: left,
            right_speed: right,
            left_gain,
            right_gain,
        };
        self.feature(MsgId::TriggerRumbleCmd, msg.as_bytes())
    }

    /// Drive Triton's dual-motor **continuous rumble** - output report `0x80` (`HapticRumble`,
    /// 10 bytes). Unlike the Deck's `0xeb`, this rides an **output** report on the interrupt-OUT
    /// endpoint. `left`/`right` are the per-motor drive (SDL feeds the 16-bit rumble magnitudes here
    /// as the field it calls `speed`); `*_gain` are per-motor dB trims; `intensity` is a finer
    /// amplitude lever (SDL passes 0). The firmware safety-times out in ~50 ms, so a sustained rumble
    /// must be **re-issued** (the reader does, ~40 ms); `(0, 0)` stops it.
    ///
    /// **HW-verified on the puck (PLAN 1.9), same levers as the Deck's `0xeb`:** `left`/`right` are
    /// the per-motor **rate** (SDL's "speed"; higher = stronger, the coarse amplitude), `*_gain` (dB)
    /// the real strength trim, and `intensity` a finer **inverted** amplitude lever (`0` = no change,
    /// larger = weaker). Param order mirrors [`Self::rumble_cmd`] (per-motor `left`/`right`, no side
    /// byte). **Triton-only.**
    pub fn rumble_triton(
        &mut self,
        intensity: u16,
        left: u16,
        right: u16,
        left_gain: i8,
        right_gain: i8,
    ) -> Result<()> {
        // rumble_type (SDL MsgHapticRumble.type) is HW-confirmed inert (swept 0..255, no effect);
        // every reference sends 0, so we do too.
        let msg = protocol::MsgHapticRumble {
            rumble_type: 0,
            intensity,
            left_speed: left,
            left_gain,
            right_speed: right,
            right_gain,
        };
        self.output(TritonOutReport::Rumble, msg.as_bytes())
    }

    /// Fire a Triton trackpad **pulse** - output report `0x81` (`HapticPulse`): `on_us` high then
    /// `off_us` low, `repeat_count` times (freq ~= `1e6/(on+off)`). **Triton-only; HW-usable both
    /// ways.** As a **train** (`repeat_count > 1`) it's a pulse/rumble usable across the whole swept
    /// range - a narrow band around ~600-700 Hz oscillates oddly (harmless, just odd), the rest behaves
    /// cleanly; it is NOT a clean *tone* path (`0x83` LfoTone / `0x84` LogSweep are - see `beep-triton`)
    /// but works well as a rumble. As a **single** pulse (`repeat_count = 1`) it's a discrete **click**
    /// whose width (`on_us`) sets strength - Gordon-style, finer than the two-step `0x82` click.
    /// **LEFT/RIGHT are physically swapped** like Gordon's `0x8f` ([`HapticSide::Left`] drives the right
    /// pad); BOTH works. `off_us` is trailing-only at `repeat_count = 1`. Probed in `haptic-triton`
    /// (`pulse` train / `clicks-pulse` single).
    pub fn pulse_triton(
        &mut self,
        side: HapticSide,
        on_us: u16,
        off_us: u16,
        repeat_count: u16
    ) -> Result<()> {
        let msg = protocol::MsgHapticPulse { side: side as u8, on_us, off_us, repeat_count };
        self.output(TritonOutReport::Pulse, msg.as_bytes())
    }

    /// Fire a Triton **haptic command / click** - output report `0x82` (`HapticCommand`, 4 bytes):
    /// `[side, style, amplitude]`. `style` is a [`HapticStyle`] (`0` off / `1` weak / `2` strong -
    /// HW: `Weak` is a light click, `Strong` a firm one; this is the **main strength lever**).
    /// `amplitude` is an **unsigned** trim, `0x00` = medium ... `0xFF` = strong (sc-controller's
    /// observed layout - SDL's struct misleadingly types this byte as a signed `gain_db`, but its own
    /// driver never sends `0x82`; HW confirms the audible effect of this byte is subtle).
    /// [`HapticSide`] is the `0/1/2` Left/Right/Both convention. **Triton-only.**
    pub fn haptic_command_triton(
        &mut self,
        side: HapticSide,
        style: HapticStyle,
        amplitude: u8,
    ) -> Result<()> {
        // `command` = the haptic style (off/weak/strong); `gain_db` carries our unsigned amplitude
        // trim (SDL types the byte i8, but HW treats it as 0=medium..255=strong - see the struct doc).
        let msg = protocol::MsgHapticCommand {
            side: side as u8,
            command: style as u8,
            gain_db: amplitude as i8,
        };
        self.output(TritonOutReport::Command, msg.as_bytes())
    }

    /// Fire a Triton **LFO tone** - output report `0x83` (`HapticLfoTone`): a firmware-synthesized
    /// tone at `freq` Hz for `dur_ms`, scaled by `gain` (dB), with an optional low-frequency-oscillator
    /// modulation (`lfo_freq` Hz = rate, `lfo_depth` = amount; `lfo_depth = 0` = a plain unmodulated
    /// tone). Triton's counterpart to the Deck's `0xEA` `cmd = Tone` [`Self::haptic_tone`] - the
    /// promising Triton audio path, and the one primitive that carries the LFO as a first-class report.
    /// **Triton-only; HW-tested:** the tone works (pitch tracks up to a ~1.9 kHz ceiling; 2 kHz+ is
    /// silent or repeats lower pitches), and the LFO makes an audible difference - subtly, not yet a
    /// predictable lever (as on the Deck's [`Self::haptic_tone`]).
    pub fn lfo_tone_triton(
        &mut self,
        side: HapticSide,
        freq: u16,
        dur_ms: u16,
        gain: i8,
        lfo_freq: u16,
        lfo_depth: u8,
    ) -> Result<()> {
        let msg = protocol::MsgHapticLfoTone {
            side: side as u8,
            gain_db: gain,
            frequency: freq,
            duration_ms: dur_ms,
            lfo_freq,
            lfo_depth,
        };
        self.output(TritonOutReport::LfoTone, msg.as_bytes())
    }

    /// Fire a Triton **log-frequency sweep** (chirp) - output report `0x84` (`HapticLogSweep`): glide
    /// `start`->`end` Hz over `dur_ms`, scaled by `gain` (dB). Triton's counterpart to the Deck's `0xEA`
    /// `cmd = LogSweep` [`Self::haptic_logsweep`]. **Triton-only; HW-tested** - glides cleanly.
    pub fn logsweep_triton(
        &mut self,
        side: HapticSide,
        start: u16,
        end: u16,
        dur_ms: u16,
        gain: i8,
    ) -> Result<()> {
        let msg = protocol::MsgHapticLogSweep {
            side: side as u8,
            gain_db: gain,
            duration_ms: dur_ms,
            start_freq: start,
            end_freq: end,
        };
        self.output(TritonOutReport::LogSweep, msg.as_bytes())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Best-effort: restore lizard mode so the controller isn't left dead
        // after we exit (PLAN 1.6). No keep-alive thread to join - the owning
        // consumer (the engine reader) drives the Deck keep-alive from its read
        // loop, so it stops the instant this Device is dropped.
        let _ = self.set_lizard_mode(true);
    }
}

/// Frame a logical command into a feature-report buffer of `buf_len` bytes: the `report_id` byte,
/// then the command zero-padded to fill the buffer (PLAN 1.4). Gordon/Neptune use `report_id`
/// `0x00` and `buf_len = 1 + 64` (64 data bytes); Triton uses `0x01` and `buf_len = 64` (the whole
/// report is 64 bytes - report id + 63 payload - or the device stalls the transfer).
fn frame(cmd: &[u8], report_id: u8, buf_len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; buf_len];
    buf[0] = report_id;
    let n = cmd.len().min(buf_len - 1);
    buf[1..1 + n].copy_from_slice(&cmd[..n]);
    buf
}

/// Frame a logical command into BLE feature segments (PLAN 1.4): split `cmd`
/// (`[id, len, payload...]`) into <=18-byte chunks, each a 20-byte Report-ID-3 report
/// `[0x03][0x80 | seg | (0x40 if last)][chunk, zero-padded]`. A single-segment
/// command's header is `0xC0`.
fn frame_ble(cmd: &[u8]) -> Vec<[u8; protocol::ble::SEGMENT_SIZE]> {
    use protocol::ble;
    let mut out = Vec::new();
    // An empty command still needs one (terminal) segment.
    let mut chunks = cmd.chunks(ble::SEGMENT_PAYLOAD).peekable();
    let mut seg_num: u8 = 0;
    loop {
        let chunk = chunks.next().unwrap_or(&[]);
        let last = chunks.peek().is_none();
        let mut buf = [0u8; ble::SEGMENT_SIZE];
        buf[0] = ble::REPORT_ID;
        buf[1] = ble::SEG_DATA_FLAG
            | (seg_num & ble::SEG_NUM_MASK)
            | if last { ble::SEG_LAST_FLAG } else { 0 };
        buf[2..2 + chunk.len()].copy_from_slice(chunk);
        out.push(buf);
        if last {
            break;
        }
        seg_num += 1;
    }
    out
}

fn clamp_timeout(timeout: Duration) -> i32 {
    timeout.as_millis().min(i32::MAX as u128) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_prepends_report_id_and_pads_to_64() {
        let f = frame(&[MsgId::SetSettingsValues as u8, 0x02, 0xAA], protocol::REPORT_ID, 1 + protocol::REPORT_LEN);
        assert_eq!(f.len(), 1 + protocol::REPORT_LEN);
        assert_eq!(f[0], protocol::REPORT_ID);
        assert_eq!(&f[1..4], &[MsgId::SetSettingsValues as u8, 0x02, 0xAA]);
        assert!(f[4..].iter().all(|&x| x == 0));
    }

    #[test]
    fn frame_truncates_overlong_command() {
        let f = frame(&[0xAB; 100], protocol::REPORT_ID, 1 + protocol::REPORT_LEN);
        assert_eq!(f.len(), 1 + protocol::REPORT_LEN);
    }
}
