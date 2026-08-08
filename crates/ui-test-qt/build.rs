//! cxx-qt build: compile the Rust bridge (→ C++/moc) and register the QML module + `main.qml`.
//! Needs a Qt6 install findable by `qmake` (via the `QMAKE` env var or `qmake` in PATH).

use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("com.deckhand.ui").qml_files(["qml/main.qml"]),
    )
    .file("src/bridge.rs")
    .qt_module("Gui")
    .build();
}
