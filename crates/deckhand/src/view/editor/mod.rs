//! The profile-editor screens — the pages reachable once a profile is loaded for editing.
//!
//! The real editor is still being built: only the **Profile** page ([`profile_screen`]) is wired
//! (the profile name today; action sets + layers land above it). The per-input tabs
//! ([`buttons_screen`] and, for now, [`stub_screen`]) are mockups previewing the intended
//! grouped-row layout — nothing there is wired (all interactions send [`Message::Ignored`]).

use iced::widget::{Space, button, column, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Theme};

use super::{card, group_header, section_header};
use crate::nav::Category;
use crate::{App, Message, style};

/// Profile screen — the top of the profile editor. For now just the profile's name; action sets and
/// layers land here below it once implemented. Reachable only while a profile is loaded, so the
/// field is always live.
pub(super) fn profile_screen(app: &App) -> Element<'_, Message> {
    let name = row![
        text("Profile name").width(140.0),
        text_input("profile name", app.editing_name())
            .on_input(Message::ProfileNameChanged)
            .width(Fill),
    ]
    .spacing(12.0)
    .align_y(Center);
    column![section_header("Profile"), name].spacing(20.0).into()
}

/// A mockup of the Steam-Deck-style **Buttons** screen — grouped input rows, each with a per-input
/// gear, and a group behavior picker — purely to preview the widgets + scrolling. Nothing is wired.
pub(super) fn buttons_screen() -> Element<'static, Message> {
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

    column![section_header("Buttons"), face, bumpers, dpad, system].spacing(20.0).into()
}

/// A group's "Behavior" selector (buttons only have Button Pad; the rest is filler) + a gear.
fn behavior_row() -> Element<'static, Message> {
    let options =
        vec!["Button Pad".to_string(), "Lorem Ipsum".to_string(), "Dolor Sit Amet".to_string()];
    let combo = pick_list(Some("Button Pad".to_string()), options, String::clone)
        .on_select(|_| Message::Ignored);
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

/// A placeholder for the profile-edit categories not yet mocked up.
pub(super) fn stub_screen(c: Category) -> Element<'static, Message> {
    column![
        section_header(c.label()),
        text("Profile-edit screen — stubbed for the toolkit test.").size(13.0),
    ]
    .spacing(10.0)
    .into()
}
