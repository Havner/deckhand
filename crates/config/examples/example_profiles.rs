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
//!
//! Deliberately **verbose**: every binding is spelled out in full so any single one can be
//! retuned without touching a shared builder. Only the leaf action constructors (`pad`/`key`/
//! `mouse`) are kept — they embed no settings, so editing one binding never affects another.

use std::collections::BTreeMap;

use config::{
    Acceleration, Action, ActionSet, Activation, ActivationMode, Activator, AsMouseSettings,
    Command, CommandSettings, ConfigDoc, Curve, Deadzone, DirectionalPadSettings, DpadLayout,
    GlobalAction, GlobalChord, GlobalConfig, GyroToMouseSettings, HapticEdge, HapticStrength,
    Haptics, InputSource, Invert, JoystickMouseSettings, JoystickSettings, Layer, LayerRef,
    MouseOutput, OneEuroFilter, RumbleSettings, Sensitivity, SourceBinding, StartProfile,
    StickOutput, SwitchMode, TriggerOutput, TriggerSettings,
};
use vocab::{GamepadButton, Key, MouseButton};

// --- leaf action constructors (no embedded settings) ------------------------------------

fn pad(b: GamepadButton) -> Action {
    Action::GamepadButton(b)
}
fn key(k: Key) -> Action {
    Action::Key(k)
}
fn mouse(b: MouseButton) -> Action {
    Action::MouseButton(b)
}

// --- game profile (the bridge mapping) --------------------------------------------------

/// Build the bridge mapping as a profile (the active/game role).
pub fn game_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // Face buttons (ButtonPad; diamond positions) → gamepad A/B/X/Y, 1:1.
    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::Y)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::A)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::X)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::B)],
                settings: Default::default(),
            }],
        },
    );

    // D-Pad (Gordon: left-pad quadrant classifiers) → gamepad dpad.
    base.insert(
        InputSource::DPad,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::DpadUp)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::DpadDown)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::DpadLeft)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::DpadRight)],
                settings: Default::default(),
            }],
        },
    );

    // Left bumper → gamepad left bumper.
    base.insert(
        InputSource::LeftBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::LeftBumper)],
                settings: Default::default(),
            }],
        },
    );
    // Right bumper → gamepad right bumper.
    base.insert(
        InputSource::RightBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::RightBumper)],
                settings: Default::default(),
            }],
        },
    );
    // View (left small top button) → gamepad Back.
    base.insert(
        InputSource::View,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::Back)],
                settings: Default::default(),
            }],
        },
    );
    // Menu (right small top button) → gamepad Start.
    base.insert(
        InputSource::Menu,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::Start)],
                settings: Default::default(),
            }],
        },
    );
    // Steam button → gamepad Guide.
    base.insert(
        InputSource::Steam,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::Guide)],
                settings: Default::default(),
            }],
        },
    );
    // Left grip → left stick click.
    base.insert(
        InputSource::LeftGrip,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::LeftStick)],
                settings: Default::default(),
            }],
        },
    );
    // Right grip → right stick click.
    base.insert(
        InputSource::RightGrip,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![pad(GamepadButton::RightStick)],
                settings: Default::default(),
            }],
        },
    );
    // Left-stick click → key L.
    base.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::L)],
                settings: Default::default(),
            }],
        },
    );

    // Left trigger → gamepad left trigger axis (no soft-pull button).
    base.insert(
        InputSource::LeftTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::Left,
                ..Default::default()
            },
            soft_pull: vec![],
        },
    );
    // Right trigger → gamepad right trigger axis (no soft-pull button).
    base.insert(
        InputSource::RightTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::Right,
                ..Default::default()
            },
            soft_pull: vec![],
        },
    );

    // Left stick → left gamepad stick.
    base.insert(
        InputSource::LeftStick,
        SourceBinding::Joystick {
            settings: JoystickSettings {
                output: StickOutput::Left,
                ..Default::default()
            },
            outer_ring: vec![],
        },
    );

    // Right pad → mouse cursor. Explicit sensitivity/acceleration/smoothing knobs.
    base.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.0, y: 1.0 },
                acceleration: Acceleration { factor: 0.05 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                ..Default::default()
            },
        },
    );
    // Right-pad click holds the mode-shift layer (left stick → right stick).
    base.insert(
        InputSource::RightPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![Action::HoldLayer(LayerRef("aim_stick".into()))],
                settings: Default::default(),
            }],
        },
    );

    // Gyro → mouse (vertical inverted, as in the bridge), gated by the left full-pull. Explicit
    // sensitivity/acceleration/smoothing/deadzone knobs.
    base.insert(
        InputSource::Gyro,
        SourceBinding::GyroToMouse {
            settings: GyroToMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 0.5, y: 0.5 },
                acceleration: Acceleration { factor: 0.02 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 1.0,
                    beta: 0.5,
                }),
                deadzone: Deadzone { inner: 0.1 },
                invert: Invert { x: false, y: true },
                activation: Activation {
                    mode: ActivationMode::HoldToEnable,
                    gaters: vec![InputSource::LeftFullPull],
                },
                ..Default::default()
            },
        },
    );

    // Mode-shift layer: while the right pad is clicked, the left stick drives the RIGHT stick,
    // and the right pad itself is nullified (so holding it for the mode-shift doesn't jitter the
    // mouse). `None` overrides the base AsMouse binding for the duration of the layer.
    let aim_stick = Layer {
        name: "aim_stick".into(),
        bindings: BTreeMap::from([
            (
                InputSource::LeftStick,
                SourceBinding::Joystick {
                    settings: JoystickSettings {
                        output: StickOutput::Right,
                        ..Default::default()
                    },
                    outer_ring: vec![],
                },
            ),
            (InputSource::RightPad, SourceBinding::None),
        ]),
    };

    ConfigDoc {
        version: 0,
        name: "Game".into(),
        action_sets: vec![ActionSet {
            name: "Game".into(),
            bindings: base,
            layers: vec![aim_stick],
        }],
        // 60 Hz feel; the global master % scales it (see `globals`).
        rumble: RumbleSettings {
            hz: 60,
            strength: 100,
            ..Default::default()
        },
    }
}

// --- desktop profile (the fallback role) ------------------------------------------------

/// A keyboard/mouse desktop mapping — the fallback role you drop to for navigating the desktop.
pub fn desktop_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // Right pad → mouse cursor. Explicit sensitivity/acceleration/smoothing knobs.
    base.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.0, y: 1.0 },
                acceleration: Acceleration { factor: 0.02 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                ..Default::default()
            },
        },
    );
    // Left pad → smooth scroll wheel. Explicit sensitivity/acceleration/smoothing knobs.
    base.insert(
        InputSource::LeftPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::SmoothScroll,
                sensitivity: Sensitivity { x: 1.0, y: 1.0 },
                acceleration: Acceleration { factor: 0.05 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                ..Default::default()
            },
        },
    );
    // Right-pad click holds the stick_mouse layer (left stick → mouse instead of arrows).
    base.insert(
        InputSource::RightPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![Action::HoldLayer(LayerRef("stick_mouse".into()))],
                settings: Default::default(),
            }],
        },
    );

    // Face diamond → navigation keys (up = Y, down = A, left = X, right = B).
    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::PageUp)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Enter)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::PageDown)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Esc)],
                settings: Default::default(),
            }],
        },
    );

    // Left stick → 4-way directional pad → arrow keys.
    base.insert(
        InputSource::LeftStick,
        SourceBinding::DirectionalPad {
            settings: DirectionalPadSettings {
                layout: DpadLayout::FourWay,
                ..Default::default()
            },
            up: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Up)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Down)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Left)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Right)],
                settings: Default::default(),
            }],
            outer_ring: vec![],
        },
    );

    // Right trigger soft-pull → left mouse click (haptic tick on press and release). Output `None`
    // so the trigger drives no gamepad axis on the desktop — just the soft-pull click.
    base.insert(
        InputSource::RightTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular,
                actions: vec![mouse(MouseButton::Left)],
                settings: CommandSettings {
                    haptics: Haptics {
                        on: HapticEdge::Both,
                        strength: HapticStrength::Low,
                    },
                    ..Default::default()
                },
            }],
        },
    );
    // Left trigger soft-pull → right mouse click (same, right button).
    base.insert(
        InputSource::LeftTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular,
                actions: vec![mouse(MouseButton::Right)],
                settings: CommandSettings {
                    haptics: Haptics {
                        on: HapticEdge::Both,
                        strength: HapticStrength::Low,
                    },
                    ..Default::default()
                },
            }],
        },
    );

    // Left-pad click → middle mouse button.
    base.insert(
        InputSource::LeftPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![mouse(MouseButton::Middle)],
                settings: Default::default(),
            }],
        },
    );
    // Left bumper → Backspace.
    base.insert(
        InputSource::LeftBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Backspace)],
                settings: Default::default(),
            }],
        },
    );
    // Right bumper → Space.
    base.insert(
        InputSource::RightBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Space)],
                settings: Default::default(),
            }],
        },
    );
    // Left-stick click → Super/Meta.
    base.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::LeftMeta)],
                settings: Default::default(),
            }],
        },
    );
    // View → Alt.
    base.insert(
        InputSource::View,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::LeftAlt)],
                settings: Default::default(),
            }],
        },
    );
    // Menu → Tab.
    base.insert(
        InputSource::Menu,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular,
                actions: vec![key(Key::Tab)],
                settings: Default::default(),
            }],
        },
    );

    // Left grip: interruptible short-tap Ctrl+C combo, plus a Long-press Shift modifier (with a
    // press haptic so you feel the long trigger and know you can release).
    base.insert(
        InputSource::LeftGrip,
        SourceBinding::Button {
            commands: vec![
                Command {
                    activator: Activator::Regular,
                    actions: vec![key(Key::LeftCtrl), key(Key::C)],
                    settings: CommandSettings {
                        interruptible: true,
                        ..Default::default()
                    },
                },
                Command {
                    activator: Activator::Long { hold_ms: 250 },
                    actions: vec![key(Key::LeftShift)],
                    settings: CommandSettings {
                        haptics: Haptics {
                            on: HapticEdge::OnPress,
                            strength: HapticStrength::Medium,
                        },
                        ..Default::default()
                    },
                },
            ],
        },
    );
    // Right grip: interruptible short-tap Ctrl+V combo, plus a Long-press Ctrl modifier.
    base.insert(
        InputSource::RightGrip,
        SourceBinding::Button {
            commands: vec![
                Command {
                    activator: Activator::Regular,
                    actions: vec![key(Key::LeftCtrl), key(Key::V)],
                    settings: CommandSettings {
                        interruptible: true,
                        ..Default::default()
                    },
                },
                Command {
                    activator: Activator::Long { hold_ms: 250 },
                    actions: vec![key(Key::LeftCtrl)],
                    settings: CommandSettings {
                        haptics: Haptics {
                            on: HapticEdge::OnPress,
                            strength: HapticStrength::Medium,
                        },
                        ..Default::default()
                    },
                },
            ],
        },
    );

    // Hold layer: while the right pad is clicked, the left stick drives the mouse (deflection→
    // rate) instead of the arrow-key dpad, and the right pad is nullified so holding it doesn't
    // also jitter the cursor. Explicit sensitivity/acceleration/deadzone knobs.
    let stick_mouse = Layer {
        name: "stick_mouse".into(),
        bindings: BTreeMap::from([
            (
                InputSource::LeftStick,
                SourceBinding::JoystickMouse {
                    settings: JoystickMouseSettings {
                        output: MouseOutput::Cursor,
                        sensitivity: Sensitivity { x: 2.0, y: 2.0 },
                        curve: Curve::Power(2.0),
                        deadzone: Deadzone { inner: 0.0 },
                        ..Default::default()
                    },
                },
            ),
            (InputSource::RightPad, SourceBinding::None),
        ]),
    };

    ConfigDoc {
        version: 0,
        name: "Desktop".into(),
        action_sets: vec![ActionSet {
            name: "Desktop".into(),
            bindings: base,
            layers: vec![stick_mouse],
        }],
        rumble: RumbleSettings::default(),
    }
}

// --- globals ----------------------------------------------------------------------------

/// The above-profile globals: full master rumble and a Steam + RightGrip fallback toggle.
pub fn globals() -> GlobalConfig {
    GlobalConfig {
        start_profile: StartProfile::Active,
        master_rumble: 100,
        chords: vec![GlobalChord {
            buttons: vec![InputSource::Steam, InputSource::RightGrip],
            action: GlobalAction::SwitchFallback {
                mode: SwitchMode::Toggle,
            },
        }],
        ..Default::default()
    }
}

fn main() -> std::io::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let pretty = ron::ser::PrettyConfig::default();

    for (name, doc_ron) in [
        (
            "game_profile.ron",
            ron::ser::to_string_pretty(&game_profile(), pretty.clone()).unwrap(),
        ),
        (
            "desktop_profile.ron",
            ron::ser::to_string_pretty(&desktop_profile(), pretty.clone()).unwrap(),
        ),
        (
            "globals.ron",
            ron::ser::to_string_pretty(&globals(), pretty.clone()).unwrap(),
        ),
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
        assert!(
            g.validate()
                .iter()
                .all(|d| d.severity != config::Severity::Error)
        );
        assert_eq!(
            ron::from_str::<GlobalConfig>(&ron::to_string(&g).unwrap()).unwrap(),
            g
        );
    }
}
