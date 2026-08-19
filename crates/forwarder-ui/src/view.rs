//! The single-screen widget layer: a controls row, an on-screen numeric keypad for the output
//! address, a master-rumble slider, and a bottom status bar.
//!
//! Everything is one window (no tray, no sidebar, no pages). The bottom bar is a trimmed version of
//! the main UI's — just daemon status, state, device, and error (no profiles/chords).

use iced::widget::{Space, button, column, container, pick_list, row, slider, text, text_input};
use iced::{Center, Element, Fill, Theme};
use ipc::RunState;

use crate::{App, INPUT_PRESETS, Message, style};

/// The whole window: content pane on top, status bar at the bottom.
pub(crate) fn view(app: &App) -> Element<'_, Message> {
    column![
        container(content(app))
            .padding(16.0)
            .width(Fill)
            .height(Fill),
        bottom_bar(app),
    ]
    .into()
}

/// The content pane: the daemon controls, the keypad + rumble, laid out top-to-bottom.
fn content(app: &App) -> Element<'_, Message> {
    column![controls(app), keypad(), rumble(app), sleep_note()]
        .spacing(16.0)
        .into()
}

// --- controls row ---------------------------------------------------------------------------

/// Refresh · Input picker · Output text field · Start/Stop — the forwarder's version of the main
/// UI's daemon bar (input has no `<network>` entry; output is a free `ip:port` text field).
fn controls(app: &App) -> Element<'_, Message> {
    let state = app.status.as_ref().map(|s| s.state);
    let start = matches!(state, Some(RunState::Idle)).then_some(Message::Start);
    let stop = matches!(state, Some(RunState::Running | RunState::WaitingForDevice))
        .then_some(Message::Stop);

    let refresh = button(text("⟳")).on_press(Message::Refresh);

    // Input presets + the daemon's live device ids (no `<network>` — a forwarder reads a local pad).
    let mut inputs: Vec<String> = INPUT_PRESETS.iter().map(|s| s.to_string()).collect();
    inputs.extend(app.devices.iter().cloned());
    let selected_input = app.status.as_ref().map(|s| s.input.clone());
    let input_pick = pick_list(selected_input, inputs, String::clone)
        .on_select(Message::InputSelected)
        .placeholder("input")
        .menu_style(style::combo_menu)
        .width(160.0);

    // Output is a free text field (edited live, applied only at Start) — not driven by the daemon.
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

/// The on-screen numeric keypad. A 3×4 phone grid (`1-9`, then `. 0 :`) with a tall backspace beside
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
    const GRID_H: f32 = 4.0 * 60.0 + 3.0 * 8.0;
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
        .height(60.0)
        .into()
}

// --- rumble ---------------------------------------------------------------------------------

/// The master-rumble slider (`0..=100 %`), writing `DeviceConfig.master_rumble`, with the same
/// "only on (re)start" caveat the main UI's Device page shows above it.
fn rumble(app: &App) -> Element<'_, Message> {
    let v = app.device_config.master_rumble;
    let note = text("This setting takes effect only on engine (re)start.").size(12.0);
    let control = row![
        text("Master rumble").size(13.0).width(140.0),
        slider(0..=100u8, v, Message::RumbleChanged).width(Fill),
        text(format!("{v}%")).size(13.0).width(48.0),
    ]
    .spacing(12.0)
    .align_y(Center);
    column![note, control].spacing(6.0).into()
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
            let id = text(format!("device: {}", s.bound.as_deref().unwrap_or("—"))).size(13.0);
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
