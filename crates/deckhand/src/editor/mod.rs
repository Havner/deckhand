//! The profile editor's **state + update** — everything the editor pages read and mutate, kept out
//! of the app's top-level `update` so the (soon large) editor logic lives in one place. The widgets
//! for these live in [`crate::view::editor`]; the lifecycle brackets that create/destroy the editor
//! state (`EditProfile` / `StopEditing`) stay App-level.
//!
//! [`update`] is the **single place a loaded profile's document is mutated**, so it is the natural
//! hook for a future undo stack: snapshot [`Editing::doc`] here before applying an edit.

use std::collections::BTreeMap;
use std::path::PathBuf;

use config::{
    Action, ActionSet, ActivationMode, Activator, Command, CommandSettings, ConfigDoc, Curve,
    DpadLayout, GyroSpace, HapticEdge, HapticStrength, InputSource, Layer, MouseOutput, SourceBinding,
    StickOutput, TriggerOutput, Turbo,
};
use iced::Task;
use ipc::ProfileRole;

use crate::{ActionTab, App, Message, NAME_FIELD_ID, Popup};

pub(crate) mod authoring;
pub(crate) use authoring::Behavior;
mod settings;

/// A profile loaded into the editor: the file it came from (edits save back here), the parsed
/// document, and which action set / layer the input pages currently target. Its presence is the
/// UI's central macro-state (see [`App::editing`]).
pub(crate) struct Editing {
    pub(crate) path: PathBuf,
    pub(crate) doc: ConfigDoc,
    /// Which action set / layer the per-input editor pages currently target (the sidebar selector).
    pub(crate) target: EditTarget,
    /// An open settings sub-page shown *instead of* the current input page, or `None` for the normal
    /// category pages. Cleared by any navigation (sidebar category or the target selector) — see
    /// [`SettingsView`].
    pub(crate) settings: Option<SettingsView>,
}

/// What the editor is pointed at: an action set (`layer: None` — its base bindings) or one of that
/// set's layers. Addressed **by name** — set names are unique among sets and layer names unique
/// among a set's own layers (both enforced by the editor), so `(set, Option<layer>)` is a stable
/// key that survives reorder/removal (an index would silently shift). Doubles as the identity a
/// Profile-page bar's gear acts on. The sidebar's ◀/▶ selector walks these in [`edit_target_list`]
/// order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EditTarget {
    pub(crate) set: String,
    pub(crate) layer: Option<String>,
}

/// Which `Vec<Command>` inside a binding a command bar edits: a plain button's own commands, a
/// button-pad / directional-pad direction, a joystick outer ring, or a trigger's soft pull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandSlot {
    Button,
    Up,
    Down,
    Left,
    Right,
    OuterRing,
    SoftPull,
}

/// A specific command within a slot — the input, the slot, and which `Command` in that slot's
/// `Vec<Command>` (each command is one activator; a slot can hold several).
#[derive(Debug, Clone)]
pub(crate) struct CommandRef {
    pub(crate) input: InputSource,
    pub(crate) slot: CommandSlot,
    pub(crate) index: usize,
}

/// Where a picked action lands. `Replace` sets an existing action (main = index 0, subcommands ≥1);
/// `AddCommand` appends a new Regular command (the first/extra command); `AddSubCommand` appends an
/// extra action (subcommand) to a command.
#[derive(Debug, Clone)]
pub(crate) enum ActionTarget {
    Replace { cmd: CommandRef, action: usize },
    AddCommand { input: InputSource, slot: CommandSlot },
    AddSubCommand { cmd: CommandRef },
}

/// A focused settings sub-page, shown *instead of* the current input page (reached from a gear
/// menu, left via Back). The general paradigm for every settings page — only per-command settings
/// exist so far; per-behaviour settings will join as a second variant on the same model.
#[derive(Debug, Clone)]
pub(crate) enum SettingsView {
    /// Per-command settings (activator kind + time, interruptible/toggle/turbo/haptics).
    Command(CommandRef),
    /// Per-behaviour settings for a rich source / button group (deadzone, curve, sensitivity, …).
    /// Addresses the input; the page reads its binding in the currently-edited set/layer.
    Behavior(InputSource),
}

/// One field-edit on a behaviour's settings — the wire form of "the user changed one control".
/// A **sum type** (one variant per editable field), NOT a struct of all fields: a control emits
/// exactly one of these, and [`apply_setting`] writes it into whichever binding field it names
/// (a no-op if the current behaviour lacks that field). The typed `config` settings structs stay
/// the source of truth; this never mirrors them wholesale.
#[derive(Debug, Clone)]
pub(crate) enum SettingEdit {
    StickOutput(StickOutput),
    TriggerOutput(TriggerOutput),
    MouseOutput(MouseOutput),
    OuterRing(f32),
    SoftPull(f32),
    Layout(DpadLayout),
    Space(GyroSpace),
    SensitivityX(f32),
    SensitivityY(f32),
    Curve(Curve),
    Acceleration(f32),
    SmoothingEnabled(bool),
    SmoothingMinCutoff(f32),
    SmoothingBeta(f32),
    Deadzone(f32),
    AntiDeadzone(f32),
    InvertX(bool),
    InvertY(bool),
    Rotation(f32),
    ActivationMode(ActivationMode),
    /// Append a gater button to the behaviour's activation (deduped). Chosen via the Button picker.
    AddGater(vocab_hid::Button),
    /// Remove the gater at this index from the behaviour's activation.
    RemoveGater(usize),
}

/// The activator kinds, as the picker/combobox value (an [`Activator`] carries a parameter, so this
/// tags just the variant; [`ActivatorKind::to_activator`] supplies the default parameter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivatorKind {
    Regular,
    Long,
    Double,
    Start,
    Release,
}

impl ActivatorKind {
    pub(crate) const ALL: &'static [ActivatorKind] = &[
        ActivatorKind::Regular,
        ActivatorKind::Long,
        ActivatorKind::Double,
        ActivatorKind::Start,
        ActivatorKind::Release,
    ];

    pub(crate) fn of(a: &Activator) -> Self {
        match a {
            Activator::Regular { .. } => ActivatorKind::Regular,
            Activator::Long { .. } => ActivatorKind::Long,
            Activator::Double { .. } => ActivatorKind::Double,
            Activator::Start => ActivatorKind::Start,
            Activator::Release => ActivatorKind::Release,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ActivatorKind::Regular => "Regular press",
            ActivatorKind::Long => "Long press",
            ActivatorKind::Double => "Double press",
            ActivatorKind::Start => "Start press",
            ActivatorKind::Release => "Release press",
        }
    }

    /// Build the activator with its default parameter (Long = 450 ms, Double = 200 ms).
    pub(crate) fn to_activator(self) -> Activator {
        match self {
            // New commands default to interruptible: harmless when it's the node's only command
            // (nothing to interrupt it → a plain hold), and the intent one wants the moment a
            // Long/Double is added alongside it.
            ActivatorKind::Regular => Activator::Regular { interruptible: true },
            ActivatorKind::Long => Activator::Long { hold_ms: 450 },
            ActivatorKind::Double => Activator::Double { window_ms: 200 },
            ActivatorKind::Start => Activator::Start,
            ActivatorKind::Release => Activator::Release,
        }
    }
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
pub(crate) enum NameEntryKind {
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
pub(crate) enum EditorMessage {
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
    /// Behaviour combobox: set an input's binding to a fresh default of the chosen behaviour, or
    /// clear it (remove the map entry) when `Behavior::Unbound`.
    SetBehavior(InputSource, Behavior),
    /// Open the output-Action picker to fill a target (replace an action / add a command / add a
    /// subcommand).
    OpenActionPicker(ActionTarget),
    /// Switch the Action picker's tab.
    ActionPickerTab(ActionTab),
    /// An action was picked — write it into the picker's target and close the picker.
    ActionPicked(Action),
    /// Open a command's gear menu (activator / settings / remove / add).
    OpenCommandMenu(CommandRef),
    /// Open the top slot bar's gear menu (multi-command: remove all / add extra).
    OpenSlotMenu(InputSource, CommandSlot),
    /// Open a layer Button's inherited/disabled gear menu (Disable, or Remove the `None`).
    OpenLayerButtonMenu(InputSource),
    /// Set a command's activator kind (keeps the menu open).
    SetActivator(CommandRef, ActivatorKind),
    /// Remove one command (clears the whole slot / drops the plain-button entry when it empties).
    RemoveCommand(CommandRef),
    /// Remove every command from a slot.
    RemoveAllCommands(InputSource, CommandSlot),
    /// Remove a subcommand (an action at index ≥1) from a command.
    RemoveSubCommand(CommandRef, usize),
    /// Open the per-command settings sub-page (the command gear menu's Settings item).
    OpenCommandSettings(CommandRef),
    /// Leave a settings sub-page (Back button), returning to the input page.
    CloseSettings,
    /// Per-command settings edits (the settings sub-page). Each mutates one field of the addressed
    /// command and autosaves.
    SetHoldMs(CommandRef, u32),
    SetWindowMs(CommandRef, u32),
    SetInterruptible(CommandRef, bool),
    SetToggle(CommandRef, bool),
    SetTurbo(CommandRef, bool),
    SetTurboInterval(CommandRef, u32),
    SetHapticEdge(CommandRef, HapticEdge),
    SetHapticStrength(CommandRef, HapticStrength),
    /// Open the per-behaviour settings sub-page for an input (the behaviour-row gear).
    OpenBehaviorSettings(InputSource),
    /// Apply one behaviour-settings field edit to an input's binding (settings sub-page).
    SetSetting(InputSource, SettingEdit),
    /// Rumble edits — the profile-level `ConfigDoc.rumble` feel (strength + curve).
    SetRumbleStrength(u8),
    SetRumbleCurve(Curve),
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
        EditorMessage::SetBehavior(input, behavior) => {
            if let Some(bindings) = current_bindings_mut(app) {
                match behavior {
                    // `Unbound` = no entry (Inherited/None); `Disabled` = explicit nullifying `None`;
                    // a real behaviour rebuilds from authoring defaults (discarding old commands).
                    Behavior::Unbound => {
                        bindings.remove(&input);
                    }
                    Behavior::Disabled => {
                        bindings.insert(input.clone(), SourceBinding::None);
                    }
                    b => {
                        bindings.insert(input.clone(), b.default_binding(&input));
                    }
                }
            }
            app.save_editing();
            // Also used from the Button gear menu (Disable) — close any open menu.
            app.popup = None;
            Task::none()
        }
        EditorMessage::OpenActionPicker(target) => {
            app.popup = Some(Popup::ActionPicker { tab: ActionTab::Gamepad, target });
            Task::none()
        }
        EditorMessage::ActionPickerTab(tab) => {
            if let Some(Popup::ActionPicker { tab: current, .. }) = &mut app.popup {
                *current = tab;
            }
            Task::none()
        }
        EditorMessage::ActionPicked(action) => {
            let target = match &app.popup {
                Some(Popup::ActionPicker { target, .. }) => Some(target.clone()),
                _ => None,
            };
            if let Some(target) = target {
                apply_action(app, target, action);
                app.save_editing();
            }
            app.popup = None;
            Task::none()
        }
        EditorMessage::OpenCommandMenu(cmd) => {
            app.popup = Some(Popup::CommandMenu { cmd });
            Task::none()
        }
        EditorMessage::OpenSlotMenu(input, slot) => {
            app.popup = Some(Popup::SlotMenu { input, slot });
            Task::none()
        }
        EditorMessage::OpenLayerButtonMenu(input) => {
            app.popup = Some(Popup::LayerButtonMenu { input });
            Task::none()
        }
        EditorMessage::SetActivator(cmd, kind) => {
            if let Some(command) = command_mut(app, &cmd) {
                command.activator = kind.to_activator();
            }
            app.save_editing();
            // Keep the menu open so the new value shows and further edits are possible.
            Task::none()
        }
        EditorMessage::RemoveCommand(cmd) => {
            remove_command(app, &cmd);
            app.save_editing();
            app.popup = None;
            Task::none()
        }
        EditorMessage::RemoveAllCommands(input, slot) => {
            clear_slot(app, &input, slot);
            app.save_editing();
            app.popup = None;
            Task::none()
        }
        EditorMessage::RemoveSubCommand(cmd, action) => {
            if let Some(command) = command_mut(app, &cmd)
                && action < command.actions.len()
            {
                command.actions.remove(action);
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::OpenCommandSettings(cmd) => {
            if let Some(ed) = &mut app.editing {
                ed.settings = Some(SettingsView::Command(cmd));
            }
            app.popup = None;
            Task::none()
        }
        EditorMessage::CloseSettings => {
            if let Some(ed) = &mut app.editing {
                ed.settings = None;
            }
            Task::none()
        }
        EditorMessage::SetInterruptible(cmd, v) => {
            if let Some(c) = command_mut(app, &cmd) {
                c.activator = Activator::Regular { interruptible: v };
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetHoldMs(cmd, ms) => {
            if let Some(c) = command_mut(app, &cmd) {
                c.activator = Activator::Long { hold_ms: ms };
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetWindowMs(cmd, ms) => {
            if let Some(c) = command_mut(app, &cmd) {
                c.activator = Activator::Double { window_ms: ms };
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetToggle(cmd, v) => {
            if let Some(c) = command_mut(app, &cmd) {
                c.settings.toggle = v;
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetTurbo(cmd, on) => {
            if let Some(c) = command_mut(app, &cmd) {
                // Enabling seeds a sensible default rate; disabling drops it entirely.
                c.settings.turbo = on.then_some(Turbo { interval_ms: DEFAULT_TURBO_INTERVAL_MS });
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetTurboInterval(cmd, ms) => {
            if let Some(c) = command_mut(app, &cmd)
                && let Some(t) = &mut c.settings.turbo
            {
                t.interval_ms = ms;
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetHapticEdge(cmd, edge) => {
            if let Some(c) = command_mut(app, &cmd) {
                c.settings.haptics.on = edge;
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetHapticStrength(cmd, strength) => {
            if let Some(c) = command_mut(app, &cmd) {
                c.settings.haptics.strength = strength;
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::OpenBehaviorSettings(input) => {
            if let Some(ed) = &mut app.editing {
                ed.settings = Some(SettingsView::Behavior(input));
            }
            Task::none()
        }
        EditorMessage::SetSetting(input, edit) => {
            if let Some(bindings) = current_bindings_mut(app)
                && let Some(binding) = bindings.get_mut(&input)
            {
                settings::apply_setting(binding, edit);
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetRumbleStrength(v) => {
            if let Some(ed) = &mut app.editing {
                ed.doc.rumble.strength = v;
            }
            app.save_editing();
            Task::none()
        }
        EditorMessage::SetRumbleCurve(c) => {
            if let Some(ed) = &mut app.editing {
                ed.doc.rumble.curve = c;
            }
            app.save_editing();
            Task::none()
        }
    }
}

/// The turbo rate a freshly-enabled turbo starts at, in milliseconds (≈10 Hz).
pub(crate) const DEFAULT_TURBO_INTERVAL_MS: u32 = 100;

/// The bindings map of the currently-edited action set / layer (read) — resolved by name from the
/// selected [`EditTarget`]. `None` when no profile is loaded or the target has drifted.
pub(crate) fn current_bindings(app: &App) -> Option<&BTreeMap<InputSource, SourceBinding>> {
    let ed = app.editing.as_ref()?;
    let set = ed.doc.action_sets.iter().find(|s| s.name == ed.target.set)?;
    match &ed.target.layer {
        None => Some(&set.bindings),
        Some(layer) => set.layers.iter().find(|l| &l.name == layer).map(|l| &l.bindings),
    }
}

/// The bindings map of the currently-edited action set / layer (mutable).
fn current_bindings_mut(app: &mut App) -> Option<&mut BTreeMap<InputSource, SourceBinding>> {
    let ed = app.editing.as_mut()?;
    let set_name = ed.target.set.clone();
    let layer_name = ed.target.layer.clone();
    let set = ed.doc.action_sets.iter_mut().find(|s| s.name == set_name)?;
    match layer_name {
        None => Some(&mut set.bindings),
        Some(layer) => set.layers.iter_mut().find(|l| l.name == layer).map(|l| &mut l.bindings),
    }
}

/// The command vector a [`CommandSlot`] addresses within a binding (read); `None` if the slot
/// doesn't apply to that binding's shape.
pub(crate) fn slot_commands(binding: &SourceBinding, slot: CommandSlot) -> Option<&Vec<Command>> {
    use CommandSlot as S;
    use SourceBinding as B;
    match (binding, slot) {
        (B::Button { commands }, S::Button) => Some(commands),
        (B::ButtonPad { up, .. } | B::DirectionalPad { up, .. }, S::Up) => Some(up),
        (B::ButtonPad { down, .. } | B::DirectionalPad { down, .. }, S::Down) => Some(down),
        (B::ButtonPad { left, .. } | B::DirectionalPad { left, .. }, S::Left) => Some(left),
        (B::ButtonPad { right, .. } | B::DirectionalPad { right, .. }, S::Right) => Some(right),
        (B::Joystick { outer_ring, .. } | B::DirectionalPad { outer_ring, .. }, S::OuterRing) => {
            Some(outer_ring)
        }
        (B::Trigger { soft_pull, .. }, S::SoftPull) => Some(soft_pull),
        _ => None,
    }
}

/// The command vector a [`CommandSlot`] addresses within a binding (mutable).
fn slot_commands_mut(binding: &mut SourceBinding, slot: CommandSlot) -> Option<&mut Vec<Command>> {
    use CommandSlot as S;
    use SourceBinding as B;
    match (binding, slot) {
        (B::Button { commands }, S::Button) => Some(commands),
        (B::ButtonPad { up, .. } | B::DirectionalPad { up, .. }, S::Up) => Some(up),
        (B::ButtonPad { down, .. } | B::DirectionalPad { down, .. }, S::Down) => Some(down),
        (B::ButtonPad { left, .. } | B::DirectionalPad { left, .. }, S::Left) => Some(left),
        (B::ButtonPad { right, .. } | B::DirectionalPad { right, .. }, S::Right) => Some(right),
        (B::Joystick { outer_ring, .. } | B::DirectionalPad { outer_ring, .. }, S::OuterRing) => {
            Some(outer_ring)
        }
        (B::Trigger { soft_pull, .. }, S::SoftPull) => Some(soft_pull),
        _ => None,
    }
}

/// The command a [`CommandRef`] points at, in the currently-edited set/layer (read).
pub(crate) fn command_at<'a>(app: &'a App, cmd: &CommandRef) -> Option<&'a Command> {
    let binding = current_bindings(app)?.get(&cmd.input)?;
    slot_commands(binding, cmd.slot)?.get(cmd.index)
}

/// How many commands a slot currently holds.
pub(crate) fn slot_len(app: &App, input: &InputSource, slot: CommandSlot) -> usize {
    current_bindings(app)
        .and_then(|b| b.get(input))
        .and_then(|binding| slot_commands(binding, slot))
        .map_or(0, |v| v.len())
}

/// The command a [`CommandRef`] points at (mutable).
fn command_mut<'a>(app: &'a mut App, cmd: &CommandRef) -> Option<&'a mut Command> {
    let binding = current_bindings_mut(app)?.get_mut(&cmd.input)?;
    slot_commands_mut(binding, cmd.slot)?.get_mut(cmd.index)
}

/// Apply a picked action to its [`ActionTarget`]. For a plain button, `AddCommand` creates the
/// `SourceBinding::Button` on demand; a command is always born with a Regular activator + default
/// settings.
fn apply_action(app: &mut App, target: ActionTarget, action: Action) {
    match target {
        ActionTarget::Replace { cmd, action: idx } => {
            if let Some(c) = command_mut(app, &cmd)
                && idx < c.actions.len()
            {
                c.actions[idx] = action;
            }
        }
        ActionTarget::AddSubCommand { cmd } => {
            if let Some(c) = command_mut(app, &cmd) {
                c.actions.push(action);
            }
        }
        ActionTarget::AddCommand { input, slot } => {
            let Some(bindings) = current_bindings_mut(app) else { return };
            if slot == CommandSlot::Button {
                // Create the plain-button binding on demand — for a missing entry (unbound/inherited)
                // *or* an explicit `None` (a layer's disabled state), since adding a command must
                // always leave a real binding (Q1: no state you can't click out of).
                let e = bindings.entry(input.clone()).or_insert(SourceBinding::None);
                if matches!(e, SourceBinding::None) {
                    *e = SourceBinding::Button { commands: Vec::new() };
                }
            }
            if let Some(binding) = bindings.get_mut(&input)
                && let Some(commands) = slot_commands_mut(binding, slot)
            {
                commands.push(Command {
                    activator: ActivatorKind::Regular.to_activator(), // the authoring default
                    actions: vec![action],
                    settings: CommandSettings::default(),
                });
            }
        }
    }
}

/// Remove one command from a slot; if that empties a plain-button binding, drop its map entry
/// (a rich binding's slot is just left empty — the binding persists).
fn remove_command(app: &mut App, cmd: &CommandRef) {
    let Some(bindings) = current_bindings_mut(app) else { return };
    let is_plain_button;
    {
        let Some(binding) = bindings.get_mut(&cmd.input) else { return };
        is_plain_button =
            matches!(binding, SourceBinding::Button { .. }) && cmd.slot == CommandSlot::Button;
        if let Some(commands) = slot_commands_mut(binding, cmd.slot)
            && cmd.index < commands.len()
        {
            commands.remove(cmd.index);
        }
    }
    let empty = bindings
        .get(&cmd.input)
        .and_then(|b| slot_commands(b, cmd.slot))
        .is_none_or(|v| v.is_empty());
    if empty && is_plain_button {
        bindings.remove(&cmd.input);
    }
}

/// Clear a slot: the plain-button slot addresses the whole InputSource entry, so it's dropped from
/// the map (a `Button` binding's commands, or a layer `None` disable, alike) → back to no-entry. A
/// rich binding's slot vector is just emptied (the behaviour persists).
fn clear_slot(app: &mut App, input: &InputSource, slot: CommandSlot) {
    let Some(bindings) = current_bindings_mut(app) else { return };
    if slot == CommandSlot::Button {
        bindings.remove(input);
    } else if let Some(binding) = bindings.get_mut(input)
        && let Some(commands) = slot_commands_mut(binding, slot)
    {
        commands.clear();
    }
}

/// Whether the editor is currently pointed at a layer (vs an action set) — Inherited/Disabled UI
/// applies only on layers.
pub(crate) fn on_layer(app: &App) -> bool {
    app.editing.as_ref().is_some_and(|e| e.target.layer.is_some())
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
                // A settings sub-page addresses a command in the *old* target — leave it on a switch.
                ed.settings = None;
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
            // `ChangeActionSet` refs resolve globally → repoint them across the whole profile.
            rename_set_refs(&mut ed.doc, &set, &name);
            // Follow the rename if the renamed set was the selected one.
            if ed.target.set == set {
                ed.target.set = name;
            }
        }
        NameEntryKind::RenameLayer { set, layer } => {
            if let Some(s) = find_set_mut(&mut ed.doc, &set) {
                if let Some(l) = s.layers.iter_mut().find(|l| l.name == layer) {
                    l.name = name.clone();
                }
                // Layer refs resolve *within their set* → repoint Hold/Add/RemoveLayer here only.
                rename_layer_refs(s, &layer, &name);
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

/// Every `Vec<Command>` inside a binding, mutably — the mut counterpart of
/// [`SourceBinding::commands`](config::SourceBinding::commands), for rewriting actions in place.
fn binding_command_slots_mut(binding: &mut SourceBinding) -> Vec<&mut Vec<Command>> {
    use SourceBinding as B;
    match binding {
        B::Button { commands } => vec![commands],
        B::ButtonPad { up, down, left, right } => vec![up, down, left, right],
        B::Joystick { outer_ring, .. } => vec![outer_ring],
        B::DirectionalPad { up, down, left, right, outer_ring, .. } => {
            vec![up, down, left, right, outer_ring]
        }
        B::Trigger { soft_pull, .. } => vec![soft_pull],
        B::AsMouse { .. } | B::JoystickMouse { .. } | B::GyroToMouse { .. } | B::None => vec![],
    }
}

/// Run `f` over every action across a bindings map's commands.
fn visit_actions(bindings: &mut BTreeMap<InputSource, SourceBinding>, f: &mut impl FnMut(&mut Action)) {
    for binding in bindings.values_mut() {
        for slot in binding_command_slots_mut(binding) {
            for cmd in slot.iter_mut() {
                for action in &mut cmd.actions {
                    f(action);
                }
            }
        }
    }
}

/// Repoint every `ChangeActionSet(old)` to `new` across the whole profile (set refs are global).
fn rename_set_refs(doc: &mut ConfigDoc, old: &str, new: &str) {
    let mut fix = |a: &mut Action| {
        if let Action::ChangeActionSet(r) = a
            && r.0 == old
        {
            r.0 = new.to_string();
        }
    };
    for set in &mut doc.action_sets {
        visit_actions(&mut set.bindings, &mut fix);
        for layer in &mut set.layers {
            visit_actions(&mut layer.bindings, &mut fix);
        }
    }
}

/// Repoint every `Hold/Add/RemoveLayer(old)` to `new` within one action set (layer refs are
/// per-set), across its base bindings and each of its layers' bindings.
fn rename_layer_refs(set: &mut ActionSet, old: &str, new: &str) {
    let mut fix = |a: &mut Action| match a {
        Action::HoldLayer(r) | Action::AddLayer(r) | Action::RemoveLayer(r) if r.0 == old => {
            r.0 = new.to_string();
        }
        _ => {}
    };
    visit_actions(&mut set.bindings, &mut fix);
    for layer in &mut set.layers {
        visit_actions(&mut layer.bindings, &mut fix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{ActionSetRef, LayerRef};

    fn button_with(actions: Vec<Action>) -> SourceBinding {
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: false },
                actions,
                settings: CommandSettings::default(),
            }],
        }
    }

    fn set(name: &str, input: InputSource, action: Action, layers: Vec<Layer>) -> ActionSet {
        ActionSet {
            name: name.into(),
            bindings: BTreeMap::from([(input, button_with(vec![action]))]),
            layers,
        }
    }

    fn action_of(binding: &SourceBinding) -> &Action {
        match binding {
            SourceBinding::Button { commands } => &commands[0].actions[0],
            _ => unreachable!(),
        }
    }

    #[test]
    fn rename_set_repoints_change_action_set_refs_across_the_profile() {
        let mut doc = ConfigDoc {
            version: 0,
            name: "p".into(),
            action_sets: vec![
                set(
                    "Game",
                    InputSource::LeftBumper,
                    Action::ChangeActionSet(ActionSetRef("Drive".into())),
                    vec![Layer {
                        name: "aim".into(),
                        bindings: BTreeMap::from([(
                            InputSource::RightBumper,
                            button_with(vec![Action::ChangeActionSet(ActionSetRef("Drive".into()))]),
                        )]),
                    }],
                ),
                ActionSet { name: "Drive".into(), bindings: BTreeMap::new(), layers: vec![] },
            ],
            rumble: Default::default(),
        };
        rename_set_refs(&mut doc, "Drive", "Racing");
        let want = Action::ChangeActionSet(ActionSetRef("Racing".into()));
        assert_eq!(action_of(&doc.action_sets[0].bindings[&InputSource::LeftBumper]), &want);
        // …including refs living inside a layer's bindings.
        assert_eq!(action_of(&doc.action_sets[0].layers[0].bindings[&InputSource::RightBumper]), &want);
    }

    #[test]
    fn rename_layer_is_scoped_to_its_own_set() {
        let hold = |name: &str| Action::HoldLayer(LayerRef(name.into()));
        let mut doc = ConfigDoc {
            version: 0,
            name: "p".into(),
            action_sets: vec![
                set("Game", InputSource::LeftBumper, hold("aim"), vec![Layer {
                    name: "aim".into(),
                    bindings: BTreeMap::new(),
                }]),
                set("Drive", InputSource::LeftBumper, hold("aim"), vec![Layer {
                    name: "aim".into(),
                    bindings: BTreeMap::new(),
                }]),
            ],
            rumble: Default::default(),
        };
        let game = doc.action_sets.iter_mut().find(|s| s.name == "Game").unwrap();
        rename_layer_refs(game, "aim", "scope");
        // Game's ref repointed; the same-named layer ref in Drive is a different layer → untouched.
        assert_eq!(action_of(&doc.action_sets[0].bindings[&InputSource::LeftBumper]), &hold("scope"));
        assert_eq!(action_of(&doc.action_sets[1].bindings[&InputSource::LeftBumper]), &hold("aim"));
    }
}
