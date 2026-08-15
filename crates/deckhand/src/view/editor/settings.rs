//! The editor's **settings sub-pages** — a focused full-width form rendered *instead of* the current
//! category page (reached from a gear menu, left via Back). Two kinds:
//!
//! - **Command settings** ([`command_settings`]) — the activator (kind + its own parameter) plus
//!   toggle / turbo / haptics.
//! - **Per-behaviour settings** ([`behavior_settings`]) — a header over the behaviour's settings
//!   **blocks** composed in the canonical field order (the per-behaviour compose fns). Each block is
//!   a reusable row (shared across every behaviour that has that field) that emits one generic
//!   `SetSetting(input, SettingEdit)`; the edit is applied by `editor::settings::apply_setting`.
//!   Adding a behaviour = one compose fn from existing blocks.
//!
//! Form primitives ([`setting_label`], [`check_setting`], [`slider_row`], …) live here; the shared
//! `CMD_SLOT` width and the page's `input_label` come from the parent module.

use std::ops::RangeInclusive;

use iced::widget::{Space, button, checkbox, column, container, pick_list, row, slider, text};
use iced::{Center, Element, Fill};

use config::{
    Activation, ActivationMode, Activator, Command, Curve, DpadLayout, GyroSpace, HapticEdge,
    HapticStrength, InputSource, Invert, MouseOutput, OneEuroFilter, Sensitivity, SourceBinding,
    StickOutput, TriggerOutput,
};

use super::{CMD_SLOT, input_label};
use crate::editor::{ActivatorKind, Behavior, CommandRef, EditorMessage, SettingEdit, SettingsView};
use crate::view::{button_chips, label_row, slot_display, small};
use crate::{App, ButtonTarget, Message, style};

/// The active settings sub-page, rendered *instead of* the current category page — `None` when no
/// settings page is open (the caller then renders the normal category). The general settings-page
/// paradigm: a focused full-width form reached from a gear menu, left via Back.
pub(in crate::view) fn settings_screen(app: &App) -> Option<Element<'static, Message>> {
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
            Some(slider_row("Double window", window_ms, 100..=1000, 25, move |v| {
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

/// A fixed-width settings-row label so the controls line up down the form. `pub(super)` because the
/// Rumble page (in the parent module) shares the same form aesthetic.
pub(super) fn setting_label(s: &'static str) -> Element<'static, Message> {
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
        button_chips(
            &a.gaters,
            move |j| Message::Editor(EditorMessage::SetSetting(rm.clone(), SettingEdit::RemoveGater(j))),
            Message::OpenButtonPicker(ButtonTarget::Gater(add.clone())),
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
    range: RangeInclusive<f32>,
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
