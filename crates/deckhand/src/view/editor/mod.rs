//! The profile-editor screens — the pages reachable once a profile is loaded for editing.
//!
//! The **Profile** page ([`profile_screen`]) and the per-input pages ([`input_screen`]) are wired:
//! they render from, and mutate, the loaded profile's [`ConfigDoc`](config::ConfigDoc). Input pages
//! are data-driven from [`Category::groups`] (headers, behaviour selectors, per-slot command
//! bars with their gear menus, subcommands). Only [`rumble_screen`] is still a mockup.

use std::collections::BTreeMap;

use std::ops::RangeInclusive;

use iced::widget::{Space, button, checkbox, column, container, pick_list, row, slider, text, text_input};
use iced::{Center, Element, Fill};

use config::{
    Action, Activation, ActivationMode, Activator, Command, Curve, DpadLayout, GyroSpace, HapticEdge,
    HapticStrength, InputSource, Invert, MouseOutput, OneEuroFilter, Sensitivity, SourceBinding,
    SourceKind, StickOutput, TriggerOutput,
};

use super::{Dot, card, group_header, label_row, section_header, slot_display, small};
use crate::editor::{
    ActionTarget, ActivatorKind, Behavior, CommandRef, CommandSlot, EditorMessage, SettingEdit,
    SettingsView,
};
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

/// Rumble screen — the profile-level rumble feel (`ConfigDoc.rumble`: strength / frequency / the
/// strength→drive curve). A **standalone** page (its own widgets + curve control), deliberately NOT
/// built from the reusable settings blocks — those stay solely for the per-behaviour pages.
pub(super) fn rumble_screen(app: &App) -> Element<'static, Message> {
    let r = app.editing.as_ref().map(|e| e.doc.rumble.clone()).unwrap_or_default();

    // Strength is a percent that may exceed 100 (u8 → 255) to boost under-driven games.
    let strength = row![
        setting_label("Strength"),
        slider(0..=255u8, r.strength, |v| Message::Editor(EditorMessage::SetRumbleStrength(v))).step(1u8),
        text(format!("{}%", r.strength)).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center);

    let frequency = row![
        setting_label("Frequency"),
        slider(30..=150u16, r.hz, |v| Message::Editor(EditorMessage::SetRumbleHz(v))).step(1u16),
        text(format!("{} Hz", r.hz)).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center);

    column![
        section_header("Rumble"),
        small(
            "Per-profile rumble feel: game force-feedback → controller rumble. Strength may exceed \
             100% to boost games that under-drive their force-feedback.",
        ),
        strength,
        frequency,
        rumble_curve(&r.curve),
    ]
    .spacing(16.0)
    .into()
}

/// This page's own Curve control (kind picker + exponent slider) — a standalone copy of the
/// settings-page shape, NOT the reusable `curve` block, so the behaviour-settings blocks stay
/// untouched. Its own `RumbleCurveKind`, so the two evolve independently (accepted duplication).
fn rumble_curve(curve: &Curve) -> Element<'static, Message> {
    let kind = if matches!(curve, Curve::Power(_)) { RumbleCurveKind::Power } else { RumbleCurveKind::Linear };
    let combo = pick_list(
        Some(kind),
        vec![RumbleCurveKind::Linear, RumbleCurveKind::Power],
        |k: &RumbleCurveKind| k.label().to_string(),
    )
    .on_select(|k| {
        let c = match k {
            RumbleCurveKind::Linear => Curve::Linear,
            RumbleCurveKind::Power => Curve::Power(1.0),
        };
        Message::Editor(EditorMessage::SetRumbleCurve(c))
    })
    .width(CMD_SLOT);
    let mut col = column![row![setting_label("Curve"), combo].spacing(12.0).align_y(Center)].spacing(8.0);
    if let Curve::Power(e) = *curve {
        col = col.push(
            row![
                setting_label("Exponent"),
                slider(0.2..=4.0f32, e, |v| Message::Editor(EditorMessage::SetRumbleCurve(Curve::Power(v))))
                    .step(0.05f32),
                text(format!("{e:.2}")).width(70.0),
            ]
            .spacing(12.0)
            .align_y(Center),
        );
    }
    col.into()
}

/// The rumble page's own curve-kind pick-list value (standalone — not the settings blocks' `CurveKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RumbleCurveKind {
    Linear,
    Power,
}

impl RumbleCurveKind {
    fn label(self) -> &'static str {
        match self {
            RumbleCurveKind::Linear => "Linear",
            RumbleCurveKind::Power => "Power",
        }
    }
}

// --- settings sub-pages ---------------------------------------------------------------------

/// The active settings sub-page, rendered *instead of* the current category page — `None` when no
/// settings page is open (the caller then renders the normal category). The general settings-page
/// paradigm: a focused full-width form reached from a gear menu, left via Back.
pub(super) fn settings_screen(app: &App) -> Option<Element<'static, Message>> {
    match app.editing.as_ref()?.settings.as_ref()? {
        SettingsView::Command(cref) => Some(command_settings(app, cref)),
        SettingsView::Behavior(input) => Some(behavior_settings(app, input)),
    }
}

/// Fixed label column for a settings row, so the controls line up down the form.
const SET_LABEL: f32 = 160.0;

/// The per-command settings form: activator (kind + its own parameter — Long/Double time or the
/// Regular's interruptible flag), then toggle / turbo / haptics. Applicability-gated (turbo hidden on
/// Release, haptic strength only when the pulse is on) — decision B, invalid-unrepresentable.
fn command_settings(app: &App, cref: &CommandRef) -> Element<'static, Message> {
    let back = button(text("‹ Back"))
        .style(style::option_button)
        .on_press(Message::Editor(EditorMessage::CloseSettings));
    let (label, dot) = slot_display(&cref.input, cref.slot);
    // Centre the command label across the space beside Back so it reads as the page title.
    let title = container(label_row(label, dot)).width(Fill).align_x(Center);
    let header = row![back, title].spacing(16.0).align_y(Center);

    // The command can vanish (removed from another surface) while this page is open — keep Back live.
    let Some(cmd) = crate::editor::command_at(app, cref) else {
        return column![header, small("This command no longer exists.")].spacing(20.0).into();
    };

    let mut col = column![header, activator_setting(cref, cmd)].spacing(16.0);
    // The activator's own parameter(s): Long/Double time, or Regular's interruptible flag.
    if let Some(extra) = activator_additional_settings(cref, cmd) {
        col = col.push(extra);
    }
    let cref_toggle = cref.clone();
    col = col.push(check_setting("Toggle", cmd.settings.toggle, move |b| {
        Message::Editor(EditorMessage::SetToggle(cref_toggle.clone(), b))
    }));
    // Turbo (rapid re-fire) is meaningless on a one-shot Release.
    if !matches!(cmd.activator, Activator::Release) {
        col = col.push(turbo_setting(cref, cmd));
    }
    col = col.push(haptic_settings(cref, cmd));
    col.into()
}

/// The activator-kind row: the same 5-way combobox as the command gear menu (kept here too so the
/// settings page is a complete home for the command's activator + its time parameter).
fn activator_setting(cref: &CommandRef, cmd: &Command) -> Element<'static, Message> {
    let cref = cref.clone();
    let current = ActivatorKind::of(&cmd.activator);
    let combo = pick_list(Some(current), ActivatorKind::ALL.to_vec(), |k: &ActivatorKind| {
        k.label().to_string()
    })
    .on_select(move |k| Message::Editor(EditorMessage::SetActivator(cref.clone(), k)))
    .width(CMD_SLOT);
    row![setting_label("Activator"), combo].spacing(12.0).align_y(Center).into()
}

/// The activator's own parameter row, when it has one: Long's hold time / Double's window, or the
/// Regular's `interruptible` toggle (suppress it when a longer activator on the same node fires).
/// `None` for the parameter-less kinds (Start/Release).
fn activator_additional_settings(cref: &CommandRef, cmd: &Command) -> Option<Element<'static, Message>> {
    match cmd.activator {
        Activator::Regular { interruptible } => {
            let cref = cref.clone();
            Some(check_setting("Interruptible", interruptible, move |b| {
                Message::Editor(EditorMessage::SetInterruptible(cref.clone(), b))
            }))
        }
        Activator::Long { hold_ms } => {
            let cref = cref.clone();
            Some(slider_row("Hold time", hold_ms, 100..=2000, 50, move |v| {
                Message::Editor(EditorMessage::SetHoldMs(cref.clone(), v))
            }))
        }
        Activator::Double { window_ms } => {
            let cref = cref.clone();
            Some(slider_row("Double window", window_ms, 100..=600, 25, move |v| {
                Message::Editor(EditorMessage::SetWindowMs(cref.clone(), v))
            }))
        }
        _ => None,
    }
}

/// Turbo: a checkbox gating a rate slider (inert same-geometry slider when off, mirroring the Globals
/// LED/idle pattern so toggling doesn't reflow the row).
fn turbo_setting(cref: &CommandRef, cmd: &Command) -> Element<'static, Message> {
    let on = cmd.settings.turbo.is_some();
    let interval =
        cmd.settings.turbo.as_ref().map_or(crate::editor::DEFAULT_TURBO_INTERVAL_MS, |t| t.interval_ms);
    let cref_toggle = cref.clone();
    let check = checkbox(on)
        .on_toggle(move |b| Message::Editor(EditorMessage::SetTurbo(cref_toggle.clone(), b)));
    let bar: Element<'static, Message> = if on {
        let cref = cref.clone();
        slider(20..=500u32, interval, move |v| {
            Message::Editor(EditorMessage::SetTurboInterval(cref.clone(), v))
        })
        .step(10u32)
        .into()
    } else {
        slider(20..=500u32, interval, |_| Message::Ignored).style(style::disabled_slider).into()
    };
    let readout = if on { format!("{interval} ms") } else { "off".to_string() };
    row![setting_label("Turbo"), check, bar, text(readout).width(70.0)]
        .spacing(12.0)
        .align_y(Center)
        .into()
}

/// Haptics: the pulse edge (Off/On press/On release/Both), plus a strength combobox that appears only
/// when the pulse is on (strength is meaningless while Off).
fn haptic_settings(cref: &CommandRef, cmd: &Command) -> Element<'static, Message> {
    let edge = cmd.settings.haptics.on.clone();
    let cref_edge = cref.clone();
    let edge_combo = pick_list(
        Some(edge.clone()),
        vec![HapticEdge::Off, HapticEdge::OnPress, HapticEdge::OnRelease, HapticEdge::Both],
        |e: &HapticEdge| haptic_edge_label(e).to_string(),
    )
    .on_select(move |e| Message::Editor(EditorMessage::SetHapticEdge(cref_edge.clone(), e)))
    .width(CMD_SLOT);
    let mut col =
        column![row![setting_label("Haptics"), edge_combo].spacing(12.0).align_y(Center)].spacing(8.0);
    if edge != HapticEdge::Off {
        let cref_strength = cref.clone();
        let strength_combo = pick_list(
            Some(cmd.settings.haptics.strength.clone()),
            vec![HapticStrength::Low, HapticStrength::Medium, HapticStrength::High],
            |s: &HapticStrength| haptic_strength_label(s).to_string(),
        )
        .on_select(move |s| Message::Editor(EditorMessage::SetHapticStrength(cref_strength.clone(), s)))
        .width(CMD_SLOT);
        col = col.push(row![setting_label("Strength"), strength_combo].spacing(12.0).align_y(Center));
    }
    col.into()
}

/// A settings-form checkbox row: a fixed-width label + a bare checkbox (label lives in the row, not
/// the widget, matching the Globals form).
fn check_setting(
    label: &'static str,
    value: bool,
    on_toggle: impl Fn(bool) -> Message + 'static,
) -> Element<'static, Message> {
    row![setting_label(label), checkbox(value).on_toggle(on_toggle)]
        .spacing(12.0)
        .align_y(Center)
        .into()
}

/// A settings-form slider row over a `u32` value: fixed label, the (step-snapped) slider, and a
/// trailing "<value> <unit>" readout.
fn slider_row(
    label: &'static str,
    value: u32,
    range: RangeInclusive<u32>,
    step: u32,
    on_change: impl Fn(u32) -> Message + 'static,
) -> Element<'static, Message> {
    row![
        setting_label(label),
        slider(range, value, on_change).step(step),
        text(format!("{value} ms")).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center)
    .into()
}

/// A fixed-width settings-row label so the controls line up down the form.
fn setting_label(s: &'static str) -> Element<'static, Message> {
    text(s).width(SET_LABEL).into()
}

/// Display label for a haptic edge (UI-owned — `config` stays presentation-free).
fn haptic_edge_label(e: &HapticEdge) -> &'static str {
    match e {
        HapticEdge::Off => "Off",
        HapticEdge::OnPress => "On press",
        HapticEdge::OnRelease => "On release",
        HapticEdge::Both => "Both",
    }
}

/// Display label for a haptic strength (UI-owned).
fn haptic_strength_label(s: &HapticStrength) -> &'static str {
    match s {
        HapticStrength::Low => "Low",
        HapticStrength::Medium => "Medium",
        HapticStrength::High => "High",
    }
}

// --- per-behaviour settings page ------------------------------------------------------------
//
// A behaviour's settings page = a header + its settings **blocks** composed in the canonical field
// order (see the per-behaviour compose fns). Each block is a reusable row (shared across every
// behaviour that has that field) that emits one generic `SetSetting(input, SettingEdit)`; the edit
// is applied by `editor::apply_setting`. Adding a behaviour = one compose fn from existing blocks.

/// The per-behaviour settings page: header (Back + input · behaviour) over the behaviour's blocks.
fn behavior_settings(app: &App, input: &InputSource) -> Element<'static, Message> {
    let back = button(text("‹ Back"))
        .style(style::option_button)
        .on_press(Message::Editor(EditorMessage::CloseSettings));

    let binding = crate::editor::current_bindings(app).and_then(|b| b.get(input));
    let Some(binding) = binding else {
        let title = container(text(input_label(input))).width(Fill).align_x(Center);
        return column![row![back, title].spacing(16.0).align_y(Center), small("This input has no behaviour settings.")]
            .spacing(20.0)
            .into();
    };
    let title = container(text(format!("{} · {}", input_label(input), Behavior::of(binding).label())))
        .width(Fill)
        .align_x(Center);
    let header = row![back, title].spacing(16.0).align_y(Center);

    let body = match binding {
        SourceBinding::Joystick { settings, .. } => joystick_view(input, settings),
        SourceBinding::DirectionalPad { settings, .. } => directional_pad_view(input, settings),
        SourceBinding::AsMouse { settings } => as_mouse_view(input, settings),
        SourceBinding::JoystickMouse { settings } => joystick_mouse_view(input, settings),
        SourceBinding::GyroToMouse { settings } => gyro_to_mouse_view(input, settings),
        SourceBinding::Trigger { settings, .. } => trigger_view(input, settings),
        _ => small("This behaviour has no settings."),
    };
    column![header, body].spacing(16.0).into()
}

// --- per-behaviour compose fns (blocks in canonical field order) ----------------------------

fn joystick_view(input: &InputSource, s: &config::JoystickSettings) -> Element<'static, Message> {
    column![
        output_stick(input, s.output.clone()),
        outer_ring(input, s.outer_ring.radius),
        curve(input, &s.curve),
        deadzone(input, s.deadzone.inner),
        anti_deadzone(input, s.anti_deadzone.amount),
        invert(input, s.invert.clone()),
        rotation(input, s.rotation.degrees),
        activation(input, &s.activation),
    ]
    .spacing(16.0)
    .into()
}

fn directional_pad_view(input: &InputSource, s: &config::DirectionalPadSettings) -> Element<'static, Message> {
    column![
        layout(input, s.layout.clone()),
        outer_ring(input, s.outer_ring.radius),
        deadzone(input, s.deadzone.inner),
        rotation(input, s.rotation.degrees),
        activation(input, &s.activation),
    ]
    .spacing(16.0)
    .into()
}

fn as_mouse_view(input: &InputSource, s: &config::AsMouseSettings) -> Element<'static, Message> {
    column![
        output_mouse(input, s.output.clone()),
        sensitivity(input, s.sensitivity.clone()),
        acceleration(input, s.acceleration.factor),
        smoothing(input, &s.smoothing),
        invert(input, s.invert.clone()),
        rotation(input, s.rotation.degrees),
        activation(input, &s.activation),
    ]
    .spacing(16.0)
    .into()
}

fn joystick_mouse_view(input: &InputSource, s: &config::JoystickMouseSettings) -> Element<'static, Message> {
    column![
        output_mouse(input, s.output.clone()),
        sensitivity(input, s.sensitivity.clone()),
        curve(input, &s.curve),
        deadzone(input, s.deadzone.inner),
        invert(input, s.invert.clone()),
        rotation(input, s.rotation.degrees),
        activation(input, &s.activation),
    ]
    .spacing(16.0)
    .into()
}

fn gyro_to_mouse_view(input: &InputSource, s: &config::GyroToMouseSettings) -> Element<'static, Message> {
    column![
        output_mouse(input, s.output.clone()),
        space(input, s.space.clone()),
        sensitivity(input, s.sensitivity.clone()),
        acceleration(input, s.acceleration.factor),
        smoothing(input, &s.smoothing),
        deadzone(input, s.deadzone.inner),
        invert(input, s.invert.clone()),
        rotation(input, s.rotation.degrees),
        activation(input, &s.activation),
    ]
    .spacing(16.0)
    .into()
}

fn trigger_view(input: &InputSource, s: &config::TriggerSettings) -> Element<'static, Message> {
    column![
        output_trigger(input, s.output.clone()),
        soft_pull(input, s.soft_pull.threshold),
        curve(input, &s.curve),
        deadzone(input, s.deadzone.inner),
    ]
    .spacing(16.0)
    .into()
}

// --- reusable settings blocks ---------------------------------------------------------------

fn output_stick(input: &InputSource, o: StickOutput) -> Element<'static, Message> {
    pick_setting(
        "Output",
        o,
        vec![StickOutput::Left, StickOutput::Right, StickOutput::None],
        |o: &StickOutput| stick_output_label(o).to_string(),
        input,
        SettingEdit::StickOutput,
    )
}

fn output_trigger(input: &InputSource, o: TriggerOutput) -> Element<'static, Message> {
    pick_setting(
        "Output",
        o,
        vec![TriggerOutput::Left, TriggerOutput::Right, TriggerOutput::None],
        |o: &TriggerOutput| trigger_output_label(o).to_string(),
        input,
        SettingEdit::TriggerOutput,
    )
}

fn output_mouse(input: &InputSource, o: MouseOutput) -> Element<'static, Message> {
    pick_setting(
        "Output",
        o,
        vec![MouseOutput::Cursor, MouseOutput::Scroll, MouseOutput::SmoothScroll],
        |o: &MouseOutput| mouse_output_label(o).to_string(),
        input,
        SettingEdit::MouseOutput,
    )
}

fn layout(input: &InputSource, l: DpadLayout) -> Element<'static, Message> {
    pick_setting(
        "Layout",
        l,
        vec![DpadLayout::FourWay, DpadLayout::EightWay],
        |l: &DpadLayout| layout_label(l).to_string(),
        input,
        SettingEdit::Layout,
    )
}

fn space(input: &InputSource, s: GyroSpace) -> Element<'static, Message> {
    pick_setting(
        "Space",
        s,
        vec![GyroSpace::Yaw, GyroSpace::Roll, GyroSpace::YawRoll, GyroSpace::PlayerSpace],
        |s: &GyroSpace| space_label(s).to_string(),
        input,
        SettingEdit::Space,
    )
}

fn outer_ring(input: &InputSource, radius: f32) -> Element<'static, Message> {
    slider_setting("Outer ring", radius, 0.0..=1.0, 0.01, format!("{radius:.2}"), input, SettingEdit::OuterRing)
}

fn soft_pull(input: &InputSource, threshold: f32) -> Element<'static, Message> {
    slider_setting("Soft pull", threshold, 0.0..=1.0, 0.01, format!("{threshold:.2}"), input, SettingEdit::SoftPull)
}

fn deadzone(input: &InputSource, inner: f32) -> Element<'static, Message> {
    slider_setting("Deadzone", inner, 0.0..=1.0, 0.01, format!("{inner:.2}"), input, SettingEdit::Deadzone)
}

fn anti_deadzone(input: &InputSource, amount: f32) -> Element<'static, Message> {
    slider_setting("Anti-deadzone", amount, 0.0..=1.0, 0.01, format!("{amount:.2}"), input, SettingEdit::AntiDeadzone)
}

fn acceleration(input: &InputSource, factor: f32) -> Element<'static, Message> {
    slider_setting("Acceleration", factor, 0.0..=0.2, 0.005, format!("{factor:.3}"), input, SettingEdit::Acceleration)
}

fn rotation(input: &InputSource, degrees: f32) -> Element<'static, Message> {
    slider_setting("Rotation", degrees, -180.0..=180.0, 1.0, format!("{degrees:.0}°"), input, SettingEdit::Rotation)
}

fn sensitivity(input: &InputSource, s: Sensitivity) -> Element<'static, Message> {
    column![
        slider_setting("Sensitivity X", s.x, 0.0..=10.0, 0.1, format!("{:.1}", s.x), input, SettingEdit::SensitivityX),
        slider_setting("Sensitivity Y", s.y, 0.0..=10.0, 0.1, format!("{:.1}", s.y), input, SettingEdit::SensitivityY),
    ]
    .spacing(8.0)
    .into()
}

fn invert(input: &InputSource, inv: Invert) -> Element<'static, Message> {
    let ix = input.clone();
    let iy = input.clone();
    let x = checkbox(inv.x).on_toggle(move |b| Message::Editor(EditorMessage::SetSetting(ix.clone(), SettingEdit::InvertX(b))));
    let y = checkbox(inv.y).on_toggle(move |b| Message::Editor(EditorMessage::SetSetting(iy.clone(), SettingEdit::InvertY(b))));
    row![setting_label("Invert"), x, text("X"), Space::new().width(16.0), y, text("Y")]
        .spacing(8.0)
        .align_y(Center)
        .into()
}

/// Curve: a Linear/Power kind picker, plus an exponent slider when Power.
fn curve(input: &InputSource, curve: &Curve) -> Element<'static, Message> {
    let kind = if matches!(curve, Curve::Power(_)) { CurveKind::Power } else { CurveKind::Linear };
    let i1 = input.clone();
    let combo = pick_list(Some(kind), vec![CurveKind::Linear, CurveKind::Power], |k: &CurveKind| {
        k.label().to_string()
    })
    .on_select(move |k| {
        let c = match k {
            CurveKind::Linear => Curve::Linear,
            CurveKind::Power => Curve::Power(1.0),
        };
        Message::Editor(EditorMessage::SetSetting(i1.clone(), SettingEdit::Curve(c)))
    })
    .width(CMD_SLOT);
    let mut col = column![row![setting_label("Curve"), combo].spacing(12.0).align_y(Center)].spacing(8.0);
    if let Curve::Power(e) = *curve {
        let i2 = input.clone();
        col = col.push(
            row![
                setting_label("Exponent"),
                slider(0.2..=5.0f32, e, move |v| Message::Editor(EditorMessage::SetSetting(
                    i2.clone(),
                    SettingEdit::Curve(Curve::Power(v))
                )))
                .step(0.05f32),
                text(format!("{e:.2}")).width(70.0),
            ]
            .spacing(12.0)
            .align_y(Center),
        );
    }
    col.into()
}

/// Smoothing: an enable checkbox gating min-cutoff / beta sliders.
fn smoothing(input: &InputSource, sm: &Option<OneEuroFilter>) -> Element<'static, Message> {
    let on = sm.is_some();
    let f = sm.clone().unwrap_or_default();
    let i0 = input.clone();
    let check = checkbox(on)
        .on_toggle(move |b| Message::Editor(EditorMessage::SetSetting(i0.clone(), SettingEdit::SmoothingEnabled(b))));
    let mut col = column![row![setting_label("Smoothing"), check].spacing(12.0).align_y(Center)].spacing(8.0);
    if on {
        col = col.push(slider_setting("Min cutoff", f.min_cutoff, 0.1..=10.0, 0.1, format!("{:.1}", f.min_cutoff), input, SettingEdit::SmoothingMinCutoff));
        col = col.push(slider_setting("Beta", f.beta, 0.0..=2.0, 0.05, format!("{:.2}", f.beta), input, SettingEdit::SmoothingBeta));
    }
    col.into()
}

/// Activation: the mode picker + the gater set (chips + the Button picker), OR-combined held buttons
/// that gate the behaviour.
fn activation(input: &InputSource, a: &Activation) -> Element<'static, Message> {
    let i = input.clone();
    let combo = pick_list(
        Some(a.mode.clone()),
        vec![ActivationMode::HoldToDisable, ActivationMode::HoldToEnable],
        |m: &ActivationMode| activation_mode_label(m).to_string(),
    )
    .on_select(move |m| Message::Editor(EditorMessage::SetSetting(i.clone(), SettingEdit::ActivationMode(m))))
    .width(CMD_SLOT);
    let rm = input.clone();
    let add = input.clone();
    let gaters = row![
        setting_label("Gaters"),
        super::button_chips(
            &a.gaters,
            move |j| Message::Editor(EditorMessage::SetSetting(rm.clone(), SettingEdit::RemoveGater(j))),
            Message::OpenButtonPicker(crate::ButtonTarget::Gater(add.clone())),
        ),
    ]
    .spacing(12.0)
    .align_y(Center);
    column![row![setting_label("Activation"), combo].spacing(12.0).align_y(Center), gaters]
        .spacing(8.0)
        .into()
}

// --- settings-block helpers -----------------------------------------------------------------

/// A pick-list settings row: fixed label + a combobox that emits one [`SettingEdit`].
fn pick_setting<T>(
    label: &'static str,
    selected: T,
    options: Vec<T>,
    to_label: impl Fn(&T) -> String + 'static,
    input: &InputSource,
    make: fn(T) -> SettingEdit,
) -> Element<'static, Message>
where
    T: Clone + PartialEq + 'static,
{
    let input = input.clone();
    let combo = pick_list(Some(selected), options, to_label)
        .on_select(move |v: T| Message::Editor(EditorMessage::SetSetting(input.clone(), make(v))))
        .width(CMD_SLOT);
    row![setting_label(label), combo].spacing(12.0).align_y(Center).into()
}

/// An `f32` slider settings row: fixed label, step-snapped slider, preformatted readout. Emits one
/// [`SettingEdit`] (built by `make`) per change.
fn slider_setting(
    label: &'static str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    readout: String,
    input: &InputSource,
    make: fn(f32) -> SettingEdit,
) -> Element<'static, Message> {
    let input = input.clone();
    row![
        setting_label(label),
        slider(range, value, move |v| Message::Editor(EditorMessage::SetSetting(input.clone(), make(v)))).step(step),
        text(readout).width(70.0),
    ]
    .spacing(12.0)
    .align_y(Center)
    .into()
}

/// Curve kind (the pick-list value; the exponent lives on a separate slider). UI-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurveKind {
    Linear,
    Power,
}

impl CurveKind {
    fn label(self) -> &'static str {
        match self {
            CurveKind::Linear => "Linear",
            CurveKind::Power => "Power",
        }
    }
}

// UI-owned display labels for the settings enums (`config` stays presentation-free).

fn stick_output_label(o: &StickOutput) -> &'static str {
    match o {
        StickOutput::Left => "Left stick",
        StickOutput::Right => "Right stick",
        StickOutput::None => "None (ring only)",
    }
}

fn trigger_output_label(o: &TriggerOutput) -> &'static str {
    match o {
        TriggerOutput::Left => "Left trigger",
        TriggerOutput::Right => "Right trigger",
        TriggerOutput::None => "None (soft-pull only)",
    }
}

fn mouse_output_label(o: &MouseOutput) -> &'static str {
    match o {
        MouseOutput::Cursor => "Cursor",
        MouseOutput::Scroll => "Scroll",
        MouseOutput::SmoothScroll => "Smooth scroll",
    }
}

fn layout_label(l: &DpadLayout) -> &'static str {
    match l {
        DpadLayout::FourWay => "4-way",
        DpadLayout::EightWay => "8-way",
    }
}

fn space_label(s: &GyroSpace) -> &'static str {
    match s {
        GyroSpace::Yaw => "Yaw",
        GyroSpace::Roll => "Roll",
        GyroSpace::YawRoll => "Yaw + Roll",
        GyroSpace::PlayerSpace => "Player space",
    }
}

fn activation_mode_label(m: &ActivationMode) -> &'static str {
    match m {
        ActivationMode::HoldToDisable => "Hold to disable",
        ActivationMode::HoldToEnable => "Hold to enable",
    }
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
    // The gear opens the behaviour settings page (active only when the behaviour has settings);
    // built before the combo's closure consumes `input`.
    let gear_el = behavior_gear(&input, current);
    // On a layer the empty `Unbound` choice reads as "Inherited"; otherwise its own label ("None").
    let combo = pick_list(Some(current), options, move |b: &Behavior| {
        if on_layer && *b == Behavior::Unbound { "Inherited".to_string() } else { b.label().to_string() }
    })
    .on_select(move |b| Message::Editor(EditorMessage::SetBehavior(input.clone(), b)))
    .width(CMD_SLOT);
    // The "Behavior" label dims when this layer entry is inherited (the passthrough state).
    let inherited = on_layer && current == Behavior::Unbound;
    let name = if inherited { text("Behavior").style(style::muted_text) } else { text("Behavior") };
    let inner = row![name, Space::new().width(Fill), combo, gear_el].spacing(12.0).align_y(Center);
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
        Regular { .. } => None,
        Long { hold_ms } => Some(format!("Long press: {hold_ms}ms")),
        Double { window_ms } => Some(format!("Double press: {window_ms}ms")),
        Start => Some("Start press".into()),
        Release => Some("Release press".into()),
    }
}

/// An active gear button that opens a menu on press.
fn gear_menu(msg: Message) -> Element<'static, Message> {
    button(text("⚙").size(16.0)).style(style::combo_button).on_press(msg).into()
}

/// The behaviour-row gear: opens the per-behaviour settings page when the behaviour has settings,
/// else an inert (greyed) gear. Only for the behaviour row — a plain Button's gear is a command menu.
fn behavior_gear(input: &InputSource, current: Behavior) -> Element<'static, Message> {
    if current.has_settings() {
        gear_menu(Message::Editor(EditorMessage::OpenBehaviorSettings(input.clone())))
    } else {
        gear(false)
    }
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
