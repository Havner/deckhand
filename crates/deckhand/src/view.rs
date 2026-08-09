//! The iced widget layer: the four-region window and the per-category screens.

use iced::widget::{
    Space, button, checkbox, column, container, pick_list, progress_bar, row, scrollable, text,
    text_input,
};
use iced::{Center, Element, Fill, Theme};
use ipc::RunState;

use crate::nav::Category;
use crate::{App, INPUT_PRESETS, Message, OUTPUT_PRESETS, style};

/// The whole window: top bar / (sidebar + content) / bottom bar.
pub fn view(app: &App) -> Element<'_, Message> {
    column![
        top_bar(app),
        row![sidebar(app), content(app)].height(Fill),
        bottom_bar(app),
    ]
    .into()
}

/// Full-width daemon bar: Start/Stop/Connect on the left, input/output pickers on the right.
fn top_bar(app: &App) -> Element<'_, Message> {
    let state = app.status.as_ref().map(|s| s.state);
    // State-driven enablement (disabled = no on_press).
    let start = matches!(state, Some(RunState::Idle)).then_some(Message::Start);
    let stop = matches!(state, Some(RunState::Running | RunState::WaitingForDevice))
        .then_some(Message::Stop);
    let connect = (!app.connected).then_some(Message::Connect);

    let controls = row![
        button(text("Start")).on_press_maybe(start).style(button::success),
        button(text("Stop")).on_press_maybe(stop).style(button::danger),
        button(text("Connect")).on_press_maybe(connect),
    ]
    .spacing(8.0);

    // Input presets + the daemon's live device ids.
    let mut inputs: Vec<String> = INPUT_PRESETS.iter().map(|s| s.to_string()).collect();
    inputs.extend(app.devices.iter().cloned());
    let selected_input = app.status.as_ref().map(|s| s.input.clone());
    let input_pick = pick_list(inputs, selected_input, Message::InputSelected).placeholder("input");

    let outputs: Vec<String> = OUTPUT_PRESETS.iter().map(|s| s.to_string()).collect();
    let selected_output = app.status.as_ref().map(|s| s.output.clone());
    let output_pick =
        pick_list(outputs, selected_output, Message::OutputSelected).placeholder("output");

    // Manual device re-enumeration (no USB hotplug). Default style → blue, matching Connect.
    let refresh = button(text("⟳")).on_press(Message::Refresh);

    let bar = row![
        controls,
        Space::new().width(Fill),
        refresh,
        text("Input:").size(13.0),
        input_pick,
        text("Output:").size(13.0),
        output_pick,
    ]
    .spacing(8.0)
    .align_y(Center)
    .padding(8.0);

    container(bar).style(container::dark).width(Fill).into()
}

/// Left sidebar: profile-edit categories up top, app-level (Settings/Globals) pinned at the bottom.
fn sidebar(app: &App) -> Element<'_, Message> {
    let mut top = column![].spacing(4.0);
    for &c in Category::PROFILE {
        top = top.push(nav_button(app, c));
    }
    let mut bottom = column![].spacing(4.0);
    for &c in Category::APP {
        bottom = bottom.push(nav_button(app, c));
    }

    let col = column![top, Space::new().height(Fill), bottom]
        .padding(8.0)
        .spacing(8.0)
        .width(Fill);

    container(col).width(180.0).height(Fill).style(style::panel).into()
}

/// One sidebar entry; the selected one gets the primary style.
fn nav_button(app: &App, c: Category) -> Element<'static, Message> {
    let style = if app.category == c { button::primary } else { button::text };
    button(text(c.label()))
        .width(Fill)
        .padding(8.0)
        .on_press(Message::Navigate(c))
        .style(style)
        .into()
}

/// The scrollable content pane; swaps entirely on the selected category.
fn content(app: &App) -> Element<'_, Message> {
    let inner: Element<'_, Message> = match app.category {
        Category::Buttons => buttons_screen(),
        Category::Settings => settings_screen(app),
        Category::Globals => globals_screen(app),
        other => stub_screen(other),
    };
    scrollable(container(inner).padding(16.0).width(Fill)).width(Fill).height(Fill).into()
}

/// Settings screen — the two start toggles + the two profile paths (each path greyed when its
/// toggle is off).
fn settings_screen(app: &App) -> Element<'_, Message> {
    let s = &app.settings;
    let start = checkbox(s.start_daemon)
        .label("Start the daemon if not running on application start")
        .on_toggle(Message::ToggleStartDaemon);
    let load_main = checkbox(s.load_main)
        .label("Load the main profile on start")
        .on_toggle(Message::ToggleLoadMain);
    let main_row =
        path_row(&s.main_path, s.load_main, Message::MainPathChanged, Message::BrowseMain);
    let load_fb = checkbox(s.load_fallback)
        .label("Load the fallback profile on start")
        .on_toggle(Message::ToggleLoadFallback);
    let fb_row = path_row(
        &s.fallback_path,
        s.load_fallback,
        Message::FallbackPathChanged,
        Message::BrowseFallback,
    );

    let theme_pick = row![
        text("Theme:").size(14.0),
        pick_list(Theme::ALL, Some(app.active_theme()), Message::SetTheme),
    ]
    .spacing(8.0)
    .align_y(Center);

    column![
        text("Settings").size(24.0),
        text("Application settings — stored separately from the daemon.").size(13.0),
        Space::new().height(8.0),
        start,
        load_main,
        main_row,
        load_fb,
        fb_row,
        Space::new().height(8.0),
        theme_pick,
    ]
    .spacing(10.0)
    .into()
}

/// A profile-path row: text field + Browse button, both inert (greyed) when `enabled` is false.
fn path_row<'a>(
    path: &'a str,
    enabled: bool,
    on_change: fn(String) -> Message,
    browse: Message,
) -> Element<'a, Message> {
    let mut input = text_input("path to .ron file", path).width(Fill);
    if enabled {
        input = input.on_input(on_change);
    }
    let mut btn = button(text("Browse…")).style(button::secondary);
    if enabled {
        btn = btn.on_press(browse);
    }
    row![input, btn].spacing(8.0).align_y(Center).into()
}

/// Globals screen — laid out for the widget preview; reads the daemon's current globals but is not
/// wired to change them yet.
fn globals_screen(app: &App) -> Element<'_, Message> {
    let mut col = column![
        text("Globals").size(24.0),
        text("Global engine config — widget preview; not wired to edit yet.").size(13.0),
        Space::new().height(8.0),
    ]
    .spacing(10.0);

    if let Some(status) = &app.status {
        let g = &status.globals;
        col = col
            .push(text(format!("Start profile: {:?}", g.start_profile)))
            .push(text(format!("Master rumble: {}%", g.master_rumble)))
            .push(progress_bar(0.0..=100.0, g.master_rumble as f32))
            .push(text(format!("LED brightness: {}", opt_pct(g.led_brightness))))
            .push(text(format!("Idle timeout: {}", opt_secs(g.idle_timeout))))
            .push(text(format!("Chords: {}", g.chords.len())));
    } else {
        col = col.push(text("Connect to the daemon to view its globals."));
    }
    col.into()
}

/// A mockup of the Steam-Deck-style **Buttons** screen — grouped input rows, each with a per-input
/// gear, and a group behavior picker — purely to preview the widgets + scrolling. Nothing is wired
/// (all interactions send [`Message::Ignored`]).
fn buttons_screen() -> Element<'static, Message> {
    // Xbox face-button glyphs, colored from theme roles (A green→success, B red→danger,
    // X blue→primary, Y yellow→warning) so they track the theme like everything else.
    let face = column![
        group_header("Face Buttons"),
        behavior_row(),
        input_row(Some(style::success_text), "A Button"),
        input_row(Some(style::danger_text), "B Button"),
        input_row(Some(style::primary_text), "X Button"),
        input_row(Some(style::warning_text), "Y Button"),
    ]
    .spacing(8.0);

    let bumpers = column![
        group_header("Bumpers"),
        input_row(None, "Left Bumper"),
        input_row(None, "Right Bumper"),
    ]
    .spacing(8.0);

    let dpad = column![
        group_header("D-Pad"),
        input_row(None, "Up"),
        input_row(None, "Down"),
        input_row(None, "Left"),
        input_row(None, "Right"),
    ]
    .spacing(8.0);

    let system = column![
        group_header("System"),
        input_row(None, "View"),
        input_row(None, "Menu"),
        input_row(None, "Steam"),
    ]
    .spacing(8.0);

    column![text("Buttons").size(24.0), face, bumpers, dpad, system].spacing(20.0).into()
}

/// A group section title.
fn group_header(title: &'static str) -> Element<'static, Message> {
    text(title).size(18.0).into()
}

/// A group's "Behavior" selector (buttons only have Button Pad; the rest is filler) + a gear.
fn behavior_row() -> Element<'static, Message> {
    let options =
        vec!["Button Pad".to_string(), "Lorem Ipsum".to_string(), "Dolor Sit Amet".to_string()];
    let combo = pick_list(options, Some("Button Pad".to_string()), |_| Message::Ignored);
    let inner = row![text("Behavior"), Space::new().width(Fill), combo, gear()]
        .spacing(12.0)
        .align_y(Center);
    card(inner)
}

/// One input row: an optional colored glyph (a theme-role text style), the input name, and a gear
/// on the right.
fn input_row(
    dot: Option<fn(&Theme) -> text::Style>,
    label: &'static str,
) -> Element<'static, Message> {
    let mut r = row![].spacing(12.0).align_y(Center);
    if let Some(role) = dot {
        r = r.push(text("●").size(16.0).style(role));
    }
    let inner = r.push(text(label)).push(Space::new().width(Fill)).push(gear());
    card(inner)
}

/// An unwired settings/gear button.
fn gear() -> Element<'static, Message> {
    button(text("⚙").size(16.0)).on_press(Message::Ignored).style(button::secondary).into()
}

/// Wrap a row as a padded card so the groups read like the Steam UI's list rows.
fn card<'a>(inner: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(inner).padding(10.0).width(Fill).style(style::panel).into()
}

/// A placeholder for the profile-edit categories, not part of this toolkit test.
fn stub_screen(c: Category) -> Element<'static, Message> {
    column![
        text(c.label()).size(24.0),
        text("Profile-edit screen — stubbed for the toolkit test.").size(13.0),
    ]
    .spacing(10.0)
    .into()
}

/// Full-width status bar from the engine status (device bound, profiles, chord count, …).
fn bottom_bar(app: &App) -> Element<'_, Message> {
    // Dot color from theme roles: success (green, matching Start) / danger (red, matching Stop).
    let (dot, label): (fn(&Theme) -> text::Style, _) = if app.connected {
        (style::success_text, "connected")
    } else {
        (style::danger_text, "disconnected")
    };
    let conn = row![text("●").size(13.0).style(dot), text(label).size(13.0)]
        .spacing(6.0)
        .align_y(Center);
    let mut bar = row![conn].spacing(10.0).align_y(Center).padding(8.0);

    if let Some(s) = &app.status {
        bar = bar
            .push(sep())
            .push(text(format!("state: {:?}", s.state)).size(13.0))
            .push(sep())
            .push(text(format!("device: {}", s.bound.as_deref().unwrap_or("—"))).size(13.0))
            .push(sep())
            .push(text(format!("main: {}", s.main.as_deref().unwrap_or("—"))).size(13.0))
            .push(sep())
            .push(text(format!("fallback: {}", s.fallback.as_deref().unwrap_or("—"))).size(13.0))
            .push(sep())
            .push(text(format!("chords: {}", s.globals.chords.len())).size(13.0));
    }
    if let Some(err) = &app.error {
        bar = bar
            .push(Space::new().width(Fill))
            .push(text(format!("⚠ {err}")).size(13.0).style(style::danger_text));
    }

    container(bar).style(container::dark).width(Fill).into()
}

fn sep() -> Element<'static, Message> {
    text("│").size(13.0).style(style::muted_text).into()
}

fn opt_pct(v: Option<u8>) -> String {
    v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into())
}

fn opt_secs(v: Option<u16>) -> String {
    v.map(|x| format!("{x}s")).unwrap_or_else(|| "default".into())
}
