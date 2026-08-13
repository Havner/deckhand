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

use std::collections::BTreeMap;

use iced::widget::{Space, button, column, container, pick_list, row, slider, text, text_input};
use iced::{Center, Element, Fill, Theme};

use config::{InputSource, SourceBinding, SourceKind};

use super::{card, group_header, section_header, small};
use crate::editor::{Behavior, CommandDest, CommandSlot, EditorMessage};
use crate::nav::{Category, InputGroup};
use crate::view::modal::action_label;
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

/// A per-input editor page (Buttons/Triggers/Joysticks/Trackpads/Gyro): rendered from the category's
/// [`InputGroup`]s over the **currently-edited** action set / layer's bindings, so every control
/// reflects and mutates the live [`ConfigDoc`](config::ConfigDoc).
pub(super) fn input_screen(app: &App, category: Category) -> Element<'static, Message> {
    let binds = crate::editor::current_bindings(app);
    let mut col = column![section_header(category.label())].spacing(20.0);
    for group in category.groups() {
        col = col.push(group_view(binds, group));
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

// --- building blocks ------------------------------------------------------------------------

/// The bindings map of the set/layer being edited, or `None` when nothing is loaded.
type Binds<'a> = Option<&'a BTreeMap<InputSource, SourceBinding>>;

/// One input group: its header, then its primary inputs, then any sub-buttons (each a plain button)
/// after a small gap.
fn group_view(binds: Binds, group: &InputGroup) -> Element<'static, Message> {
    let mut col = column![group_header(group.header)].spacing(8.0);
    for input in group.primary {
        col = col.push(primary_view(binds, input));
    }
    if !group.sub.is_empty() {
        col = col.push(Space::new().height(4.0));
        for input in group.sub {
            col = col.push(command_bar(binds, input, CommandSlot::Button, input_label(input), None));
        }
    }
    col.into()
}

/// A primary input: a plain button is one command bar; a button group / rich analog source gets a
/// behaviour selector plus the command bars its chosen behaviour exposes.
fn primary_view(binds: Binds, input: &InputSource) -> Element<'static, Message> {
    match input.kind() {
        SourceKind::Button => command_bar(binds, input, CommandSlot::Button, input_label(input), None),
        SourceKind::ButtonGroup => group_view_input(binds, input),
        kind => rich_view(binds, input, kind),
    }
}

/// A 4-button cluster (Face Buttons / D-Pad): a behaviour selector, and — when it's a Button Pad —
/// the four member command bars. Face Buttons carry the Xbox glyph colours.
fn group_view_input(binds: Binds, input: &InputSource) -> Element<'static, Message> {
    let binding = binds.and_then(|b| b.get(input));
    let current = binding.map_or(Behavior::Unbound, Behavior::of);
    let mut col = column![behavior_row(input, current, SourceKind::ButtonGroup)].spacing(8.0);
    if matches!(binding, Some(SourceBinding::ButtonPad { .. })) {
        for (slot, label, dot) in group_members(input) {
            col = col.push(command_bar(binds, input, slot, label, dot));
        }
    }
    col.into()
}

/// A rich analog source (Pad/Stick/Trigger/Gyro): a behaviour selector, then the virtual-button
/// command bars its chosen behaviour exposes (none for the mouse behaviours).
fn rich_view(binds: Binds, input: &InputSource, kind: SourceKind) -> Element<'static, Message> {
    let binding = binds.and_then(|b| b.get(input));
    let current = binding.map_or(Behavior::Unbound, Behavior::of);
    let mut col = column![behavior_row(input, current, kind)].spacing(8.0);
    if let Some(b) = binding {
        for (slot, label) in virtual_buttons(b) {
            col = col.push(command_bar(binds, input, slot, label, None));
        }
    }
    col.into()
}

/// The command slots (virtual buttons) a rich behaviour exposes, with display labels.
fn virtual_buttons(binding: &SourceBinding) -> Vec<(CommandSlot, &'static str)> {
    use CommandSlot::*;
    match binding {
        SourceBinding::Joystick { .. } => vec![(OuterRing, "Outer Ring")],
        SourceBinding::DirectionalPad { .. } => {
            vec![(Up, "Up"), (Down, "Down"), (Left, "Left"), (Right, "Right"), (OuterRing, "Outer Ring")]
        }
        SourceBinding::Trigger { .. } => vec![(SoftPull, "Soft Pull")],
        _ => Vec::new(),
    }
}

/// The four members of a button cluster, mapped to their `ButtonPad` slot (diamond positions:
/// up=top, down=bottom, left/right=sides) with per-input labels and glyph colours.
type Dot = Option<fn(&Theme) -> text::Style>;
fn group_members(input: &InputSource) -> Vec<(CommandSlot, &'static str, Dot)> {
    use CommandSlot::*;
    match input {
        InputSource::FaceButtons => vec![
            (Down, "A Button", Some(style::success_text)),
            (Right, "B Button", Some(style::danger_text)),
            (Left, "X Button", Some(style::primary_text)),
            (Up, "Y Button", Some(style::warning_text)),
        ],
        InputSource::DPad => {
            vec![(Up, "Up", None), (Down, "Down", None), (Left, "Left", None), (Right, "Right", None)]
        }
        _ => Vec::new(),
    }
}

/// A group's "Behavior" selector: the [`Behavior`] set valid for the source kind, current value
/// reflected; selecting one rebuilds the binding from authoring defaults (or clears it via `None`).
/// The gear (behaviour settings) is a later pass.
fn behavior_row(input: &InputSource, current: Behavior, kind: SourceKind) -> Element<'static, Message> {
    let options = Behavior::valid_for(kind).to_vec();
    let input = input.clone();
    let combo = pick_list(Some(current), options, |b: &Behavior| b.label().to_string())
        .on_select(move |b| Message::Editor(EditorMessage::SetBehavior(input.clone(), b)))
        .width(CMD_SLOT);
    let inner = row![text("Behavior"), Space::new().width(Fill), combo, gear(true)]
        .spacing(12.0)
        .align_y(Center);
    card(inner)
}

/// One command bar: an optional colour dot, the slot's name, then the command button (shows the
/// bound action or `<unbound>`) and the gear. Clicking the button always opens the Action picker
/// (create or change the action); the gear is active only once a command exists (its function —
/// settings / unbind — is a later pass, so it's an inert placeholder for now).
fn command_bar(
    binds: Binds,
    input: &InputSource,
    slot: CommandSlot,
    label: &'static str,
    dot: Dot,
) -> Element<'static, Message> {
    let action = binds
        .and_then(|b| b.get(input))
        .and_then(|binding| crate::editor::slot_commands(binding, slot))
        .and_then(|cmds| cmds.first())
        .and_then(|cmd| cmd.actions.first());
    let bound = action.is_some();
    let btn_label = action.map(action_label).unwrap_or_else(|| "<unbound>".to_string());

    let dest = CommandDest { input: input.clone(), slot };
    let command_btn = button(text(btn_label).center())
        .width(CMD_SLOT)
        .style(style::combo_button)
        .on_press(Message::Editor(EditorMessage::OpenActionPicker(dest)));

    let mut r = row![].spacing(12.0).align_y(Center);
    if let Some(role) = dot {
        r = r.push(text("●").size(16.0).style(role));
    }
    let inner = r.push(text(label)).push(Space::new().width(Fill)).push(command_btn).push(gear(bound));
    card(inner)
}

/// A settings/gear button, coloured to match the comboboxes on the same cards. `active` toggles
/// whether it's clickable (a bound command) or greyed (an unbound one).
fn gear(active: bool) -> Element<'static, Message> {
    let b = button(text("⚙").size(16.0)).style(style::combo_button);
    if active { b.on_press(Message::Ignored).into() } else { b.into() }
}

/// A human-readable label for an input, for the mock rows and the button picker.
pub(crate) fn input_label(input: &InputSource) -> &'static str {
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
