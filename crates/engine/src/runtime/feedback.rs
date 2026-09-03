//! Command feedback (PLAN 7): turn a [`FeedbackReq`] (`side` + resolved [`Effect`]) into a short,
//! self-advancing sequence of notes played on the trackpad haptic actuator - a click, an audio tone,
//! or a sweep. Owned by the reader thread (the single `Device` writer); rumble stays in [`reader`].
//!
//! Two stages, per PLAN 7.4:
//! - [`expand`] turns an `Effect` into notes + inter-note intervals. **Device-independent** (only
//!   duration/gap consts), so it needs no [`DeviceKind`]; the interval before a note is the
//!   *previous* note's play duration plus a gap (audio), or just the gap (a click is an impulse).
//! - [`fire_note`] realizes one note on the bound device - the only device-aware step (pitch ->
//!   frequency, length -> duration, the packet per kind). Clicks play on the triggering `side`; audio
//!   plays on both actuators (heard, not felt); chirps are a no-op on Gordon (no sweep path).
//!
//! Playback (PLAN 7.4): [`Sequencer::start`] fires note 0 immediately (fire-on-arrival) and keeps any
//! tail; the reader calls [`Sequencer::advance`] each lap to fire notes as they come due. A new
//! request replaces the active sequencer (latest-wins).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use config::{Click, Effect, Side, Sweep, Tone};
use mapper::FeedbackReq;
use steam_hid::{
    Device, HapticIntensity, HapticPosition, HapticPulse, HapticSide, HapticStyle, HapticType, Result,
};

use super::reader::DeviceTuning;

// --- timing (device-independent; shared by `expand` and `fire_note`) ----------------------------

/// Audio tone play durations (ms) for the two [`Tone`] lengths - `fire_note` plays the tone this
/// long, and `expand` spaces the next note this far past its onset. Shared so both agree (PLAN 7.4).
const SHORT_MS: u16 = 30;
const LONG_MS: u16 = 100;

/// Silence between consecutive notes of a pattern (onset-to-onset = the previous note's duration
/// plus this gap; a click's duration is ~0 so its onset-to-onset is just the gap). Placeholders to
/// tune on hardware.
const AUDIO_DOUBLE_GAP_MS: u16 = 40;
const AUDIO_TRIPLE_GAP_MS: u16 = 30;
const HAPTIC_DOUBLE_GAP_MS: u16 = 50;
const HAPTIC_TRIPLE_GAP_MS: u16 = 40;

// --- audio parameters (starting points; will be fine-tuned on hardware) --------------------------

/// Tone pitch -> carrier frequency (Hz). One table for every device for now (all under the ~1.9 kHz
/// per-device ceilings); `fire_note` could diverge per kind later if a device needs it.
const TONE_LOW_HZ: u16 = 800;
const TONE_MED_HZ: u16 = 1200;
const TONE_HIGH_HZ: u16 = 1600;

/// Chirp glide duration (ms) and the three shapes' frequency ranges (`(lo, hi)`; `Up*` glides
/// `lo -> hi`, `Down*` `hi -> lo`).
const SWEEP_MS: u16 = 200;
const SWEEP1: (u16, u16) = (800, 2000);
const SWEEP2: (u16, u16) = (800, 1400);
const SWEEP3: (u16, u16) = (1400, 2000);

/// The duty cycle Gordon's audio volume (`GordonTuning::audio_duty`, 0..=100 %) maps onto: a
/// full-volume knob drives the square-wave to this fraction of the period, where the beep saturates
/// (HW: ~50 % duty). Analogous to the rumble path's `RUMBLE_MAX_DUTY`. Deck/Triton volume is the
/// per-device `audio_gain` (dB) instead.
const AUDIO_MAX_DUTY: f32 = 0.5;

/// Trailing off-phase of the single click pulse (irrelevant to the felt tick at `count = 1`).
const CLICK_INTERVAL_US: u16 = 1000;

// --- notes + sequencer ---------------------------------------------------------------------------

/// One playable note (built from an [`Effect`], realized by [`fire_note`]). Carries config presets,
/// not concrete device parameters - the resolution to freq/duration/packet is per-device.
enum Note {
    Haptic(Click),
    Audio(Tone),
    Chirp(Sweep),
}

/// A note plus the wait before it fires (onset relative to the previous note; PLAN 7.4). Note 0's
/// interval is unused - it fires on arrival.
struct SeqNote {
    interval_ms: u16,
    note: Note,
}

/// A short feedback sequence in flight: the remaining (not-yet-played) notes, the side (for clicks),
/// and when the front note is due. Fire-on-arrival + pre-note pop-front (PLAN 7.4): the front's
/// `interval_ms` is how long until it plays; playing = `pop_front`.
pub(super) struct Sequencer {
    side: Side,
    notes: VecDeque<SeqNote>,
    next_due: Instant,
}

impl Sequencer {
    /// Build from a request and **fire note 0 immediately** (its interval ignored), then keep any
    /// tail. Returns `None` for a single-note effect (already played, nothing to schedule) - so the
    /// reader clears its slot. A new call replaces the previous sequencer (latest-wins).
    pub(super) fn start(req: FeedbackReq, tuning: &DeviceTuning, device: &mut Device) -> Option<Sequencer> {
        let mut notes: VecDeque<SeqNote> = expand(&req.effect).into();
        let side = req.side;
        if let Some(head) = notes.pop_front() {
            fire(tuning, device, &head.note, &side);
        }
        let next_due = Instant::now() + Duration::from_millis(notes.front()?.interval_ms as u64);
        Some(Sequencer { side, notes, next_due })
    }

    /// Fire every note now due (usually 0 or 1 per ~4 ms reader lap), advancing the schedule
    /// drift-free. Returns `false` once the last note has played (the reader then clears its slot).
    pub(super) fn advance(&mut self, now: Instant, tuning: &DeviceTuning, device: &mut Device) -> bool {
        while now >= self.next_due {
            let Some(due) = self.notes.pop_front() else { return false };
            fire(tuning, device, &due.note, &self.side);
            match self.notes.front() {
                Some(next) => self.next_due += Duration::from_millis(next.interval_ms as u64),
                None => return false,
            }
        }
        true
    }
}

/// Expand an effect into its notes + inter-note intervals (device-independent). Note 0 has interval 0
/// (fired on arrival); each later note's interval is the *previous* note's play duration plus the
/// pattern gap - audio carries the tone length, a click is treated as ~0 duration (gap only).
fn expand(effect: &Effect) -> Vec<SeqNote> {
    let haptic = |c: Click, interval_ms: u16| SeqNote { interval_ms, note: Note::Haptic(c) };
    let audio = |t: Tone, interval_ms: u16| SeqNote { interval_ms, note: Note::Audio(t) };
    match effect {
        Effect::Haptic(a) => vec![haptic(*a, 0)],
        Effect::HapticDouble(a, b) => vec![haptic(*a, 0), haptic(*b, HAPTIC_DOUBLE_GAP_MS)],
        Effect::HapticTriple(a, b, c) => {
            vec![haptic(*a, 0), haptic(*b, HAPTIC_TRIPLE_GAP_MS), haptic(*c, HAPTIC_TRIPLE_GAP_MS)]
        }
        Effect::Audio(a) => vec![audio(*a, 0)],
        Effect::AudioDouble(a, b) => {
            vec![audio(*a, 0), audio(*b, tone_dur_ms(*a) + AUDIO_DOUBLE_GAP_MS)]
        }
        Effect::AudioTriple(a, b, c) => vec![
            audio(*a, 0),
            audio(*b, tone_dur_ms(*a) + AUDIO_TRIPLE_GAP_MS),
            audio(*c, tone_dur_ms(*b) + AUDIO_TRIPLE_GAP_MS),
        ],
        Effect::Chirp(s) => vec![SeqNote { interval_ms: 0, note: Note::Chirp(*s) }],
    }
}

/// Fire one note on the bound device, logging (non-fatal) on write error - a transient hiccup must
/// not tear down the reader.
fn fire(tuning: &DeviceTuning, device: &mut Device, note: &Note, side: &Side) {
    if let Err(e) = fire_note(tuning, device, note, side) {
        log::warn!("feedback write failed: {e}");
    }
}

/// Realize one note - the only device-aware step (the [`DeviceTuning`] variant is both the device
/// discriminant and its audio volume). A click plays on the triggering `side`; audio (tone / chirp)
/// plays on both actuators (audio is heard, not felt - PLAN 7.1), `side` ignored.
fn fire_note(tuning: &DeviceTuning, device: &mut Device, note: &Note, side: &Side) -> Result<()> {
    match note {
        Note::Haptic(c) => fire_click(tuning, device, side, c),
        Note::Audio(t) => fire_tone(tuning, device, tone_freq(*t), tone_dur_ms(*t)),
        Note::Chirp(s) => {
            let (start, end) = sweep_range(*s);
            fire_chirp(tuning, device, start, end)
        }
    }
}

// --- tone / chirp presets ------------------------------------------------------------------------

/// A tone's carrier frequency (Hz) - the pitch half of the preset.
fn tone_freq(t: Tone) -> u16 {
    match t {
        Tone::ShortLow | Tone::LongLow => TONE_LOW_HZ,
        Tone::ShortMedium | Tone::LongMedium => TONE_MED_HZ,
        Tone::ShortHigh | Tone::LongHigh => TONE_HIGH_HZ,
    }
}

/// A tone's play duration (ms) - the length half of the preset. Also drives `expand`'s intervals.
fn tone_dur_ms(t: Tone) -> u16 {
    match t {
        Tone::ShortLow | Tone::ShortMedium | Tone::ShortHigh => SHORT_MS,
        Tone::LongLow | Tone::LongMedium | Tone::LongHigh => LONG_MS,
    }
}

/// A sweep's `(start, end)` frequencies - `Up*` glides low->high, `Down*` high->low.
fn sweep_range(s: Sweep) -> (u16, u16) {
    let up = |(lo, hi): (u16, u16)| (lo, hi);
    let down = |(lo, hi): (u16, u16)| (hi, lo);
    match s {
        Sweep::Up1 => up(SWEEP1),
        Sweep::Down1 => down(SWEEP1),
        Sweep::Up2 => up(SWEEP2),
        Sweep::Down2 => down(SWEEP2),
        Sweep::Up3 => up(SWEEP3),
        Sweep::Down3 => down(SWEEP3),
    }
}

// --- per-device firing ---------------------------------------------------------------------------

/// Play a `freq`-Hz tone for `dur_ms` on both actuators. Gordon has no synthesized-tone path, so it
/// uses a `0x8f` square-wave pulse (`duration == interval` = 50 % duty, `count ~ freq*ms/1000`
/// cycles) on each pad; the Deck/Triton use their firmware tone (`0xEA cmd=Tone` / `0x83 LfoTone`),
/// which tracks pitch cleanly.
fn fire_tone(tuning: &DeviceTuning, device: &mut Device, freq: u16, dur_ms: u16) -> Result<()> {
    match tuning {
        DeviceTuning::Gordon(t) => {
            // Volume = the square wave's duty, mapped across the usable audio-duty band.
            let period = (1_000_000 / freq.max(1) as u32).clamp(2, u16::MAX as u32);
            let duty_frac = (t.audio_duty as f32 / 100.0 * AUDIO_MAX_DUTY).clamp(0.0, 1.0);
            let duration = ((period as f32 * duty_frac) as u32).clamp(1, period - 1) as u16;
            let interval = period as u16 - duration;
            let count = ((freq as u32 * dur_ms as u32) / 1000).clamp(1, u16::MAX as u32) as u16;
            let pulse = HapticPulse { duration, interval, count, gain: 0 };
            // Gordon has no `pad=2`, so both actuators are fired separately (they run concurrently).
            device.haptic_pulse(HapticPosition::Left, pulse.clone())?;
            device.haptic_pulse(HapticPosition::Right, pulse)?;
            Ok(())
        }
        DeviceTuning::Neptune(t) => {
            device.haptic_tone(HapticSide::Both, freq, dur_ms as i16, t.audio_gain, 0, 0)
        }
        DeviceTuning::Triton(t) => {
            device.lfo_tone_triton(HapticSide::Both, freq, dur_ms, t.audio_gain, 0, 0)
        }
    }
}

/// Glide `start -> end` Hz over [`SWEEP_MS`] on both actuators, at the device's audio gain. **No-op on
/// Gordon** (no sweep path); the Deck/Triton use their firmware log-sweep (`0xEA cmd=LogSweep` / `0x84
/// LogSweep`).
fn fire_chirp(tuning: &DeviceTuning, device: &mut Device, start: u16, end: u16) -> Result<()> {
    match tuning {
        DeviceTuning::Gordon(_) => Ok(()),
        DeviceTuning::Neptune(t) => {
            device.haptic_logsweep(HapticSide::Both, start, end, SWEEP_MS as i16, t.audio_gain)
        }
        DeviceTuning::Triton(t) => {
            device.logsweep_triton(HapticSide::Both, start, end, SWEEP_MS, t.audio_gain)
        }
    }
}

/// Fire one command-haptic click on its pad, mapping the `strength` level to the device: Gordon uses
/// a `0x8f` pulse whose **duration** encodes strength (gain inert -> 0); the Deck uses a `0xea`
/// `haptic_cmd` (`Click`) whose **gain** encodes strength, **per side** (the two motors differ);
/// Triton uses its `0x82` command click. `Side::Left`->left actuator, `Side::Right`->right. (A click
/// is felt, not heard, so the device's audio volume doesn't apply.)
fn fire_click(tuning: &DeviceTuning, device: &mut Device, side: &Side, strength: &Click) -> Result<()> {
    match tuning {
        DeviceTuning::Gordon(_) => {
            // Gordon's `0x8f` uses the swapped `HapticPosition` (Right=0/Left=1), no "both".
            let position = match side {
                Side::Left => HapticPosition::Left,
                Side::Right => HapticPosition::Right,
            };
            let duration = gordon_click_duration(strength);
            device.haptic_pulse(position, HapticPulse { duration, interval: CLICK_INTERVAL_US, count: 1, gain: 0 })
        }
        DeviceTuning::Neptune(_) => {
            let gain = neptune_click_gain(side, strength);
            // Intensity stays System (0): 0..2 are identical on HW, and gain is the strength lever here.
            device.haptic_cmd(haptic_side(side), HapticType::Click, HapticIntensity::System, gain)
        }
        DeviceTuning::Triton(_) => {
            let (style, amp) = triton_click(strength);
            device.haptic_command_triton(haptic_side(side), style, amp)
        }
    }
}

/// The `0/1/2` [`HapticSide`] for a mapper [`Side`] (the Deck's `0xEA` / Triton's `0x82` convention;
/// a click is always one-sided, so `Both` never arises here).
fn haptic_side(side: &Side) -> HapticSide {
    match side {
        Side::Left => HapticSide::Left,
        Side::Right => HapticSide::Right,
    }
}

/// Gordon `0x8f` click pulse duration (us) for a strength level - the duration is the strength lever
/// (gain is inert on Gordon). HW-tuned via the `haptic` example.
fn gordon_click_duration(strength: &Click) -> u16 {
    match strength {
        Click::Weak => 500,
        Click::Medium => 1000,
        Click::Strong => 2000,
    }
}

/// Deck `0xea` click gain (dB) for a strength level, **per side** (HW-tuned - the two motors differ,
/// the right needing a couple dB more for a comparable feel).
fn neptune_click_gain(side: &Side, strength: &Click) -> i8 {
    match (side, strength) {
        (Side::Left, Click::Weak) => -2,
        (Side::Left, Click::Medium) => 1,
        (Side::Left, Click::Strong) => 4,
        (Side::Right, Click::Weak) => -2,
        (Side::Right, Click::Medium) => 2,
        (Side::Right, Click::Strong) => 6,
    }
}

/// Triton command-click (output report `0x82`) per strength level: a [`HapticStyle`] effect + an
/// amplitude byte. **HW-confirmed:** the amplitude byte is **inert** and even `Weak` is a fairly firm
/// click, so `style` is the only working lever - in practice just **two** distinct strengths. Weak/Med
/// both use `Weak` (Med carries a max amplitude only so there's a gradient if firmware ever activates
/// the byte); Strong = `Strong`.
fn triton_click(strength: &Click) -> (HapticStyle, u8) {
    match strength {
        Click::Weak => (HapticStyle::Weak, 0),
        Click::Medium => (HapticStyle::Weak, 255),
        Click::Strong => (HapticStyle::Strong, 0),
    }
}
