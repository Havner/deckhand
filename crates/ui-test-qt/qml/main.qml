// The Qt/QML implementation of the shared bake-off layout (PLAN §5.1). Backed by the Rust `Bridge`
// QObject (see src/bridge.rs): properties are read via bindings, invokables requested on interaction.
import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts
import QtQuick.Dialogs
import com.deckhand.ui

ApplicationWindow {
    id: win
    visible: true
    width: 960
    height: 640
    title: "deckhand — UI toolkit test (Qt/QML)"

    Material.theme: bridge.theme === "Light" ? Material.Light : Material.Dark
    Material.accent: Material.Blue

    // Sidebar selection is pure QML state (nav is not part of the daemon bridge).
    property string category: "Settings"

    Bridge {
        id: bridge
        Component.onCompleted: bridge.startMonitor()
    }

    FileDialog {
        id: mainDialog
        nameFilters: ["RON profile (*.ron)"]
        onAccepted: bridge.updateMainPath(stripFile(selectedFile))
    }
    FileDialog {
        id: fallbackDialog
        nameFilters: ["RON profile (*.ron)"]
        onAccepted: bridge.updateFallbackPath(stripFile(selectedFile))
    }
    function stripFile(url) {
        var s = url.toString();
        return s.startsWith("file://") ? s.substring(7) : s;
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // --- top daemon bar ---
        Pane {
            Layout.fillWidth: true
            Material.elevation: 2
            RowLayout {
                anchors.fill: parent
                spacing: 8
                Button {
                    text: "Start"
                    enabled: bridge.stateText === "Idle"
                    Material.background: Material.Green
                    onClicked: bridge.start()
                }
                Button {
                    text: "Stop"
                    enabled: bridge.stateText === "Running" || bridge.stateText === "WaitingForDevice"
                    Material.background: Material.Red
                    onClicked: bridge.stop()
                }
                Button {
                    text: "Connect"
                    enabled: !bridge.connected
                    onClicked: bridge.refresh()
                }
                Item { Layout.fillWidth: true }
                ToolButton { text: "⟳"; onClicked: bridge.refresh() }
                Label { text: "Input:" }
                ComboBox {
                    id: inputCombo
                    Layout.preferredWidth: 170
                    model: bridge.inputs
                    currentIndex: Math.max(0, bridge.inputs.indexOf(bridge.input))
                    onActivated: bridge.chooseInput(currentText)
                }
                Label { text: "Output:" }
                ComboBox {
                    Layout.preferredWidth: 120
                    model: bridge.outputs
                    currentIndex: Math.max(0, bridge.outputs.indexOf(bridge.output))
                    onActivated: bridge.chooseOutput(currentText)
                }
            }
        }

        // --- middle: sidebar + content ---
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            // sidebar
            Pane {
                Layout.preferredWidth: 180
                Layout.fillHeight: true
                padding: 6
                ColumnLayout {
                    anchors.fill: parent
                    spacing: 4
                    Repeater {
                        model: ["Buttons", "Triggers", "Joysticks", "Trackpads", "Gyro"]
                        ItemDelegate {
                            Layout.fillWidth: true
                            text: modelData
                            highlighted: win.category === modelData
                            onClicked: win.category = modelData
                        }
                    }
                    Item { Layout.fillHeight: true }
                    Repeater {
                        model: ["Settings", "Globals"]
                        ItemDelegate {
                            Layout.fillWidth: true
                            text: modelData
                            highlighted: win.category === modelData
                            onClicked: win.category = modelData
                        }
                    }
                }
            }

            // scrollable content
            ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                contentWidth: availableWidth
                Loader {
                    width: parent.width
                    sourceComponent: win.category === "Settings" ? settingsView
                        : win.category === "Globals" ? globalsView
                        : win.category === "Buttons" ? buttonsView
                        : stubView
                }
            }
        }

        // --- bottom status bar ---
        Pane {
            Layout.fillWidth: true
            Material.elevation: 2
            RowLayout {
                anchors.fill: parent
                spacing: 12
                Label {
                    text: bridge.connected ? "● connected" : "● disconnected"
                    color: bridge.connected ? "#4caf50" : "#e05252"
                }
                Label { text: "state: " + bridge.stateText }
                Label { text: "device: " + bridge.boundText }
                Label { text: "main: " + bridge.mainText }
                Label { text: "fallback: " + bridge.fallbackText }
                Label { text: "chords: " + bridge.chordCount }
                Item { Layout.fillWidth: true }
                Label {
                    text: bridge.errorText.length > 0 ? "⚠ " + bridge.errorText : ""
                    color: "#e05252"
                }
            }
        }
    }

    // --- content screens ---

    Component {
        id: settingsView
        ColumnLayout {
            width: parent ? parent.width : 0
            spacing: 10
            Label { text: "Settings"; font.pixelSize: 22 }
            Label { text: "Application settings — stored separately from the daemon." }
            CheckBox {
                text: "Start the daemon if not running on start"
                checked: bridge.startDaemon
                onToggled: bridge.toggleStartDaemon(checked)
            }
            CheckBox {
                text: "Load the main profile on start"
                checked: bridge.loadMain
                onToggled: bridge.toggleLoadMain(checked)
            }
            RowLayout {
                Layout.fillWidth: true
                TextField {
                    Layout.fillWidth: true
                    placeholderText: "path to .ron file"
                    text: bridge.mainPath
                    enabled: bridge.loadMain
                    onEditingFinished: bridge.updateMainPath(text)
                }
                Button { text: "Browse…"; enabled: bridge.loadMain; onClicked: mainDialog.open() }
            }
            CheckBox {
                text: "Load the fallback profile on start"
                checked: bridge.loadFallback
                onToggled: bridge.toggleLoadFallback(checked)
            }
            RowLayout {
                Layout.fillWidth: true
                TextField {
                    Layout.fillWidth: true
                    placeholderText: "path to .ron file"
                    text: bridge.fallbackPath
                    enabled: bridge.loadFallback
                    onEditingFinished: bridge.updateFallbackPath(text)
                }
                Button { text: "Browse…"; enabled: bridge.loadFallback; onClicked: fallbackDialog.open() }
            }
            RowLayout {
                Label { text: "Theme:" }
                ComboBox {
                    model: ["Dark", "Light"]
                    currentIndex: bridge.theme === "Light" ? 1 : 0
                    onActivated: bridge.updateTheme(currentText)
                }
            }
        }
    }

    Component {
        id: globalsView
        ColumnLayout {
            width: parent ? parent.width : 0
            spacing: 10
            Label { text: "Globals"; font.pixelSize: 22 }
            Label { text: "Global engine config — widget preview; not wired to edit yet." }
            Label { text: "Chords: " + bridge.chordCount }
            ProgressBar { from: 0; to: 100; value: 100 }
        }
    }

    Component {
        id: stubView
        ColumnLayout {
            width: parent ? parent.width : 0
            spacing: 10
            Label { text: win.category; font.pixelSize: 22 }
            Label { text: "Profile-edit screen — stubbed for the toolkit test." }
        }
    }

    // The Buttons mockup: Steam-Deck-style groups with a behavior picker + per-input gear.
    Component {
        id: buttonsView
        ColumnLayout {
            width: parent ? parent.width : 0
            spacing: 16
            Label { text: "Buttons"; font.pixelSize: 22 }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                Label { text: "Face Buttons"; font.pixelSize: 16; font.bold: true }
                Frame {
                    Layout.fillWidth: true
                    RowLayout {
                        anchors.fill: parent
                        Label { text: "Behavior" }
                        Item { Layout.fillWidth: true }
                        ComboBox { model: ["Button Pad", "Lorem Ipsum", "Dolor Sit Amet"] }
                        ToolButton { text: "⚙" }
                    }
                }
                Repeater {
                    model: [
                        { name: "A Button", dot: "#4caf50" },
                        { name: "B Button", dot: "#e05252" },
                        { name: "X Button", dot: "#4a7fe0" },
                        { name: "Y Button", dot: "#d2b43c" }
                    ]
                    Frame {
                        Layout.fillWidth: true
                        RowLayout {
                            anchors.fill: parent
                            Label { text: "●"; color: modelData.dot }
                            Label { text: modelData.name }
                            Item { Layout.fillWidth: true }
                            ToolButton { text: "⚙" }
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                Label { text: "Bumpers"; font.pixelSize: 16; font.bold: true }
                Repeater {
                    model: ["Left Bumper", "Right Bumper"]
                    Frame {
                        Layout.fillWidth: true
                        RowLayout {
                            anchors.fill: parent
                            Label { text: modelData }
                            Item { Layout.fillWidth: true }
                            ToolButton { text: "⚙" }
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                Label { text: "D-Pad"; font.pixelSize: 16; font.bold: true }
                Repeater {
                    model: ["Up", "Down", "Left", "Right"]
                    Frame {
                        Layout.fillWidth: true
                        RowLayout {
                            anchors.fill: parent
                            Label { text: modelData }
                            Item { Layout.fillWidth: true }
                            ToolButton { text: "⚙" }
                        }
                    }
                }
            }
        }
    }
}
