//! Device screen — a live editor over the UI-owned [`DeviceConfig`](config::DeviceConfig). Every
//! edit persists to `device_config.ron` and ships to the daemon (`App::apply_device_config`); this always renders
//! `app.device_config` (the source of truth), never the daemon's status snapshot. Chords aren't editable
//! yet (that lands with the profile editor) — only their count is shown.

use config::{ChordAction, Chord, SwitchMode};
use iced::widget::{Space, button, checkbox, column, pick_list, row, slider, text, text_input};
use iced::{Center, Element, Fill};

use super::{button_chips, group_header, section_header, setting_label, small};
use crate::{App, ButtonTarget, ChordActionKind, IDLE_TIMEOUT_MINUTES, Message, style};

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

    // Rumble frequency: the Gordon pulse-train rate (same format as the old profile Rumble page).
    let frequency = row![
        setting_label("Frequency"),
        slider(30..=150u16, d.rumble_hz, Message::DeviceRumbleHz).step(1u16),
        text(format!("{} Hz", d.rumble_hz)).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center);

    let note = small(
        "'LED brightness', 'Idle timeout', 'Master rumble' and 'Frequency' take effect only on \
         engine start.",
    );

    column![
        section_header("Device config"),
        note,
        led,
        idle,
        master,
        frequency,
        chords_section(d),
    ]
    .spacing(16.0)
    .into()
}

/// The chords editor: one bar per chord (trigger chips + action) and an "Add chord" button. Chords
/// are AND-combined buttons firing a [`ChordAction`] (profile switch / run command).
fn chords_section(d: &config::DeviceConfig) -> Element<'_, Message> {
    let mut col = column![group_header("Chords")].spacing(8.0);
    for (i, chord) in d.chords.iter().enumerate() {
        col = col.push(chord_bar(i, chord));
    }
    col = col.push(
        button(text("Add chord")).style(button::secondary).on_press(Message::ChordAdd),
    );
    col.into()
}

/// One chord bar: the trigger buttons (chips + picker), the action-kind combobox and its detail
/// (switch mode / command line), and a ✕ to remove the whole chord.
fn chord_bar(i: usize, chord: &Chord) -> Element<'static, Message> {
    let trigger = button_chips(
        &chord.buttons,
        move |j| Message::ChordRemoveButton(i, j),
        Message::OpenButtonPicker(ButtonTarget::Chord(i)),
    );

    let kind = match chord.action {
        ChordAction::SwitchProfile { .. } => ChordActionKind::SwitchProfile,
        ChordAction::CommandExecute { .. } => ChordActionKind::CommandExecute,
    };
    let kind_combo = pick_list(
        Some(kind),
        vec![ChordActionKind::SwitchProfile, ChordActionKind::CommandExecute],
        |k: &ChordActionKind| chord_kind_label(k).to_string(),
    )
    .on_select(move |k| Message::ChordSetKind(i, k))
    .menu_style(style::combo_menu)
    .width(150.0);

    let detail: Element<'static, Message> = match &chord.action {
        ChordAction::SwitchProfile { mode } => pick_list(
            Some(mode.clone()),
            vec![SwitchMode::HoldFallback, SwitchMode::Toggle, SwitchMode::SetMain, SwitchMode::SetFallback],
            |m: &SwitchMode| switch_mode_label(m).to_string(),
        )
        .on_select(move |m| Message::ChordSetMode(i, m))
        .menu_style(style::combo_menu)
        .width(150.0)
        .into(),
        ChordAction::CommandExecute { command, args } => text_input("command args…", command_line(command, args))
            .on_input(move |s| Message::ChordSetCommandLine(i, s))
            .width(240.0)
            .into(),
    };

    let remove = button(text("✕").size(15.0)).style(style::combo_button).on_press(Message::ChordRemove(i));

    super::card(
        row![trigger, Space::new().width(Fill), kind_combo, detail, remove]
            .spacing(12.0)
            .align_y(Center),
    )
}

/// The command line shown/edited for a `CommandExecute` chord: command + args joined by a single
/// space. Paired with the split-on-`' '` parse in `App::update` so the field's exact text round-trips.
fn command_line(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string()).chain(args.iter().cloned()).collect::<Vec<_>>().join(" ")
}

fn chord_kind_label(k: &ChordActionKind) -> &'static str {
    match k {
        ChordActionKind::SwitchProfile => "Switch profile",
        ChordActionKind::CommandExecute => "Run command",
    }
}

fn switch_mode_label(m: &SwitchMode) -> &'static str {
    match m {
        SwitchMode::HoldFallback => "Hold fallback",
        SwitchMode::Toggle => "Toggle",
        SwitchMode::SetMain => "Set main",
        SwitchMode::SetFallback => "Set fallback",
    }
}

/// A fixed-width trailing percentage readout (`None` → "default"), keeping the sliders aligned.
fn pct_text(v: Option<u8>) -> Element<'static, Message> {
    text(v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into())).width(60.0).into()
}
