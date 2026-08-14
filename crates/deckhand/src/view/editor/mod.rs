//! The profile-editor screens — the pages reachable once a profile is loaded for editing.
//!
//! The **Profile** page ([`profile_screen`]) and the per-input pages ([`input_screen`]) are wired:
//! they render from, and mutate, the loaded profile's [`ConfigDoc`](config::ConfigDoc). Input pages
//! are data-driven from [`Category::groups`] (headers, behaviour selectors, per-slot command
//! bars with their gear menus, subcommands). Only [`rumble_screen`] is still a mockup.

use std::collections::BTreeMap;

use iced::widget::{Space, button, column, container, pick_list, row, slider, text, text_input};
use iced::{Center, Element, Fill};

use config::{Action, InputSource, SourceBinding, SourceKind};

use super::{Dot, card, group_header, label_row, section_header, slot_display, small};
use crate::editor::{ActionTarget, Behavior, CommandRef, CommandSlot, EditorMessage};
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
    let on_layer = crate::editor::on_layer(app);
    let mut col = column![section_header(category.label())].spacing(20.0);
    for group in category.groups() {
        col = col.push(group_view(binds, group, on_layer));
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
fn group_view(binds: Binds, group: &InputGroup, on_layer: bool) -> Element<'static, Message> {
    let mut col = column![group_header(group.header)].spacing(8.0);
    for input in group.primary {
        col = col.push(primary_view(binds, input, on_layer));
    }
    if !group.sub.is_empty() {
        col = col.push(Space::new().height(4.0));
        for input in group.sub {
            col = col.push(slot_view(binds, input, CommandSlot::Button, on_layer));
        }
    }
    col.into()
}

/// A primary input: a plain button is one command bar; a button group / rich analog source gets a
/// behaviour selector plus the command bars its chosen behaviour exposes.
fn primary_view(binds: Binds, input: &InputSource, on_layer: bool) -> Element<'static, Message> {
    match input.kind() {
        SourceKind::Button => slot_view(binds, input, CommandSlot::Button, on_layer),
        SourceKind::ButtonGroup => group_view_input(binds, input, on_layer),
        kind => rich_view(binds, input, kind, on_layer),
    }
}

/// A 4-button cluster (Face Buttons / D-Pad): a behaviour selector, and — when it's a Button Pad —
/// the four member command bars.
fn group_view_input(binds: Binds, input: &InputSource, on_layer: bool) -> Element<'static, Message> {
    let binding = binds.and_then(|b| b.get(input));
    let current = binding.map_or(Behavior::Unbound, Behavior::of);
    let mut col = column![behavior_row(input, current, SourceKind::ButtonGroup, on_layer)].spacing(8.0);
    if matches!(binding, Some(SourceBinding::ButtonPad { .. })) {
        for slot in group_member_slots(input) {
            col = col.push(slot_view(binds, input, slot, on_layer));
        }
    }
    col.into()
}

/// A rich analog source (Pad/Stick/Trigger/Gyro): a behaviour selector, then the virtual-button
/// command bars its chosen behaviour exposes (none for the mouse behaviours).
fn rich_view(binds: Binds, input: &InputSource, kind: SourceKind, on_layer: bool) -> Element<'static, Message> {
    let binding = binds.and_then(|b| b.get(input));
    let current = binding.map_or(Behavior::Unbound, Behavior::of);
    let mut col = column![behavior_row(input, current, kind, on_layer)].spacing(8.0);
    if let Some(b) = binding {
        for slot in virtual_slots(b) {
            col = col.push(slot_view(binds, input, slot, on_layer));
        }
    }
    col.into()
}

/// The command slots (virtual buttons) a rich behaviour exposes, in display order (labels come from
/// [`slot_display`]).
fn virtual_slots(binding: &SourceBinding) -> Vec<CommandSlot> {
    use CommandSlot::*;
    match binding {
        SourceBinding::Joystick { .. } => vec![OuterRing],
        SourceBinding::DirectionalPad { .. } => vec![Up, Down, Left, Right, OuterRing],
        SourceBinding::Trigger { .. } => vec![SoftPull],
        _ => Vec::new(),
    }
}

/// The members of a button cluster, as their `ButtonPad` slots in display order (A, B, X, Y for Face
/// Buttons; Up/Down/Left/Right for the D-pad). Labels/colours come from [`slot_display`].
fn group_member_slots(input: &InputSource) -> Vec<CommandSlot> {
    use CommandSlot::*;
    match input {
        InputSource::FaceButtons => vec![Down, Right, Left, Up],
        InputSource::DPad => vec![Up, Down, Left, Right],
        _ => Vec::new(),
    }
}

/// A group's "Behavior" selector: the [`Behavior`] set valid for the source kind, current value
/// reflected; selecting one rebuilds the binding from authoring defaults (or clears it via `None`).
/// The gear (behaviour settings) is a later pass.
fn behavior_row(input: &InputSource, current: Behavior, kind: SourceKind, on_layer: bool) -> Element<'static, Message> {
    let options = Behavior::valid_for(kind, on_layer);
    let input = input.clone();
    // On a layer the empty `Unbound` choice reads as "Inherited"; otherwise its own label ("None").
    let combo = pick_list(Some(current), options, move |b: &Behavior| {
        if on_layer && *b == Behavior::Unbound { "Inherited".to_string() } else { b.label().to_string() }
    })
    .on_select(move |b| Message::Editor(EditorMessage::SetBehavior(input.clone(), b)))
    .width(CMD_SLOT);
    // The "Behavior" label dims when this layer entry is inherited (the passthrough state).
    let inherited = on_layer && current == Behavior::Unbound;
    let name = if inherited { text("Behavior").style(style::muted_text) } else { text("Behavior") };
    // The gear (→ behaviour settings, later) is inert unless there's a real behaviour to configure.
    let inner = row![name, Space::new().width(Fill), combo, gear(current.is_real())]
        .spacing(12.0)
        .align_y(Center);
    card(inner)
}

/// A slot's whole view: `<unbound>` when empty; a single command bar (+ its subcommands) when it
/// holds one command; or a top bar + one command bar per command (+ their subcommands) when it holds
/// several. Each command bar carries its main action (`actions[0]`) and a gear menu; subcommands
/// (`actions[1..]`) are double-indented remove-able bars.
fn slot_view(binds: Binds, input: &InputSource, slot: CommandSlot, on_layer: bool) -> Element<'static, Message> {
    let (label, dot) = slot_display(input, slot);
    let entry = binds.and_then(|b| b.get(input));

    // On a layer, a top-level Button's "empty" state splits into Inherited (no entry) / Disabled
    // (explicit `None`) — each a special bar (see `layer_button_bar`); a real Button binding falls
    // through to the normal command layout below.
    if on_layer && slot == CommandSlot::Button {
        match entry {
            None => return layer_button_bar(input, label, false),
            Some(SourceBinding::None) => return layer_button_bar(input, label, true),
            _ => {}
        }
    }

    let commands = entry.and_then(|bind| crate::editor::slot_commands(bind, slot)).map(Vec::as_slice);
    match commands {
        None | Some([]) => unbound_bar(input, slot, label, dot),
        Some([cmd]) => command_block(input, slot, 0, cmd, label, dot, 0.0),
        Some(cmds) => {
            let mut col = column![main_slot_bar(input, slot, label, dot)].spacing(8.0);
            for (i, cmd) in cmds.iter().enumerate() {
                col = col.push(command_block(input, slot, i, cmd, "Command", None, 1.0));
            }
            col.into()
        }
    }
}

/// A layer top-level Button in its inherited (`<inherited>`, muted label) or disabled (`<disabled>`,
/// normal label) state. The action button is always clickable — clicking adds a command (→ a real
/// binding). The gear opens the inherited/disabled menu (Disable, or Remove the `None`).
fn layer_button_bar(input: &InputSource, label: &'static str, disabled: bool) -> Element<'static, Message> {
    let target = ActionTarget::AddCommand { input: input.clone(), slot: CommandSlot::Button };
    let text_str = if disabled { "<disabled>" } else { "<inherited>" };
    let btn = button(text(text_str).center())
        .width(CMD_SLOT)
        .style(style::combo_button)
        .on_press(Message::Editor(EditorMessage::OpenActionPicker(target)));
    // Only the left label dims (and only for inherited — the passthrough state).
    let name = if disabled { text(label) } else { text(label).style(style::muted_text) };
    let gear = gear_menu(Message::Editor(EditorMessage::OpenLayerButtonMenu(input.clone())));
    card(row![name, Space::new().width(Fill), btn, gear].spacing(12.0).align_y(Center))
}

/// Indent an element by `level` steps (0 = none).
fn indent(level: f32, el: Element<'static, Message>) -> Element<'static, Message> {
    if level == 0.0 {
        el
    } else {
        row![Space::new().width(24.0 * level), el].into()
    }
}

/// An unbound slot: the `<unbound>` button (opens the picker to add the first command) + inert gear.
fn unbound_bar(input: &InputSource, slot: CommandSlot, label: &'static str, dot: Dot) -> Element<'static, Message> {
    let target = ActionTarget::AddCommand { input: input.clone(), slot };
    let btn = button(text("<unbound>").center())
        .width(CMD_SLOT)
        .style(style::combo_button)
        .on_press(Message::Editor(EditorMessage::OpenActionPicker(target)));
    card(label_row(label, dot).push(Space::new().width(Fill)).push(btn).push(gear(false)))
}

/// The top bar of a multi-command slot: label + gear (Remove all / Add extra), no action button.
fn main_slot_bar(input: &InputSource, slot: CommandSlot, label: &'static str, dot: Dot) -> Element<'static, Message> {
    let msg = Message::Editor(EditorMessage::OpenSlotMenu(input.clone(), slot));
    card(label_row(label, dot).push(Space::new().width(Fill)).push(gear_menu(msg)))
}

/// A single command bar at `level` (its main action + gear menu) plus its subcommand bars below.
/// `base` is the bar's name ("Command", or the slot label when it's the sole command); `dot` colours
/// the sole-command label.
fn command_block(
    input: &InputSource,
    slot: CommandSlot,
    index: usize,
    cmd: &config::Command,
    base: &'static str,
    dot: Dot,
    level: f32,
) -> Element<'static, Message> {
    let cref = CommandRef { input: input.clone(), slot, index };

    // Label: base (+ a blue "(activator)" suffix for non-Regular).
    let mut label = label_row(base, dot);
    if let Some(suffix) = activator_suffix(&cmd.activator) {
        label = label.push(text(format!("({suffix})")).style(style::primary_text));
    }

    let action_btn = action_button(
        cmd.actions.first(),
        ActionTarget::Replace { cmd: cref.clone(), action: 0 },
    );
    let menu = Message::Editor(EditorMessage::OpenCommandMenu(cref.clone()));
    let bar = card(label.push(Space::new().width(Fill)).push(action_btn).push(gear_menu(menu)));

    let mut col = column![indent(level, bar)].spacing(8.0);
    // Subcommands (actions[1..]) — always double-indented, with a remove button instead of a gear.
    for (ai, action) in cmd.actions.iter().enumerate().skip(1) {
        col = col.push(indent(2.0, subcommand_bar(&cref, ai, action)));
    }
    col.into()
}

/// A subcommand bar: "Sub command", its (re-pickable) action, and a remove button — no gear/menu.
fn subcommand_bar(cref: &CommandRef, action_idx: usize, action: &Action) -> Element<'static, Message> {
    let action_btn =
        action_button(Some(action), ActionTarget::Replace { cmd: cref.clone(), action: action_idx });
    let remove = button(text("✕").size(15.0))
        .style(style::combo_button)
        .on_press(Message::Editor(EditorMessage::RemoveSubCommand(cref.clone(), action_idx)));
    card(row![text("Sub command"), Space::new().width(Fill), action_btn, remove]
        .spacing(12.0)
        .align_y(Center))
}

/// The command-slot-width button that shows an action (or `<unbound>`) and opens the picker for it.
fn action_button(action: Option<&Action>, target: ActionTarget) -> Element<'static, Message> {
    let label = action.map(action_label).unwrap_or_else(|| "<unbound>".to_string());
    button(text(label).center())
        .width(CMD_SLOT)
        .style(style::combo_button)
        .on_press(Message::Editor(EditorMessage::OpenActionPicker(target)))
        .into()
}

/// The blue "(…)" suffix shown after a command's label for any non-Regular activator.
fn activator_suffix(a: &config::Activator) -> Option<String> {
    use config::Activator::*;
    match a {
        Regular => None,
        Start => Some("Start press".into()),
        Long { hold_ms } => Some(format!("Long press: {hold_ms}ms")),
        Double { window_ms } => Some(format!("Double press: {window_ms}ms")),
        Release => Some("On release".into()),
    }
}

/// An active gear button that opens a menu on press.
fn gear_menu(msg: Message) -> Element<'static, Message> {
    button(text("⚙").size(16.0)).style(style::combo_button).on_press(msg).into()
}

/// A settings/gear button. `active=false` greys it (an unbound slot has no menu yet).
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
