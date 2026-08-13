//! The profile editor's **state + update** — everything the editor pages read and mutate, kept out
//! of the app's top-level `update` so the (soon large) editor logic lives in one place. The widgets
//! for these live in [`crate::view::editor`]; the lifecycle brackets that create/destroy the editor
//! state (`EditProfile` / `StopEditing`) stay App-level.
//!
//! [`update`] is the **single place a loaded profile's document is mutated**, so it is the natural
//! hook for a future undo stack: snapshot [`Editing::doc`] here before applying an edit.

use std::path::PathBuf;

use config::ConfigDoc;
use iced::Task;
use ipc::ProfileRole;

use crate::{App, Message};

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
/// set's layers. The sidebar's ◀/▶ selector walks these in [`edit_target_list`] order. `Default` =
/// the first action set, no layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct EditTarget {
    pub(crate) set: usize,
    pub(crate) layer: Option<usize>,
}

/// The flat, ordered list of edit targets for a profile: each action set followed by its own layers
/// (a set with no layers contributes just itself). The sequence the sidebar selector steps through.
pub(crate) fn edit_target_list(doc: &ConfigDoc) -> Vec<EditTarget> {
    let mut targets = Vec::new();
    for (si, set) in doc.action_sets.iter().enumerate() {
        targets.push(EditTarget { set: si, layer: None });
        for li in 0..set.layers.len() {
            targets.push(EditTarget { set: si, layer: Some(li) });
        }
    }
    targets
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
                ed.target = list[next as usize];
            }
        }
    }
}
