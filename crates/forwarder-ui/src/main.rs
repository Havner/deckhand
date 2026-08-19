//! `deckhand-forwarder` — a minimal, touch-friendly UI for running the Deck as a **network
//! forwarder**: pick a local controller as input, type a `host:port` output, press Start.
//!
//! It's a deliberate, scoped duplicate of the parts of the main `deckhand` UI it reuses (the daemon
//! client + connect loop, RON persistence, a few styles) — no shared crate yet; this is a prototype
//! to see whether the single-screen forwarder is worth keeping. Unlike the main UI it has:
//!
//! - a single always-visible window (no tray), forced to the persisted theme (default Dark);
//! - **fixed** daemon policy (launch-if-absent, restore-input, never auto-start — see [`daemon`]);
//! - one screen: the daemon controls, an on-screen numeric keypad for the output address, and a
//!   master-rumble slider; plus a status bar.

// Windows: GUI subsystem so launching never spawns a console (matches the main UI). Errors surface
// in the status bar; there's no console logging.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod daemon;
mod device;
mod persist;
mod settings;
mod style;
mod view;

use std::hash::{Hash, Hasher};

use config::DeviceConfig;
use daemon::{Client, DaemonUpdate, Handle, run_event_loop};
use iced::futures::stream::BoxStream;
use iced::window;
use iced::{Size, Subscription, Task, Theme};
use ipc::{Event, RunState, StatusSnapshot};
use settings::Settings;

/// Preset input selections offered before the daemon's live `list-devices` is appended. Same grammar
/// the daemon parses for `SetInput`, but **without** the `<network>` entry — a forwarder's input is
/// always a local controller.
pub(crate) const INPUT_PRESETS: &[&str] = &["auto", "dongle", "wired", "bt"];

/// Global UI scale — the Steam Deck's touch display makes default-sized widgets too small, so the
/// whole UI is drawn at 2×. A window/render scale (not per-widget sizes) so *everything* grows
/// uniformly: text, buttons, comboboxes and their dropdown menus, the slider, padding. Note it halves
/// the *logical* space, so the window opens larger (see `Settings` defaults) and the content pane
/// scrolls if it doesn't fit.
pub(crate) const UI_SCALE: f32 = 1.75;

fn main() -> iced::Result {
    let daemon = daemon::handle();

    let boot = {
        let daemon = daemon.clone();
        move || (App::new(daemon.clone()), App::open_window())
    };
    let result = iced::daemon(boot, App::update, App::view)
        .title(|_app: &App, _window| "deckhand forwarder".to_string())
        .subscription(App::subscription)
        .theme(|app: &App, _window| app.active_theme())
        .scale_factor(|_app: &App, _window| UI_SCALE)
        .run();
    daemon::shutdown_managed(&daemon);
    result
}

/// The whole application state.
pub(crate) struct App {
    /// The forwarder's own persisted settings (theme / window size / last input+output).
    settings: Settings,
    /// The device config (shared `devcfg.ron`); only `master_rumble` is edited here.
    device_config: DeviceConfig,
    /// The **live** content of the output (`ip:port`) text field. User-owned: seeded from
    /// `settings.last_output`, edited freely (typing or keypad), and **never** overwritten by the
    /// daemon — it's applied to the daemon only at Start.
    output_text: String,
    /// Socket/pipe override (`None` = default). The forwarder never overrides it.
    socket: Option<String>,
    /// Whether the daemon is currently reachable.
    connected: bool,
    /// Latest engine status (seeds the bottom bar + input selection); `None` when disconnected.
    status: Option<StatusSnapshot>,
    /// The daemon's live device ids (appended to the input presets).
    devices: Vec<String>,
    /// Last error, surfaced in the status bar.
    error: Option<String>,
    /// The UI-managed daemon handle (shared with the connect loop).
    daemon: Handle,
    /// The window id while open (captured on open); used to persist size on close.
    window: Option<window::Id>,
}

/// Everything the view can emit.
#[derive(Debug, Clone)]
pub(crate) enum Message {
    /// An update from the event-stream subscription.
    Daemon(DaemonUpdate),
    /// Re-enumerate devices + refresh status (no USB hotplug — the manual trigger).
    Refresh,
    /// Daemon controls. Start sets the output from the text field, then starts the engine.
    Start,
    Stop,
    /// Input selection (a spec string).
    InputSelected(String),
    /// The output text field was edited directly.
    OutputChanged(String),
    /// An on-screen keypad key (a digit / `.` / `:`) — appended to the output field.
    Key(char),
    /// The keypad backspace — drops the last char of the output field.
    Backspace,
    /// The master-rumble slider moved.
    RumbleChanged(u8),
    /// Results of daemon calls run off the render thread (stringified — `io::Error` isn't `Clone`).
    StatusFetched(Result<StatusSnapshot, String>),
    DevicesFetched(Result<Vec<String>, String>),
    CmdDone(Result<(), String>),
    /// Window lifecycle: open captures the id; a close request persists size + exits; resize tracks
    /// the size to persist.
    WindowOpened(window::Id),
    CloseRequested,
    WindowClosed(window::Id),
    WindowResized(Size),
}

impl App {
    fn new(daemon: Handle) -> Self {
        let settings = Settings::load();
        let output_text = settings.last_output_network.clone();
        App {
            settings,
            device_config: device::load(),
            output_text,
            socket: None,
            connected: false,
            status: None,
            devices: Vec::new(),
            error: None,
            daemon,
            window: None,
        }
    }

    /// The engine's current run state, or `None` when disconnected.
    fn run_state(&self) -> Option<RunState> {
        self.status.as_ref().map(|s| s.state)
    }

    /// Whether the engine is started (not idle) — the condition for restarting it on an input change.
    fn engine_started(&self) -> bool {
        matches!(
            self.run_state(),
            Some(RunState::Running | RunState::WaitingForDevice)
        )
    }

    /// Stage a new input spec: remember it as the last input, apply it — restarting the engine if it
    /// is already running (staged input only takes effect at start).
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

    /// Persist the edited output text as the last output (it's applied to the daemon only at Start).
    fn remember_output(&mut self) {
        self.settings.last_output_network = self.output_text.clone();
        self.save_settings();
    }

    /// Persist the device config to `devcfg.ron` and ship it to the daemon (the file, the in-memory
    /// copy, and the daemon stay in lock-step; the daemon's echoed `DeviceConfigSet` re-lands it).
    fn apply_device_config(&mut self) -> Task<Message> {
        if let Err(e) = device::save(&self.device_config) {
            self.error = Some(format!("save device config: {e}"));
        }
        let d = self.device_config.clone();
        self.cmd_task(move |c| c.set_device_config(d))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Daemon(DaemonUpdate::Connected) => {
                self.connected = true;
                self.error = None;
                return self.refresh_task();
            }
            Message::Daemon(DaemonUpdate::Disconnected) => {
                self.connected = false;
                self.status = None;
                self.devices.clear();
            }
            Message::Daemon(DaemonUpdate::Event(ev)) => self.apply_event(ev),
            Message::Daemon(DaemonUpdate::Error(e)) => self.error = Some(e),

            Message::Refresh => return self.refresh_task(),
            Message::Start => {
                let out = self.output_text.trim().to_string();
                if out.is_empty() {
                    self.error = Some("output (ip:port) is empty".into());
                    return Task::none();
                }
                // Output is set from the text field verbatim, only now — then start. A rejected spec
                // aborts the start and surfaces as the error.
                return Task::batch([
                    self.cmd_task(move |c| {
                        c.set_output(out)?;
                        c.start()
                    }),
                    self.refresh_task(),
                ]);
            }
            Message::Stop => {
                return Task::batch([self.cmd_task(|c| c.stop()), self.refresh_task()]);
            }
            Message::InputSelected(spec) => return self.apply_input(spec),
            Message::OutputChanged(t) => {
                self.output_text = t;
                self.remember_output();
            }
            Message::Key(c) => {
                self.output_text.push(c);
                self.remember_output();
            }
            Message::Backspace => {
                self.output_text.pop();
                self.remember_output();
            }
            Message::RumbleChanged(v) => {
                self.device_config.master_rumble = v;
                return self.apply_device_config();
            }

            Message::StatusFetched(Ok(s)) => self.status = Some(s),
            Message::StatusFetched(Err(e)) => {
                self.status = None;
                self.error = Some(e);
            }
            Message::DevicesFetched(Ok(d)) => self.devices = d,
            Message::DevicesFetched(Err(e)) => self.error = Some(e),
            Message::CmdDone(Ok(())) => self.error = None,
            Message::CmdDone(Err(e)) => self.error = Some(e),

            Message::WindowOpened(id) => self.window = Some(id),
            Message::WindowClosed(id) => {
                if self.window == Some(id) {
                    self.window = None;
                }
            }
            Message::WindowResized(size) => {
                self.settings.window_width = size.width as u32;
                self.settings.window_height = size.height as u32;
            }
            Message::CloseRequested => {
                self.save_settings();
                return iced::exit();
            }
        }
        Task::none()
    }

    /// The active iced theme, resolved from the persisted theme **name** (falls back to Dark).
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

    fn subscription(&self) -> Subscription<Message> {
        let data = SubData {
            socket: self.socket.clone(),
            managed: self.daemon.clone(),
        };
        Subscription::batch([
            Subscription::run_with(data, daemon_events),
            window::close_requests().map(|_id| Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(_id, size)| Message::WindowResized(size)),
        ])
    }

    /// Open the main window, routing its id back as [`Message::WindowOpened`]. `exit_on_close_request`
    /// is off so the WM close button reaches our handler (which persists the size before exit).
    fn open_window() -> Task<Message> {
        let settings = Settings::load();
        let size = Size::new(settings.window_width as f32, settings.window_height as f32);
        #[allow(unused_mut)]
        let mut win = window::Settings {
            exit_on_close_request: false,
            size,
            ..window::Settings::default()
        };
        #[cfg(target_os = "linux")]
        {
            win.platform_specific.application_id = "deckhand-forwarder".to_string();
        }
        window::open(win).1.map(Message::WindowOpened)
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
            async move {
                Client::new(socket)
                    .list_devices()
                    .map_err(|e| e.to_string())
            },
            Message::DevicesFetched,
        )
    }

    /// Status + device list together (on connect / manual refresh).
    fn refresh_task(&self) -> Task<Message> {
        Task::batch([self.status_task(), self.devices_task()])
    }

    /// Run a daemon command off the render thread; its outcome returns as [`Message::CmdDone`].
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

    /// Apply one daemon event to the cached status in place. Events are absolute-valued, so after the
    /// initial seed the bar stays current without refetching. The forwarder tracks only what it
    /// displays (state / controller / bound device / input) plus the shared device config; the rest
    /// is ignored. The output text field is **never** touched by an event — it's user-owned.
    fn apply_event(&mut self, ev: Event) {
        // A device-config change (our echoed push, or another client's) syncs the UI copy + file.
        if let Event::DeviceConfigSet(d) = &ev {
            self.device_config = d.clone();
            if let Err(e) = device::save(&self.device_config) {
                self.error = Some(format!("save device config: {e}"));
            }
        }
        let Some(status) = self.status.as_mut() else {
            return;
        };
        match ev {
            Event::State(s) => {
                status.state = s;
                if s == RunState::Idle {
                    status.controller = None;
                    status.active = None;
                }
            }
            Event::ControllerConnected(c) => status.controller = Some(c),
            Event::BindingRemoved => status.bound = None,
            Event::BindingAcquired(id) => status.bound = Some(id),
            Event::InputStaged(i) => status.input = i,
            Event::DeviceConfigSet(d) => status.device_config = d,
            // Not shown by the forwarder — output field is user-owned; no profiles/chords/role/battery.
            Event::OutputStaged(_)
            | Event::ActiveRole(_)
            | Event::ProfileSet { .. }
            | Event::ChordsSet(_)
            | Event::Battery { .. } => {}
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("save settings: {e}"));
        }
    }
}

/// The data handed to the daemon-events subscription: the socket override plus the managed-daemon
/// handle. Its [`Hash`] covers only the socket (the subscription's identity), so the handle (cloned
/// fresh every render) never restarts the loop.
#[derive(Clone)]
struct SubData {
    socket: Option<String>,
    managed: Handle,
}

impl Hash for SubData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.socket.hash(state);
    }
}

/// The event-stream builder handed to `Subscription::run_with`. Spawns the blocking
/// [`run_event_loop`] on a thread and bridges its callback into iced's async output.
fn daemon_events(data: &SubData) -> BoxStream<'static, Message> {
    use iced::futures::StreamExt;
    let socket = data.socket.clone();
    let managed = data.managed.clone();
    iced::stream::channel(
        64,
        move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
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
        },
    )
    .boxed()
}
