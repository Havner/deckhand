//! `deckhand` — the tray/configurator UI, and the project's user-facing binary.
//!
//! A four-region window (top daemon bar / left sidebar / scrollable content / bottom status bar)
//! driven by the real daemon over `ipc`, built on iced (Elm architecture). The widget layer lives
//! in [`view`]; everything toolkit-independent is split into internal modules so the widget code
//! stays about widgets:
//!
//! - [`settings`] — the application's own settings (separate from the daemon), RON-persisted.
//! - [`daemon`] — a thin blocking command client + resilient event-subscribe loop over `ipc`.
//! - [`nav`] — the left-sidebar navigation model.
//!
//! Scaffolded from the `ui-test-iced` bake-off prototype (PLAN §5.2); the profile-edit screens are
//! still stubs pending the real config-editing UI.

mod daemon;
mod globals;
mod nav;
mod profiles;
mod settings;
mod style;
mod tray;
mod view;

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use config::{ConfigDoc, GlobalConfig, StartProfile};
use daemon::{Client, DaemonUpdate, Shared, run_event_loop};
use iced::futures::stream::BoxStream;
use iced::window;
use iced::{Size, Subscription, Task, Theme};
use ipc::{Event, ProfileRole, RunState, StatusSnapshot};
use nav::Category;
use settings::AppSettings;

/// Preset input selections offered before the daemon's live `list-devices` is appended (the same
/// grammar the daemon parses for `SetInput`).
pub const INPUT_PRESETS: &[&str] = &["auto", "dongle", "wired", "bt"];

/// Preset output selections (`host:port` is typed, not listed).
pub const OUTPUT_PRESETS: &[&str] = &["local"];

/// Value `led_brightness` snaps to when its checkbox is first enabled (mid-range).
pub const DEFAULT_LED_BRIGHTNESS: u8 = 50;

/// Value `idle_timeout` snaps to when its checkbox is first enabled — 5 minutes (the shortest
/// offered option). In **seconds**, matching [`GlobalConfig::idle_timeout`].
pub const DEFAULT_IDLE_TIMEOUT: u16 = 300;

/// The idle-timeout options offered in the Globals combobox, in **minutes**.
pub const IDLE_TIMEOUT_MINUTES: &[u16] = &[5, 10, 15];

/// The sentinel pick-list entry that opens the network (`host:port`) popup instead of staging a
/// value directly.
pub const NETWORK_OPTION: &str = "<network>";

/// Widget id of the network popup's text field (so it can be focused when the popup opens).
pub const NETWORK_FIELD_ID: &str = "network-host";

/// Which selector the network popup is editing.
#[derive(Debug, Clone, Copy)]
pub enum IoTarget {
    Input,
    Output,
}

/// The network input/output modal: which selector it targets and the current `host:port` text.
#[derive(Debug, Clone)]
pub struct Popup {
    pub target: IoTarget,
    pub text: String,
}

fn main() -> iced::Result {
    // The UI-managed daemon handle, shared with the connect loop (populated when we launch a daemon)
    // and retained here so we can stop it on exit.
    let daemon = daemon::shared();

    // A `daemon` (not `application`): it survives with zero windows, so hide-to-tray can actually
    // CLOSE the window (the only way to hide on Wayland — winit can't toggle visibility there) and
    // reopen it to show. Boot opens the initial window unless we're starting hidden into the tray.
    let boot = {
        let daemon = daemon.clone();
        move || {
            let app = App::new(daemon.clone());
            let task = if app.hidden { Task::none() } else { app.open_window() };
            (app, task)
        }
    };
    let result = iced::daemon(boot, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(|app: &App, _window| app.active_theme())
        .run();
    // App exited: stop the daemon we launched (if any) before the process exits.
    daemon::shutdown_managed(&daemon);
    result
}

/// A profile loaded into the **editor**: the file it came from (edits save back here) and the parsed
/// document. Its presence is the UI's central macro-state (see [`App::editing`]).
struct Editing {
    path: PathBuf,
    doc: ConfigDoc,
}

/// The whole application state (Elm-architecture `State`).
pub struct App {
    /// The UI's own settings (Settings screen), persisted separately from the daemon.
    settings: AppSettings,
    /// The UI's central macro-state: a profile loaded **for editing**, or not. The whole app has
    /// exactly two modes — **no profile loaded** (`None`: the profile-editor sidebar tabs are
    /// disabled; only profile *management*, Globals, Settings, and the daemon controls work) and
    /// **a profile loaded** (`Some`: the editor tabs are active and the title bar shows the path).
    /// Editing is entirely local to the UI — it is separate from whatever profiles are applied to
    /// the daemon's roles.
    editing: Option<Editing>,
    /// The `.ron` file names in the active profiles directory, for the Profiles combobox. Refreshed
    /// on demand and when the directory setting changes.
    profile_files: Vec<String>,
    /// The Profiles combobox selection: a bare filename from `profile_files`, or a full path chosen
    /// via the disk picker (`None` until the user picks). Resolved by [`App::selected_profile_path`].
    selected_profile: Option<String>,
    /// The UI-owned global (engine) config — the Globals screen's source of truth. Loaded from
    /// `globals.ron` at boot and kept in lock-step with that file and the daemon (see
    /// [`globals`] and [`Self::apply_globals`]).
    globals: GlobalConfig,
    /// Socket/pipe override (`None` = default). Identifies the event subscription and seeds the
    /// short-lived per-command connections opened on the executor thread.
    socket: Option<String>,
    /// Whether the daemon is currently reachable.
    connected: bool,
    /// Latest engine status (seeds the top/bottom bars); `None` when disconnected.
    status: Option<StatusSnapshot>,
    /// The daemon's live device ids (appended to the input presets).
    devices: Vec<String>,
    /// The selected sidebar screen.
    category: Category,
    /// Last error, surfaced in the status bar.
    error: Option<String>,
    /// The UI-managed daemon handle (shared with the connect loop); drives launch-on-connect and
    /// stop-on-exit.
    daemon: Shared,
    /// The main window's id while open, captured on open (needed to close it). `None` when hidden.
    window: Option<window::Id>,
    /// Whether the window is currently hidden in the tray (no window open).
    hidden: bool,
    /// The live tray, when enabled and successfully started (`None` = no tray).
    tray: Option<tray::Tray>,
    /// The network input/output modal, when open.
    popup: Option<Popup>,
}

/// Everything the view can emit.
#[derive(Debug, Clone)]
pub enum Message {
    /// Sidebar navigation.
    Navigate(Category),
    /// An update from the event-stream subscription.
    Daemon(DaemonUpdate),
    /// Top-bar daemon controls.
    Start,
    Stop,
    /// Re-enumerate devices + refresh status (no USB hotplug — this is the manual trigger).
    Refresh,
    /// Input/output selection (spec string).
    InputSelected(String),
    OutputSelected(String),
    /// Results of daemon calls run off the render thread (on iced's executor). Errors are
    /// stringified — `io::Error` isn't `Clone`, and `Message` must be.
    StatusFetched(Result<StatusSnapshot, String>),
    DevicesFetched(Result<Vec<String>, String>),
    CmdDone(Result<(), String>),
    /// Settings-screen edits.
    ToggleStartDaemon(bool),
    ToggleStartEngine(bool),
    ToggleLoadMain(bool),
    ToggleLoadFallback(bool),
    MainPathChanged(String),
    FallbackPathChanged(String),
    BrowseMain,
    BrowseFallback,
    /// Restore last input/output on connect.
    ToggleRestoreIo(bool),
    /// Custom-profiles-directory setting (Settings screen).
    ToggleCustomProfileDir(bool),
    CustomProfileDirChanged(String),
    BrowseCustomProfileDir,
    /// Profiles screen: re-scan the directory, pick a listed file, or browse for one off-disk.
    ProfileRefresh,
    ProfileSelected(String),
    ProfileBrowse,
    /// Send the *selected* (on-disk) profile to a daemon role (Profiles-page Set-as-Main/Fallback).
    SendProfile(ProfileRole),
    /// Clear a daemon role (Profiles-page Clear-Main/Fallback).
    ClearProfile(ProfileRole),
    /// Send the *loaded* (in-memory, being-edited) profile to a role (top-bar Main/Fallback).
    SendEditingProfile(ProfileRole),
    /// Load the selected profile into the editor (Edit button) / rename it / unload it.
    EditProfile,
    ProfileNameChanged(String),
    UnloadProfile,
    /// Globals-screen edits. Each mutates the UI-owned `globals`, then persists it and ships it to
    /// the daemon ([`App::apply_globals`]). The two `Option` fields toggle via the `*Enabled` pair.
    GlobalsStartProfile(StartProfile),
    GlobalsMasterRumble(u8),
    GlobalsLedEnabled(bool),
    GlobalsLedBrightness(u8),
    GlobalsIdleEnabled(bool),
    GlobalsIdleTimeout(u16),
    /// Tray settings.
    ToggleUseTray(bool),
    ToggleCloseToTray(bool),
    ToggleStartHidden(bool),
    /// Network (`host:port`) input/output popup.
    PopupTextChanged(String),
    PopupConfirm,
    PopupCancel,
    /// Window lifecycle: open captures the id; a close *request* (WM button) is intercepted for
    /// close-to-tray; closed clears the id; resize tracks the size to persist.
    WindowOpened(window::Id),
    CloseRequested(window::Id),
    WindowClosed(window::Id),
    WindowResized(Size),
    /// A tray menu item was clicked.
    TrayMenu(tray::MenuAction),
    /// Theme selection (any built-in iced theme).
    SetTheme(Theme),
    /// A no-op for unwired mockup widgets (the Buttons tab).
    Ignored,
}

impl App {
    fn new(daemon: Shared) -> Self {
        let settings = AppSettings::load();
        let want_hidden = settings.use_tray && settings.start_hidden;
        // Start the tray if enabled. We only *actually* start hidden if it came up — otherwise
        // there'd be no way to restore the window.
        let (tray, error) = if settings.use_tray {
            match tray::Tray::enable(want_hidden) {
                Ok(t) => (Some(t), None),
                Err(e) => (None, Some(format!("tray: {e}"))),
            }
        } else {
            (None, None)
        };
        // Make sure the profiles directory exists so listing/saving there just works.
        let _ = profiles::ensure_dir(&settings);
        let profile_files = profiles::list(&settings);
        App {
            settings,
            editing: None,
            profile_files,
            selected_profile: None,
            globals: globals::load(),
            socket: None,
            connected: false,
            status: None,
            devices: Vec::new(),
            category: Category::Profiles,
            error,
            daemon,
            window: None,
            hidden: want_hidden && tray.is_some(),
            tray,
            popup: None,
        }
    }

    /// The engine's current run state, or `None` when disconnected.
    fn run_state(&self) -> Option<RunState> {
        self.status.as_ref().map(|s| s.state)
    }

    /// Whether the engine is started (not idle) — the condition for restarting it on an input/output
    /// change.
    fn engine_started(&self) -> bool {
        matches!(self.run_state(), Some(RunState::Running | RunState::WaitingForDevice))
    }

    /// Stage a new input spec: remember it as the last input, and apply it — restarting the engine
    /// if it's already running (staged input only takes effect at start).
    fn apply_input(&mut self, spec: String) -> Task<Message> {
        self.settings.last_input = spec.clone();
        self.save_settings();
        let restart = self.engine_started();
        self.cmd_task(move |c| {
            c.set_input(spec)?;
            if restart {
                c.stop()?;
                c.start()?;
            }
            Ok(())
        })
    }

    /// Stage a new output spec (see [`apply_input`](Self::apply_input)).
    fn apply_output(&mut self, spec: String) -> Task<Message> {
        self.settings.last_output = spec.clone();
        self.save_settings();
        let restart = self.engine_started();
        self.cmd_task(move |c| {
            c.set_output(spec)?;
            if restart {
                c.stop()?;
                c.start()?;
            }
            Ok(())
        })
    }

    /// Persist the UI-owned globals to `globals.ron` and ship them to the daemon. Called after every
    /// Globals-screen edit — the file, the in-memory copy, and the daemon stay in lock-step (the
    /// daemon's echoed `GlobalConfigSet` re-lands the identical value in [`Self::apply_event`], a
    /// harmless no-op).
    fn apply_globals(&mut self) -> Task<Message> {
        if let Err(e) = globals::save(&self.globals) {
            self.error = Some(format!("save globals: {e}"));
        }
        let g = self.globals.clone();
        self.cmd_task(move |c| c.set_globals(g))
    }

    /// The window title: `deckhand`, or `deckhand: <path>` while a profile is loaded for editing.
    fn title(&self, _window: window::Id) -> String {
        match &self.editing {
            Some(ed) => format!("deckhand: {}", ed.path.display()),
            None => "deckhand".to_string(),
        }
    }

    /// Whether a profile is currently loaded for editing (the editor-tabs-active macro-state).
    fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// The loaded profile's name, if any (for the Profiles name field).
    fn editing_name(&self) -> &str {
        self.editing.as_ref().map(|e| e.doc.name.as_str()).unwrap_or("")
    }

    /// Recreate + re-list the profiles directory after a directory-setting change.
    fn refresh_profile_dir(&mut self) {
        let _ = profiles::ensure_dir(&self.settings);
        self.profile_files = profiles::list(&self.settings);
    }

    /// Resolve the Profiles combobox selection to a path: a bare filename joins the active profiles
    /// directory; an absolute path (from the disk picker) is used as-is.
    fn selected_profile_path(&self) -> Option<PathBuf> {
        let sel = self.selected_profile.as_ref()?;
        let p = Path::new(sel);
        Some(if p.is_absolute() { p.to_path_buf() } else { profiles::dir(&self.settings).join(sel) })
    }

    /// Save the loaded profile back to the file it was loaded from (after a name/... edit).
    fn save_editing(&mut self) {
        if let Some(ed) = &self.editing
            && let Err(e) = profiles::save(&ed.path, &ed.doc)
        {
            self.error = Some(format!("save profile: {e}"));
        }
    }

    /// Persist the current window size (called on hide / quit, not on every resize).
    fn persist_window_size(&mut self) {
        self.save_settings();
    }

    /// Open the main window, routing its id back as [`Message::WindowOpened`]. `exit_on_close_request`
    /// is off so the WM close button reaches our [`Message::CloseRequested`] handler (which decides
    /// close-to-tray vs quit) instead of iced auto-closing the window — in daemon mode an auto-close
    /// would just leave the app running with no window and no way to have intercepted it.
    fn open_window(&self) -> Task<Message> {
        let size = Size::new(self.settings.window_width as f32, self.settings.window_height as f32);
        let settings =
            window::Settings { exit_on_close_request: false, size, ..window::Settings::default() };
        window::open(settings).1.map(Message::WindowOpened)
    }

    /// Hide or show the window, and retitle the tray's Show/Hide item. Hiding **closes** the window
    /// (unmapping the surface — the only way to hide on Wayland, where winit can't toggle
    /// visibility); showing opens a fresh one.
    fn set_hidden(&mut self, hidden: bool) -> Task<Message> {
        self.hidden = hidden;
        if let Some(t) = &self.tray {
            t.set_label(hidden);
        }
        if hidden {
            self.persist_window_size();
            match self.window.take() {
                Some(id) => window::close(id),
                None => Task::none(),
            }
        } else if self.window.is_none() {
            self.open_window()
        } else {
            Task::none()
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // --- arms that dispatch async daemon work (off the render thread) ---
            // On (re)connect, seed once with a full snapshot + device list; after that the event
            // stream carries every state change (events are absolute-valued and cover all status
            // fields), so `apply_event` mutates the cached status in place — we never refetch.
            Message::Daemon(DaemonUpdate::Connected) => {
                self.connected = true;
                self.error = None;
                return self.refresh_task();
            }
            Message::Refresh => return self.refresh_task(),
            // Start/Stop also re-enumerate devices + refresh status, as if Refresh was clicked too.
            Message::Start => {
                return Task::batch([self.cmd_task(|c| c.start()), self.refresh_task()]);
            }
            Message::Stop => {
                return Task::batch([self.cmd_task(|c| c.stop()), self.refresh_task()]);
            }
            Message::InputSelected(spec) => {
                if spec == NETWORK_OPTION {
                    let text = self.settings.last_input_network.clone();
                    self.popup = Some(Popup { target: IoTarget::Input, text });
                    return iced::widget::operation::focus(NETWORK_FIELD_ID);
                }
                return self.apply_input(spec);
            }
            Message::OutputSelected(spec) => {
                if spec == NETWORK_OPTION {
                    let text = self.settings.last_output_network.clone();
                    self.popup = Some(Popup { target: IoTarget::Output, text });
                    return iced::widget::operation::focus(NETWORK_FIELD_ID);
                }
                return self.apply_output(spec);
            }
            // Globals edits: mutate the in-memory config, then persist + push to the daemon. The
            // two Option fields default to a sensible value when their checkbox is switched on.
            Message::GlobalsStartProfile(p) => {
                self.globals.start_profile = p;
                return self.apply_globals();
            }
            Message::GlobalsMasterRumble(v) => {
                self.globals.master_rumble = v;
                return self.apply_globals();
            }
            Message::GlobalsLedEnabled(on) => {
                self.globals.led_brightness = on.then_some(DEFAULT_LED_BRIGHTNESS);
                return self.apply_globals();
            }
            Message::GlobalsLedBrightness(v) => {
                self.globals.led_brightness = Some(v);
                return self.apply_globals();
            }
            Message::GlobalsIdleEnabled(on) => {
                self.globals.idle_timeout = on.then_some(DEFAULT_IDLE_TIMEOUT);
                return self.apply_globals();
            }
            Message::GlobalsIdleTimeout(secs) => {
                self.globals.idle_timeout = Some(secs);
                return self.apply_globals();
            }

            // Network popup edits.
            Message::PopupTextChanged(t) => {
                if let Some(p) = &mut self.popup {
                    p.text = t;
                }
            }
            Message::PopupConfirm => {
                if let Some(p) = self.popup.take() {
                    let spec = p.text.trim().to_string();
                    if !spec.is_empty() {
                        return match p.target {
                            IoTarget::Input => {
                                self.settings.last_input_network = spec.clone();
                                self.apply_input(spec)
                            }
                            IoTarget::Output => {
                                self.settings.last_output_network = spec.clone();
                                self.apply_output(spec)
                            }
                        };
                    }
                }
            }
            Message::PopupCancel => self.popup = None,

            // --- window + tray arms (may drive a window Task) ---
            Message::WindowOpened(id) => self.window = Some(id),
            Message::WindowClosed(id) => {
                // Only clear if it's the current window (a stale close from a prior hide could
                // otherwise wipe a freshly reopened window's id).
                if self.window == Some(id) {
                    self.window = None;
                }
            }
            Message::WindowResized(size) => {
                // In-memory only; flushed to disk on hide/quit.
                self.settings.window_width = size.width as u32;
                self.settings.window_height = size.height as u32;
            }
            Message::CloseRequested(_id) => {
                if self.settings.close_to_tray && self.tray.is_some() {
                    return self.set_hidden(true);
                }
                self.persist_window_size();
                return iced::exit();
            }
            Message::TrayMenu(tray::MenuAction::ToggleWindow) => {
                return self.set_hidden(!self.hidden);
            }
            Message::TrayMenu(tray::MenuAction::Quit) => {
                self.persist_window_size();
                return iced::exit();
            }
            Message::ToggleUseTray(v) => {
                self.settings.use_tray = v;
                self.save_settings();
                if v {
                    match tray::Tray::enable(self.hidden) {
                        Ok(t) => {
                            self.tray = Some(t);
                            self.error = None;
                        }
                        Err(e) => self.error = Some(format!("tray: {e}")),
                    }
                } else if let Some(t) = self.tray.take() {
                    t.disable();
                    // Without a tray we can't restore a hidden window — show it.
                    if self.hidden {
                        return self.set_hidden(false);
                    }
                }
            }

            // --- arms that only mutate state (fall through to Task::none()) ---
            Message::ToggleCloseToTray(v) => {
                self.settings.close_to_tray = v;
                self.save_settings();
            }
            Message::ToggleStartHidden(v) => {
                self.settings.start_hidden = v;
                self.save_settings();
            }
            Message::ToggleRestoreIo(v) => {
                self.settings.restore_io = v;
                self.save_settings();
            }

            // --- profiles: directory setting + management (select / send / edit) ---
            Message::ToggleCustomProfileDir(v) => {
                self.settings.use_custom_profile_dir = v;
                self.save_settings();
                self.refresh_profile_dir();
            }
            Message::CustomProfileDirChanged(p) => {
                self.settings.custom_profile_dir = p;
                self.save_settings();
                self.refresh_profile_dir();
            }
            Message::BrowseCustomProfileDir => {
                if let Some(p) = pick_folder() {
                    self.settings.custom_profile_dir = p;
                    self.save_settings();
                    self.refresh_profile_dir();
                }
            }
            Message::ProfileRefresh => self.profile_files = profiles::list(&self.settings),
            Message::ProfileSelected(name) => self.selected_profile = Some(name),
            Message::ProfileBrowse => {
                if let Some(p) = pick_ron() {
                    self.selected_profile = Some(p);
                }
            }
            Message::SendProfile(role) => {
                let Some(path) = self.selected_profile_path() else {
                    self.error = Some("no profile selected".into());
                    return Task::none();
                };
                return self.cmd_task(move |c| {
                    let doc = profiles::load(&path).map_err(std::io::Error::other)?;
                    c.apply(role, Some(Box::new(doc)))
                });
            }
            Message::ClearProfile(role) => return self.cmd_task(move |c| c.apply(role, None)),
            Message::SendEditingProfile(role) => {
                let Some(ed) = &self.editing else { return Task::none() };
                let doc = ed.doc.clone();
                return self.cmd_task(move |c| c.apply(role, Some(Box::new(doc))));
            }
            Message::EditProfile => {
                let Some(path) = self.selected_profile_path() else {
                    self.error = Some("no profile selected".into());
                    return Task::none();
                };
                match profiles::load(&path) {
                    Ok(doc) => {
                        // Stay on the Profiles page; loading just enables the editor tabs.
                        self.editing = Some(Editing { path, doc });
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::ProfileNameChanged(name) => {
                if let Some(ed) = &mut self.editing {
                    ed.doc.name = name;
                    self.save_editing();
                }
            }
            Message::UnloadProfile => {
                self.editing = None;
                // Leave any now-disabled editor tab for the management page.
                if self.category.is_editor() {
                    self.category = nav::Category::Profiles;
                }
            }

            Message::Navigate(c) => self.category = c,
            Message::Daemon(DaemonUpdate::Disconnected) => {
                self.connected = false;
                self.status = None;
                self.devices.clear();
            }
            Message::Daemon(DaemonUpdate::Event(ev)) => self.apply_event(ev),
            Message::Daemon(DaemonUpdate::Error(e)) => self.error = Some(e),
            // Seeding status must NOT clear the error: the seed is dispatched by the same
            // `Connected` that precedes the on-connect setup, so clearing here would wipe a setup
            // error (bad profile path, start refused) the instant the async seed lands.
            Message::StatusFetched(Ok(s)) => self.status = Some(s),
            Message::StatusFetched(Err(e)) => {
                self.status = None;
                self.error = Some(e);
            }
            Message::DevicesFetched(Ok(d)) => self.devices = d,
            Message::DevicesFetched(Err(e)) => self.error = Some(e),
            // A command's effect also arrives as an event, so success just clears the error;
            // failures report themselves.
            Message::CmdDone(Ok(())) => self.error = None,
            Message::CmdDone(Err(e)) => self.error = Some(e),

            Message::ToggleStartDaemon(v) => {
                self.settings.start_daemon = v;
                self.save_settings();
            }
            Message::ToggleStartEngine(v) => {
                self.settings.start_engine = v;
                self.save_settings();
            }
            Message::ToggleLoadMain(v) => {
                self.settings.load_main = v;
                self.save_settings();
            }
            Message::ToggleLoadFallback(v) => {
                self.settings.load_fallback = v;
                self.save_settings();
            }
            Message::MainPathChanged(p) => {
                self.settings.main_path = p;
                self.save_settings();
            }
            Message::FallbackPathChanged(p) => {
                self.settings.fallback_path = p;
                self.save_settings();
            }
            Message::BrowseMain => {
                if let Some(p) = pick_ron() {
                    self.settings.main_path = p;
                    self.save_settings();
                }
            }
            Message::BrowseFallback => {
                if let Some(p) = pick_ron() {
                    self.settings.fallback_path = p;
                    self.save_settings();
                }
            }
            Message::SetTheme(t) => {
                self.settings.theme = t.to_string();
                self.save_settings();
            }
            Message::Ignored => {}
        }
        Task::none()
    }

    /// The active iced theme, resolved from the persisted theme **name** (falls back to Dark for an
    /// empty/unknown name).
    fn active_theme(&self) -> Theme {
        Theme::ALL
            .iter()
            .find(|t| t.to_string() == self.settings.theme)
            .cloned()
            .unwrap_or(Theme::Dark)
    }

    fn view(&self, _window: window::Id) -> iced::Element<'_, Message> {
        view::view(self)
    }

    /// The app's subscriptions: the daemon event stream, window open/close events, and tray menu
    /// clicks. The daemon sub is keyed on the socket so it stays alive for the app's life; the
    /// managed-daemon handle rides along in the data but is excluded from its identity (see
    /// [`SubData`]) so the subscription isn't restarted every render.
    fn subscription(&self) -> Subscription<Message> {
        let data = SubData { socket: self.socket.clone(), managed: self.daemon.clone() };
        Subscription::batch([
            Subscription::run_with(data, daemon_events),
            // Window ids come from mapping `window::open`'s task (see `open_window`), so we only need
            // close-request (WM button) and closed (id cleanup) here.
            window::close_requests().map(Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(_id, size)| Message::WindowResized(size)),
            Subscription::run_with((), tray_events),
        ])
    }

    /// Fetch a fresh status snapshot on a short-lived connection off the render thread.
    fn status_task(&self) -> Task<Message> {
        let socket = self.socket.clone();
        Task::perform(
            async move { Client::new(socket).status().map_err(|e| e.to_string()) },
            Message::StatusFetched,
        )
    }

    /// Re-enumerate devices off the render thread.
    fn devices_task(&self) -> Task<Message> {
        let socket = self.socket.clone();
        Task::perform(
            async move { Client::new(socket).list_devices().map_err(|e| e.to_string()) },
            Message::DevicesFetched,
        )
    }

    /// Status + device list together (on connect / manual refresh).
    fn refresh_task(&self) -> Task<Message> {
        Task::batch([self.status_task(), self.devices_task()])
    }

    /// Run a daemon command off the render thread; its outcome returns as [`Message::CmdDone`],
    /// which then triggers a refresh so the bars reflect it.
    fn cmd_task(
        &self,
        op: impl FnOnce(&mut Client) -> std::io::Result<()> + Send + 'static,
    ) -> Task<Message> {
        let socket = self.socket.clone();
        Task::perform(
            async move { op(&mut Client::new(socket)).map_err(|e| e.to_string()) },
            Message::CmdDone,
        )
    }

    /// Apply one daemon event to the cached status in place. Events are absolute-valued and cover
    /// every status field, so after the initial seed the bars stay current without refetching.
    /// Ignored until the first snapshot has seeded `status` (the seed, fetched after subscribe, is
    /// itself absolute, so nothing is missed).
    fn apply_event(&mut self, ev: Event) {
        // A global-config change (our own echoed push, or another client's `SetGlobals`) syncs the
        // UI-owned copy and the file regardless of seed state — the Globals screen reads
        // `self.globals`, and the three (file / UI / daemon) stay in lock-step.
        if let Event::GlobalConfigSet(g) = &ev {
            self.globals = g.clone();
            if let Err(e) = globals::save(&self.globals) {
                self.error = Some(format!("save globals: {e}"));
            }
        }
        let Some(status) = self.status.as_mut() else { return };
        match ev {
            Event::State(s) => status.state = s,
            Event::BindingRemoved => status.bound = None,
            Event::BindingAcquired(id) => status.bound = Some(id),
            Event::InputStaged(i) => status.input = i,
            Event::OutputStaged(o) => status.output = o,
            Event::ProfileSet { role, name } => match role {
                ProfileRole::Main => status.main = name,
                ProfileRole::Fallback => status.fallback = name,
            },
            Event::GlobalConfigSet(g) => status.globals = g,
            // Controller presence + battery have no field in the bars (yet). `Event` is
            // #[non_exhaustive], so a `_` covers these and any future variant.
            _ => {}
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("save settings: {e}"));
        }
    }
}

/// The data handed to the daemon-events subscription: the socket override plus the managed-daemon
/// handle. Its [`Hash`] covers only the socket — that is the subscription's identity, so the handle
/// (cloned fresh every render) never restarts the loop. The builder is a plain `fn` pointer that
/// can't capture, so the handle has to travel in the data.
#[derive(Clone)]
struct SubData {
    socket: Option<String>,
    managed: Shared,
}

impl Hash for SubData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.socket.hash(state);
    }
}

/// The event-stream builder handed to `Subscription::run_with`. Spawns the blocking
/// [`run_event_loop`] on a thread and bridges its callback into iced's async output via an unbounded
/// channel.
fn daemon_events(data: &SubData) -> BoxStream<'static, Message> {
    use iced::futures::StreamExt;
    let socket = data.socket.clone();
    let managed = data.managed.clone();
    iced::stream::channel(64, move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
        use iced::futures::{SinkExt, StreamExt};
        let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            run_event_loop(socket, managed, move |u| tx.unbounded_send(u).is_ok());
        });
        while let Some(update) = rx.next().await {
            if output.send(Message::Daemon(update)).await.is_err() {
                break;
            }
        }
    })
    .boxed()
}

/// Forward tray menu clicks into the app. The tray's menu callbacks send [`tray::MenuAction`]s on a
/// process-global channel; a thread blocks on that receiver and bridges each into a
/// [`Message::TrayMenu`].
fn tray_events(_: &()) -> BoxStream<'static, Message> {
    use iced::futures::StreamExt;
    iced::stream::channel(16, move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
        use iced::futures::{SinkExt, StreamExt};
        let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let events = tray::menu_receiver();
            while let Ok(action) = events.recv() {
                if tx.unbounded_send(action).is_err() {
                    break;
                }
            }
        });
        while let Some(action) = rx.next().await {
            if output.send(Message::TrayMenu(action)).await.is_err() {
                break;
            }
        }
    })
    .boxed()
}

/// Open a native "pick a .ron file" dialog (blocking); returns the chosen path as a string.
fn pick_ron() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("RON profile", &["ron"])
        .pick_file()
        .map(|p| p.display().to_string())
}

/// Open a native folder-picker dialog (blocking); returns the chosen directory as a string.
fn pick_folder() -> Option<String> {
    rfd::FileDialog::new().pick_folder().map(|p| p.display().to_string())
}
