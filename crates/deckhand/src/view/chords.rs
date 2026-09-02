//! Chords screen - a live editor over the UI-owned [`Chords`](config::Chords). Every edit persists
//! to `chords.ron` and ships to the daemon (`App::apply_chords`); this always renders `app.chords`
//! (the source of truth), never the daemon's status snapshot.

use config::{Chord, ChordAction, SwitchMode};
use iced::widget::{Space, button, column, pick_list, row, text, text_input};
use iced::{Center, Element, Fill};

use super::{button_chips, section_header};
use crate::{App, ButtonTarget, ChordActionKind, Message, style};

/// The chords page (Category::Chords): one bar per chord (trigger chips + action) and an "Add chord"
/// button. Chords are AND-combined buttons firing a [`ChordAction`] (profile switch / run command).
pub(super) fn chords_screen(app: &App) -> Element<'_, Message> {
    let mut list = column![].spacing(8.0);
    for (i, chord) in app.chords.chords.iter().enumerate() {
        list = list.push(chord_bar(i, chord));
    }
    list = list.push(button(text("Add chord")).style(button::secondary).on_press(Message::ChordAdd));
    column![section_header("Chords"), list].spacing(16.0).into()
}

/// One chord bar: the trigger buttons (chips + picker), the action-kind combobox and its detail
/// (switch mode / command line), and a x to remove the whole chord.
fn chord_bar(i: usize, chord: &Chord) -> Element<'static, Message> {
    let trigger = button_chips(
        &chord.buttons,
        move |j| Message::ChordRemoveButton(i, j),
        Message::OpenButtonPicker(ButtonTarget::Chord(i)),
    );

    let kind = match chord.action {
        ChordAction::SwitchProfile { .. } => ChordActionKind::SwitchProfile,
        ChordAction::CommandExecute { .. } => ChordActionKind::CommandExecute,
    };
    let kind_combo = pick_list(
        Some(kind),
        vec![ChordActionKind::SwitchProfile, ChordActionKind::CommandExecute],
        |k: &ChordActionKind| chord_kind_label(k).to_string(),
    )
    .on_select(move |k| Message::ChordSetKind(i, k))
    .menu_style(style::combo_menu)
    .width(150.0);

    let detail: Element<'static, Message> = match &chord.action {
        ChordAction::SwitchProfile { mode } => pick_list(
            Some(mode.clone()),
            vec![SwitchMode::HoldFallback, SwitchMode::Toggle, SwitchMode::SetMain, SwitchMode::SetFallback],
            |m: &SwitchMode| switch_mode_label(m).to_string(),
        )
        .on_select(move |m| Message::ChordSetMode(i, m))
        .menu_style(style::combo_menu)
        .width(150.0)
        .into(),
        ChordAction::CommandExecute { command, args } => text_input("command args…", command_line(command, args))
            .on_input(move |s| Message::ChordSetCommandLine(i, s))
            .width(240.0)
            .into(),
    };

    let remove = button(text("✕").size(15.0)).style(style::combo_button).on_press(Message::ChordRemove(i));

    super::card(
        row![trigger, Space::new().width(Fill), kind_combo, detail, remove]
            .spacing(12.0)
            .align_y(Center),
    )
}

/// The command line shown/edited for a `CommandExecute` chord: command + args joined by a single
/// space. Paired with the split-on-`' '` parse in `App::update` so the field's exact text round-trips.
fn command_line(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string()).chain(args.iter().cloned()).collect::<Vec<_>>().join(" ")
}

fn chord_kind_label(k: &ChordActionKind) -> &'static str {
    match k {
        ChordActionKind::SwitchProfile => "Switch profile",
        ChordActionKind::CommandExecute => "Run command",
    }
}

fn switch_mode_label(m: &SwitchMode) -> &'static str {
    match m {
        SwitchMode::HoldFallback => "Hold fallback",
        SwitchMode::Toggle => "Toggle",
        SwitchMode::SetMain => "Set main",
        SwitchMode::SetFallback => "Set fallback",
    }
}
