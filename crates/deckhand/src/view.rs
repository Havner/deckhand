//! The iced widget layer: the four-region window and the per-category screens.

use std::path::Path;

use config::StartProfile;
use iced::widget::{
    Space, button, center, checkbox, column, container, mouse_area, opaque, pick_list, row, rule,
    scrollable, slider, stack, text, text_input,
};
use iced::{Center, Element, Fill, Theme};
use ipc::{ProfileRole, RunState};

use crate::nav::Category;
use crate::{
    App, IDLE_TIMEOUT_MINUTES, INPUT_PRESETS, IoTarget, Message, NETWORK_OPTION, OUTPUT_PRESETS,
    Popup, style,
};

/// The whole window: top bar / (sidebar + content) / bottom bar, with the network popup layered on
/// top as a modal when open.
pub fn view(app: &App) -> Element<'_, Message> {
    let base: Element<'_, Message> = column![
        top_bar(app),
        row![sidebar(app), content(app)].height(Fill),
        bottom_bar(app),
    ]
    .into();
    match &app.popup {
        Some(popup) => modal(base, popup),
        None => base,
    }
}

/// A screen's big title (e.g. "Settings", "Buttons") — the largest heading on a page.
fn section_header<'a>(title: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(title).size(24.0).into()
}

/// A sub-group heading within a screen (e.g. "UI", "Face Buttons", "Profile") — the smaller
/// subtitle under a [`section_header`].
fn group_header<'a>(title: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(title).size(20.0).into()
}

/// Regular body copy inside a content screen (14 px) — the unified default text size for pure text
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

/// Layer the network popup over the base as a modal: a dimmed, input-blocking backdrop (click it to
/// cancel) with the card centered on top.
fn modal<'a>(base: Element<'a, Message>, popup: &'a Popup) -> Element<'a, Message> {
    stack![
        base,
        opaque(
            mouse_area(center(opaque(popup_card(popup))).style(style::scrim))
                .on_press(Message::PopupCancel)
        )
    ]
    .into()
}

/// The network-spec card: a `host:port` field (Enter confirms) + Cancel / OK.
fn popup_card(popup: &Popup) -> Element<'_, Message> {
    let title = match popup.target {
        IoTarget::Input => "Network input",
        IoTarget::Output => "Network output",
    };
    let field = text_input("host:port", &popup.text)
        .id(crate::NETWORK_FIELD_ID)
        .on_input(Message::PopupTextChanged)
        .on_submit(Message::PopupConfirm)
        .padding(6.0);
    let buttons = row![
        button(text("Cancel")).style(button::danger).on_press(Message::PopupCancel),
        Space::new().width(Fill),
        button(text("OK")).style(button::success).on_press(Message::PopupConfirm),
    ]
    .align_y(Center);

    let card = column![text(title).size(18.0), field, buttons].spacing(12.0);
    container(card).padding(16.0).width(320.0).style(container::rounded_box).into()
}

/// Full-width daemon bar: Start/Stop/Connect on the left, input/output pickers on the right.
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
            button(text("Set as Main")).style(button::success).on_press(Message::SendEditingProfile(ProfileRole::Main)),
            button(text("Set as Fallback")).style(button::primary).on_press(Message::SendEditingProfile(ProfileRole::Fallback)),
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

/// Left sidebar: Profiles at the top, a separator, then the profile-editor categories; app-level
/// (Globals/Settings) pinned at the bottom.
fn sidebar(app: &App) -> Element<'_, Message> {
    let mut top = column![nav_button(app, Category::Profiles), rule::horizontal(1)].spacing(4.0);
    for &c in Category::EDITOR {
        top = top.push(nav_button(app, c));
    }
    let mut bottom = column![].spacing(4.0);
    for &c in Category::APP {
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

/// The scrollable content pane; swaps entirely on the selected category.
fn content(app: &App) -> Element<'_, Message> {
    let inner: Element<'_, Message> = match app.category {
        Category::Profiles => profiles_screen(app),
        Category::Profile => profile_screen(app),
        Category::Buttons => buttons_screen(),
        Category::Settings => settings_screen(app),
        Category::Globals => globals_screen(app),
        other => stub_screen(other),
    };
    scrollable(container(inner).padding(16.0).width(Fill)).width(Fill).height(Fill).into()
}

/// Profiles screen — profile *management*: pick a profile (from the directory or off-disk), then
/// send it to / clear a daemon role, or load it for editing / unload it.
fn profiles_screen(app: &App) -> Element<'_, Message> {
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
        cell("Edit profile", button::secondary, Message::EditProfile, has_sel),
    ]
    .spacing(8.0);
    // Bottom row: clear a role (only when that role has a profile) · Create new (always) · unload
    // the loaded profile.
    let grid_bot = row![
        cell("Clear Main", button::secondary, Message::ClearProfile(ProfileRole::Main), main.is_some()),
        cell("Clear Fallback", button::secondary, Message::ClearProfile(ProfileRole::Fallback), fallback.is_some()),
        gap(),
        cell("Create new", button::secondary, Message::ProfileCreateNew, true),
        cell("Unload profile", button::danger, Message::UnloadProfile, loaded),
    ]
    .spacing(8.0);

    // The role assignments, mirrored here so you have context while managing/editing — e.g. what
    // "Set as …" would replace. Mirrors the bottom bar; `—` when empty/disconnected.
    let applied = column![
        group_header("Currently applied"),
        body(format!("Main: {}", main.as_deref().unwrap_or("—"))),
        body(format!("Fallback: {}", fallback.as_deref().unwrap_or("—"))),
    ]
    .spacing(4.0);

    // Point the user at the daemon control tool and the launcher-hook trick, so they know role
    // assignment isn't UI-only. The two `deckhandctl` invocations are on their own lines for
    // copy-ability.
    let info = column![
        group_header("Additional information"),
        body(
            "The active profile (Main or Fallback) can be switched with chords (see the \
             Globals page). If only one of the two is assigned, it is always active."
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

/// Profile screen — the top of the profile editor. For now just the profile's name; action sets and
/// layers land here below it once implemented. Reachable only while a profile is loaded, so the
/// field is always live.
fn profile_screen(app: &App) -> Element<'_, Message> {
    let name = row![
        text("Profile name").width(140.0),
        text_input("profile name", app.editing_name())
            .on_input(Message::ProfileNameChanged)
            .width(Fill),
    ]
    .spacing(12.0)
    .align_y(Center);
    column![section_header("Profile"), name].spacing(20.0).into()
}

/// Settings screen — a **UI** section (theme) and a **Daemon** section (the on-connect behaviour +
/// the two profile paths, each path greyed when its toggle is off).
fn settings_screen(app: &App) -> Element<'_, Message> {
    let s = &app.settings;

    let theme_pick = row![
        text("Theme:").size(14.0),
        pick_list(Some(app.active_theme()), Theme::ALL, |t: &Theme| t.to_string())
            .on_select(Message::SetTheme),
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

/// Globals screen — a live editor over the UI-owned [`GlobalConfig`](config::GlobalConfig). Every
/// edit persists to `globals.ron` and ships to the daemon (`App::apply_globals`); this always renders
/// `app.globals` (the source of truth), never the daemon's status snapshot. Chords aren't editable
/// yet (that lands with the profile editor) — only their count is shown.
fn globals_screen(app: &App) -> Element<'_, Message> {
    let g = &app.globals;

    // Start profile: Main / Fallback. The label closure supplies the display strings, so the enum
    // needs no `Display` impl.
    let start = row![
        glabel("Start profile:"),
        pick_list(
            Some(g.start_profile.clone()),
            vec![StartProfile::Main, StartProfile::Fallback],
            |p: &StartProfile| match p {
                StartProfile::Main => "Main",
                StartProfile::Fallback => "Fallback",
            }
            .to_string(),
        )
        .on_select(Message::GlobalsStartProfile)
        .width(160.0),
    ]
    .spacing(12.0)
    .align_y(Center);

    // Master rumble: a 0–100% slider with a live readout.
    let master = row![
        glabel("Master rumble:"),
        slider(0..=100u8, g.master_rumble, Message::GlobalsMasterRumble),
        pct_text(Some(g.master_rumble)),
    ]
    .spacing(12.0)
    .align_y(Center);

    // LED brightness: an `Option` — the checkbox gates a 0–100% slider. When off it's the same
    // slider widget (identical geometry) but styled inert and non-interactive, so `None` reads as
    // "leave default" without the jarring size change a different widget would cause.
    let led_on = g.led_brightness.is_some();
    let led_val = g.led_brightness.unwrap_or(crate::DEFAULT_LED_BRIGHTNESS);
    let led_bar: Element<'_, Message> = if led_on {
        slider(0..=100u8, led_val, Message::GlobalsLedBrightness).into()
    } else {
        slider(0..=100u8, led_val, |_| Message::Ignored).style(style::disabled_slider).into()
    };
    let led = row![
        glabel("LED brightness:"),
        checkbox(led_on).on_toggle(Message::GlobalsLedEnabled),
        led_bar,
        pct_text(led_on.then_some(led_val)),
    ]
    .spacing(12.0)
    .align_y(Center);

    // Idle timeout: an `Option` — the checkbox gates a minutes combobox (values stored as seconds).
    let idle_on = g.idle_timeout.is_some();
    let idle_min = g.idle_timeout.map(|s| s / 60).unwrap_or(crate::DEFAULT_IDLE_TIMEOUT / 60);
    let mut idle_combo = pick_list(
        idle_on.then_some(idle_min),
        IDLE_TIMEOUT_MINUTES.to_vec(),
        |m: &u16| format!("{m} minutes"),
    )
    .placeholder("default")
    .width(160.0);
    if idle_on {
        idle_combo = idle_combo.on_select(|m| Message::GlobalsIdleTimeout(m * 60));
    }
    let idle = row![
        glabel("Idle timeout:"),
        checkbox(idle_on).on_toggle(Message::GlobalsIdleEnabled),
        idle_combo,
    ]
    .spacing(12.0)
    .align_y(Center);

    // Chords: count only — editing is deferred to the profile editor (the count still round-trips
    // through the file/daemon untouched).
    let chords = row![glabel("Chords:"), text(format!("{}", g.chords.len()))]
        .spacing(12.0)
        .align_y(Center);

    let note =
        small("'Start profile', 'LED brightness' and 'Idle timeout' take effect only on engine start.");

    column![section_header("Global daemon settings"), note, start, master, led, idle, chords]
        .spacing(16.0)
        .into()
}

/// A fixed-width row label for the Globals screen, so the controls line up in a column.
fn glabel(s: &'static str) -> Element<'static, Message> {
    text(s).width(140.0).into()
}

/// A fixed-width trailing percentage readout (`None` → "default"), keeping the sliders aligned.
fn pct_text(v: Option<u8>) -> Element<'static, Message> {
    text(v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into())).width(60.0).into()
}

/// A mockup of the Steam-Deck-style **Buttons** screen — grouped input rows, each with a per-input
/// gear, and a group behavior picker — purely to preview the widgets + scrolling. Nothing is wired
/// (all interactions send [`Message::Ignored`]).
fn buttons_screen() -> Element<'static, Message> {
    // Xbox face-button glyphs, colored from theme roles (A green→success, B red→danger,
    // X blue→primary, Y yellow→warning) so they track the theme like everything else.
    let face = column![
        group_header("Face Buttons"),
        behavior_row(),
        input_row(Some(style::success_text), "A Button"),
        input_row(Some(style::danger_text), "B Button"),
        input_row(Some(style::primary_text), "X Button"),
        input_row(Some(style::warning_text), "Y Button"),
    ]
    .spacing(8.0);

    let bumpers = column![
        group_header("Bumpers"),
        input_row(None, "Left Bumper"),
        input_row(None, "Right Bumper"),
    ]
    .spacing(8.0);

    let dpad = column![
        group_header("D-Pad"),
        input_row(None, "Up"),
        input_row(None, "Down"),
        input_row(None, "Left"),
        input_row(None, "Right"),
    ]
    .spacing(8.0);

    let system = column![
        group_header("System"),
        input_row(None, "View"),
        input_row(None, "Menu"),
        input_row(None, "Steam"),
    ]
    .spacing(8.0);

    column![section_header("Buttons"), face, bumpers, dpad, system].spacing(20.0).into()
}

/// A group's "Behavior" selector (buttons only have Button Pad; the rest is filler) + a gear.
fn behavior_row() -> Element<'static, Message> {
    let options =
        vec!["Button Pad".to_string(), "Lorem Ipsum".to_string(), "Dolor Sit Amet".to_string()];
    let combo = pick_list(Some("Button Pad".to_string()), options, String::clone)
        .on_select(|_| Message::Ignored);
    let inner = row![text("Behavior"), Space::new().width(Fill), combo, gear()]
        .spacing(12.0)
        .align_y(Center);
    card(inner)
}

/// One input row: an optional colored glyph (a theme-role text style), the input name, and a gear
/// on the right.
fn input_row(
    dot: Option<fn(&Theme) -> text::Style>,
    label: &'static str,
) -> Element<'static, Message> {
    let mut r = row![].spacing(12.0).align_y(Center);
    if let Some(role) = dot {
        r = r.push(text("●").size(16.0).style(role));
    }
    let inner = r.push(text(label)).push(Space::new().width(Fill)).push(gear());
    card(inner)
}

/// An unwired settings/gear button.
fn gear() -> Element<'static, Message> {
    button(text("⚙").size(16.0)).on_press(Message::Ignored).style(button::secondary).into()
}

/// Wrap a row as a padded card so the groups read like the Steam UI's list rows.
fn card<'a>(inner: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(inner).padding(10.0).width(Fill).style(style::panel).into()
}

/// A placeholder for the profile-edit categories, not part of this toolkit test.
fn stub_screen(c: Category) -> Element<'static, Message> {
    column![
        section_header(c.label()),
        text("Profile-edit screen — stubbed for the toolkit test.").size(13.0),
    ]
    .spacing(10.0)
    .into()
}

/// Full-width status bar from the engine status (device bound, profiles, chord count, …).
fn bottom_bar(app: &App) -> Element<'_, Message> {
    // Dot color from theme roles: success (green, matching Start) / danger (red, matching Stop).
    let (dot, label): (fn(&Theme) -> text::Style, _) = if app.connected {
        (style::success_text, "connected")
    } else {
        (style::danger_text, "disconnected")
    };
    let conn = row![text("●").size(13.0).style(dot), text(label).size(13.0)]
        .spacing(6.0)
        .align_y(Center);
    let mut bar = row![conn].spacing(10.0).align_y(Center).padding(8.0);

    if let Some(s) = &app.status {
        bar = bar
            .push(sep())
            .push(text(format!("state: {:?}", s.state)).size(13.0))
            .push(sep())
            .push(text(format!("device: {}", s.bound.as_deref().unwrap_or("—"))).size(13.0))
            .push(sep())
            .push(text(format!("main: {}", s.main.as_deref().unwrap_or("—"))).size(13.0))
            .push(sep())
            .push(text(format!("fallback: {}", s.fallback.as_deref().unwrap_or("—"))).size(13.0))
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

fn sep() -> Element<'static, Message> {
    text("│").size(13.0).style(style::muted_text).into()
}
