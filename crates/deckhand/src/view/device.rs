//! Device screen - a live editor over the UI-owned [`DeviceConfig`](config::DeviceConfig). Every
//! edit persists to `devcfg.ron` and ships to the daemon (`App::apply_device_config`); this always
//! renders `app.device_config` (the source of truth), never the daemon's status snapshot.

use iced::widget::{checkbox, column, pick_list, row, slider, text};
use iced::{Center, Element};

use config::Lever;

use super::{group_header, section_header, setting_label, small};
use crate::{
    App, AUDIO_GAIN_MAX_DB, AUDIO_GAIN_MIN_DB, GAIN_MAX_DB, GAIN_MIN_DB, IDLE_TIMEOUT_MINUTES, Message,
    RumbleLeverEdit, RumbleLeverId, style,
};

/// The device-config page (Category::Device).
pub(super) fn device_screen(app: &App) -> Element<'_, Message> {
    let d = &app.device_config;

    // LED brightness: an `Option` - the checkbox gates a 0-100% slider. When off it's the same
    // slider widget (identical geometry) but styled inert and non-interactive, so `None` reads as
    // "leave default" without the jarring size change a different widget would cause.
    let led_on = d.led_brightness.is_some();
    let led_val = d.led_brightness.unwrap_or(crate::DEFAULT_LED_BRIGHTNESS);
    let led_bar: Element<'_, Message> = if led_on {
        slider(0..=100u8, led_val, Message::DeviceLedBrightness).into()
    } else {
        slider(0..=100u8, led_val, |_| Message::Ignored).style(style::disabled_slider).into()
    };
    let led = row![
        setting_label("LED brightness"),
        checkbox(led_on).on_toggle(Message::DeviceLedEnabled),
        led_bar,
        pct_text(led_on.then_some(led_val)),
    ]
    .spacing(12.0)
    .align_y(Center);

    // Idle timeout: an `Option` - the checkbox gates a minutes combobox (values stored as seconds).
    let idle_on = d.idle_timeout.is_some();
    let idle_min = d.idle_timeout.map(|s| s / 60).unwrap_or(crate::DEFAULT_IDLE_TIMEOUT / 60);
    let mut idle_combo = pick_list(
        idle_on.then_some(idle_min),
        IDLE_TIMEOUT_MINUTES.to_vec(),
        |m: &u16| format!("{m} minutes"),
    )
    .placeholder("default")
    .menu_style(style::combo_menu)
    .width(160.0);
    if idle_on {
        idle_combo = idle_combo.on_select(|m| Message::DeviceIdleTimeout(m * 60));
    }
    let idle = row![
        setting_label("Idle timeout"),
        checkbox(idle_on).on_toggle(Message::DeviceIdleEnabled),
        idle_combo,
    ]
    .spacing(12.0)
    .align_y(Center);

    // Rumble + audio: three device-specific sections. Gordon has trackpad actuators driven as a
    // pulse-train (a duty lever + a tunable frequency); Neptune and Triton have real motors whose
    // speed + gain each map from the game's rumble strength via a lever (fixed, or scaled into a
    // band). Each group ends with a fixed feedback-audio volume knob (Gordon duty / motor gain).
    column![
        section_header("Device config"),
        led,
        idle,
        group_header("Gordon"),
        speed_lever_row("Rumble duty", &d.gordon.rumble_duty, RumbleLeverId::GordonDuty),
        rumble_frequency_row(d.gordon.rumble_freq),
        audio_duty_row(d.gordon.audio_duty),
        group_header("Neptune"),
        speed_lever_row("Rumble speed", &d.neptune.rumble_speed, RumbleLeverId::NeptuneSpeed),
        gain_lever_row("Rumble gain", &d.neptune.rumble_gain, RumbleLeverId::NeptuneGain),
        audio_gain_row(d.neptune.audio_gain, Message::DeviceNeptuneAudioGain),
        group_header("Triton"),
        speed_lever_row("Rumble speed", &d.triton.rumble_speed, RumbleLeverId::TritonSpeed),
        gain_lever_row("Rumble gain", &d.triton.rumble_gain, RumbleLeverId::TritonGain),
        audio_gain_row(d.triton.audio_gain, Message::DeviceTritonAudioGain),
    ]
    .spacing(16.0)
    .into()
}

/// The Gordon rumble-frequency row (30-150 Hz pulse-train rate).
fn rumble_frequency_row(freq: u16) -> Element<'static, Message> {
    row![
        setting_label("Rumble frequency"),
        slider(30..=150u16, freq, Message::DeviceRumbleHz).step(1u16),
        text(format!("{freq} Hz")).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center)
    .into()
}

/// Gordon's feedback-audio **volume** row: a plain 0..=100 % duty slider (not strength-scaled).
fn audio_duty_row(value: u8) -> Element<'static, Message> {
    row![
        setting_label("Audio duty"),
        slider(0..=100u8, value, Message::DeviceGordonAudioDuty),
        pct_text(Some(value)),
    ]
    .spacing(12.0)
    .align_y(Center)
    .into()
}

/// A motor device's feedback-audio **gain** row (dB): a plain slider over the audio-gain range
/// (distinct from the rumble gain range), routed to the given per-device message.
fn audio_gain_row(value: i8, msg: fn(i16) -> Message) -> Element<'static, Message> {
    let range = AUDIO_GAIN_MIN_DB as i16..=AUDIO_GAIN_MAX_DB as i16;
    row![
        setting_label("Audio gain"),
        slider(range, value as i16, msg),
        db_text(value),
    ]
    .spacing(12.0)
    .align_y(Center)
    .into()
}

/// A **speed/rate** lever row (percent). The checkbox toggles fixed<->scaled: fixed shows one slider,
/// scaled shows a min + max pair. `id` routes every edit to the right lever ([`Message::DeviceRumbleLever`]).
fn speed_lever_row<'a>(label: &'static str, lever: &Lever<u8>, id: RumbleLeverId) -> Element<'a, Message> {
    let controls: Element<'a, Message> = match *lever {
        Lever::Fixed(v) => row![
            slider(0..=100u8, v, move |x| edit(id, RumbleLeverEdit::Fixed(x as i16))),
            pct_text(Some(v)),
        ]
        .spacing(12.0)
        .align_y(Center)
        .into(),
        Lever::Scaled { min, max } => row![
            small("min"),
            slider(0..=100u8, min, move |x| edit(id, RumbleLeverEdit::Min(x as i16))),
            pct_text(Some(min)),
            small("max"),
            slider(0..=100u8, max, move |x| edit(id, RumbleLeverEdit::Max(x as i16))),
            pct_text(Some(max)),
        ]
        .spacing(8.0)
        .align_y(Center)
        .into(),
    };
    lever_row(label, matches!(lever, Lever::Scaled { .. }), id, controls)
}

/// A **gain** lever row (dB). Same shape as [`speed_lever_row`] over the dB range.
fn gain_lever_row<'a>(label: &'static str, lever: &Lever<i8>, id: RumbleLeverId) -> Element<'a, Message> {
    // `slider`'s wrapper requires `From<u8>`, which `i8` lacks - drive the dB sliders as `i16` (the
    // message already carries `i16`); the readouts use the underlying `i8`.
    let range = GAIN_MIN_DB as i16..=GAIN_MAX_DB as i16;
    let controls: Element<'a, Message> = match *lever {
        Lever::Fixed(v) => row![
            slider(range.clone(), v as i16, move |x| edit(id, RumbleLeverEdit::Fixed(x))),
            db_text(v),
        ]
        .spacing(12.0)
        .align_y(Center)
        .into(),
        Lever::Scaled { min, max } => row![
            small("min"),
            slider(range.clone(), min as i16, move |x| edit(id, RumbleLeverEdit::Min(x))),
            db_text(min),
            small("max"),
            slider(range, max as i16, move |x| edit(id, RumbleLeverEdit::Max(x))),
            db_text(max),
        ]
        .spacing(8.0)
        .align_y(Center)
        .into(),
    };
    lever_row(label, matches!(lever, Lever::Scaled { .. }), id, controls)
}

/// The shared frame for a lever row: label + "Scale with strength" checkbox + the variant's sliders.
fn lever_row<'a>(
    label: &'static str,
    scaled: bool,
    id: RumbleLeverId,
    controls: Element<'a, Message>,
) -> Element<'a, Message> {
    row![
        setting_label(label),
        checkbox(scaled).on_toggle(move |b| edit(id, RumbleLeverEdit::Scaled(b))),
        small("scale"),
        controls,
    ]
    .spacing(12.0)
    .align_y(Center)
    .into()
}

/// Shorthand for a lever-edit message.
fn edit(id: RumbleLeverId, e: RumbleLeverEdit) -> Message {
    Message::DeviceRumbleLever(id, e)
}

/// A fixed-width trailing percentage readout (`None` -> "default"), keeping the sliders aligned.
fn pct_text(v: Option<u8>) -> Element<'static, Message> {
    text(v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into())).width(60.0).into()
}

/// A fixed-width trailing dB readout (signed), keeping the gain sliders aligned.
fn db_text(v: i8) -> Element<'static, Message> {
    text(format!("{v:+} dB")).width(60.0).into()
}
