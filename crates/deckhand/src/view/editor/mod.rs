//! The profile-editor screens — the pages reachable once a profile is loaded for editing.
//!
//! The real editor is still being built. Only the **Profile** page's name field is wired
//! ([`profile_screen`]); everything else here is a **mockup** to preview the intended layout so it
//! can be reviewed (all interactions send [`Message::Ignored`]). The per-input pages
//! ([`input_screen`]) are rendered *data-driven* from [`Category::groups`], so the mock mirrors the
//! real input→page mapping (headers, rich-source behaviour rows, nested click/touch sub-buttons).
//! [`rumble_screen`] mocks the profile-level rumble feel. These get replaced screen-by-screen as the
//! real editor lands.

use iced::widget::{Space, button, column, pick_list, row, slider, text, text_input};
use iced::{Center, Element, Fill, Theme};

use config::{InputSource, SourceKind};

use super::{card, group_header, section_header, small};
use crate::nav::{Category, InputGroup};
use crate::{App, Message, style};

/// Profile screen — the top of the profile editor: the profile name (wired), then mockups of the
/// action sets and layers that will live below it.
pub(super) fn profile_screen(app: &App) -> Element<'_, Message> {
    let name = row![
        text("Profile name").width(140.0),
        text_input("profile name", app.editing_name())
            .on_input(Message::ProfileNameChanged)
            .width(Fill),
    ]
    .spacing(12.0)
    .align_y(Center);

    // Mockup — action sets + layers live below the name (not wired yet).
    let base_set =
        card(row![text("base"), Space::new().width(Fill), gear()].spacing(12.0).align_y(Center));
    let action_sets = column![
        group_header("Action Sets"),
        small("Full-controller modes; one active at a time. Mockup — not wired yet."),
        base_set,
        button(text("+ Add action set")).style(button::secondary).on_press(Message::Ignored),
    ]
    .spacing(8.0);
    let layers = column![
        group_header("Layers"),
        small("Stackable overlays on the active action set. Mockup — not wired yet."),
        button(text("+ Add layer")).style(button::secondary).on_press(Message::Ignored),
    ]
    .spacing(8.0);

    column![section_header("Profile"), name, action_sets, layers].spacing(20.0).into()
}

/// A per-input editor page (Buttons/Triggers/Joysticks/Trackpads/Gyro), rendered from the category's
/// [`InputGroup`]s so the mock stays in lock-step with the real input→page mapping.
pub(super) fn input_screen(category: Category) -> Element<'static, Message> {
    let mut col = column![section_header(category.label())].spacing(20.0);
    for group in category.groups() {
        col = col.push(mock_group(group));
    }
    col.into()
}

/// Rumble screen — the profile-level rumble feel (game rumble → trackpad haptics). Mockup.
pub(super) fn rumble_screen() -> Element<'static, Message> {
    let slider_row = |label: &'static str, value: u16, max: u16, readout: String| {
        row![
            text(label).width(140.0),
            slider(0..=max, value, |_| Message::Ignored).style(style::disabled_slider),
            text(readout).width(60.0),
        ]
        .spacing(12.0)
        .align_y(Center)
    };
    column![
        section_header("Rumble"),
        small("Per-profile rumble feel (game rumble → trackpad haptics). Mockup — not wired yet."),
        slider_row("Strength", 100, 200, "100%".into()),
        slider_row("Frequency", 60, 200, "60 Hz".into()),
    ]
    .spacing(16.0)
    .into()
}

// --- mock building blocks -------------------------------------------------------------------

/// One input group: its header, then its primary inputs (rich sources get a behaviour row and, for
/// button clusters, one row per button), then any sub-buttons after a small gap.
fn mock_group(group: &InputGroup) -> Element<'static, Message> {
    let mut col = column![group_header(group.header)].spacing(8.0);
    for input in group.primary {
        col = col.push(mock_primary(input));
    }
    if !group.sub.is_empty() {
        col = col.push(Space::new().height(4.0));
        for input in group.sub {
            col = col.push(input_row(None, input_label(input)));
        }
    }
    col.into()
}

/// A primary input: a plain button is one row; a button group (Face Buttons / D-Pad) gets a
/// behaviour row + its members; a rich analog source gets a behaviour row + a row for itself.
fn mock_primary(input: &InputSource) -> Element<'static, Message> {
    match input.kind() {
        SourceKind::Button => input_row(None, input_label(input)),
        SourceKind::ButtonGroup => button_group_mock(input),
        kind => column![behavior_row(kind), input_row(None, input_label(input))].spacing(8.0).into(),
    }
}

/// A 4-button cluster mock: a behaviour row (Button Pad) + the four members. Face Buttons carry the
/// Xbox glyph colors (A green→success, B red→danger, X blue→primary, Y yellow→warning).
fn button_group_mock(input: &InputSource) -> Element<'static, Message> {
    let members: Vec<(Option<fn(&Theme) -> text::Style>, &'static str)> = match input {
        InputSource::FaceButtons => vec![
            (Some(style::success_text), "A Button"),
            (Some(style::danger_text), "B Button"),
            (Some(style::primary_text), "X Button"),
            (Some(style::warning_text), "Y Button"),
        ],
        InputSource::DPad => {
            vec![(None, "Up"), (None, "Down"), (None, "Left"), (None, "Right")]
        }
        _ => Vec::new(),
    };
    let mut col = column![behavior_row(SourceKind::ButtonGroup)].spacing(8.0);
    for (dot, label) in members {
        col = col.push(input_row(dot, label));
    }
    col.into()
}

/// A group's "Behavior" selector, filled with plausible options for the source kind + a gear. Mock.
fn behavior_row(kind: SourceKind) -> Element<'static, Message> {
    let (selected, options) = behavior_choices(kind);
    let options: Vec<String> = options.iter().map(|s| s.to_string()).collect();
    let combo =
        pick_list(Some(selected.to_string()), options, String::clone).on_select(|_| Message::Ignored);
    let inner = row![text("Behavior"), Space::new().width(Fill), combo, gear()]
        .spacing(12.0)
        .align_y(Center);
    card(inner)
}

/// Plausible behaviour options per source kind (mock only — not the authoritative behaviour set;
/// that arrives with the real editor). Returns the default plus the list shown in the picker.
fn behavior_choices(kind: SourceKind) -> (&'static str, &'static [&'static str]) {
    match kind {
        SourceKind::ButtonGroup => ("Button Pad", &["Button Pad", "Directional Pad"]),
        SourceKind::Pad => ("As Mouse", &["As Mouse", "Joystick", "Directional Pad", "Scroll Wheel"]),
        SourceKind::Stick => ("Joystick", &["Joystick", "Joystick Mouse", "Directional Pad"]),
        SourceKind::Trigger => ("Trigger", &["Trigger", "Soft Pull"]),
        SourceKind::Gyro => ("Gyro to Mouse", &["Gyro to Mouse", "Off"]),
        SourceKind::Button => ("", &[]),
    }
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

/// A human-readable label for an input, for the mock rows.
fn input_label(input: &InputSource) -> &'static str {
    match input {
        InputSource::FaceButtons => "Face Buttons",
        InputSource::DPad => "D-Pad",
        InputSource::LeftPad => "Left Trackpad",
        InputSource::RightPad => "Right Trackpad",
        InputSource::LeftStick => "Left Stick",
        InputSource::RightStick => "Right Stick",
        InputSource::LeftTrigger => "Left Trigger",
        InputSource::RightTrigger => "Right Trigger",
        InputSource::Gyro => "Gyro",
        InputSource::LeftBumper => "Left Bumper",
        InputSource::RightBumper => "Right Bumper",
        InputSource::LeftTriggerFull => "Left Trigger (full pull)",
        InputSource::RightTriggerFull => "Right Trigger (full pull)",
        InputSource::LeftGrip => "Left Grip",
        InputSource::RightGrip => "Right Grip",
        InputSource::LeftGrip2 => "Left Grip 2",
        InputSource::RightGrip2 => "Right Grip 2",
        InputSource::View => "View",
        InputSource::Menu => "Menu",
        InputSource::Steam => "Steam",
        InputSource::QuickAccess => "Quick Access",
        InputSource::LeftStickClick => "Left Stick Click",
        InputSource::RightStickClick => "Right Stick Click",
        InputSource::LeftPadClick => "Left Trackpad Click",
        InputSource::RightPadClick => "Right Trackpad Click",
        InputSource::LeftPadTouch => "Left Trackpad Touch",
        InputSource::RightPadTouch => "Right Trackpad Touch",
    }
}
