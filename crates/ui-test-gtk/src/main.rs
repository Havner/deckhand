//! `ui-test-gtk` — the GTK4 implementation of the deckhand UI-toolkit bake-off (PLAN §5.1).
//!
//! Pure gtk4 (not libadwaita — impractical on Windows, PLAN §5.2). Retained-mode/GObject: build the
//! widgets once, hold handles, and mutate them on events. Off-thread work (the event monitor + each
//! daemon command) runs on std threads that post `DaemonMsg`s over an `async-channel`; the glib main
//! loop drains it via `glib::spawn_future_local`. Toolkit-independent bits come from `ui-test-common`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::thread;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use gtk::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DropDown, Entry,
    Frame, Label, ListBox, ListBoxRow, Orientation, ScrolledWindow, SelectionMode, Stack, StringList,
};
use ipc::{Event, ProfileRole, RunState, StatusSnapshot};
use ui_test_common::{
    AppSettings, Category, Client, DaemonUpdate, INPUT_PRESETS, OUTPUT_PRESETS, run_event_loop,
};

const APP_ID: &str = "com.deckhand.ui.gtk";

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

/// A result posted from an off-thread worker to the glib main loop.
enum DaemonMsg {
    Update(DaemonUpdate),
    Status(Result<StatusSnapshot, String>),
    Devices(Result<Vec<String>, String>),
    Cmd(Result<(), String>),
    Browse { main: bool, path: Option<String> },
}

/// The daemon-driven UI state (single-threaded — glib main loop).
struct State {
    settings: AppSettings,
    connected: bool,
    status: Option<StatusSnapshot>,
    input_options: Vec<String>,
    error: Option<String>,
}

/// All the widgets the event handlers mutate, plus shared state + the worker channel. `Rc<Ui>` is
/// cloned into every closure.
struct Ui {
    state: RefCell<State>,
    suppress: Cell<bool>, // guard: don't treat programmatic dropdown updates as user selections
    tx: async_channel::Sender<DaemonMsg>,
    conn: Label,
    state_l: Label,
    bound: Label,
    main_l: Label,
    fallback_l: Label,
    chords: Label,
    error: Label,
    start: Button,
    stop: Button,
    connect: Button,
    input_dd: DropDown,
    output_dd: DropDown,
    main_entry: Entry,
    fb_entry: Entry,
}

impl Ui {
    // --- off-thread daemon work (each on its own short-lived connection) --------------------

    fn spawn_status(&self) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let r = Client::new(None).status().map_err(|e| e.to_string());
            let _ = tx.send_blocking(DaemonMsg::Status(r));
        });
    }

    fn spawn_devices(&self) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let r = Client::new(None).list_devices().map_err(|e| e.to_string());
            let _ = tx.send_blocking(DaemonMsg::Devices(r));
        });
    }

    fn spawn_cmd(&self, op: impl FnOnce(&mut Client) -> std::io::Result<()> + Send + 'static) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let r = op(&mut Client::new(None)).map_err(|e| e.to_string());
            let _ = tx.send_blocking(DaemonMsg::Cmd(r));
        });
    }

    fn spawn_browse(&self, main: bool) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new()
                .add_filter("RON profile", &["ron"])
                .pick_file()
                .map(|p| p.display().to_string());
            let _ = tx.send_blocking(DaemonMsg::Browse { main, path });
        });
    }

    // --- applying worker results ------------------------------------------------------------

    fn apply(&self, msg: DaemonMsg) {
        match msg {
            // On (re)connect, seed once; afterwards events carry every change (seed-then-deltas).
            DaemonMsg::Update(DaemonUpdate::Connected) => {
                {
                    let mut st = self.state.borrow_mut();
                    st.connected = true;
                    st.error = None;
                }
                self.spawn_status();
                self.spawn_devices();
                self.refresh();
            }
            DaemonMsg::Update(DaemonUpdate::Disconnected) => {
                {
                    let mut st = self.state.borrow_mut();
                    st.connected = false;
                    st.status = None;
                }
                self.refresh();
            }
            DaemonMsg::Update(DaemonUpdate::Event(ev)) => {
                self.apply_event(ev);
                self.refresh();
                self.sync_dropdowns();
            }
            DaemonMsg::Status(Ok(s)) => {
                {
                    let mut st = self.state.borrow_mut();
                    st.status = Some(s);
                    st.error = None;
                }
                self.refresh();
                self.sync_dropdowns();
            }
            DaemonMsg::Status(Err(e)) => {
                {
                    let mut st = self.state.borrow_mut();
                    st.status = None;
                    st.error = Some(e);
                }
                self.refresh();
            }
            DaemonMsg::Devices(Ok(d)) => {
                {
                    let mut st = self.state.borrow_mut();
                    st.input_options =
                        INPUT_PRESETS.iter().map(|s| s.to_string()).chain(d).collect();
                }
                self.sync_dropdowns();
            }
            DaemonMsg::Devices(Err(e)) => {
                self.state.borrow_mut().error = Some(e);
                self.refresh();
            }
            DaemonMsg::Cmd(Ok(())) => {
                self.state.borrow_mut().error = None;
                self.refresh();
            }
            DaemonMsg::Cmd(Err(e)) => {
                self.state.borrow_mut().error = Some(e);
                self.refresh();
            }
            DaemonMsg::Browse { main, path } => {
                if let Some(p) = path {
                    {
                        let mut st = self.state.borrow_mut();
                        if main {
                            st.settings.main_path = p.clone();
                        } else {
                            st.settings.fallback_path = p.clone();
                        }
                        let _ = st.settings.save();
                    }
                    if main {
                        self.main_entry.set_text(&p);
                    } else {
                        self.fb_entry.set_text(&p);
                    }
                }
            }
        }
    }

    /// Apply one absolute-valued event to the cached status in place (no refetch).
    fn apply_event(&self, ev: Event) {
        let mut st = self.state.borrow_mut();
        let Some(s) = st.status.as_mut() else { return };
        match ev {
            Event::State(x) => s.state = x,
            Event::BindingLost => s.state = RunState::WaitingForDevice,
            Event::BindingAcquired(id) => s.bound = Some(id),
            Event::InputStaged(i) => s.input = i,
            Event::OutputStaged(o) => s.output = o,
            Event::ProfileSet { role, name } => match role {
                ProfileRole::Main => s.main = name,
                ProfileRole::Fallback => s.fallback = name,
            },
            Event::GlobalConfigSet(g) => s.globals = g,
            _ => {}
        }
    }

    /// Update the top/bottom bars from the cached state.
    fn refresh(&self) {
        let st = self.state.borrow();
        let (dot, txt) = if st.connected {
            ("#4caf50", "connected")
        } else {
            ("#e05252", "disconnected")
        };
        self.conn.set_markup(&format!("<span foreground='{dot}'>●</span> {txt}"));

        match &st.status {
            Some(s) => {
                self.state_l.set_text(&format!("state: {:?}", s.state));
                self.bound.set_text(&format!("device: {}", s.bound.as_deref().unwrap_or("—")));
                self.main_l.set_text(&format!("main: {}", s.main.as_deref().unwrap_or("—")));
                self.fallback_l
                    .set_text(&format!("fallback: {}", s.fallback.as_deref().unwrap_or("—")));
                self.chords.set_text(&format!("chords: {}", s.globals.chords.len()));
                self.start.set_sensitive(matches!(s.state, RunState::Idle));
                self.stop.set_sensitive(matches!(
                    s.state,
                    RunState::Running | RunState::WaitingForDevice
                ));
            }
            None => {
                for (l, t) in [
                    (&self.state_l, "state: —"),
                    (&self.bound, "device: —"),
                    (&self.main_l, "main: —"),
                    (&self.fallback_l, "fallback: —"),
                    (&self.chords, "chords: —"),
                ] {
                    l.set_text(t);
                }
                self.start.set_sensitive(false);
                self.stop.set_sensitive(false);
            }
        }
        self.connect.set_sensitive(!st.connected);
        self.error.set_markup(&match &st.error {
            Some(e) => format!("<span foreground='#e05252'>⚠ {}</span>", glib::markup_escape_text(e)),
            None => String::new(),
        });
    }

    /// Rebuild the input model (presets + devices) and set both dropdowns' selection to the staged
    /// values, suppressing the resulting change signal so it isn't taken as a user pick.
    fn sync_dropdowns(&self) {
        let st = self.state.borrow();
        let cur_in = st.status.as_ref().map(|s| s.input.clone()).unwrap_or_default();
        let cur_out = st.status.as_ref().map(|s| s.output.clone()).unwrap_or_default();
        let refs: Vec<&str> = st.input_options.iter().map(|s| s.as_str()).collect();
        let model = StringList::new(&refs);
        let in_idx = st.input_options.iter().position(|o| *o == cur_in).unwrap_or(0) as u32;
        let out_idx = OUTPUT_PRESETS.iter().position(|o| *o == cur_out).unwrap_or(0) as u32;
        drop(st);

        self.suppress.set(true);
        self.input_dd.set_model(Some(&model));
        self.input_dd.set_selected(in_idx);
        self.output_dd.set_selected(out_idx);
        self.suppress.set(false);
    }
}

fn build_ui(app: &Application) {
    let settings = AppSettings::load();

    // --- widgets that events mutate ---
    let make_label = || {
        let l = Label::new(None);
        l.set_xalign(0.0);
        l
    };
    let conn = make_label();
    let state_l = make_label();
    let bound = make_label();
    let main_l = make_label();
    let fallback_l = make_label();
    let chords = make_label();
    let error = make_label();
    error.set_hexpand(true);
    error.set_xalign(1.0);

    let start = Button::with_label("Start");
    start.add_css_class("suggested-action");
    let stop = Button::with_label("Stop");
    stop.add_css_class("destructive-action");
    let connect = Button::with_label("Connect");
    let refresh_btn = Button::with_label("⟳");

    let input_dd = DropDown::from_strings(INPUT_PRESETS);
    input_dd.set_size_request(170, -1);
    let output_dd = DropDown::from_strings(OUTPUT_PRESETS);

    let main_entry = Entry::builder().hexpand(true).placeholder_text("path to .ron file").build();
    main_entry.set_text(&settings.main_path);
    main_entry.set_sensitive(settings.load_main);
    let fb_entry = Entry::builder().hexpand(true).placeholder_text("path to .ron file").build();
    fb_entry.set_text(&settings.fallback_path);
    fb_entry.set_sensitive(settings.load_fallback);

    // --- shared state + worker channel ---
    let (tx, rx) = async_channel::unbounded::<DaemonMsg>();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            run_event_loop(None, move |u| tx.send_blocking(DaemonMsg::Update(u)).is_ok());
        });
    }

    let ui = Rc::new(Ui {
        state: RefCell::new(State {
            settings,
            connected: false,
            status: None,
            input_options: INPUT_PRESETS.iter().map(|s| s.to_string()).collect(),
            error: None,
        }),
        suppress: Cell::new(false),
        tx,
        conn,
        state_l,
        bound,
        main_l,
        fallback_l,
        chords,
        error,
        start,
        stop,
        connect,
        input_dd,
        output_dd,
        main_entry,
        fb_entry,
    });

    // --- top bar ---
    let top = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    top.append(&ui.start);
    top.append(&ui.stop);
    top.append(&ui.connect);
    let top_spacer = GtkBox::new(Orientation::Horizontal, 0);
    top_spacer.set_hexpand(true);
    top.append(&top_spacer);
    top.append(&refresh_btn);
    top.append(&Label::new(Some("Input:")));
    top.append(&ui.input_dd);
    top.append(&Label::new(Some("Output:")));
    top.append(&ui.output_dd);

    // --- content stack + sidebar ---
    let stack = Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    for cat in Category::PROFILE.iter().chain(Category::APP.iter()) {
        let page: gtk::Widget = match cat {
            Category::Buttons => build_buttons_page().upcast(),
            Category::Settings => build_settings_page(&ui).upcast(),
            Category::Globals => build_globals_page(&ui).upcast(),
            _ => {
                let b = page_box();
                b.append(&heading(cat.label()));
                b.append(&Label::new(Some("Profile-edit screen — stubbed for the toolkit test.")));
                b.upcast()
            }
        };
        let scroller = ScrolledWindow::builder().hexpand(true).vexpand(true).child(&page).build();
        stack.add_named(&scroller, Some(cat.label()));
    }
    stack.set_visible_child_name(Category::Settings.label());

    let sidebar = build_sidebar(&stack);

    let middle = GtkBox::new(Orientation::Horizontal, 0);
    middle.append(&sidebar);
    middle.append(&stack);
    middle.set_vexpand(true);

    // --- bottom status bar ---
    let bottom = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(8)
        .margin_end(8)
        .build();
    for w in [&ui.conn, &ui.state_l, &ui.bound, &ui.main_l, &ui.fallback_l, &ui.chords] {
        bottom.append(w);
    }
    bottom.append(&ui.error);

    let root = GtkBox::new(Orientation::Vertical, 0);
    root.append(&top);
    root.append(&gtk::Separator::new(Orientation::Horizontal));
    root.append(&middle);
    root.append(&gtk::Separator::new(Orientation::Horizontal));
    root.append(&bottom);

    // --- wiring ---
    ui.start.connect_clicked(handler(&ui, |ui| ui.spawn_cmd(|c| c.start())));
    ui.stop.connect_clicked(handler(&ui, |ui| ui.spawn_cmd(|c| c.stop())));
    ui.connect.connect_clicked(handler(&ui, |ui| {
        ui.spawn_status();
        ui.spawn_devices();
    }));
    refresh_btn.connect_clicked(handler(&ui, |ui| {
        ui.spawn_status();
        ui.spawn_devices();
    }));
    {
        let ui2 = ui.clone();
        ui.input_dd.connect_selected_notify(move |dd| {
            if ui2.suppress.get() {
                return;
            }
            let idx = dd.selected() as usize;
            let (opt, cur) = {
                let st = ui2.state.borrow();
                (
                    st.input_options.get(idx).cloned(),
                    st.status.as_ref().map(|s| s.input.clone()).unwrap_or_default(),
                )
            };
            if let Some(spec) = opt
                && spec != cur
            {
                ui2.spawn_cmd(move |c| c.set_input(spec));
            }
        });
    }
    {
        let ui2 = ui.clone();
        ui.output_dd.connect_selected_notify(move |dd| {
            if ui2.suppress.get() {
                return;
            }
            let idx = dd.selected() as usize;
            let cur = ui2.state.borrow().status.as_ref().map(|s| s.output.clone()).unwrap_or_default();
            if let Some(spec) = OUTPUT_PRESETS.get(idx).map(|s| s.to_string())
                && spec != cur
            {
                ui2.spawn_cmd(move |c| c.set_output(spec));
            }
        });
    }

    // Apply the persisted theme (system-driven; gtk switches only light/dark).
    apply_theme(&ui.state.borrow().settings.theme);

    // Drain worker results on the glib main loop.
    {
        let ui = ui.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                ui.apply(msg);
            }
        });
    }

    ui.refresh();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("deckhand — UI toolkit test (GTK4)")
        .default_width(960)
        .default_height(640)
        .child(&root)
        .build();
    window.present();
}

/// Wrap a `Fn(&Rc<Ui>)` click handler, cloning `Rc<Ui>` in — trims closure boilerplate.
fn handler(ui: &Rc<Ui>, f: impl Fn(&Rc<Ui>) + 'static) -> impl Fn(&Button) + 'static {
    let ui = ui.clone();
    move |_| f(&ui)
}

fn build_sidebar(stack: &Stack) -> GtkBox {
    let sidebar = GtkBox::new(Orientation::Vertical, 0);
    sidebar.set_size_request(180, -1);

    let profile = nav_list(stack, Category::PROFILE);
    let app_level = nav_list(stack, Category::APP);
    let spacer = GtkBox::new(Orientation::Vertical, 0);
    spacer.set_vexpand(true);

    sidebar.append(&profile);
    sidebar.append(&spacer);
    sidebar.append(&app_level);
    sidebar
}

fn nav_list(stack: &Stack, cats: &'static [Category]) -> ListBox {
    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Single);
    for cat in cats {
        let row = ListBoxRow::new();
        let label = Label::new(Some(cat.label()));
        label.set_xalign(0.0);
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_margin_start(10);
        row.set_child(Some(&label));
        list.append(&row);
    }
    let stack = stack.clone();
    list.connect_row_activated(move |_, row| {
        if let Some(cat) = cats.get(row.index() as usize) {
            stack.set_visible_child_name(cat.label());
        }
    });
    list
}

fn page_box() -> GtkBox {
    GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build()
}

fn heading(text: &str) -> Label {
    let l = Label::new(None);
    l.set_markup(&format!("<span size='x-large'>{}</span>", glib::markup_escape_text(text)));
    l.set_xalign(0.0);
    l
}

fn build_settings_page(ui: &Rc<Ui>) -> GtkBox {
    let page = page_box();
    page.append(&heading("Settings"));
    page.append(&Label::new(Some("Application settings — stored separately from the daemon.")));

    let s = ui.state.borrow().settings.clone();

    let start_cb = CheckButton::with_label("Start the daemon if not running on start");
    start_cb.set_active(s.start_daemon);
    {
        let ui = ui.clone();
        start_cb.connect_toggled(move |c| {
            ui.state.borrow_mut().settings.start_daemon = c.is_active();
            let _ = ui.state.borrow().settings.save();
        });
    }
    page.append(&start_cb);

    // Main profile
    let main_cb = CheckButton::with_label("Load the main profile on start");
    main_cb.set_active(s.load_main);
    page.append(&main_cb);
    let main_browse = Button::with_label("Browse…");
    main_browse.set_sensitive(s.load_main);
    let main_row = GtkBox::new(Orientation::Horizontal, 8);
    main_row.append(&ui.main_entry);
    main_row.append(&main_browse);
    page.append(&main_row);
    {
        let ui = ui.clone();
        let entry = ui.main_entry.clone();
        let browse = main_browse.clone();
        main_cb.connect_toggled(move |c| {
            let a = c.is_active();
            ui.state.borrow_mut().settings.load_main = a;
            let _ = ui.state.borrow().settings.save();
            entry.set_sensitive(a);
            browse.set_sensitive(a);
        });
    }
    {
        let ui = ui.clone();
        let entry = ui.main_entry.clone();
        entry.connect_changed(move |e| {
            ui.state.borrow_mut().settings.main_path = e.text().to_string();
            let _ = ui.state.borrow().settings.save();
        });
    }
    {
        let ui = ui.clone();
        main_browse.connect_clicked(move |_| ui.spawn_browse(true));
    }

    // Fallback profile
    let fb_cb = CheckButton::with_label("Load the fallback profile on start");
    fb_cb.set_active(s.load_fallback);
    page.append(&fb_cb);
    let fb_browse = Button::with_label("Browse…");
    fb_browse.set_sensitive(s.load_fallback);
    let fb_row = GtkBox::new(Orientation::Horizontal, 8);
    fb_row.append(&ui.fb_entry);
    fb_row.append(&fb_browse);
    page.append(&fb_row);
    {
        let ui = ui.clone();
        let entry = ui.fb_entry.clone();
        let browse = fb_browse.clone();
        fb_cb.connect_toggled(move |c| {
            let a = c.is_active();
            ui.state.borrow_mut().settings.load_fallback = a;
            let _ = ui.state.borrow().settings.save();
            entry.set_sensitive(a);
            browse.set_sensitive(a);
        });
    }
    {
        let ui = ui.clone();
        let entry = ui.fb_entry.clone();
        entry.connect_changed(move |e| {
            ui.state.borrow_mut().settings.fallback_path = e.text().to_string();
            let _ = ui.state.borrow().settings.save();
        });
    }
    {
        let ui = ui.clone();
        fb_browse.connect_clicked(move |_| ui.spawn_browse(false));
    }

    // Theme
    let theme_row = GtkBox::new(Orientation::Horizontal, 8);
    theme_row.append(&Label::new(Some("Theme:")));
    let theme_dd = DropDown::from_strings(&["Dark", "Light"]);
    theme_dd.set_selected(if s.theme == "Light" { 1 } else { 0 });
    {
        let ui = ui.clone();
        theme_dd.connect_selected_notify(move |d| {
            let t = if d.selected() == 1 { "Light" } else { "Dark" };
            ui.state.borrow_mut().settings.theme = t.to_string();
            let _ = ui.state.borrow().settings.save();
            apply_theme(t);
        });
    }
    theme_row.append(&theme_dd);
    page.append(&theme_row);

    page
}

fn build_globals_page(ui: &Rc<Ui>) -> GtkBox {
    let page = page_box();
    page.append(&heading("Globals"));
    page.append(&Label::new(Some("Global engine config — widget preview; not wired to edit yet.")));
    let chords = ui.state.borrow().status.as_ref().map(|s| s.globals.chords.len()).unwrap_or(0);
    page.append(&Label::new(Some(&format!("Chords: {chords}"))));
    let bar = gtk::ProgressBar::new();
    bar.set_fraction(1.0);
    page.append(&bar);
    page
}

/// The Buttons mockup: Steam-Deck-style groups with a behavior picker + per-input gear.
fn build_buttons_page() -> GtkBox {
    let page = page_box();
    page.append(&heading("Buttons"));

    let group = |title: &str| {
        let g = GtkBox::new(Orientation::Vertical, 6);
        let l = Label::new(None);
        l.set_markup(&format!("<b>{title}</b>"));
        l.set_xalign(0.0);
        g.append(&l);
        g
    };
    let card = |content: &GtkBox| {
        let f = Frame::new(None);
        content.set_margin_top(6);
        content.set_margin_bottom(6);
        content.set_margin_start(8);
        content.set_margin_end(8);
        f.set_child(Some(content));
        f
    };
    let row = |dot: Option<&str>, name: &str, extra: Option<&gtk::Widget>| {
        let r = GtkBox::new(Orientation::Horizontal, 8);
        if let Some(c) = dot {
            let d = Label::new(None);
            d.set_markup(&format!("<span foreground='{c}'>●</span>"));
            r.append(&d);
        }
        r.append(&Label::new(Some(name)));
        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        r.append(&spacer);
        if let Some(w) = extra {
            r.append(w);
        }
        let gear = Button::with_label("⚙");
        gear.set_valign(Align::Center);
        r.append(&gear);
        r
    };

    // Face buttons (with a behavior dropdown)
    let face = group("Face Buttons");
    let behavior = DropDown::from_strings(&["Button Pad", "Lorem Ipsum", "Dolor Sit Amet"]);
    face.append(&card(&row(None, "Behavior", Some(behavior.upcast_ref()))));
    for (dot, name) in [
        ("#4caf50", "A Button"),
        ("#e05252", "B Button"),
        ("#4a7fe0", "X Button"),
        ("#d2b43c", "Y Button"),
    ] {
        face.append(&card(&row(Some(dot), name, None)));
    }
    page.append(&face);

    let bumpers = group("Bumpers");
    for name in ["Left Bumper", "Right Bumper"] {
        bumpers.append(&card(&row(None, name, None)));
    }
    page.append(&bumpers);

    let dpad = group("D-Pad");
    for name in ["Up", "Down", "Left", "Right"] {
        dpad.append(&card(&row(None, name, None)));
    }
    page.append(&dpad);

    page
}

/// Switch GTK's light/dark preference (the only theming a plain gtk4 app does at runtime).
fn apply_theme(theme: &str) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(theme != "Light");
    }
}
