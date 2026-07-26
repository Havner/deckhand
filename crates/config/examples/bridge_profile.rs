//! `bridge_profile` — build the `virt-out` bridge mapping as a real `ConfigDoc` and write
//! it to RON, as a worked example of the whole config model.
//!
//! Mirrors `crates/virt-out/examples/bridge.rs` (Gordon → virtual Xbox pad + kbd/mouse +
//! gyro-mouse), expressed in the config vocabulary: an action set of per-source bindings,
//! a mode-shift **layer** (left stick → right stick while the right pad is clicked), the
//! gyro gated by the left full-pull, and a `GlobalConfig` with a fallback-switch chord.
//!
//! Run `cargo run -p config --example bridge_profile [out_dir]` to write
//! `bridge_profile.ron` + `bridge_global.ron` (defaults to the temp dir). The test writes
//! to a temp file and checks it validates and round-trips.

use std::collections::BTreeMap;

use config::{
    Action, ActionSet, Activation, ActivationMode, AsMouseSettings, Command, Activator, ConfigDoc,
    GlobalAction, GlobalChord, GlobalConfig, GyroToMouseSettings, InputSource, Invert,
    JoystickSettings, Layer, LayerRef, MouseOutput, RumbleSettings, SourceBinding, StickOutput,
    SwitchMode, TriggerOutput, TriggerSettings,
};
use vocab::{GamepadButton, Key};

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

/// Build the bridge mapping as a profile.
pub fn bridge_profile() -> ConfigDoc {
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
        SourceBinding::AsMouse { settings: AsMouseSettings { output: MouseOutput::Cursor, ..Default::default() } },
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
        name: "Bridge".into(),
        action_sets: vec![ActionSet { name: "Game".into(), bindings: base, layers: vec![aim_stick] }],
        // 60 Hz feel; the global master % scales it (see bridge_global).
        rumble: RumbleSettings { hz: 60, strength: 100, ..Default::default() },
    }
}

/// The above-profile globals: 50% master rumble and a Steam+LeftGrip fallback toggle.
pub fn bridge_global() -> GlobalConfig {
    GlobalConfig {
        master_rumble: 50,
        chords: vec![GlobalChord {
            buttons: vec![InputSource::Steam, InputSource::LeftGrip],
            action: GlobalAction::SwitchFallback { mode: SwitchMode::Toggle },
        }],
        ..Default::default()
    }
}

fn main() -> std::io::Result<()> {
    let dir = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);

    let doc = bridge_profile();
    let global = bridge_global();
    assert!(doc.validate().iter().all(|d| d.severity != config::Severity::Error), "profile has errors");

    let pretty = ron::ser::PrettyConfig::default();
    let profile_path = dir.join("bridge_profile.ron");
    let global_path = dir.join("bridge_global.ron");
    std::fs::write(&profile_path, ron::ser::to_string_pretty(&doc, pretty.clone()).unwrap())?;
    std::fs::write(&global_path, ron::ser::to_string_pretty(&global, pretty).unwrap())?;

    println!("wrote {}", profile_path.display());
    println!("wrote {}", global_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_valid_and_round_trips_via_a_file() {
        let doc = bridge_profile();
        // No validation errors (dangling refs, kind mismatches, bad gaters…).
        let diags = doc.validate();
        assert!(
            diags.iter().all(|d| d.severity != config::Severity::Error),
            "unexpected errors: {diags:?}"
        );

        // Serialize to a temp file, read it back, and check it round-trips.
        let path =
            std::env::temp_dir().join(format!("deckhand-bridge-{}.ron", std::process::id()));
        let ron = ron::ser::to_string_pretty(&doc, ron::ser::PrettyConfig::default()).unwrap();
        std::fs::write(&path, &ron).unwrap();

        let back: ConfigDoc = ron::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc, back);
        let _ = std::fs::remove_file(&path);

        // Globals too.
        let g = bridge_global();
        assert!(g.validate().iter().all(|d| d.severity != config::Severity::Error));
        let g_ron = ron::to_string(&g).unwrap();
        assert_eq!(ron::from_str::<GlobalConfig>(&g_ron).unwrap(), g);
    }
}
