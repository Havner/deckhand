//! Globals screen — a live editor over the UI-owned [`GlobalConfig`](config::GlobalConfig). Every
//! edit persists to `globals.ron` and ships to the daemon (`App::apply_globals`); this always renders
//! `app.globals` (the source of truth), never the daemon's status snapshot. Chords aren't editable
//! yet (that lands with the profile editor) — only their count is shown.

use config::StartProfile;
use iced::widget::{checkbox, column, pick_list, row, slider, text};
use iced::{Center, Element};

use super::{section_header, small};
use crate::{App, IDLE_TIMEOUT_MINUTES, Message, style};

/// The global-config page (Category::Globals).
pub(super) fn globals_screen(app: &App) -> Element<'_, Message> {
    let g = &app.globals;

    // Start profile: Main / Fallback. The label closure supplies the display strings, so the enum
    // needs no `Display` impl.
    let start = row![
        glabel("Start profile:"),
        pick_list(
            Some(g.start_profile.clone()),
            vec![StartProfile::Main, StartProfile::Fallback],
            |p: &StartProfile| match p {
                StartProfile::Main => "Main",
                StartProfile::Fallback => "Fallback",
            }
            .to_string(),
        )
        .on_select(Message::GlobalsStartProfile)
        .width(160.0),
    ]
    .spacing(12.0)
    .align_y(Center);

    // Master rumble: a 0–100% slider with a live readout.
    let master = row![
        glabel("Master rumble:"),
        slider(0..=100u8, g.master_rumble, Message::GlobalsMasterRumble),
        pct_text(Some(g.master_rumble)),
    ]
    .spacing(12.0)
    .align_y(Center);

    // LED brightness: an `Option` — the checkbox gates a 0–100% slider. When off it's the same
    // slider widget (identical geometry) but styled inert and non-interactive, so `None` reads as
    // "leave default" without the jarring size change a different widget would cause.
    let led_on = g.led_brightness.is_some();
    let led_val = g.led_brightness.unwrap_or(crate::DEFAULT_LED_BRIGHTNESS);
    let led_bar: Element<'_, Message> = if led_on {
        slider(0..=100u8, led_val, Message::GlobalsLedBrightness).into()
    } else {
        slider(0..=100u8, led_val, |_| Message::Ignored).style(style::disabled_slider).into()
    };
    let led = row![
        glabel("LED brightness:"),
        checkbox(led_on).on_toggle(Message::GlobalsLedEnabled),
        led_bar,
        pct_text(led_on.then_some(led_val)),
    ]
    .spacing(12.0)
    .align_y(Center);

    // Idle timeout: an `Option` — the checkbox gates a minutes combobox (values stored as seconds).
    let idle_on = g.idle_timeout.is_some();
    let idle_min = g.idle_timeout.map(|s| s / 60).unwrap_or(crate::DEFAULT_IDLE_TIMEOUT / 60);
    let mut idle_combo = pick_list(
        idle_on.then_some(idle_min),
        IDLE_TIMEOUT_MINUTES.to_vec(),
        |m: &u16| format!("{m} minutes"),
    )
    .placeholder("default")
    .width(160.0);
    if idle_on {
        idle_combo = idle_combo.on_select(|m| Message::GlobalsIdleTimeout(m * 60));
    }
    let idle = row![
        glabel("Idle timeout:"),
        checkbox(idle_on).on_toggle(Message::GlobalsIdleEnabled),
        idle_combo,
    ]
    .spacing(12.0)
    .align_y(Center);

    // Chords: count only — editing is deferred to the profile editor (the count still round-trips
    // through the file/daemon untouched).
    let chords = row![glabel("Chords:"), text(format!("{}", g.chords.len()))]
        .spacing(12.0)
        .align_y(Center);

    let note =
        small("'Start profile', 'LED brightness' and 'Idle timeout' take effect only on engine start.");

    column![section_header("Global daemon settings"), note, start, master, led, idle, chords]
        .spacing(16.0)
        .into()
}

/// A fixed-width row label for the Globals screen, so the controls line up in a column.
fn glabel(s: &'static str) -> Element<'static, Message> {
    text(s).width(140.0).into()
}

/// A fixed-width trailing percentage readout (`None` → "default"), keeping the sliders aligned.
fn pct_text(v: Option<u8>) -> Element<'static, Message> {
    text(v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into())).width(60.0).into()
}
