//! The single-screen widget layer: a controls row, an on-screen numeric keypad for the output
//! address, a master-rumble slider, and a bottom status bar.
//!
//! Everything is one window (no tray, no sidebar, no pages). The bottom bar is a trimmed version of
//! the main UI's - just daemon status, state, device, and error (no profiles/chords).

use iced::widget::{
    Space, button, checkbox, column, container, pick_list, row, scrollable, slider, text,
    text_input,
};
use iced::{Center, Element, Fill, Theme};

use config::{Lever, Shape};
use ipc::RunState;

use crate::{
    App, AUDIO_GAIN_MAX_DB, AUDIO_GAIN_MIN_DB, GAIN_MAX_DB, GAIN_MIN_DB, INPUT_PRESETS, Message,
    RumbleLeverEdit, RumbleLeverId, style,
};

/// The whole window: content pane on top, status bar at the bottom.
pub(crate) fn view(app: &App) -> Element<'_, Message> {
    // The content scrolls if the 2x UI scale leaves it taller than the window's logical height.
    column![
        scrollable(container(content(app)).padding(16.0).width(Fill))
            .width(Fill)
            .height(Fill),
        bottom_bar(app),
    ]
    .into()
}

/// The content pane: the daemon controls, the keypad, the sleep note, then the rumble settings, laid
/// out top-to-bottom.
fn content(app: &App) -> Element<'_, Message> {
    column![controls(app), keypad(), sleep_note(), rumble(app)]
        .spacing(16.0)
        .into()
}

// --- controls row ---------------------------------------------------------------------------

/// Refresh * Input picker * Output text field * Start/Stop - the forwarder's version of the main
/// UI's daemon bar (input has no `<network>` entry; output is a free `ip:port` text field).
fn controls(app: &App) -> Element<'_, Message> {
    let state = app.status.as_ref().map(|s| s.state);
    let start = matches!(state, Some(RunState::Idle)).then_some(Message::Start);
    let stop = matches!(state, Some(RunState::Running | RunState::WaitingForDevice))
        .then_some(Message::Stop);

    let refresh = button(text("⟳")).on_press(Message::Refresh);

    // Input presets + the daemon's live device ids (no `<network>` - a forwarder reads a local pad).
    let mut inputs: Vec<String> = INPUT_PRESETS.iter().map(|s| s.to_string()).collect();
    inputs.extend(app.devices.iter().cloned());
    let selected_input = app.status.as_ref().map(|s| s.input.clone());
    let input_pick = pick_list(selected_input, inputs, String::clone)
        .on_select(Message::InputSelected)
        .placeholder("input")
        .menu_style(style::combo_menu)
        .width(160.0);

    // Output is a free text field (edited live, applied only at Start) - not driven by the daemon.
    let output = text_input("ip:port", &app.output_text)
        .on_input(Message::OutputChanged)
        .width(Fill);

    let daemon_controls = row![
        button(text("Start"))
            .on_press_maybe(start)
            .style(button::success),
        button(text("Stop"))
            .on_press_maybe(stop)
            .style(button::danger),
    ]
    .spacing(8.0);

    row![
        refresh,
        text("Input:").size(13.0),
        input_pick,
        text("Output:").size(13.0),
        output,
        daemon_controls,
    ]
    .spacing(8.0)
    .align_y(Center)
    .into()
}

// --- keypad ---------------------------------------------------------------------------------

/// The on-screen numeric keypad. A 3x4 phone grid (`1-9`, then `. 0 :`) with a tall backspace beside
/// it. Every key edits the output field in place, regardless of focus.
fn keypad() -> Element<'static, Message> {
    const ROWS: [[char; 3]; 4] = [
        ['1', '2', '3'],
        ['4', '5', '6'],
        ['7', '8', '9'],
        ['.', '0', ':'],
    ];
    let mut grid = column![].spacing(8.0);
    for r in ROWS {
        let mut line = row![].spacing(8.0);
        for c in r {
            line = line.push(key(c));
        }
        grid = grid.push(line);
    }
    // Backspace matches the grid's exact height (4 keys + 3 gaps) so it aligns flush with the bottom
    // row rather than overhanging it.
    const GRID_H: f32 = 4.0 * 42.0 + 3.0 * 8.0;
    let back = button(text("⌫").size(22.0).center())
        .on_press(Message::Backspace)
        .width(70.0)
        .height(GRID_H);
    // Centered in the window: the keypad block is its natural width, centered horizontally.
    container(row![grid, back].spacing(8.0))
        .center_x(Fill)
        .into()
}

/// One keypad key: a fixed-size button that appends its character to the output field.
fn key(c: char) -> Element<'static, Message> {
    button(text(c.to_string()).size(22.0).center())
        .on_press(Message::Key(c))
        .width(70.0)
        .height(42.0)
        .into()
}

// --- rumble ---------------------------------------------------------------------------------

/// The rumble settings for the **bound** device - the same levers the main UI's Device page shows
/// (Gordon = pulse duty + frequency; Neptune/Triton = per-motor speed + gain), but only for whatever
/// controller is bound, with no device-name header and **nothing at all before a device is bound**
/// (there is no global master rumble - each device's levers scale its own strength).
fn rumble(app: &App) -> Element<'_, Message> {
    let d = &app.device_config;
    let Some(shape) = app.status.as_ref().and_then(|s| s.bound.as_ref()).map(|b| &b.shape) else {
        // Nothing bound -> no rumble UI (an empty element).
        return column![].into();
    };
    let rows = match shape {
        Shape::Gordon => column![
            speed_lever_row("Rumble duty", &d.gordon.rumble_duty, RumbleLeverId::GordonDuty),
            rumble_frequency_row(d.gordon.rumble_freq),
            audio_duty_row(d.gordon.audio_duty),
        ],
        Shape::Neptune => column![
            speed_lever_row("Rumble speed", &d.neptune.rumble_speed, RumbleLeverId::NeptuneSpeed),
            gain_lever_row("Rumble gain", &d.neptune.rumble_gain, RumbleLeverId::NeptuneGain),
            audio_gain_row(d.neptune.audio_gain, Message::NeptuneAudioGain),
        ],
        Shape::Triton => column![
            speed_lever_row("Rumble speed", &d.triton.rumble_speed, RumbleLeverId::TritonSpeed),
            gain_lever_row("Rumble gain", &d.triton.rumble_gain, RumbleLeverId::TritonGain),
            audio_gain_row(d.triton.audio_gain, Message::TritonAudioGain),
        ],
    };
    rows.spacing(16.0).into()
}

/// The Gordon rumble-frequency row (30-150 Hz pulse-train rate).
fn rumble_frequency_row(hz: u16) -> Element<'static, Message> {
    row![
        setting_label("Rumble frequency"),
        slider(30..=150u16, hz, Message::RumbleHzChanged).step(1u16),
        text(format!("{hz} Hz")).size(13.0).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center)
    .into()
}

/// Gordon's feedback-audio **volume** row: a plain 0..=100 % duty slider (not strength-scaled).
fn audio_duty_row(value: u8) -> Element<'static, Message> {
    row![
        setting_label("Audio duty"),
        slider(0..=100u8, value, Message::GordonAudioDuty),
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
/// scaled shows a min + max pair. `id` routes every edit to the right lever.
fn speed_lever_row<'a>(
    label: &'static str,
    lever: &Lever<u8>,
    id: RumbleLeverId,
) -> Element<'a, Message> {
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
fn gain_lever_row<'a>(
    label: &'static str,
    lever: &Lever<i8>,
    id: RumbleLeverId,
) -> Element<'a, Message> {
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

/// The shared frame for a lever row: label + "scale" checkbox + the variant's sliders.
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
    Message::RumbleLever(id, e)
}

/// A fixed-width row label, so the controls line up down the form.
fn setting_label(s: &'static str) -> Element<'static, Message> {
    text(s).size(13.0).width(140.0).into()
}

/// Small/secondary copy (min/max/scale captions).
fn small(s: &'static str) -> Element<'static, Message> {
    text(s).size(11.0).into()
}

/// A fixed-width trailing percentage readout, keeping the sliders aligned.
fn pct_text(v: Option<u8>) -> Element<'static, Message> {
    text(v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into()))
        .size(13.0)
        .width(60.0)
        .into()
}

/// A fixed-width trailing dB readout (signed), keeping the gain sliders aligned.
fn db_text(v: i8) -> Element<'static, Message> {
    text(format!("{v:+} dB")).size(13.0).width(60.0).into()
}

/// A centered, prominent reminder that the app may hold a sleep inhibitor while running (the managed
/// daemon is launched with `--prevent-sleep`).
fn sleep_note() -> Element<'static, Message> {
    container(
        text("Sleep may be prevented while this app is running.")
            .size(18.0)
            .center(),
    )
    .center_x(Fill)
    .into()
}

// --- bottom bar -----------------------------------------------------------------------------

/// The status-bar separator glyph.
fn sep() -> Element<'static, Message> {
    text("│").size(13.0).style(style::muted_text).into()
}

/// Full-width status bar: connection state, engine state, bound device (+ controller dot), and any
/// error on the right. No profile/chords segments (this app configures none).
fn bottom_bar(app: &App) -> Element<'_, Message> {
    let (dot, label): (fn(&Theme) -> text::Style, _) = if app.connected {
        let label = if crate::daemon::is_managed(&app.daemon) {
            "managed"
        } else {
            "connected"
        };
        (style::success_text, label)
    } else {
        (style::danger_text, "disconnected")
    };
    let conn = row![text("●").size(13.0).style(dot), text(label).size(13.0)]
        .spacing(6.0)
        .align_y(Center);
    let mut bar = row![conn].spacing(10.0).align_y(Center).padding(8.0);

    if let Some(s) = &app.status {
        let device: Element<'_, Message> = {
            let dev = s.bound.as_ref().map(|b| b.id.as_str()).unwrap_or("—");
            // Battery (wireless controller) is appended as " (B%)"; omitted when unknown.
            let batt = s.battery.map_or_else(String::new, |pct| format!(" ({pct}%)"));
            let id = text(format!("device: {dev}{batt}")).size(13.0);
            match s.controller {
                Some(present) => {
                    let d: fn(&Theme) -> text::Style = if present {
                        style::success_text
                    } else {
                        style::danger_text
                    };
                    row![text("●").size(13.0).style(d), id]
                        .spacing(6.0)
                        .align_y(Center)
                        .into()
                }
                None => id.into(),
            }
        };
        bar = bar
            .push(sep())
            .push(text(format!("state: {:?}", s.state)).size(13.0))
            .push(sep())
            .push(device);
    }
    if let Some(err) = &app.error {
        bar = bar.push(Space::new().width(Fill)).push(
            text(format!("⚠ {err}"))
                .size(13.0)
                .style(style::danger_text),
        );
    }

    container(bar).style(container::dark).width(Fill).into()
}
