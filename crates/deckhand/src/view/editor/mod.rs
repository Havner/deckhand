//! The profile-editor screens — the pages reachable once a profile is loaded for editing.
//!
//! The real editor is still being built. The **Profile** page is wired ([`profile_screen`]: the
//! name field, plus the action-set/layer list with per-bar gear context menus and add/rename/remove
//! dialogs); everything else here is a **mockup** to preview the intended layout so it can be
//! reviewed (all interactions send [`Message::Ignored`]). The per-input pages
//! ([`input_screen`]) are rendered *data-driven* from [`Category::groups`], so the mock mirrors the
//! real input→page mapping (headers, rich-source behaviour rows, nested click/touch sub-buttons).
//! [`rumble_screen`] mocks the profile-level rumble feel. These get replaced screen-by-screen as the
//! real editor lands.

use iced::widget::{Space, button, column, container, pick_list, row, slider, text, text_input};
use iced::{Center, Element, Fill, Theme};

use config::{InputSource, SourceKind};

use super::{card, group_header, section_header, small};
use crate::editor::{Behavior, EditorMessage};
use crate::nav::{Category, InputGroup};
use crate::{App, Message, style};

/// Fixed width shared by a behaviour row's combobox and each input row's "Add command" button, so
/// the right-hand controls line up down the page.
const CMD_SLOT: f32 = 200.0;

/// Profile screen — the top of the profile editor: the profile name, then the action sets with
/// their layers nested beneath, each bar's gear opening a context menu (rename/remove, and add-layer
/// on a set). All edits go through [`EditorMessage`] and autosave.
pub(super) fn profile_screen(app: &App) -> Element<'_, Message> {
    let name = row![
        text("Profile name").width(140.0),
        text_input("profile name", app.editing_name())
            .on_input(|s| Message::Editor(EditorMessage::NameChanged(s)))
            .width(Fill),
    ]
    .spacing(12.0)
    .align_y(Center);

    // The action sets, each with its layers nested (indented) beneath it, like Steam. Every bar
    // carries its own gear that opens a context menu acting on *that* set/layer (add layer / rename
    // / remove); the gear's target is the bar's own name-addressed [`EditTarget`].
    let header = row![group_header("Action Sets"), Space::new().width(Fill)].align_y(Center);
    let mut list = column![header].spacing(8.0);
    if let Some(ed) = &app.editing {
        for set in &ed.doc.action_sets {
            list = list.push(set_bar(&set.name));
            for layer in &set.layers {
                list = list.push(layer_bar(&set.name, &layer.name));
            }
        }
    }
    // A little breathing room above "Add action set" so it reads as separate from the set/layer
    // list rather than as another bar (the container top-pad adds to the column's row spacing).
    list = list.push(
        container(
            button(text("Add action set"))
                .style(button::secondary)
                .on_press(Message::Editor(EditorMessage::AddSet)),
        )
        .padding(iced::padding::top(6.0)),
    );

    column![section_header("Profile"), name, list].spacing(20.0).into()
}

/// A full-width action-set bar with its gear (opens the set's context menu).
fn set_bar(name: &str) -> Element<'static, Message> {
    let target = crate::editor::EditTarget { set: name.to_string(), layer: None };
    card(row![text(name.to_string()), Space::new().width(Fill), menu_gear(target)]
        .spacing(12.0)
        .align_y(Center))
}

/// A layer bar, indented under its parent set, with its gear (opens the layer's context menu).
fn layer_bar(set: &str, name: &str) -> Element<'static, Message> {
    let target =
        crate::editor::EditTarget { set: set.to_string(), layer: Some(name.to_string()) };
    let bar = card(
        row![text(name.to_string()), Space::new().width(Fill), menu_gear(target)]
            .spacing(12.0)
            .align_y(Center),
    );
    row![Space::new().width(24.0), bar].into()
}

/// The gear on a Profile-page bar: opens the context menu for `target`.
fn menu_gear(target: crate::editor::EditTarget) -> Element<'static, Message> {
    button(text("⚙").size(16.0))
        .style(style::combo_button)
        .on_press(Message::Editor(EditorMessage::OpenMenu(target)))
        .into()
}

/// The sidebar action-set / layer selector: ◀ / ▶ arrows around two stacked labels. Walks
/// [`crate::edit_target_list`] over the loaded profile; the arrows disable at the ends. The label
/// you're **editing** is normal-colored, its context muted: on an action set the set name is active
/// (layer line blank); on a layer the set name is muted context and the layer name is active.
pub(super) fn action_set_selector(app: &App) -> Element<'static, Message> {
    // Always rendered so loading/unloading a profile doesn't shift the sidebar. Inert when nothing
    // is loaded: both arrows disabled, both labels blank (blank lines still hold the height).
    let (top, bottom, prev_enabled, next_enabled) = match &app.editing {
        Some(ed) => {
            let list = crate::editor::edit_target_list(&ed.doc);
            let pos = list.iter().position(|t| *t == ed.target).unwrap_or(0);
            // Top = the action set name (muted when a layer is the active target — it's just
            // context); bottom = the layer name, or a blank line to hold the height.
            let top = {
                let t = text(ed.target.set.clone()).size(13.0);
                if ed.target.layer.is_some() { t.style(style::muted_text) } else { t }
            };
            let bottom = match &ed.target.layer {
                Some(layer) => text(layer.clone()).size(13.0),
                None => text(" ").size(13.0),
            };
            (top, bottom, pos > 0, pos + 1 < list.len())
        }
        None => (text(" ").size(13.0), text(" ").size(13.0), false, false),
    };
    let labels = column![top, bottom].align_x(Center).spacing(2.0).width(Fill);

    let arrow = |glyph: &'static str, enabled: bool, msg: Message| -> Element<'static, Message> {
        let mut b = button(text(glyph).size(14.0)).style(style::combo_button);
        if enabled {
            b = b.on_press(msg);
        }
        b.into()
    };
    row![
        arrow("◀", prev_enabled, Message::Editor(EditorMessage::TargetPrev)),
        labels,
        arrow("▶", next_enabled, Message::Editor(EditorMessage::TargetNext)),
    ]
    .align_y(Center)
    .spacing(6.0)
    .into()
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
        // Rich analog source: the group header already names it, so just its behaviour selector
        // (its clicks/touches appear as sub-buttons below).
        kind => behavior_row(kind),
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

/// A group's "Behavior" selector: the real [`Behavior`] set for the source kind (default = first),
/// plus a gear. Still a mock — selection isn't wired to construct a binding yet (`Message::Ignored`).
fn behavior_row(kind: SourceKind) -> Element<'static, Message> {
    let options = Behavior::valid_for(kind).to_vec();
    let selected = options.first().copied();
    let combo = pick_list(selected, options, |b: &Behavior| b.label().to_string())
        .on_select(|_| Message::Ignored)
        .width(CMD_SLOT);
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
    // "Add command" fills the same slot + width as a behaviour row's combobox, so the right-hand
    // controls line up down the page; it will open the output-selector modal. Then the gear.
    let add_command = button(text("<unbound>").center())
        .width(CMD_SLOT)
        .style(style::combo_button)
        .on_press(Message::Ignored);
    let inner = r.push(text(label)).push(Space::new().width(Fill)).push(add_command).push(gear());
    card(inner)
}

/// An unwired settings/gear button, colored to match the comboboxes on the same cards.
fn gear() -> Element<'static, Message> {
    button(text("⚙").size(16.0)).on_press(Message::Ignored).style(style::combo_button).into()
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
