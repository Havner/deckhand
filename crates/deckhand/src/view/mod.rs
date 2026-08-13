//! The iced widget layer: the four-region window and the per-category screens.
//!
//! This module owns the **chrome** — the top daemon bar, left sidebar, scrollable content pane, and
//! bottom status bar — and dispatches the content pane to a per-category screen. The screens live in
//! submodules: [`profiles`] (profile management), [`settings`], [`globals`], and [`editor`] (the
//! profile-edit pages — the Profile page plus the still-mock per-input tabs).
//!
//! Small shared building blocks (headings, body/caption/monospace text, the panel card, the status
//! separator, the modal shell) stay here; the submodules reach them via `super::`.

mod editor;
mod globals;
pub(crate) mod modal;
mod profiles;
mod settings;

use iced::widget::{Space, button, column, container, pick_list, row, rule, scrollable, text};
use iced::{Center, Element, Fill, Theme};
use ipc::{ProfileRole, RunState};

use crate::editor::EditorMessage;
use crate::nav::Category;
use crate::{App, INPUT_PRESETS, Message, NETWORK_OPTION, OUTPUT_PRESETS, daemon, style};

/// The whole window: top bar / (sidebar + content) / bottom bar, with a modal layered on top when
/// one is open.
pub(crate) fn view(app: &App) -> Element<'_, Message> {
    let base: Element<'_, Message> = column![
        top_bar(app),
        row![sidebar(app), content(app)].height(Fill),
        bottom_bar(app),
    ]
    .into();
    modal::overlay(app, base)
}

// --- shared building blocks (used across the screen submodules via `super::`) ---------------

/// A screen's big title (e.g. "Settings", "Buttons") — the largest heading on a page.
fn section_header<'a>(title: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(title).size(24.0).into()
}

/// A sub-group heading within a screen (e.g. "UI", "Face Buttons", "Profile") — the smaller
/// subtitle under a [`section_header`].
fn group_header<'a>(title: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(title).size(20.0).into()
}

/// Regular body copy inside a content screen (13 px) — the unified default text size for pure text
/// (paragraphs, captions, readouts); not for control/combobox labels.
fn body<'a>(fragment: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(fragment).size(13.0).into()
}

/// Small/secondary copy inside a content screen (12 px) — captions and hints. Same use as [`body`].
fn small<'a>(fragment: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(fragment).size(12.0).into()
}

/// Monospaced copy (12 px) — command lines and other verbatim text. Same use as [`body`].
fn monospace<'a>(fragment: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(fragment).size(12.0).font(iced::Font::MONOSPACE).into()
}

/// Wrap a row as a padded card so grouped list rows read like the Steam UI.
fn card<'a>(inner: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(inner).padding(10.0).width(Fill).style(style::panel).into()
}

/// The status-bar separator glyph.
fn sep() -> Element<'static, Message> {
    text("│").size(13.0).style(style::muted_text).into()
}

// --- top bar --------------------------------------------------------------------------------

/// Full-width daemon bar: Start/Stop on the left, input/output pickers on the right.
fn top_bar(app: &App) -> Element<'_, Message> {
    let state = app.status.as_ref().map(|s| s.state);
    // State-driven enablement (disabled = no on_press).
    let start = matches!(state, Some(RunState::Idle)).then_some(Message::Start);
    let stop = matches!(state, Some(RunState::Running | RunState::WaitingForDevice))
        .then_some(Message::Stop);

    let controls = row![
        button(text("Start")).on_press_maybe(start).style(button::success),
        button(text("Stop")).on_press_maybe(stop).style(button::danger),
    ]
    .spacing(8.0);

    // Input presets + the daemon's live device ids + the `<network>` entry (opens the host:port
    // popup). A staged network spec shows in the box even though it isn't a listed option.
    let mut inputs: Vec<String> = INPUT_PRESETS.iter().map(|s| s.to_string()).collect();
    inputs.extend(app.devices.iter().cloned());
    inputs.push(NETWORK_OPTION.to_string());
    let selected_input = app.status.as_ref().map(|s| s.input.clone());
    let input_pick = pick_list(selected_input, inputs, String::clone)
        .on_select(Message::InputSelected)
        .placeholder("input")
        .width(200.0);

    let mut outputs: Vec<String> = OUTPUT_PRESETS.iter().map(|s| s.to_string()).collect();
    outputs.push(NETWORK_OPTION.to_string());
    let selected_output = app.status.as_ref().map(|s| s.output.clone());
    let output_pick = pick_list(selected_output, outputs, String::clone)
        .on_select(Message::OutputSelected)
        .placeholder("output")
        .width(200.0);

    // Manual device re-enumeration (no USB hotplug).
    let refresh = button(text("⟳")).on_press(Message::Refresh);

    // Left side: the loaded profile's name + Main/Fallback, which send the **in-memory** edited
    // profile to a role (distinct from the Profiles page, which sends the selected on-disk file).
    // Shown only while a profile is loaded for editing.
    let editing: Element<'_, Message> = if app.is_editing() {
        row![
            text(app.editing_name()).size(13.0),
            sep(),
            button(text("Set as Main")).style(button::success).on_press(Message::Editor(EditorMessage::SendToRole(ProfileRole::Main))),
            button(text("Set as Fallback")).style(button::primary).on_press(Message::Editor(EditorMessage::SendToRole(ProfileRole::Fallback))),
        ]
        .spacing(8.0)
        .align_y(Center)
        .into()
    } else {
        Space::new().into()
    };

    // Editing controls on the left; I/O selectors + Start/Stop group right-aligned.
    let bar = row![
        editing,
        Space::new().width(Fill),
        refresh,
        text("Input:").size(13.0),
        input_pick,
        text("Output:").size(13.0),
        output_pick,
        controls,
    ]
    .spacing(8.0)
    .align_y(Center)
    .padding(8.0);

    container(bar).style(container::dark).width(Fill).into()
}

// --- sidebar --------------------------------------------------------------------------------

/// Left sidebar: the profile-editor bands (Profile / per-input pages / Rumble, each split by a rule,
/// with the action-set/layer selector heading the input band) at the top; profile management +
/// app-level pages (Profiles / Globals / Settings) pinned at the bottom.
fn sidebar(app: &App) -> Element<'_, Message> {
    // Top: the editor bands, a rule between each. The action-set/layer selector heads the per-input
    // band (index 1); always shown (inert when no profile is loaded) so the layout never shifts.
    let mut top = column![].spacing(4.0);
    for (i, band) in Category::EDITOR_BANDS.iter().enumerate() {
        if i > 0 {
            top = top.push(rule::horizontal(1));
        }
        if i == 1 {
            top = top.push(editor::action_set_selector(app));
        }
        for &c in *band {
            top = top.push(nav_button(app, c));
        }
    }
    // Bottom: profile management + app-level pages (Profiles / Globals / Settings), no separators.
    let mut bottom = column![].spacing(4.0);
    for &c in Category::BOTTOM {
        bottom = bottom.push(nav_button(app, c));
    }

    let col = column![top, Space::new().height(Fill), bottom]
        .padding(8.0)
        .spacing(8.0)
        .width(Fill);

    container(col).width(180.0).height(Fill).style(style::panel).into()
}

/// One sidebar entry; the selected one gets the primary style. Profile-editor categories are
/// disabled (no `on_press` → iced greys them) until a profile is loaded for editing.
fn nav_button(app: &App, c: Category) -> Element<'static, Message> {
    let enabled = !c.is_editor() || app.is_editing();
    let style = if app.category == c { button::primary } else { button::text };
    let mut btn = button(text(c.label())).width(Fill).padding(8.0).style(style);
    if enabled {
        btn = btn.on_press(Message::Navigate(c));
    }
    btn.into()
}

// --- content dispatch -----------------------------------------------------------------------

/// The scrollable content pane; swaps entirely on the selected category.
fn content(app: &App) -> Element<'_, Message> {
    let inner: Element<'_, Message> = match app.category {
        Category::Profiles => profiles::profiles_screen(app),
        Category::Profile => editor::profile_screen(app),
        Category::Rumble => editor::rumble_screen(),
        Category::Settings => settings::settings_screen(app),
        Category::Globals => globals::globals_screen(app),
        // The per-input editor pages (Buttons/Triggers/Joysticks/Trackpads/Gyro) are data-driven
        // mockups rendered from the category's input groups.
        cat => editor::input_screen(app, cat),
    };
    scrollable(container(inner).padding(16.0).width(Fill)).width(Fill).height(Fill).into()
}

// --- bottom bar -----------------------------------------------------------------------------

/// Full-width status bar from the engine status (device bound, profiles, chord count, …).
fn bottom_bar(app: &App) -> Element<'_, Message> {
    // Dot color from theme roles: success (green, matching Start) / danger (red, matching Stop).
    // "managed" (green dot) when the UI launched the daemon and will shut it down on exit; a plain
    // externally-started daemon reads "connected".
    let (dot, label): (fn(&Theme) -> text::Style, _) = if app.connected {
        let label = if daemon::is_managed(&app.daemon) { "managed" } else { "connected" };
        (style::success_text, label)
    } else {
        (style::danger_text, "disconnected")
    };
    let conn = row![text("●").size(13.0).style(dot), text(label).size(13.0)]
        .spacing(6.0)
        .align_y(Center);
    let mut bar = row![conn].spacing(10.0).align_y(Center).padding(8.0);

    if let Some(s) = &app.status {
        // Controller-presence dot, left of the device id: green = connected, red = disconnected,
        // and *no dot* when there's no local reader (`controller: None` — idle, or the network
        // server role), so a stopped engine shows just "device: —".
        let device: Element<'_, Message> = {
            let id = text(format!("device: {}", s.bound.as_deref().unwrap_or("—"))).size(13.0);
            match s.controller {
                Some(present) => {
                    let dot: fn(&Theme) -> text::Style =
                        if present { style::success_text } else { style::danger_text };
                    row![text("●").size(13.0).style(dot), id].spacing(6.0).align_y(Center).into()
                }
                None => id.into(),
            }
        };
        bar = bar
            .push(sep())
            .push(text(format!("state: {:?}", s.state)).size(13.0))
            .push(sep())
            .push(device)
            .push(sep())
            .push(role_label("main", s.main.as_deref(), s.active == Some(ProfileRole::Main)))
            .push(sep())
            .push(role_label("fallback", s.fallback.as_deref(), s.active == Some(ProfileRole::Fallback)))
            .push(sep())
            .push(text(format!("chords: {}", s.globals.chords.len())).size(13.0));
    }
    if let Some(err) = &app.error {
        bar = bar
            .push(Space::new().width(Fill))
            .push(text(format!("⚠ {err}")).size(13.0).style(style::danger_text));
    }

    container(bar).style(container::dark).width(Fill).into()
}

/// A bottom-bar profile slot — `"<name>: <profile>"` (or `—` when the role is unassigned/
/// disconnected), with a green ▶ prefix when it's the **live** role. Literal: the arrow follows
/// `status.active` verbatim (it can sit on an empty `—` when a chord/boot put us in an empty slot),
/// per the design decision. Only one of main/fallback is ever active, so at most one arrow shows.
fn role_label(name: &str, profile: Option<&str>, active: bool) -> Element<'static, Message> {
    let label = text(format!("{name}: {}", profile.unwrap_or("—"))).size(13.0);
    if active {
        row![text("▶").size(13.0).style(style::success_text), label]
            .spacing(4.0)
            .align_y(Center)
            .into()
    } else {
        label.into()
    }
}
