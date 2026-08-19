//! Settings screen — a **UI** section (theme, tray, profiles directory) and a **Daemon** section
//! (the on-connect behaviour + the two profile paths, each greyed when its toggle is off).

use iced::widget::{Space, button, checkbox, column, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Theme};

use super::{group_header, section_header, small};
use crate::settings::ShowInputs;
use crate::{App, Message, style};

/// The application-settings page (Category::Settings).
pub(super) fn settings_screen(app: &App) -> Element<'_, Message> {
    let s = &app.settings;

    let theme_pick = row![
        text("Theme:").size(14.0),
        pick_list(Some(app.active_theme()), Theme::ALL, |t: &Theme| t.to_string())
            .on_select(Message::SetTheme)
            .menu_style(style::combo_menu),
    ]
    .spacing(8.0)
    .align_y(Center);

    // Tray: the master toggle plus two options greyed out until it's on.
    let use_tray = checkbox(s.use_tray)
        .label("Enable the system tray icon")
        .on_toggle(Message::ToggleUseTray);
    let mut close_to_tray = checkbox(s.close_to_tray).label("Close to tray (hide instead of quit)");
    let mut start_hidden = checkbox(s.start_hidden).label("Start hidden in the tray");
    if s.use_tray {
        close_to_tray = close_to_tray.on_toggle(Message::ToggleCloseToTray);
        start_hidden = start_hidden.on_toggle(Message::ToggleStartHidden);
    }

    // Which controller's inputs the profile editor shows (per-source; Auto follows the bound device).
    let show_inputs = row![
        text("Show inputs:").size(14.0),
        pick_list(Some(s.show_inputs), ShowInputs::ALL, |v: &ShowInputs| v.label().to_string())
            .on_select(Message::SetShowInputs)
            .menu_style(style::combo_menu),
    ]
    .spacing(8.0)
    .align_y(Center);

    // Custom profiles directory: a toggle plus a path row greyed out until it's on.
    let use_custom_dir = checkbox(s.use_custom_profile_dir)
        .label("Use a custom profile directory")
        .on_toggle(Message::ToggleCustomProfileDir);
    let custom_dir_row = path_row(
        "profiles directory",
        &s.custom_profile_dir,
        s.use_custom_profile_dir,
        Message::CustomProfileDirChanged,
        Message::BrowseCustomProfileDir,
    );

    let launch = checkbox(s.start_daemon)
        .label("Launch the daemon if not running on connect attempt")
        .on_toggle(Message::ToggleStartDaemon);
    let load_main = checkbox(s.load_main)
        .label("Load the main profile on connect")
        .on_toggle(Message::ToggleLoadMain);
    let main_row = path_row(
        "path to .ron file",
        &s.main_path,
        s.load_main,
        Message::MainPathChanged,
        Message::BrowseMain,
    );
    let load_fb = checkbox(s.load_fallback)
        .label("Load the fallback profile on connect")
        .on_toggle(Message::ToggleLoadFallback);
    let fb_row = path_row(
        "path to .ron file",
        &s.fallback_path,
        s.load_fallback,
        Message::FallbackPathChanged,
        Message::BrowseFallback,
    );
    let restore_io = checkbox(s.restore_io)
        .label("Restore last input/output on connect")
        .on_toggle(Message::ToggleRestoreIo);
    let start_engine = checkbox(s.start_engine)
        .label("Start the engine on connect")
        .on_toggle(Message::ToggleStartEngine);

    column![
        section_header("Application settings"),
        group_header("UI"),
        theme_pick,
        show_inputs,
        use_tray,
        close_to_tray,
        start_hidden,
        use_custom_dir,
        custom_dir_row,
        Space::new().height(8.0),
        group_header("Daemon"),
        small("The options below are applied only on daemon connect — usually equivalent to application start."),
        launch,
        load_main,
        main_row,
        load_fb,
        fb_row,
        restore_io,
        start_engine,
    ]
    .spacing(10.0)
    .into()
}

/// A path row: text field + Browse button, both inert (greyed) when `enabled` is false.
fn path_row<'a>(
    placeholder: &'a str,
    path: &'a str,
    enabled: bool,
    on_change: fn(String) -> Message,
    browse: Message,
) -> Element<'a, Message> {
    let mut input = text_input(placeholder, path).width(Fill);
    if enabled {
        input = input.on_input(on_change);
    }
    let mut btn = button(text("Browse…")).style(button::secondary);
    if enabled {
        btn = btn.on_press(browse);
    }
    row![input, btn].spacing(8.0).align_y(Center).into()
}
