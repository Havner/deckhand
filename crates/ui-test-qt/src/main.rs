//! `ui-test-qt` — the cxx-qt (Qt6/QML) implementation of the deckhand UI-toolkit bake-off.
//!
//! Unlike the Rust-native builds, the UI is **QML** (`qml/main.qml`) backed by a Rust `Bridge`
//! QObject (`bridge.rs`); this file just boots `QGuiApplication` + `QQmlApplicationEngine` and loads
//! the QML module the `build.rs` registered. Needs a Qt6 install at build time (see PLAN §5.2).

mod bridge;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    // Use the Material style so the light/dark theme can be switched from QML at runtime.
    // SAFETY: set before Qt / any threads start.
    unsafe { std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Material") };

    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    // Ensure the QML module compiled by build.rs is initialized before we load its main.qml.
    cxx_qt::init_qml_module!("com.deckhand.ui");

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/com/deckhand/ui/qml/main.qml"));
    }
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
