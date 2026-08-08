//! The cxx-qt bridge: a `Bridge` QObject exposing the app state to QML as properties + invokables.
//!
//! The daemon wiring mirrors the iced/egui builds but marshaled the Qt way: blocking work runs on
//! std threads, and results are pushed back onto the Qt thread with `CxxQtThread::queue` (the only
//! safe way to touch a QObject off-thread). Properties are read by QML (bindings); QML calls
//! invokables to request changes. Toolkit-independent bits come from `ui-test-common`.

use cxx_qt_lib::{QString, QStringList};
use ipc::{Event, ProfileRole, RunState};
use ui_test_common::{AppSettings, Client, DaemonUpdate, INPUT_PRESETS, OUTPUT_PRESETS, run_event_loop};

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    // Expose Rust snake_case names to QML/C++ as camelCase (`start_monitor` → `startMonitor`,
    // `state_text` → `stateText`, …), matching idiomatic QML.
    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, connected)]
        #[qproperty(QString, input)]
        #[qproperty(QString, output)]
        #[qproperty(QString, state_text)]
        #[qproperty(QString, bound_text)]
        #[qproperty(QString, main_text)]
        #[qproperty(QString, fallback_text)]
        #[qproperty(i32, chord_count)]
        #[qproperty(QStringList, inputs)]
        #[qproperty(QStringList, outputs)]
        #[qproperty(QString, error_text)]
        #[qproperty(bool, start_daemon)]
        #[qproperty(bool, load_main)]
        #[qproperty(QString, main_path)]
        #[qproperty(bool, load_fallback)]
        #[qproperty(QString, fallback_path)]
        #[qproperty(QString, theme)]
        type Bridge = super::BridgeRust;

        /// Start the persistent event monitor (call once from QML `Component.onCompleted`).
        #[qinvokable]
        fn start_monitor(self: Pin<&mut Bridge>);
        /// Seed a fresh status + device list (connect / manual refresh).
        #[qinvokable]
        fn refresh(self: Pin<&mut Bridge>);
        #[qinvokable]
        fn start(self: Pin<&mut Bridge>);
        #[qinvokable]
        fn stop(self: Pin<&mut Bridge>);
        #[qinvokable]
        fn choose_input(self: Pin<&mut Bridge>, spec: QString);
        #[qinvokable]
        fn choose_output(self: Pin<&mut Bridge>, spec: QString);
        #[qinvokable]
        fn toggle_start_daemon(self: Pin<&mut Bridge>, v: bool);
        #[qinvokable]
        fn toggle_load_main(self: Pin<&mut Bridge>, v: bool);
        #[qinvokable]
        fn update_main_path(self: Pin<&mut Bridge>, p: QString);
        #[qinvokable]
        fn toggle_load_fallback(self: Pin<&mut Bridge>, v: bool);
        #[qinvokable]
        fn update_fallback_path(self: Pin<&mut Bridge>, p: QString);
        #[qinvokable]
        fn update_theme(self: Pin<&mut Bridge>, t: QString);
    }

    impl cxx_qt::Threading for Bridge {}
}

use core::pin::Pin;
use cxx_qt::Threading;
use qobject::Bridge;

/// The Rust side of the `Bridge` QObject. Fields map 1:1 to the `#[qproperty(...)]` declarations;
/// `Default` seeds the settings-mirroring properties from disk.
pub struct BridgeRust {
    connected: bool,
    input: QString,
    output: QString,
    state_text: QString,
    bound_text: QString,
    main_text: QString,
    fallback_text: QString,
    chord_count: i32,
    inputs: QStringList,
    outputs: QStringList,
    error_text: QString,
    start_daemon: bool,
    load_main: bool,
    main_path: QString,
    load_fallback: bool,
    fallback_path: QString,
    theme: QString,
}

impl Default for BridgeRust {
    fn default() -> Self {
        let s = AppSettings::load();
        BridgeRust {
            connected: false,
            input: QString::default(),
            output: QString::default(),
            state_text: QString::from("—"),
            bound_text: QString::from("—"),
            main_text: QString::from("—"),
            fallback_text: QString::from("—"),
            chord_count: 0,
            inputs: INPUT_PRESETS.iter().map(|s| QString::from(*s)).collect(),
            outputs: OUTPUT_PRESETS.iter().map(|s| QString::from(*s)).collect(),
            error_text: QString::default(),
            start_daemon: s.start_daemon,
            load_main: s.load_main,
            main_path: QString::from(s.main_path.as_str()),
            load_fallback: s.load_fallback,
            fallback_path: QString::from(s.fallback_path.as_str()),
            theme: QString::from(s.theme.as_str()),
        }
    }
}

impl qobject::Bridge {
    fn start_monitor(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            run_event_loop(None, move |u| qt.queue(move |b| apply_update(b, u)).is_ok());
        });
    }

    fn refresh(mut self: Pin<&mut Self>) {
        self.as_mut().fetch_status();
        self.fetch_devices();
    }

    fn start(self: Pin<&mut Self>) {
        self.run_cmd(|c| c.start());
    }

    fn stop(self: Pin<&mut Self>) {
        self.run_cmd(|c| c.stop());
    }

    fn choose_input(self: Pin<&mut Self>, spec: QString) {
        let s = spec.to_string();
        self.run_cmd(move |c| c.set_input(s));
    }

    fn choose_output(self: Pin<&mut Self>, spec: QString) {
        let s = spec.to_string();
        self.run_cmd(move |c| c.set_output(s));
    }

    fn toggle_start_daemon(mut self: Pin<&mut Self>, v: bool) {
        self.as_mut().set_start_daemon(v);
        persist(&self);
    }

    fn toggle_load_main(mut self: Pin<&mut Self>, v: bool) {
        self.as_mut().set_load_main(v);
        persist(&self);
    }

    fn update_main_path(mut self: Pin<&mut Self>, p: QString) {
        self.as_mut().set_main_path(p);
        persist(&self);
    }

    fn toggle_load_fallback(mut self: Pin<&mut Self>, v: bool) {
        self.as_mut().set_load_fallback(v);
        persist(&self);
    }

    fn update_fallback_path(mut self: Pin<&mut Self>, p: QString) {
        self.as_mut().set_fallback_path(p);
        persist(&self);
    }

    fn update_theme(mut self: Pin<&mut Self>, t: QString) {
        self.as_mut().set_theme(t);
        persist(&self);
    }

    // --- off-thread daemon work -----------------------------------------------------------

    /// Fetch a status snapshot on a worker thread; apply it on the Qt thread.
    fn fetch_status(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let r = Client::new(None).status();
            let _ = qt.queue(move |mut b| match r {
                Ok(s) => {
                    b.as_mut().set_connected(true);
                    b.as_mut().set_error_text(QString::default());
                    b.as_mut().set_input(QString::from(s.input.as_str()));
                    b.as_mut().set_output(QString::from(s.output.as_str()));
                    b.as_mut().set_state_text(QString::from(format!("{:?}", s.state).as_str()));
                    b.as_mut().set_bound_text(QString::from(s.bound.as_deref().unwrap_or("—")));
                    b.as_mut().set_main_text(QString::from(s.main.as_deref().unwrap_or("—")));
                    b.as_mut().set_fallback_text(QString::from(s.fallback.as_deref().unwrap_or("—")));
                    b.as_mut().set_chord_count(s.globals.chords.len() as i32);
                }
                Err(e) => b.as_mut().set_error_text(QString::from(e.to_string().as_str())),
            });
        });
    }

    /// Re-enumerate devices on a worker thread; the presets + device ids become the input model.
    fn fetch_devices(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let r = Client::new(None).list_devices();
            let _ = qt.queue(move |mut b| match r {
                Ok(devs) => {
                    let list: QStringList = INPUT_PRESETS
                        .iter()
                        .map(|s| QString::from(*s))
                        .chain(devs.iter().map(|d| QString::from(d.as_str())))
                        .collect();
                    b.as_mut().set_inputs(list);
                }
                Err(e) => b.as_mut().set_error_text(QString::from(e.to_string().as_str())),
            });
        });
    }

    /// Run a daemon command on a worker thread; the command's effect returns as an event, so on
    /// success we just clear the error, on failure we surface it.
    fn run_cmd(self: Pin<&mut Self>, op: impl FnOnce(&mut Client) -> std::io::Result<()> + Send + 'static) {
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let r = op(&mut Client::new(None));
            let _ = qt.queue(move |mut b| match r {
                Ok(()) => b.as_mut().set_error_text(QString::default()),
                Err(e) => b.as_mut().set_error_text(QString::from(e.to_string().as_str())),
            });
        });
    }
}

/// Apply one monitor update on the Qt thread. On connect, seed once; afterwards events carry deltas.
fn apply_update(mut b: Pin<&mut Bridge>, u: DaemonUpdate) {
    match u {
        DaemonUpdate::Connected => {
            b.as_mut().set_connected(true);
            b.as_mut().set_error_text(QString::default());
            b.refresh();
        }
        DaemonUpdate::Disconnected => {
            b.as_mut().set_connected(false);
            b.as_mut().set_state_text(QString::from("—"));
            b.as_mut().set_bound_text(QString::from("—"));
        }
        DaemonUpdate::Event(ev) => apply_event(b, ev),
    }
}

/// Apply one absolute-valued daemon event to the properties in place (no refetch).
fn apply_event(mut b: Pin<&mut Bridge>, ev: Event) {
    match ev {
        Event::State(s) => b.as_mut().set_state_text(QString::from(format!("{s:?}").as_str())),
        Event::BindingLost => {
            b.as_mut().set_state_text(QString::from(format!("{:?}", RunState::WaitingForDevice).as_str()))
        }
        Event::BindingAcquired(id) => b.as_mut().set_bound_text(QString::from(id.as_str())),
        Event::InputStaged(i) => b.as_mut().set_input(QString::from(i.as_str())),
        Event::OutputStaged(o) => b.as_mut().set_output(QString::from(o.as_str())),
        Event::ProfileSet { role, name } => {
            let text = QString::from(name.as_deref().unwrap_or("—"));
            match role {
                ProfileRole::Main => b.as_mut().set_main_text(text),
                ProfileRole::Fallback => b.as_mut().set_fallback_text(text),
            }
        }
        Event::GlobalConfigSet(g) => b.as_mut().set_chord_count(g.chords.len() as i32),
        _ => {}
    }
}

/// Persist the settings-mirroring properties to the UI's RON file.
fn persist(b: &Bridge) {
    let s = AppSettings {
        start_daemon: *b.start_daemon(),
        load_main: *b.load_main(),
        main_path: b.main_path().to_string(),
        load_fallback: *b.load_fallback(),
        fallback_path: b.fallback_path().to_string(),
        theme: b.theme().to_string(),
    };
    let _ = s.save();
}
