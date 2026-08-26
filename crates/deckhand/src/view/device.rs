//! Device screen — a live editor over the UI-owned [`DeviceConfig`](config::DeviceConfig). Every
//! edit persists to `devcfg.ron` and ships to the daemon (`App::apply_device_config`); this always
//! renders `app.device_config` (the source of truth), never the daemon's status snapshot.

use iced::widget::{checkbox, column, pick_list, row, slider, text};
use iced::{Center, Element};

use super::{section_header, setting_label};
use crate::{App, IDLE_TIMEOUT_MINUTES, Message, style};

/// The device-config page (Category::Device).
pub(super) fn device_screen(app: &App) -> Element<'_, Message> {
    let d = &app.device_config;

    // Master rumble: a 0–100% slider with a live readout.
    let master = row![
        setting_label("Master rumble"),
        slider(0..=100u8, d.master_rumble, Message::DeviceMasterRumble),
        pct_text(Some(d.master_rumble)),
    ]
    .spacing(12.0)
    .align_y(Center);

    // LED brightness: an `Option` — the checkbox gates a 0–100% slider. When off it's the same
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

    // Idle timeout: an `Option` — the checkbox gates a minutes combobox (values stored as seconds).
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

    // Rumble frequency: the Gordon pulse-train rate.
    let frequency = row![
        setting_label("Frequency"),
        slider(30..=150u16, d.rumble_hz, Message::DeviceRumbleHz).step(1u16),
        text(format!("{} Hz", d.rumble_hz)).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center);

    column![section_header("Device config"), led, idle, master, frequency]
        .spacing(16.0)
        .into()
}

/// A fixed-width trailing percentage readout (`None` → "default"), keeping the sliders aligned.
fn pct_text(v: Option<u8>) -> Element<'static, Message> {
    text(v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into())).width(60.0).into()
}
