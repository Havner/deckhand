//! Modals - the one home for everything that layers over the base window as a dismissable overlay.
//!
//! Owns the modal **shell** ([`overlay`]/`shell`), the [`Popup`] state enum, and every popup's
//! card: the network I/O dialog, the Profile-page context menu, the name-entry dialog, and the two
//! big pickers ([`action`] = choose an output/mode [`Action`](config::Action); [`button`] = choose a
//! `SourceKind::Button` [`InputSource`](config::InputSource) for gaters/chords). Only one popup is
//! shown at a time. Rendering lives here; the *semantic* types it produces (`EditTarget`,
//! `NameEntryKind`, the picked values) stay in [`crate::editor`].

mod action;
mod buttons;

use iced::widget::{
    Space, button, center, column, container, mouse_area, opaque, pick_list, row, stack, text,
    text_input,
};
use iced::{Center, Element, Fill};

pub(in crate::view) use action::action_label;

use crate::editor::{
    ActionTarget, ActivatorKind, CommandRef, CommandSlot, EditTarget, EditorMessage, NameEntryKind,
};
use crate::{App, IoTarget, Message, style};
use config::{InputSource, SourceBinding};

/// The one modal shown at a time (the view layers exactly one over the base): network I/O staging,
/// the profile editor's context menu / name dialogs, and the two pickers.
#[derive(Debug, Clone)]
pub(crate) enum Popup {
    /// Network input/output staging: which selector it targets + the current `host:port` text.
    Network { target: IoTarget, text: String },
    /// A Profile-page set/layer context menu, opened by that bar's gear.
    Menu(EditTarget),
    /// The add-set / add-layer / rename name dialog: what confirming does + the current text.
    NameEntry { kind: NameEntryKind, text: String },
    /// The output-Action picker (tabbed), currently on this tab, writing to `target` on confirm.
    ActionPicker { tab: ActionTab, target: ActionTarget },
    /// A command's gear menu (activator / settings / remove / add).
    CommandMenu { cmd: CommandRef },
    /// The top slot bar's gear menu (multi-command: remove all / add extra).
    SlotMenu { input: InputSource, slot: CommandSlot },
    /// A layer Button's inherited/disabled gear menu (Disable, or Remove the `None`).
    LayerButtonMenu { input: InputSource },
    /// The button (gater/chord) picker; `target` is where the picked button lands.
    ButtonPicker { target: crate::ButtonTarget },
}

/// The tabs of the Action picker - one output/mode category each (mirrors the sidebar categories a
/// binding's action can target). Steam's SYSTEM/CAMERA are dropped (no vocab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionTab {
    Gamepad,
    Mouse,
    Keyboard,
    Numpad,
    ActionSets,
}

/// Layer the open [`Popup`] (if any) over `base` as a centered, dismissable overlay.
pub(super) fn overlay<'a>(app: &'a App, base: Element<'a, Message>) -> Element<'a, Message> {
    match &app.popup {
        Some(popup) => shell(base, card(app, popup), Message::PopupCancel),
        // Wrap even the no-popup case in a `stack!` so the root widget type stays `Stack`
        // whether or not a modal is open. iced matches widget state (incl. a scrollable's
        // offset) by position + widget type; if the root flipped Column<->Stack when a modal
        // opened, the whole tree would rebuild and every scrollable would snap back to the top.
        // Keeping `base` as stack child 0 in both states preserves scroll position.
        None => stack![base].into(),
    }
}

/// Layer `content` over `base`: a dimmed, input-blocking backdrop (click it -> `on_dismiss`) with
/// `content` centered on top. Content-agnostic.
fn shell<'a>(
    base: Element<'a, Message>,
    content: Element<'a, Message>,
    on_dismiss: Message,
) -> Element<'a, Message> {
    stack![
        base,
        opaque(mouse_area(center(opaque(content)).style(style::scrim)).on_press(on_dismiss))
    ]
    .into()
}

/// Render the open modal's card.
fn card<'a>(app: &'a App, popup: &'a Popup) -> Element<'a, Message> {
    match popup {
        Popup::Network { target, text } => network_card(*target, text),
        Popup::Menu(target) => menu_card(app, target),
        Popup::NameEntry { kind, text } => name_entry_card(app, kind, text),
        Popup::ActionPicker { tab, .. } => action::card(app, *tab),
        Popup::CommandMenu { cmd } => command_menu_card(app, cmd),
        Popup::SlotMenu { input, slot } => slot_menu_card(app, *slot, input),
        Popup::LayerButtonMenu { input } => layer_button_menu_card(app, input),
        Popup::ButtonPicker { .. } => buttons::card(),
    }
}

/// A full-width menu row: an active combo-styled button, or a greyed one when `msg` is `None`.
fn menu_item<'a>(label: &'a str, msg: Option<Message>) -> Element<'a, Message> {
    let b = button(text(label)).width(Fill).style(style::combo_button);
    match msg {
        Some(m) => b.on_press(m),
        None => b,
    }
    .into()
}

/// The shared text-entry dialog shell: a title, a single-line field (`id` for focus, Enter
/// confirms), and a Cancel / OK row - OK and Enter both inert while `confirm` is `None`. Both the
/// network-spec and name-entry dialogs are this shape; only their title/placeholder/id/message and
/// validity rule differ.
fn text_entry_card<'a>(
    title: &'a str,
    id: &'static str,
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
    confirm: Option<Message>,
) -> Element<'a, Message> {
    let field = text_input(placeholder, value)
        .id(id)
        .on_input(on_input)
        .on_submit_maybe(confirm.clone())
        .padding(6.0);
    let buttons = row![
        button(text("Cancel")).style(button::danger).on_press(Message::PopupCancel),
        Space::new().width(Fill),
        button(text("OK")).style(button::success).on_press_maybe(confirm),
    ]
    .align_y(Center);

    let card = column![text(title).size(18.0), field, buttons].spacing(12.0);
    container(card).padding(16.0).width(320.0).style(style::modal_card).into()
}

/// The network-spec card: a `host:port` field (Enter confirms) + Cancel / OK. OK/Enter are inert
/// while the field is empty (an empty spec is invalid).
fn network_card(target: IoTarget, spec: &str) -> Element<'_, Message> {
    let title = match target {
        IoTarget::Input => "Network input",
        IoTarget::Output => "Network output",
    };
    let confirm = (!spec.trim().is_empty()).then_some(Message::PopupConfirm);
    text_entry_card(
        title,
        crate::NETWORK_FIELD_ID,
        "host:port",
        spec,
        Message::PopupTextChanged,
        confirm,
    )
}

/// A Profile-page set/layer context menu: a column of actions for the gear's target. Sets offer
/// Rename / Remove (disabled when it's the only set) / Add layer; layers offer Rename / Remove.
fn menu_card<'a>(app: &'a App, target: &'a EditTarget) -> Element<'a, Message> {
    let is_set = target.layer.is_none();
    let title = target.layer.as_deref().unwrap_or(target.set.as_str());

    // Remove at the top (disabled for the last remaining set - at least one must exist).
    let remove_msg = if is_set {
        app.editing
            .as_ref()
            .is_some_and(|e| e.doc.action_sets.len() > 1)
            .then_some(Message::Editor(EditorMessage::MenuRemove))
    } else {
        Some(Message::Editor(EditorMessage::MenuRemove))
    };
    let mut col = column![text(title).size(16.0)].spacing(8.0);
    col = col.push(menu_item("Remove", remove_msg));
    col = col.push(menu_item("Rename", Some(Message::Editor(EditorMessage::MenuRename))));
    if is_set {
        col = col.push(menu_item("Add layer", Some(Message::Editor(EditorMessage::MenuAddLayer))));
    }
    container(col).padding(12.0).width(220.0).style(style::modal_card).into()
}

/// The add-set / add-layer / rename name dialog. OK/Enter are inert unless the name is valid
/// (non-empty and unique in scope - see [`crate::editor::name_entry_ok`]).
fn name_entry_card<'a>(
    app: &'a App,
    kind: &'a NameEntryKind,
    value: &'a str,
) -> Element<'a, Message> {
    let ok = app.editing.as_ref().is_some_and(|e| crate::editor::name_entry_ok(&e.doc, kind, value));
    let confirm = ok.then_some(Message::Editor(EditorMessage::DialogConfirm));
    text_entry_card(
        kind.title(),
        crate::NAME_FIELD_ID,
        "name",
        value,
        |s| Message::Editor(EditorMessage::DialogTextChanged(s)),
        confirm,
    )
}

/// A command's gear menu: its label title, an activator combobox, a (disabled) Settings row, Remove
/// command, Add extra command (only when it's the sole command), and Add sub command. The title is
/// the bar's own label (via [`super::slot_display`]) when it's the sole command, else "Command".
fn command_menu_card<'a>(app: &'a App, cmd: &'a CommandRef) -> Element<'a, Message> {
    let ed = crate::editor::EditorMessage::SetActivator;
    let current = crate::editor::command_at(app, cmd).map(|c| ActivatorKind::of(&c.activator));
    let sole = crate::editor::slot_len(app, &cmd.input, cmd.slot) == 1;
    let (label, dot) =
        if sole { super::slot_display(&cmd.input, cmd.slot) } else { ("Command", None) };

    let cmd_for_select = cmd.clone();
    let activator = pick_list(current, ActivatorKind::ALL.to_vec(), |k: &ActivatorKind| {
        k.label().to_string()
    })
    .menu_style(style::combo_menu)
    .on_select(move |k| Message::Editor(ed(cmd_for_select.clone(), k)))
    .width(Fill);

    let mut col = column![super::label_row(label, dot)].spacing(8.0);
    // Disable (sets an explicit `None`) sits at the top - layer + top-level Button only; a sole
    // command bar *is* the top-level bar, so it belongs here (multi-command puts it on the slot menu).
    if crate::editor::on_layer(app) && cmd.slot == CommandSlot::Button && sole {
        col = col.push(disable_item(&cmd.input));
    }
    col = col.push(menu_item(
        "Remove",
        Some(Message::Editor(EditorMessage::RemoveCommand(cmd.clone()))),
    ));
    col = col.push(activator);
    col = col.push(menu_item(
        "Settings",
        Some(Message::Editor(EditorMessage::OpenCommandSettings(cmd.clone()))),
    ));
    if sole {
        col = col.push(menu_item(
            "Add extra command",
            Some(Message::Editor(EditorMessage::OpenActionPicker(ActionTarget::AddCommand {
                input: cmd.input.clone(),
                slot: cmd.slot,
            }))),
        ));
    }
    col = col.push(menu_item(
        "Add sub command",
        Some(Message::Editor(EditorMessage::OpenActionPicker(ActionTarget::AddSubCommand {
            cmd: cmd.clone(),
        }))),
    ));
    container(col).padding(12.0).width(240.0).style(style::modal_card).into()
}

/// The top slot bar's gear menu (multi-command): its label title, an optional Disable (layer +
/// top-level Button), Remove all, and Add extra command.
fn slot_menu_card<'a>(app: &'a App, slot: CommandSlot, input: &'a InputSource) -> Element<'a, Message> {
    let (label, dot) = super::slot_display(input, slot);
    let mut col = column![super::label_row(label, dot)].spacing(8.0);
    if crate::editor::on_layer(app) && slot == CommandSlot::Button {
        col = col.push(disable_item(input));
    }
    col = col.push(menu_item(
        "Remove all",
        Some(Message::Editor(EditorMessage::RemoveAllCommands(input.clone(), slot))),
    ));
    col = col.push(menu_item(
        "Add extra command",
        Some(Message::Editor(EditorMessage::OpenActionPicker(ActionTarget::AddCommand {
            input: input.clone(),
            slot,
        }))),
    ));
    container(col).padding(12.0).width(220.0).style(style::modal_card).into()
}

/// The inherited/disabled gear menu for a layer's top-level Button: **Disable** when it's currently
/// inherited (no entry), else **Remove** (drops the explicit `None` -> back to inherited).
fn layer_button_menu_card<'a>(app: &'a App, input: &'a InputSource) -> Element<'a, Message> {
    let (label, _) = super::slot_display(input, CommandSlot::Button);
    let disabled =
        matches!(crate::editor::current_bindings(app).and_then(|b| b.get(input)), Some(SourceBinding::None));
    let action = if disabled {
        menu_item(
            "Remove",
            Some(Message::Editor(EditorMessage::RemoveAllCommands(input.clone(), CommandSlot::Button))),
        )
    } else {
        disable_item(input)
    };
    container(column![super::label_row(label, None), action].spacing(8.0))
        .padding(12.0)
        .width(220.0)
        .style(style::modal_card)
        .into()
}

/// The shared "Disable" menu row -> sets an explicit `SourceBinding::None` (via `SetBehavior`).
fn disable_item(input: &InputSource) -> Element<'static, Message> {
    menu_item(
        "Disable",
        Some(Message::Editor(EditorMessage::SetBehavior(input.clone(), crate::editor::Behavior::Disabled))),
    )
}
