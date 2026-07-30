//! `example_profiles` — build real `ConfigDoc`s (a **game** profile and a **desktop** profile)
//! plus a `GlobalConfig`, and write them to RON. A worked example of the whole config model and
//! the source of the profiles `deckhand-run` drives for manual testing.
//!
//! - `game_profile` mirrors `crates/virt-out/examples/bridge.rs` (Gordon → virtual Xbox pad +
//!   kbd/mouse + gyro-mouse): a mode-shift **layer** (left stick → right stick while the right
//!   pad is clicked), gyro gated by the left full-pull (vertical inverted, as in the bridge).
//! - `desktop_profile` is a keyboard/mouse mapping for the fallback (desktop) role.
//! - `globals` carries the master rumble and a **Steam + RightGrip** fallback-toggle chord.
//!
//! Run `cargo run -p config --example example_profiles [out_dir]` to write `game_profile.ron`,
//! `desktop_profile.ron`, and `globals.ron` (defaults to the temp dir). The test builds each and
//! checks it validates and round-trips.

use std::collections::BTreeMap;

use config::{
    Action, ActionSet, Activation, ActivationMode, Activator, AsMouseSettings, Command,
    CommandSettings, ConfigDoc, DirectionalPadSettings, DpadLayout, GlobalAction, GlobalChord,
    GlobalConfig, GyroToMouseSettings, InputSource, Invert, JoystickSettings, Layer, LayerRef,
    MouseOutput, RumbleSettings, SourceBinding, StickOutput, SwitchMode, TriggerOutput,
    TriggerSettings,
};
use vocab::{GamepadButton, Key, MouseButton};

// --- small builders ---------------------------------------------------------------------

/// A single Regular-press command firing one action.
fn press(action: Action) -> Command {
    Command { activator: Activator::Regular, actions: vec![action], settings: Default::default() }
}

/// A standalone button bound to one Regular action.
fn button(action: Action) -> SourceBinding {
    SourceBinding::Button { commands: vec![press(action)] }
}

fn pad(b: GamepadButton) -> Action {
    Action::GamepadButton(b)
}
fn key(k: Key) -> Action {
    Action::Key(k)
}
fn mouse(b: MouseButton) -> Action {
    Action::MouseButton(b)
}

/// An **interruptible** Regular command firing an ordered key combo (modifiers wrap the key):
/// suppressed when a sibling (e.g. a Long press) fires, so short-tap = combo, hold = the sibling.
fn interruptible(keys: &[Key]) -> Command {
    Command {
        activator: Activator::Regular,
        actions: keys.iter().cloned().map(Action::Key).collect(),
        settings: CommandSettings { interruptible: true, ..Default::default() },
    }
}

/// A Long-press command firing one key.
fn long(k: Key) -> Command {
    Command {
        activator: Activator::Long { hold_ms: 250 },
        actions: vec![Action::Key(k)],
        settings: Default::default(),
    }
}

// --- game profile (the bridge mapping) --------------------------------------------------

/// Build the bridge mapping as a profile (the active/game role).
pub fn game_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // Face buttons (ButtonPad; diamond positions) → gamepad A/B/X/Y, 1:1.
    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            up: vec![press(pad(GamepadButton::Y))],
            down: vec![press(pad(GamepadButton::A))],
            left: vec![press(pad(GamepadButton::X))],
            right: vec![press(pad(GamepadButton::B))],
        },
    );

    // D-Pad (Gordon: left-pad quadrant classifiers) → gamepad dpad.
    base.insert(
        InputSource::DPad,
        SourceBinding::ButtonPad {
            up: vec![press(pad(GamepadButton::DpadUp))],
            down: vec![press(pad(GamepadButton::DpadDown))],
            left: vec![press(pad(GamepadButton::DpadLeft))],
            right: vec![press(pad(GamepadButton::DpadRight))],
        },
    );

    // Bumpers, system buttons, grips → stick clicks, left-stick click → key L.
    base.insert(InputSource::LeftBumper, button(pad(GamepadButton::LeftBumper)));
    base.insert(InputSource::RightBumper, button(pad(GamepadButton::RightBumper)));
    base.insert(InputSource::View, button(pad(GamepadButton::Back)));
    base.insert(InputSource::Menu, button(pad(GamepadButton::Start)));
    base.insert(InputSource::Steam, button(pad(GamepadButton::Guide)));
    base.insert(InputSource::LeftGrip, button(pad(GamepadButton::LeftStick)));
    base.insert(InputSource::RightGrip, button(pad(GamepadButton::RightStick)));
    base.insert(InputSource::LeftStickClick, button(Action::Key(Key::L)));

    // Triggers → gamepad triggers.
    base.insert(
        InputSource::LeftTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings { output: TriggerOutput::Left, ..Default::default() },
            soft_pull: vec![],
        },
    );
    base.insert(
        InputSource::RightTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings { output: TriggerOutput::Right, ..Default::default() },
            soft_pull: vec![],
        },
    );

    // Left stick → left gamepad stick.
    base.insert(
        InputSource::LeftStick,
        SourceBinding::Joystick {
            settings: JoystickSettings { output: StickOutput::Left, ..Default::default() },
            outer_ring: vec![],
        },
    );

    // Right pad → mouse cursor; its click holds the mode-shift layer.
    base.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings { output: MouseOutput::Cursor, ..Default::default() },
        },
    );
    base.insert(InputSource::RightPadClick, button(Action::HoldLayer(LayerRef("aim_stick".into()))));

    // Gyro → mouse (vertical inverted, as in the bridge), gated by the left full-pull.
    base.insert(
        InputSource::Gyro,
        SourceBinding::GyroToMouse {
            settings: GyroToMouseSettings {
                output: MouseOutput::Cursor,
                invert: Invert { x: false, y: true },
                activation: Activation {
                    mode: ActivationMode::HoldToEnable,
                    gaters: vec![InputSource::LeftFullPull],
                },
                ..Default::default()
            },
        },
    );

    // Mode-shift layer: while the right pad is clicked, the left stick drives the RIGHT stick.
    let aim_stick = Layer {
        name: "aim_stick".into(),
        bindings: BTreeMap::from([(
            InputSource::LeftStick,
            SourceBinding::Joystick {
                settings: JoystickSettings { output: StickOutput::Right, ..Default::default() },
                outer_ring: vec![],
            },
        )]),
    };

    ConfigDoc {
        version: 0,
        name: "Game".into(),
        action_sets: vec![ActionSet { name: "Game".into(), bindings: base, layers: vec![aim_stick] }],
        // 60 Hz feel; the global master % scales it (see `globals`).
        rumble: RumbleSettings { hz: 60, strength: 100, ..Default::default() },
    }
}

// --- desktop profile (the fallback role) ------------------------------------------------

/// A keyboard/mouse desktop mapping — the fallback role you drop to for navigating the desktop.
pub fn desktop_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // Pads → mouse: right = cursor, left = scroll wheel.
    base.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings { output: MouseOutput::Cursor, ..Default::default() },
        },
    );
    base.insert(
        InputSource::LeftPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings { output: MouseOutput::Scroll, ..Default::default() },
        },
    );

    // Face diamond → navigation keys (up = Y, down = A, left = X, right = B).
    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            up: vec![press(key(Key::PageUp))],     // Y
            down: vec![press(key(Key::Enter))],    // A
            left: vec![press(key(Key::PageDown))], // X
            right: vec![press(key(Key::Esc))],     // B
        },
    );

    // Left stick → 4-way directional pad → arrow keys.
    base.insert(
        InputSource::LeftStick,
        SourceBinding::DirectionalPad {
            settings: DirectionalPadSettings { layout: DpadLayout::FourWay, ..Default::default() },
            up: vec![press(key(Key::Up))],
            down: vec![press(key(Key::Down))],
            left: vec![press(key(Key::Left))],
            right: vec![press(key(Key::Right))],
            outer_ring: vec![],
        },
    );

    // Trigger soft-pulls → mouse buttons (right → left click, left → right click).
    base.insert(
        InputSource::RightTrigger,
        SourceBinding::Trigger {
            settings: Default::default(),
            soft_pull: vec![press(mouse(MouseButton::Left))],
        },
    );
    base.insert(
        InputSource::LeftTrigger,
        SourceBinding::Trigger {
            settings: Default::default(),
            soft_pull: vec![press(mouse(MouseButton::Right))],
        },
    );

    // Buttons.
    base.insert(InputSource::LeftPadClick, button(mouse(MouseButton::Middle)));
    base.insert(InputSource::LeftBumper, button(key(Key::Backspace)));
    base.insert(InputSource::RightBumper, button(key(Key::Space)));
    base.insert(InputSource::LeftStickClick, button(key(Key::LeftMeta)));
    base.insert(InputSource::View, button(key(Key::LeftAlt)));
    base.insert(InputSource::Menu, button(key(Key::Tab)));

    // Grips: an interruptible Regular combo (short tap) plus a Long modifier (hold).
    base.insert(
        InputSource::LeftGrip,
        SourceBinding::Button {
            commands: vec![interruptible(&[Key::LeftCtrl, Key::C]), long(Key::LeftShift)],
        },
    );
    base.insert(
        InputSource::RightGrip,
        SourceBinding::Button {
            commands: vec![interruptible(&[Key::LeftCtrl, Key::V]), long(Key::LeftCtrl)],
        },
    );

    ConfigDoc {
        version: 0,
        name: "Desktop".into(),
        action_sets: vec![ActionSet { name: "Desktop".into(), bindings: base, layers: vec![] }],
        rumble: RumbleSettings::default(),
    }
}

// --- globals ----------------------------------------------------------------------------

/// The above-profile globals: 50% master rumble and a Steam + RightGrip fallback toggle.
pub fn globals() -> GlobalConfig {
    GlobalConfig {
        master_rumble: 50,
        chords: vec![GlobalChord {
            buttons: vec![InputSource::Steam, InputSource::RightGrip],
            action: GlobalAction::SwitchFallback { mode: SwitchMode::Toggle },
        }],
        ..Default::default()
    }
}

fn main() -> std::io::Result<()> {
    let dir =
        std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let pretty = ron::ser::PrettyConfig::default();

    for (name, doc_ron) in [
        ("game_profile.ron", ron::ser::to_string_pretty(&game_profile(), pretty.clone()).unwrap()),
        ("desktop_profile.ron", ron::ser::to_string_pretty(&desktop_profile(), pretty.clone()).unwrap()),
        ("globals.ron", ron::ser::to_string_pretty(&globals(), pretty.clone()).unwrap()),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, doc_ron)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_valid_and_round_trips(doc: &ConfigDoc) {
        let diags = doc.validate();
        assert!(
            diags.iter().all(|d| d.severity != config::Severity::Error),
            "unexpected errors: {diags:?}"
        );
        let doc_ron = ron::ser::to_string_pretty(doc, ron::ser::PrettyConfig::default()).unwrap();
        let back: ConfigDoc = ron::from_str(&doc_ron).unwrap();
        assert_eq!(doc, &back);
    }

    #[test]
    fn profiles_are_valid_and_round_trip() {
        assert_valid_and_round_trips(&game_profile());
        assert_valid_and_round_trips(&desktop_profile());

        let g = globals();
        assert!(g.validate().iter().all(|d| d.severity != config::Severity::Error));
        assert_eq!(ron::from_str::<GlobalConfig>(&ron::to_string(&g).unwrap()).unwrap(), g);
    }
}
