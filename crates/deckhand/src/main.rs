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
mod nav;
mod settings;
mod style;
mod view;

use daemon::{Client, DaemonUpdate, run_event_loop};
use iced::futures::stream::BoxStream;
use iced::{Subscription, Task, Theme};
use ipc::{Event, ProfileRole, StatusSnapshot};
use nav::Category;
use settings::AppSettings;

/// Preset input selections offered before the daemon's live `list-devices` is appended (the same
/// grammar the daemon parses for `SetInput`).
pub const INPUT_PRESETS: &[&str] = &["auto", "dongle", "wired", "bt"];

/// Preset output selections (`host:port` is typed, not listed).
pub const OUTPUT_PRESETS: &[&str] = &["local"];

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("deckhand")
        .subscription(App::subscription)
        .theme(App::active_theme)
        .run()
}

/// The whole application state (Elm-architecture `State`).
pub struct App {
    /// The UI's own settings (Settings screen), persisted separately from the daemon.
    settings: AppSettings,
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
}

/// Everything the view can emit.
#[derive(Debug, Clone)]
pub enum Message {
    /// Sidebar navigation.
    Navigate(Category),
    /// An update from the event-stream subscription.
    Daemon(DaemonUpdate),
    /// Top-bar daemon controls.
    Connect,
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
    ToggleLoadMain(bool),
    ToggleLoadFallback(bool),
    MainPathChanged(String),
    FallbackPathChanged(String),
    BrowseMain,
    BrowseFallback,
    /// Theme selection (any built-in iced theme).
    SetTheme(Theme),
    /// A no-op for unwired mockup widgets (the Buttons tab).
    Ignored,
}

impl App {
    fn new() -> Self {
        App {
            settings: AppSettings::load(),
            socket: None,
            connected: false,
            status: None,
            devices: Vec::new(),
            category: Category::Settings,
            error: None,
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
            Message::Connect | Message::Refresh => return self.refresh_task(),
            Message::Start => return self.cmd_task(|c| c.start()),
            Message::Stop => return self.cmd_task(|c| c.stop()),
            Message::InputSelected(spec) => return self.cmd_task(move |c| c.set_input(spec)),
            Message::OutputSelected(spec) => return self.cmd_task(move |c| c.set_output(spec)),

            // --- arms that only mutate state (fall through to Task::none()) ---
            Message::Navigate(c) => self.category = c,
            Message::Daemon(DaemonUpdate::Disconnected) => {
                self.connected = false;
                self.status = None;
                self.devices.clear();
            }
            Message::Daemon(DaemonUpdate::Event(ev)) => self.apply_event(ev),
            Message::StatusFetched(Ok(s)) => {
                self.status = Some(s);
                self.error = None;
            }
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

    fn view(&self) -> iced::Element<'_, Message> {
        view::view(self)
    }

    /// The event-stream subscription, keyed on the socket so it stays alive for the app's life.
    fn subscription(&self) -> Subscription<Message> {
        Subscription::run_with(self.socket.clone(), daemon_events)
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

/// The event-stream builder handed to `Subscription::run_with` (a plain fn pointer — it captures
/// nothing but the socket it is given). Spawns the blocking [`run_event_loop`] on a thread and
/// bridges its callback into iced's async output via an unbounded channel.
fn daemon_events(socket: &Option<String>) -> BoxStream<'static, Message> {
    use iced::futures::StreamExt;
    let socket = socket.clone();
    iced::stream::channel(64, move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
        use iced::futures::{SinkExt, StreamExt};
        let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            run_event_loop(socket, move |u| tx.unbounded_send(u).is_ok());
        });
        while let Some(update) = rx.next().await {
            if output.send(Message::Daemon(update)).await.is_err() {
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
