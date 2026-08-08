//! `ui-test-egui` — the egui/eframe implementation of the deckhand UI-toolkit bake-off.
//!
//! The **immediate-mode** counterpart to the iced reference (PLAN §5.1): the same four-region layout
//! and daemon wiring, but redrawn every frame with no message/task loop. Off-thread work (the event
//! monitor + each daemon command) runs on plain threads that push results over an `mpsc` channel and
//! call `ctx.request_repaint()`; the UI drains the channel at the top of each frame. Toolkit-
//! independent bits (settings, the `ipc` client, the subscribe loop, the nav model) come from
//! `ui-test-common`; this crate is only the egui widget layer + that channel plumbing.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui;
use egui::Color32;
use egui::Panel;
use egui::containers::CentralPanel;
use ipc::{Event, ProfileRole, RunState, StatusSnapshot};
use ui_test_common::{
    AppSettings, Category, Client, DaemonUpdate, INPUT_PRESETS, OUTPUT_PRESETS, run_event_loop,
};

// Accent colors, roughly matching the iced build (Start/connected green, Stop/error red, Connect
// blue) + the Xbox face-button hues.
const GREEN: Color32 = Color32::from_rgb(60, 160, 90);
const RED: Color32 = Color32::from_rgb(190, 75, 75);
const BLUE: Color32 = Color32::from_rgb(70, 120, 200);
const YELLOW: Color32 = Color32::from_rgb(210, 180, 60);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([940.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "deckhand — UI toolkit test (egui)",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// A result posted back from an off-thread daemon call (or the event monitor) to the UI thread.
enum DaemonMsg {
    Update(DaemonUpdate),
    Status(Result<StatusSnapshot, String>),
    Devices(Result<Vec<String>, String>),
    Cmd(Result<(), String>),
}

struct App {
    settings: AppSettings,
    socket: Option<String>,
    connected: bool,
    status: Option<StatusSnapshot>,
    devices: Vec<String>,
    category: Category,
    error: Option<String>,
    /// Cloned into every worker thread; workers post `DaemonMsg`s back here.
    tx: Sender<DaemonMsg>,
    rx: Receiver<DaemonMsg>,
    /// For workers to wake the UI (`request_repaint`) and to spawn command threads.
    ctx: egui::Context,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let ctx = cc.egui_ctx.clone();
        let socket: Option<String> = None;

        // Persistent event monitor: forward every DaemonUpdate to the UI + wake it.
        {
            let (tx, ctx, socket) = (tx.clone(), ctx.clone(), socket.clone());
            thread::spawn(move || {
                run_event_loop(socket, move |u| {
                    let ok = tx.send(DaemonMsg::Update(u)).is_ok();
                    ctx.request_repaint();
                    ok
                });
            });
        }

        App {
            settings: AppSettings::load(),
            socket,
            connected: false,
            status: None,
            devices: Vec::new(),
            category: Category::Settings,
            error: None,
            tx,
            rx,
            ctx,
        }
    }

    // --- off-thread daemon work (each on its own short-lived connection) --------------------

    fn spawn_status(&self) {
        let (tx, ctx, socket) = (self.tx.clone(), self.ctx.clone(), self.socket.clone());
        thread::spawn(move || {
            let r = Client::new(socket).status().map_err(|e| e.to_string());
            let _ = tx.send(DaemonMsg::Status(r));
            ctx.request_repaint();
        });
    }

    fn spawn_devices(&self) {
        let (tx, ctx, socket) = (self.tx.clone(), self.ctx.clone(), self.socket.clone());
        thread::spawn(move || {
            let r = Client::new(socket).list_devices().map_err(|e| e.to_string());
            let _ = tx.send(DaemonMsg::Devices(r));
            ctx.request_repaint();
        });
    }

    fn spawn_cmd(&self, op: impl FnOnce(&mut Client) -> std::io::Result<()> + Send + 'static) {
        let (tx, ctx, socket) = (self.tx.clone(), self.ctx.clone(), self.socket.clone());
        thread::spawn(move || {
            let r = op(&mut Client::new(socket)).map_err(|e| e.to_string());
            let _ = tx.send(DaemonMsg::Cmd(r));
            ctx.request_repaint();
        });
    }

    /// Seed status + devices (on connect / manual refresh).
    fn refresh(&self) {
        self.spawn_status();
        self.spawn_devices();
    }

    // --- state maintenance ------------------------------------------------------------------

    /// Drain everything the workers posted since the last frame.
    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                // On (re)connect, seed once; after that events carry every change (seed-then-deltas).
                DaemonMsg::Update(DaemonUpdate::Connected) => {
                    self.connected = true;
                    self.error = None;
                    self.refresh();
                }
                DaemonMsg::Update(DaemonUpdate::Disconnected) => {
                    self.connected = false;
                    self.status = None;
                    self.devices.clear();
                }
                DaemonMsg::Update(DaemonUpdate::Event(ev)) => self.apply_event(ev),
                DaemonMsg::Status(Ok(s)) => {
                    self.status = Some(s);
                    self.error = None;
                }
                DaemonMsg::Status(Err(e)) => {
                    self.status = None;
                    self.error = Some(e);
                }
                DaemonMsg::Devices(Ok(d)) => self.devices = d,
                DaemonMsg::Devices(Err(e)) => self.error = Some(e),
                DaemonMsg::Cmd(Ok(())) => self.error = None,
                DaemonMsg::Cmd(Err(e)) => self.error = Some(e),
            }
        }
    }

    /// Apply one absolute-valued event to the cached status in place (no refetch). Ignored until the
    /// first snapshot has seeded `status`.
    fn apply_event(&mut self, ev: Event) {
        let Some(status) = self.status.as_mut() else { return };
        match ev {
            Event::State(s) => status.state = s,
            Event::BindingLost => status.state = RunState::WaitingForDevice,
            Event::BindingAcquired(id) => status.bound = Some(id),
            Event::InputStaged(i) => status.input = i,
            Event::OutputStaged(o) => status.output = o,
            Event::ProfileSet { role, name } => match role {
                ProfileRole::Main => status.main = name,
                ProfileRole::Fallback => status.fallback = name,
            },
            Event::GlobalConfigSet(g) => status.globals = g,
            _ => {}
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("save settings: {e}"));
        }
    }

    // --- widget layer -----------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let state = self.status.as_ref().map(|s| s.state);
            let idle = matches!(state, Some(RunState::Idle));
            let running = matches!(state, Some(RunState::Running | RunState::WaitingForDevice));

            if ui.add_enabled(idle, egui::Button::new("Start").fill(GREEN)).clicked() {
                self.spawn_cmd(|c| c.start());
            }
            if ui.add_enabled(running, egui::Button::new("Stop").fill(RED)).clicked() {
                self.spawn_cmd(|c| c.stop());
            }
            if ui.add_enabled(!self.connected, egui::Button::new("Connect").fill(BLUE)).clicked() {
                self.refresh();
            }

            // Pickers + refresh, pushed to the right (added right-to-left).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.output_combo(ui);
                ui.label("Output:");
                self.input_combo(ui);
                ui.label("Input:");
                if ui.add(egui::Button::new("⟳").fill(BLUE)).on_hover_text("Refresh devices").clicked()
                {
                    self.refresh();
                }
            });
        });
        ui.add_space(4.0);
    }

    fn input_combo(&mut self, ui: &mut egui::Ui) {
        let current = self.status.as_ref().map(|s| s.input.clone()).unwrap_or_default();
        let mut options: Vec<String> = INPUT_PRESETS.iter().map(|s| s.to_string()).collect();
        options.extend(self.devices.iter().cloned());

        let mut chosen: Option<String> = None;
        egui::ComboBox::from_id_salt("input")
            .selected_text(if current.is_empty() { "input" } else { current.as_str() })
            .show_ui(ui, |ui| {
                for opt in &options {
                    if ui.selectable_label(*opt == current, opt).clicked() {
                        chosen = Some(opt.clone());
                    }
                }
            });
        if let Some(spec) = chosen
            && spec != current
        {
            self.spawn_cmd(move |c| c.set_input(spec));
        }
    }

    fn output_combo(&mut self, ui: &mut egui::Ui) {
        let current = self.status.as_ref().map(|s| s.output.clone()).unwrap_or_default();
        let mut chosen: Option<String> = None;
        egui::ComboBox::from_id_salt("output")
            .selected_text(if current.is_empty() { "output" } else { current.as_str() })
            .show_ui(ui, |ui| {
                for opt in OUTPUT_PRESETS {
                    if ui.selectable_label(*opt == current, *opt).clicked() {
                        chosen = Some(opt.to_string());
                    }
                }
            });
        if let Some(spec) = chosen
            && spec != current
        {
            self.spawn_cmd(move |c| c.set_output(spec));
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        for &c in Category::PROFILE {
            self.nav_item(ui, c);
        }
        // Pin the application-level entries to the bottom (Settings above Globals).
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            for &c in Category::APP.iter().rev() {
                self.nav_item(ui, c);
            }
        });
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, c: Category) {
        let selected = self.category == c;
        let w = ui.available_width();
        if ui.add_sized([w, 26.0], egui::Button::selectable(selected, c.label())).clicked() {
            self.category = c;
        }
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        match self.category {
            Category::Buttons => buttons_screen(ui),
            Category::Settings => self.settings_screen(ui),
            Category::Globals => self.globals_screen(ui),
            other => {
                ui.heading(other.label());
                ui.label("Profile-edit screen — stubbed for the toolkit test.");
            }
        }
    }

    fn settings_screen(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.label("Application settings — stored separately from the daemon.");
        ui.add_space(8.0);

        let mut changed = false;
        changed |= ui
            .checkbox(&mut self.settings.start_daemon, "Start the daemon if not running on start")
            .changed();

        changed |=
            ui.checkbox(&mut self.settings.load_main, "Load the main profile on start").changed();
        changed |= self.path_row(ui, true);

        changed |= ui
            .checkbox(&mut self.settings.load_fallback, "Load the fallback profile on start")
            .changed();
        changed |= self.path_row(ui, false);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Theme:");
            let mut theme = self.settings.theme.clone();
            egui::ComboBox::from_id_salt("theme").selected_text(theme.as_str()).show_ui(ui, |ui| {
                for name in ["Dark", "Light"] {
                    ui.selectable_value(&mut theme, name.to_string(), name);
                }
            });
            if theme != self.settings.theme {
                self.settings.theme = theme;
                changed = true;
            }
        });

        if changed {
            self.save_settings();
        }
    }

    /// A profile-path row (Main when `main` else Fallback): text field + Browse, greyed when its
    /// toggle is off. Returns whether the path changed.
    fn path_row(&mut self, ui: &mut egui::Ui, main: bool) -> bool {
        let enabled = if main { self.settings.load_main } else { self.settings.load_fallback };
        let mut changed = false;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                let path = if main {
                    &mut self.settings.main_path
                } else {
                    &mut self.settings.fallback_path
                };
                if ui.text_edit_singleline(path).changed() {
                    changed = true;
                }
                if ui.button("Browse…").clicked()
                    && let Some(p) = pick_ron()
                {
                    if main {
                        self.settings.main_path = p;
                    } else {
                        self.settings.fallback_path = p;
                    }
                    changed = true;
                }
            });
        });
        changed
    }

    fn globals_screen(&mut self, ui: &mut egui::Ui) {
        ui.heading("Globals");
        ui.label("Global engine config — widget preview; not wired to edit yet.");
        ui.add_space(8.0);
        if let Some(status) = &self.status {
            let g = &status.globals;
            ui.label(format!("Start profile: {:?}", g.start_profile));
            ui.label(format!("Master rumble: {}%", g.master_rumble));
            ui.add(egui::ProgressBar::new(g.master_rumble as f32 / 100.0).desired_width(220.0));
            ui.label(format!("LED brightness: {}", opt_pct(g.led_brightness)));
            ui.label(format!("Idle timeout: {}", opt_secs(g.idle_timeout)));
            ui.label(format!("Chords: {}", g.chords.len()));
        } else {
            ui.label("Connect to the daemon to view its globals.");
        }
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let (dot, label) =
                if self.connected { (GREEN, "connected") } else { (RED, "disconnected") };
            ui.colored_label(dot, "●");
            ui.label(label);

            if let Some(s) = &self.status {
                for cell in [
                    format!("state: {:?}", s.state),
                    format!("device: {}", s.bound.as_deref().unwrap_or("—")),
                    format!("main: {}", s.main.as_deref().unwrap_or("—")),
                    format!("fallback: {}", s.fallback.as_deref().unwrap_or("—")),
                    format!("chords: {}", s.globals.chords.len()),
                ] {
                    ui.separator();
                    ui.label(cell);
                }
            }

            if let Some(err) = &self.error {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(RED, format!("⚠ {err}"));
                });
            }
        });
        ui.add_space(2.0);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain();
        let dark = self.settings.theme != "Light";
        ui.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });

        Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        Panel::bottom("status_bar").show(ui, |ui| self.bottom_bar(ui));
        Panel::left("sidebar").resizable(false).exact_size(180.0).show(ui, |ui| self.sidebar(ui));
        CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.content(ui));
        });
    }
}

// --- the Buttons mockup (Steam-Deck-style groups; nothing wired) ---------------------------

fn buttons_screen(ui: &mut egui::Ui) {
    ui.heading("Buttons");
    group(ui, "Face Buttons", |ui| {
        behavior_card(ui);
        input_card(ui, Some(GREEN), "A Button");
        input_card(ui, Some(RED), "B Button");
        input_card(ui, Some(BLUE), "X Button");
        input_card(ui, Some(YELLOW), "Y Button");
    });
    group(ui, "Bumpers", |ui| {
        input_card(ui, None, "Left Bumper");
        input_card(ui, None, "Right Bumper");
    });
    group(ui, "D-Pad", |ui| {
        for n in ["Up", "Down", "Left", "Right"] {
            input_card(ui, None, n);
        }
    });
    group(ui, "System", |ui| {
        for n in ["View", "Menu", "Steam"] {
            input_card(ui, None, n);
        }
    });
}

fn group(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(title).size(16.0).strong());
    ui.add_space(2.0);
    contents(ui);
}

/// A group's "Behavior" selector (buttons only have Button Pad; the rest is filler) + a gear.
fn behavior_card(ui: &mut egui::Ui) {
    card(ui, |ui| {
        ui.label("Behavior");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let _ = ui.button("⚙");
            let mut sel = "Button Pad".to_string();
            egui::ComboBox::from_id_salt("behavior").selected_text(sel.as_str()).show_ui(ui, |ui| {
                for o in ["Button Pad", "Lorem Ipsum", "Dolor Sit Amet"] {
                    ui.selectable_value(&mut sel, o.to_string(), o);
                }
            });
        });
    });
}

/// One input row: an optional colored glyph + the input name, and a gear on the right.
fn input_card(ui: &mut egui::Ui, dot: Option<Color32>, label: &str) {
    card(ui, |ui| {
        if let Some(c) = dot {
            ui.colored_label(c, "●");
        }
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let _ = ui.button("⚙");
        });
    });
}

/// A full-width bordered card holding a single horizontal row.
fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(contents);
    });
}

fn pick_ron() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("RON profile", &["ron"])
        .pick_file()
        .map(|p| p.display().to_string())
}

fn opt_pct(v: Option<u8>) -> String {
    v.map(|x| format!("{x}%")).unwrap_or_else(|| "default".into())
}

fn opt_secs(v: Option<u16>) -> String {
    v.map(|x| format!("{x}s")).unwrap_or_else(|| "default".into())
}
