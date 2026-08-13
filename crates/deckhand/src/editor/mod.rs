//! The profile editor's **state + update** — everything the editor pages read and mutate, kept out
//! of the app's top-level `update` so the (soon large) editor logic lives in one place. The widgets
//! for these live in [`crate::view::editor`]; the lifecycle brackets that create/destroy the editor
//! state (`EditProfile` / `StopEditing`) stay App-level.
//!
//! [`update`] is the **single place a loaded profile's document is mutated**, so it is the natural
//! hook for a future undo stack: snapshot [`Editing::doc`] here before applying an edit.

use std::path::PathBuf;

use config::{Action, ActionSet, ConfigDoc, Layer};
use iced::Task;
use ipc::ProfileRole;

use crate::{ActionTab, App, Message, NAME_FIELD_ID, Popup};

pub(crate) mod authoring;
pub(crate) use authoring::Behavior;

/// A profile loaded into the editor: the file it came from (edits save back here), the parsed
/// document, and which action set / layer the input pages currently target. Its presence is the
/// UI's central macro-state (see [`App::editing`]).
pub(crate) struct Editing {
    pub(crate) path: PathBuf,
    pub(crate) doc: ConfigDoc,
    /// Which action set / layer the per-input editor pages currently target (the sidebar selector).
    pub(crate) target: EditTarget,
}

/// What the editor is pointed at: an action set (`layer: None` — its base bindings) or one of that
/// set's layers. Addressed **by name** — set names are unique among sets and layer names unique
/// among a set's own layers (both enforced by the editor), so `(set, Option<layer>)` is a stable
/// key that survives reorder/removal (an index would silently shift). Doubles as the identity a
/// Profile-page bar's gear acts on. The sidebar's ◀/▶ selector walks these in [`edit_target_list`]
/// order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditTarget {
    pub(crate) set: String,
    pub(crate) layer: Option<String>,
}

/// The initial target for a freshly-loaded profile: its first action set (no layer). A profile
/// always has at least one set; the empty fallback only bites a malformed/empty doc.
pub(crate) fn first_target(doc: &ConfigDoc) -> EditTarget {
    EditTarget { set: doc.action_sets.first().map(|s| s.name.clone()).unwrap_or_default(), layer: None }
}

/// The flat, ordered list of edit targets for a profile: each action set followed by its own layers
/// (a set with no layers contributes just itself). The sequence the sidebar selector steps through.
pub(crate) fn edit_target_list(doc: &ConfigDoc) -> Vec<EditTarget> {
    let mut targets = Vec::new();
    for set in &doc.action_sets {
        targets.push(EditTarget { set: set.name.clone(), layer: None });
        for layer in &set.layers {
            targets.push(EditTarget { set: set.name.clone(), layer: Some(layer.name.clone()) });
        }
    }
    targets
}

/// What confirming a name-entry dialog does, carrying the target it operates on. The dialog is the
/// one modal reused by add-set / add-layer / rename (see [`crate::Popup::NameEntry`]).
#[derive(Debug, Clone)]
pub enum NameEntryKind {
    /// Create a new action set with the entered name.
    NewSet,
    /// Create a new layer (entered name) under this action set.
    NewLayer { set: String },
    /// Rename this action set to the entered name.
    RenameSet { set: String },
    /// Rename this layer (of `set`) to the entered name.
    RenameLayer { set: String, layer: String },
}

impl NameEntryKind {
    /// The dialog's title.
    pub(crate) fn title(&self) -> &'static str {
        match self {
            NameEntryKind::NewSet => "New action set",
            NameEntryKind::NewLayer { .. } => "New layer",
            NameEntryKind::RenameSet { .. } => "Rename action set",
            NameEntryKind::RenameLayer { .. } => "Rename layer",
        }
    }
}

/// Editor-scoped messages, nested under [`Message::Editor`]. Everything that reads or mutates the
/// loaded profile flows through here (the top-level `Message` stays about the app/daemon). Grows as
/// the editor does.
#[derive(Debug, Clone)]
pub enum EditorMessage {
    /// Rename the loaded profile (Profile page name field).
    NameChanged(String),
    /// Send the in-memory edited profile to a daemon role (top-bar Set as Main/Fallback).
    SendToRole(ProfileRole),
    /// Sidebar action-set/layer selector: step to the previous / next edit target.
    TargetPrev,
    TargetNext,
    /// Profile page: open a set/layer's gear context menu (that bar's own [`EditTarget`]).
    OpenMenu(EditTarget),
    /// Context-menu actions, acting on the menu's target (read from the open [`Popup::Menu`]).
    MenuRename,
    MenuRemove,
    MenuAddLayer,
    /// "Add action set" button: open the new-set name dialog.
    AddSet,
    /// Name-entry dialog: the text field changed / confirmed (Enter or OK).
    DialogTextChanged(String),
    DialogConfirm,
    /// Open the output-Action picker (from an input page's `<unbound>` bar).
    OpenActionPicker,
    /// Switch the Action picker's tab.
    ActionPickerTab(ActionTab),
    /// An action was picked (debug-wired: printed, not yet stored — the input pages don't hold
    /// bindings yet). Closes the picker.
    ActionPicked(Action),
}

/// Handle one editor message against the app state. The **single doc-mutation site** — the place to
/// snapshot for undo later.
pub(crate) fn update(app: &mut App, msg: EditorMessage) -> Task<Message> {
    match msg {
        EditorMessage::NameChanged(name) => {
            if let Some(ed) = &mut app.editing {
                ed.doc.name = name;
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SendToRole(role) => {
            let Some(ed) = &app.editing else { return Task::none() };
            let doc = ed.doc.clone();
            app.cmd_task(move |c| c.apply(role, Some(Box::new(doc))))
        }
        EditorMessage::TargetPrev => {
            step_target(app, -1);
            Task::none()
        }
        EditorMessage::TargetNext => {
            step_target(app, 1);
            Task::none()
        }
        EditorMessage::OpenMenu(target) => {
            app.popup = Some(Popup::Menu(target));
            Task::none()
        }
        EditorMessage::MenuRename => {
            // Turn the open context menu into a rename dialog seeded with the target's current name.
            let Some(Popup::Menu(target)) = &app.popup else { return Task::none() };
            let (kind, text) = match &target.layer {
                None => (NameEntryKind::RenameSet { set: target.set.clone() }, target.set.clone()),
                Some(layer) => (
                    NameEntryKind::RenameLayer { set: target.set.clone(), layer: layer.clone() },
                    layer.clone(),
                ),
            };
            app.popup = Some(Popup::NameEntry { kind, text });
            iced::widget::operation::focus(NAME_FIELD_ID)
        }
        EditorMessage::MenuAddLayer => {
            let Some(Popup::Menu(target)) = &app.popup else { return Task::none() };
            let kind = NameEntryKind::NewLayer { set: target.set.clone() };
            app.popup = Some(Popup::NameEntry { kind, text: String::new() });
            iced::widget::operation::focus(NAME_FIELD_ID)
        }
        EditorMessage::MenuRemove => {
            let Some(Popup::Menu(target)) = app.popup.take() else { return Task::none() };
            remove_target(app, &target);
            app.save_editing();
            Task::none()
        }
        EditorMessage::AddSet => {
            app.popup = Some(Popup::NameEntry { kind: NameEntryKind::NewSet, text: String::new() });
            iced::widget::operation::focus(NAME_FIELD_ID)
        }
        EditorMessage::DialogTextChanged(text) => {
            if let Some(Popup::NameEntry { text: t, .. }) = &mut app.popup {
                *t = text;
            }
            Task::none()
        }
        EditorMessage::DialogConfirm => {
            // OK/Enter are inert unless the name is valid (see `name_entry_ok`), so this is reachable
            // only for a valid name; re-check anyway before mutating.
            let Some(Popup::NameEntry { kind, text }) = app.popup.take() else { return Task::none() };
            let Some(ed) = &app.editing else { return Task::none() };
            if !name_entry_ok(&ed.doc, &kind, &text) {
                return Task::none();
            }
            apply_name_entry(app, kind, text.trim().to_string());
            app.save_editing();
            Task::none()
        }
        EditorMessage::OpenActionPicker => {
            app.popup = Some(Popup::ActionPicker { tab: ActionTab::Gamepad });
            Task::none()
        }
        EditorMessage::ActionPickerTab(tab) => {
            if let Some(Popup::ActionPicker { tab: current }) = &mut app.popup {
                *current = tab;
            }
            Task::none()
        }
        EditorMessage::ActionPicked(action) => {
            // Debug wiring: the input pages don't store bindings yet, so just report the pick.
            println!("[action picker] picked: {action:?}");
            app.popup = None;
            Task::none()
        }
    }
}

/// Move the editor's target by `delta` steps through [`edit_target_list`] (−1 = previous, +1 = next),
/// clamped at the ends (the sidebar arrows disable there).
fn step_target(app: &mut App, delta: isize) {
    if let Some(ed) = &mut app.editing {
        let list = edit_target_list(&ed.doc);
        if let Some(pos) = list.iter().position(|t| *t == ed.target) {
            let next = pos as isize + delta;
            if next >= 0 && (next as usize) < list.len() {
                ed.target = list[next as usize].clone();
            }
        }
    }
}

/// Whether a name-entry dialog's current text is an acceptable name for its [`NameEntryKind`]:
/// non-empty (trimmed) and unique in scope — action-set names unique among sets, layer names unique
/// among a set's own siblings (a rename may keep its own current name). Drives the OK button's
/// enabled state, so invalid names are simply unconfirmable (decision B — invalid-unrepresentable).
pub(crate) fn name_entry_ok(doc: &ConfigDoc, kind: &NameEntryKind, text: &str) -> bool {
    let name = text.trim();
    if name.is_empty() {
        return false;
    }
    let set_taken = |exclude: Option<&str>| {
        doc.action_sets.iter().any(|s| Some(s.name.as_str()) != exclude && s.name == name)
    };
    let layer_taken = |set: &str, exclude: Option<&str>| {
        find_set(doc, set).is_some_and(|s| {
            s.layers.iter().any(|l| Some(l.name.as_str()) != exclude && l.name == name)
        })
    };
    match kind {
        NameEntryKind::NewSet => !set_taken(None),
        NameEntryKind::RenameSet { set } => !set_taken(Some(set)),
        NameEntryKind::NewLayer { set } => !layer_taken(set, None),
        NameEntryKind::RenameLayer { set, layer } => !layer_taken(set, Some(layer)),
    }
}

/// Apply a confirmed name-entry dialog to the loaded doc, then re-point the selection at the
/// affected set/layer so the sidebar/pages follow the edit.
fn apply_name_entry(app: &mut App, kind: NameEntryKind, name: String) {
    let Some(ed) = &mut app.editing else { return };
    match kind {
        NameEntryKind::NewSet => {
            ed.doc.action_sets.push(ActionSet {
                name: name.clone(),
                bindings: Default::default(),
                layers: Vec::new(),
            });
            ed.target = EditTarget { set: name, layer: None };
        }
        NameEntryKind::NewLayer { set } => {
            if let Some(s) = find_set_mut(&mut ed.doc, &set) {
                s.layers.push(Layer { name: name.clone(), bindings: Default::default() });
                ed.target = EditTarget { set, layer: Some(name) };
            }
        }
        NameEntryKind::RenameSet { set } => {
            if let Some(s) = find_set_mut(&mut ed.doc, &set) {
                s.name = name.clone();
            }
            // Follow the rename if the renamed set was the selected one.
            if ed.target.set == set {
                ed.target.set = name;
            }
        }
        NameEntryKind::RenameLayer { set, layer } => {
            if let Some(s) = find_set_mut(&mut ed.doc, &set)
                && let Some(l) = s.layers.iter_mut().find(|l| l.name == layer)
            {
                l.name = name.clone();
            }
            if ed.target.set == set && ed.target.layer.as_deref() == Some(layer.as_str()) {
                ed.target.layer = Some(name);
            }
        }
    }
}

/// Remove the set or layer a context menu targeted, then re-point the selection if it pointed at
/// the removed thing (a removed layer falls back to its parent set; a removed set to the first set).
/// The last action set is never removed (the menu disables Remove there), so a set-removal always
/// leaves at least one behind.
fn remove_target(app: &mut App, target: &EditTarget) {
    let Some(ed) = &mut app.editing else { return };
    match &target.layer {
        None => {
            if ed.doc.action_sets.len() <= 1 {
                return;
            }
            ed.doc.action_sets.retain(|s| s.name != target.set);
            if ed.target.set == target.set {
                ed.target = first_target(&ed.doc);
            }
        }
        Some(layer) => {
            if let Some(s) = find_set_mut(&mut ed.doc, &target.set) {
                s.layers.retain(|l| &l.name != layer);
            }
            if ed.target.set == target.set && ed.target.layer.as_deref() == Some(layer.as_str()) {
                ed.target = EditTarget { set: target.set.clone(), layer: None };
            }
        }
    }
}

fn find_set<'a>(doc: &'a ConfigDoc, name: &str) -> Option<&'a ActionSet> {
    doc.action_sets.iter().find(|s| s.name == name)
}

fn find_set_mut<'a>(doc: &'a mut ConfigDoc, name: &str) -> Option<&'a mut ActionSet> {
    doc.action_sets.iter_mut().find(|s| s.name == name)
}
