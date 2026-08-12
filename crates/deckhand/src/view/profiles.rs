//! Profiles screen — profile *management*: pick a profile (from the directory or off-disk), then
//! send it to / clear a daemon role, or load it for editing / unload it.

use std::path::Path;

use iced::widget::{Space, button, column, container, pick_list, row, text};
use iced::{Center, Element, Fill, Theme};
use ipc::ProfileRole;

use super::{body, group_header, monospace, section_header, small};
use crate::{App, Message, style};

/// The profile-management page (Category::Profiles).
pub(super) fn profiles_screen(app: &App) -> Element<'_, Message> {
    // Row 1: refresh + a wide combobox of the directory's files + an off-disk picker. Every entry
    // (listed file *or* a picked absolute path) renders as its basename to keep the control tidy; a
    // caption below shows the full resolved path so the current selection is never ambiguous.
    let refresh = button(text("⟳")).on_press(Message::ProfileRefresh);
    let combo = pick_list(app.selected_profile.clone(), app.profile_files.clone(), |s: &String| {
        Path::new(s).file_name().and_then(|n| n.to_str()).unwrap_or(s.as_str()).to_string()
    })
    .on_select(Message::ProfileSelected)
    .placeholder("select a profile")
    .width(Fill);
    let disk = button(text("Select from disk")).style(button::secondary).on_press(Message::ProfileBrowse);
    let row1 = row![refresh, combo, disk].spacing(8.0).align_y(Center);

    // The full resolved path of the current selection — the unambiguous "this is what the buttons
    // below act on" line (a placeholder when nothing is selected). Kept at the normal text color;
    // the small size alone reads as secondary (the muted role was near-invisible).
    let caption: Element<'_, Message> = match app.selected_profile_path() {
        Some(p) => small(p.display().to_string()),
        None => small("no profile selected"),
    };
    let selector = column![row1, caption].spacing(4.0);

    // A 2×4 grid of equal-width action buttons. `on` gates the button (no `on_press` → greyed).
    // A short fixed gap between the left pair (role assign/clear) and the right pair (duplicate/
    // create · edit/unload) visually groups the two halves without a full empty column.
    let cell = |label, style: fn(&Theme, button::Status) -> button::Style, msg, on: bool| {
        let b = button(text(label).center()).width(Fill).style(style);
        if on { b.on_press(msg) } else { b }
    };
    let gap = || Space::new().width(24.0);
    let has_sel = app.selected_profile.is_some();
    let loaded = app.is_editing();
    // What the daemon currently has on each role (by profile *name*, from status); `None` when a
    // role is empty or the daemon is disconnected.
    let (main, fallback) = app
        .status
        .as_ref()
        .map(|s| (s.main.clone(), s.fallback.clone()))
        .unwrap_or((None, None));
    // Top row: act on the **selected on-disk** profile (need a selection) · Duplicate it · Edit it.
    let grid_top = row![
        cell("Set file as Main", button::success, Message::SendProfile(ProfileRole::Main), has_sel),
        cell("Set file as Fallback", button::primary, Message::SendProfile(ProfileRole::Fallback), has_sel),
        gap(),
        cell("Duplicate", button::secondary, Message::ProfileDuplicate, has_sel),
        cell("Edit profile", button::warning, Message::EditProfile, has_sel),
    ]
    .spacing(8.0);
    // Bottom row: clear a role (only when that role has a profile) · Create new (always) · unload
    // the loaded profile.
    let grid_bot = row![
        cell("Clear Main", button::secondary, Message::ClearProfile(ProfileRole::Main), main.is_some()),
        cell("Clear Fallback", button::secondary, Message::ClearProfile(ProfileRole::Fallback), fallback.is_some()),
        gap(),
        cell("Create new", button::secondary, Message::ProfileCreateNew, true),
        cell("Stop editing", button::warning, Message::StopEditing, loaded),
    ]
    .spacing(8.0);

    // The role assignments, mirrored here so you have context while managing/editing — e.g. what
    // "Set as …" would replace. Mirrors the bottom bar; `—` when empty/disconnected. A green ▶ marks
    // the **live** role (from `status.active`; literal — may sit on an empty role), in a fixed-width
    // leading slot so both lines align; absent entirely when the engine is stopped (`active: None`).
    let active = app.status.as_ref().and_then(|s| s.active);
    let applied_row = |label: String, is_active: bool| -> Element<'_, Message> {
        let glyph: Element<'_, Message> = if is_active {
            text("▶").size(13.0).style(style::success_text).into()
        } else {
            Space::new().into()
        };
        row![container(glyph).width(14.0), body(label)].align_y(Center).into()
    };
    let applied = column![
        group_header("Currently applied"),
        applied_row(format!("Main: {}", main.as_deref().unwrap_or("—")), active == Some(ProfileRole::Main)),
        applied_row(
            format!("Fallback: {}", fallback.as_deref().unwrap_or("—")),
            active == Some(ProfileRole::Fallback),
        ),
    ]
    .spacing(4.0);

    // Point the user at the daemon control tool and the launcher-hook trick, so they know role
    // assignment isn't UI-only. The two `deckhandctl` invocations are on their own lines for
    // copy-ability.
    let info = column![
        group_header("Additional information"),
        body(
            "The active profile (Main or Fallback) can be switched with chords (see the \
             Globals page). If only one of the two is assigned, it is always active. The active \
             role is tracked even while its slot is empty — the ▶ above marks it — so assigning a \
             profile to the role that's currently active makes it take over the controller right \
             away, rather than the other one continuing to drive."
        ),
        body("Profiles can also be set or cleared directly with the daemon control tool:"),
        monospace("deckhandctl main \"PATH_TO_PROFILE\""),
        monospace("deckhandctl main \"\""),
        body(
            "This can enable automatic, per-game switching: some launchers run a script on game \
             launch and exit — for example Heroic's \"Scripts to run\" (before launch / after \
             exit) — so you can set a profile when a game starts and clear it when it quits."
        ),
        body(
            "A handy setup is to assign a desktop profile as Fallback and leave Main empty. \
             When a game launches, assign that game's profile as Main with the control tool; \
             while playing, Main is active, but you can still switch to Fallback with chords \
             when needed. When the game quits, clear Main and the desktop profile becomes \
             active again."
        ),
    ]
    .spacing(6.0);

    column![section_header("Profile Management"), selector, grid_top, grid_bot, applied, info]
        .spacing(16.0)
        .into()
}
