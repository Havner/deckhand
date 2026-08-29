//! `example_profiles` — build real `ConfigDoc`s (a **game** profile and a **desktop** profile)
//! plus a `DeviceConfig`, and write them to RON. A worked example of the whole config model and
//! the source of the profiles `deckhand-run` drives for manual testing.
//!
//! - `desktop_profile` is a keyboard/mouse mapping for the fallback (desktop) role.
//! - `cp2077_profile` mirrors `crates/virt-out/examples/bridge.rs` (Gordon → virtual Xbox pad +
//!   kbd/mouse + gyro-mouse): a mode-shift **layer** (left stick → right stick while the right
//!   pad is clicked), gyro gated by the left full-pull (vertical inverted, as in the bridge).
//! - `device_config` carries the per-device rumble shaping; `chords` carries the
//!   **Steam/QuickAccess + grip** profile-switch chords.
//!
//! Run `cargo run -p config --example example_profiles [out_dir]` to write the profiles into
//! `<out_dir>/profiles/` and `devcfg.ron` into `<out_dir>` — the same layout the UI uses under
//! `$XDG_CONFIG_HOME/deckhand`, so `out_dir` can be your deckhand config dir (defaults to the temp
//! dir). The test builds each and checks it validates and round-trips.
//!
//! Deliberately **verbose**: every binding is spelled out in full so any single one can be
//! retuned without touching a shared builder. Only the leaf action constructors (`pad`/`key`/
//! `mouse`) are kept — they embed no settings, so editing one binding never affects another.

use std::collections::BTreeMap;

use config::{
    Acceleration, Action, ActionSet, Activation, ActivationMode, Activator, AsMouseSettings, Axis, Command, CommandSettings, ConfigDoc, Curve, Deadzone, DirectionalPadSettings, DpadLayout, ChordAction, Chord, Chords, DeviceConfig, GyroSpace, GyroToMouseSettings, HapticEdge, HapticStrength, Haptics, InputSource, Invert, JoystickMouseSettings, JoystickSettings, Layer, LayerRef, MouseOutput, OneEuroFilter, Rotation, RumbleSettings, Sensitivity, SoftPull, SourceBinding, StickOutput, SwitchMode, TriggerOutput, TriggerSettings
};
use vocab_hid::Button;
use vocab_out::{GamepadButton, Key, MouseButton};

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

// --- system layer (used by all profiles) ------------------------------------------------

pub fn system_keys_layer() -> Layer {
    Layer {
        name: "system_keys".into(),
        bindings: BTreeMap::from([(
            InputSource::FaceButtons,
            SourceBinding::ButtonPad {
                up: vec![Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::VolumeUp)],
                    settings: Default::default(),
                }],
                down: vec![Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::VolumeDown)],
                    settings: Default::default(),
                }],
                left: vec![Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::Rewind)],
                    settings: Default::default(),
                }],
                right: vec![Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::PlayPause)],
                    settings: Default::default(),
                }],
            },
        )]),
    }
}

// --- desktop profile (the fallback role) ------------------------------------------------

pub fn desktop_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // ----- BUTTONS -----

    // Face diamond → navigation keys (up = Y, down = A, left = X, right = B).
    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::PageUp)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Enter)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::PageDown)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Esc)],
                settings: Default::default(),
            }],
        },
    );

    // D-Pad → arrows.
    base.insert(
        InputSource::DPad,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Up)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Down)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Left)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Right)],
                settings: Default::default(),
            }],
        },
    );

    // Left bumper → Backspace.
    base.insert(
        InputSource::LeftBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Space)],
                settings: Default::default(),
            }],
        },
    );

    // Left grip → Shift.
    base.insert(
        InputSource::LeftGrip,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftShift)],
                settings: Default::default(),
            }],
        },
    );
    // Left grip → Ctrl.
    base.insert(
        InputSource::LeftGrip2,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftCtrl)],
                settings: Default::default(),
            }],
        },
    );
    // Right grip → Ctrl+C combo.
    base.insert(
        InputSource::RightGrip,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftCtrl), key(Key::C)],
                settings: Default::default(),
            }],
        },
    );
    // Right grip2 → Ctrl+V combo.
    base.insert(
        InputSource::RightGrip2,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftCtrl), key(Key::V)],
                settings: Default::default(),
            }],
        },
    );

    // View → Alt.
    base.insert(
        InputSource::View,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Tab)],
                settings: Default::default(),
            }],
        },
    );
    // Steam button → system_keys layer.
    base.insert(
        InputSource::Steam,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![Action::HoldLayer(LayerRef("system_keys".into()))],
                settings: Default::default(),
            }],
        },
    );
    // Quick access button → none (toggle profile).
    base.insert(
        InputSource::QuickAccess,
        SourceBinding::None,
    );

    // ----- TRIGGERS -----

    // Right trigger soft-pull → left mouse click (haptic tick on press and release). Output `None`
    // so the trigger drives no gamepad axis on the desktop — just the soft-pull click.
    base.insert(
        InputSource::RightTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                soft_pull: SoftPull { threshold: 0.3 },
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
    // Right trigger full-pull → none.
    base.insert(
        InputSource::RightTriggerFull,
        SourceBinding::None,
    );

    // Left trigger soft-pull → right mouse click (same, right button).
    base.insert(
        InputSource::LeftTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                soft_pull: SoftPull { threshold: 0.3 },
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
    // Left trigger full-pull → none.
    base.insert(
        InputSource::LeftTriggerFull,
        SourceBinding::None,
    );

    // ----- JOYSTICKS -----

    // Left stick → mouse smooth scroll.
    base.insert(
        InputSource::LeftStick,
        SourceBinding::JoystickMouse {
            settings: JoystickMouseSettings {
                output: MouseOutput::SmoothScroll,
                sensitivity: Sensitivity { x: 1.2, y: 1.2 },
                curve: Curve::Power(2.0),
                deadzone: Deadzone { inner: 0.1 },
                ..Default::default()
            },
        },
    );
    // Left-stick click → middle mouse button.
    base.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![mouse(MouseButton::Middle)],
                settings: Default::default()
            }],
        },
    );

    // Right stick → mouse cursor.
    base.insert(
        InputSource::RightStick,
        SourceBinding::JoystickMouse {
            settings: JoystickMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.2, y: 1.2 },
                curve: Curve::Power(2.0),
                deadzone: Deadzone { inner: 0.05 },
                ..Default::default()
            },
        },
    );
    // Right-stick click → Super/Meta.
    base.insert(
        InputSource::RightStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftMeta)],
                settings: Default::default()
            }],
        },
    );

    // ----- TRACKPADS -----

    // Left pad → smooth scroll wheel.
    base.insert(
        InputSource::LeftPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::SmoothScroll,
                sensitivity: Sensitivity { x: 1.5, y: 1.5 },
                acceleration: Acceleration { factor: 0.05 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                ..Default::default()
            },
        },
    );
    // Left-pad click → middle mouse button.
    base.insert(
        InputSource::LeftPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![mouse(MouseButton::Middle)],
                settings: Default::default(),
            }],
        },
    );

    // Right pad → mouse cursor.
    base.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.5, y: 1.5 },
                acceleration: Acceleration { factor: 0.06 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                rotation: Rotation { degrees: 0.0 },
                ..Default::default()
            },
        },
    );
    // Right-pad click → gyro layer.
    base.insert(
        InputSource::RightPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![Action::HoldLayer(LayerRef("gyro".into()))],
                settings: CommandSettings {
                    haptics: Haptics { on: HapticEdge::Both, strength: HapticStrength::Medium },
                    ..Default::default()
                },
            }],
        },
    );

    // ----- GYRO -----

    // Gyro → none.
    base.insert(
        InputSource::Gyro,
        SourceBinding::None,
    );

    // ----- LAYERS -----

    let gyro = Layer {
        name: "gyro".into(),
        bindings: BTreeMap::from([
            (
                InputSource::Gyro,
                SourceBinding::GyroToMouse {
                    settings: GyroToMouseSettings {
                        output: MouseOutput::Cursor,
                        acceleration: Acceleration { factor: 0.02 },
                        space: GyroSpace::PlayerSpace,
                        smoothing: Some(OneEuroFilter {
                            min_cutoff: 5.0,
                            beta: 0.5,
                        }),
                        deadzone: Deadzone { inner: 0.01 },
                        activation: Activation {
                            mode: ActivationMode::HoldToDisable,
                            gaters: vec![],
                        },
                        ..Default::default()
                    },
                },
            ),
            (
                InputSource::RightPad,
                SourceBinding::None
            ),
        ]),
    };

    // ----- CONFIG -----

    ConfigDoc {
        version: 0,
        name: "Desktop".into(),
        rumble: RumbleSettings::default(),
        action_sets: vec![ActionSet {
            name: "base".into(),
            bindings: base,
            layers: vec![system_keys_layer(), gyro],
        }],
    }
}

// --- desktop profile for Gordon (the fallback role) ------------------------------------------------

pub fn desktop_gordon_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // ----- BUTTONS -----

    // Face diamond → navigation keys (up = Y, down = A, left = X, right = B).
    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::PageUp)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Enter)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::PageDown)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Esc)],
                settings: Default::default(),
            }],
        },
    );

    // D-Pad → none.
    base.insert(
        InputSource::DPad,
        SourceBinding::None,
    );

    // Left bumper → Backspace.
    base.insert(
        InputSource::LeftBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Space)],
                settings: Default::default(),
            }],
        },
    );

    // Left grip: interruptible short-tap Ctrl+C combo, plus a Long-press Ctrl modifier (with a
    // press haptic so you feel the long trigger and know you can release).
    base.insert(
        InputSource::LeftGrip,
        SourceBinding::Button {
            commands: vec![
                Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::LeftCtrl), key(Key::C)],
                    settings: Default::default(),
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
    // Right grip: interruptible short-tap Ctrl+V combo, plus a Long-press Shift modifier.
    base.insert(
        InputSource::RightGrip,
        SourceBinding::Button {
            commands: vec![
                Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::LeftCtrl), key(Key::V)],
                    settings: Default::default(),
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

    // View → Alt.
    base.insert(
        InputSource::View,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Tab)],
                settings: Default::default(),
            }],
        },
    );
    // Steam button → system_keys layer.
    base.insert(
        InputSource::Steam,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![Action::HoldLayer(LayerRef("system_keys".into()))],
                settings: Default::default(),
            }],
        },
    );
    // Quick access button → none (toggle profile).
    base.insert(
        InputSource::QuickAccess,
        SourceBinding::None,
    );

    // ----- TRIGGERS -----

    // Right trigger soft-pull → left mouse click (haptic tick on press and release). Output `None`
    // so the trigger drives no gamepad axis on the desktop — just the soft-pull click.
    base.insert(
        InputSource::RightTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                soft_pull: SoftPull { threshold: 0.3 },
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
    // Right trigger full-pull → none.
    base.insert(
        InputSource::RightTriggerFull,
        SourceBinding::None,
    );

    // Left trigger soft-pull → right mouse click (same, right button).
    base.insert(
        InputSource::LeftTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                soft_pull: SoftPull { threshold: 0.3 },
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
    // Left trigger full-pull → none.
    base.insert(
        InputSource::LeftTriggerFull,
        SourceBinding::None,
    );

    // ----- JOYSTICKS -----

    // Left stick → 4-way directional pad → arrow keys.
    base.insert(
        InputSource::LeftStick,
        SourceBinding::DirectionalPad {
            settings: DirectionalPadSettings {
                layout: DpadLayout::FourWay,
                deadzone: Deadzone { inner: 0.3 },
                ..Default::default()
            },
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Up)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Down)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Left)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Right)],
                settings: Default::default(),
            }],
            outer_ring: vec![],
        },
    );
    // Left-stick click → Super/Meta.
    base.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftMeta)],
                settings: Default::default(),
            }],
        },
    );

    // Right stick → mouse cursor.
    base.insert(
        InputSource::RightStick,
        SourceBinding::JoystickMouse {
            settings: JoystickMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.2, y: 1.2 },
                curve: Curve::Power(2.0),
                deadzone: Deadzone { inner: 0.05 },
                ..Default::default()
            },
        },
    );
    // Right-stick click → none.
    base.insert(
        InputSource::RightStickClick,
        SourceBinding::None,
    );

    // ----- TRACKPADS -----

    // Left pad → smooth scroll wheel.
    base.insert(
        InputSource::LeftPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::SmoothScroll,
                sensitivity: Sensitivity { x: 1.5, y: 1.5 },
                acceleration: Acceleration { factor: 0.05 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                ..Default::default()
            },
        },
    );
    // Left-pad click → middle mouse button.
    base.insert(
        InputSource::LeftPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![mouse(MouseButton::Middle)],
                settings: Default::default(),
            }],
        },
    );

    // Alternative Left pad testing
    // base.insert(
    //     InputSource::LeftPad,
    //     SourceBinding::DirectionalPad {
    //         up: vec![Command {
    //             activator: Activator::Regular { interruptible: true },
    //             actions: vec![key(Key::Up)],
    //             settings: Default::default(),
    //         }],
    //         down: vec![Command {
    //             activator: Activator::Regular { interruptible: true },
    //             actions: vec![key(Key::Down)],
    //             settings: Default::default(),
    //         }],
    //         left: vec![Command {
    //             activator: Activator::Regular { interruptible: true },
    //             actions: vec![key(Key::Left)],
    //             settings: Default::default(),
    //         }],
    //         right: vec![Command {
    //             activator: Activator::Regular { interruptible: true },
    //             actions: vec![key(Key::Right)],
    //             settings: Default::default(),
    //         }],
    //         outer_ring: vec![Command {
    //             activator: Activator::Regular { interruptible: true },
    //             actions: vec![key(Key::LeftShift)],
    //             settings: Default::default(),
    //         }],
    //         settings: DirectionalPadSettings {
    //             deadzone: Deadzone { inner: 0.0 },
    //             layout: DpadLayout::FourWay,
    //             outer_ring: OuterRing { radius: 0.9 },
    //             rotation: Rotation { degrees: 0.0 },
    //             activation: Activation {
    //                 mode: ActivationMode::HoldToEnable,
    //                 gaters: vec![InputSource::LeftPadClick],
    //             },
    //         },
    //     },
    // );

    // Right pad → mouse cursor.
    base.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.5, y: 1.5 },
                acceleration: Acceleration { factor: 0.06 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                rotation: Rotation { degrees: 0.0 },
                ..Default::default()
            },
        },
    );
    // Right-pad click holds the alt_mouse layer (left stick → mouse instead of arrows).
    base.insert(
        InputSource::RightPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![Action::HoldLayer(LayerRef("alt_mouse".into()))],
                settings: Default::default(),
            }],
        },
    );

    // ----- GYRO -----

    // Gyro → none.
    base.insert(
        InputSource::Gyro,
        SourceBinding::None,
    );

    // ----- LAYERS -----

    // Hold layer: while the right pad is clicked, the left stick drives the mouse (deflection→
    // rate) instead of the arrow-key dpad, and the right pad is nullified so holding it doesn't
    // also jitter the cursor.
    let alt_mouse = Layer {
        name: "alt_mouse".into(),
        bindings: BTreeMap::from([
            (
                InputSource::LeftStick,
                SourceBinding::JoystickMouse {
                    settings: JoystickMouseSettings {
                        output: MouseOutput::Cursor,
                        sensitivity: Sensitivity { x: 1.2, y: 1.2 },
                        curve: Curve::Power(2.0),
                        deadzone: Deadzone { inner: 0.05 },
                        ..Default::default()
                    },
                },
            ),
            (
                InputSource::LeftPad,
                SourceBinding::AsMouse {
                    settings: AsMouseSettings {
                        output: MouseOutput::Scroll,
                        axis: Axis::Vertical,
                        sensitivity: Sensitivity { x: 1.5, y: 1.5 },
                        acceleration: Acceleration { factor: 0.05 },
                        smoothing: Some(OneEuroFilter {
                            min_cutoff: 3.0,
                            beta: 0.5,
                        }),
                        ..Default::default()
                    },
                },
            ),
            (
                InputSource::Gyro,
                SourceBinding::GyroToMouse {
                    settings: GyroToMouseSettings {
                        output: MouseOutput::Cursor,
                        acceleration: Acceleration { factor: 0.02 },
                        space: GyroSpace::PlayerSpace,
                        smoothing: Some(OneEuroFilter {
                            min_cutoff: 5.0,
                            beta: 0.5,
                        }),
                        deadzone: Deadzone { inner: 0.01 },
                        activation: Activation {
                            mode: ActivationMode::HoldToDisable,
                            gaters: vec![],
                        },
                        ..Default::default()
                    },
                },
            ),
            (
                InputSource::RightPad,
                SourceBinding::None
            ),
        ]),
    };

    // ----- CONFIG -----

    ConfigDoc {
        version: 0,
        name: "Desktop Gordon".into(),
        rumble: RumbleSettings::default(),
        action_sets: vec![ActionSet {
            name: "base".into(),
            bindings: base,
            layers: vec![system_keys_layer(), alt_mouse],
        }],
    }
}

// --- Xbox 1:1 controller profile --------------------------------------------------

pub fn xbox_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // ----- BUTTONS -----

    // Face buttons (ButtonPad; diamond positions) → gamepad A/B/X/Y, 1:1.
    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::A)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::B)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::X)],
                settings: Default::default(),
            }],
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::Y)],
                settings: Default::default(),
            }],
        },
    );

    // D-Pad → gamepad dpad.
    base.insert(
        InputSource::DPad,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::DpadUp)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::DpadDown)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::DpadLeft)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::RightBumper)],
                settings: Default::default(),
            }],
        },
    );

    // Left grip → left stick click.
    base.insert(
        InputSource::LeftGrip,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::RightStick)],
                settings: Default::default(),
            }],
        },
    );
    // Left grip 2 → left stick click.
    base.insert(
        InputSource::LeftGrip2,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::LeftStick)],
                settings: Default::default(),
            }],
        },
    );
    // Right grip 2 → right stick click.
    base.insert(
        InputSource::RightGrip2,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::RightStick)],
                settings: Default::default(),
            }],
        },
    );

    // View (left small top button) → gamepad Back.
    base.insert(
        InputSource::View,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::Start)],
                settings: Default::default(),
            }],
        },
    );
    // Steam button → system_keys layer.
    base.insert(
        InputSource::Steam,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![Action::HoldLayer(LayerRef("system_keys".into()))],
                settings: Default::default(),
            }],
        },
    );
    // Quick access button → none (toggle profile).
    base.insert(
        InputSource::QuickAccess,
        SourceBinding::None,
    );

    // ----- TRIGGERS -----

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
    base.insert(
        InputSource::RightTriggerFull,
        SourceBinding::None,
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
    base.insert(
        InputSource::LeftTriggerFull,
        SourceBinding::None,
    );

    // ----- JOYSTICKS -----

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
    // Left-stick click → left stick click.
    base.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::LeftStick)],
                settings: Default::default(),
            }],
        },
    );

    // Right stick → right gamepad stick.
    base.insert(
        InputSource::RightStick,
        SourceBinding::Joystick {
            settings: JoystickSettings {
                output: StickOutput::Right,
                ..Default::default()
            },
            outer_ring: vec![],
        },
    );
    // Right-stick click → right stick click.
    base.insert(
        InputSource::RightStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![pad(GamepadButton::RightStick)],
                settings: Default::default(),
            }],
        },
    );

    // ----- TRACKPADS -----

    // Right pad → none.
    base.insert(
        InputSource::RightPad,
        SourceBinding::None,
    );
    // Right-pad click → none.
    base.insert(
        InputSource::RightPadClick,
        SourceBinding::None,
    );

    // Left pad → none.
    base.insert(
        InputSource::LeftPad,
        SourceBinding::None,
    );
    // Left-pad click → none.
    base.insert(
        InputSource::LeftPadClick,
        SourceBinding::None,
    );

    // ----- GYRO -----

    // Gyro → none
    base.insert(
        InputSource::Gyro,
        SourceBinding::None,
    );

    // ----- LAYERS -----

    // ----- CONFIG -----

    ConfigDoc {
        version: 0,
        name: "Xbox".into(),
        rumble: RumbleSettings::default(),
        action_sets: vec![ActionSet {
            name: "base".into(),
            bindings: base,
            layers: vec![system_keys_layer()],
        }],
    }
}

// --- Xbox + Mouse mixed controls profile --------------------------------------------------

pub fn xbox_mouse_profile() -> ConfigDoc {
    let mut profile = xbox_profile();
    profile.name = "Xbox+Mouse".into();

    // Right grip 2 click adds the mode-shift layer (left stick → right stick).
    profile.action_sets[0].bindings.insert(
        InputSource::RightGrip2,
        SourceBinding::Button {
            commands: vec![
                Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![Action::AddLayer(LayerRef("aim_stick_right".into()))],
                    settings: Default::default(),
                },
                Command {
                    activator: Activator::Long { hold_ms: 200 },
                    actions: vec![Action::HoldLayer(LayerRef("aim_stick_right".into()))],
                    settings: Default::default(),
                },
            ],
        },
    );

    // Right stick → mouse cursor.
    profile.action_sets[0].bindings.insert(
        InputSource::RightStick,
        SourceBinding::JoystickMouse {
            settings: JoystickMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.2, y: 1.2 },
                curve: Curve::Power(2.0),
                deadzone: Deadzone { inner: 0.05 },
                ..Default::default()
            },
        },
    );
    // Right-stick click stays right stick click.

    // Right pad → mouse cursor.
    profile.action_sets[0].bindings.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.0, y: 1.0 },
                acceleration: Acceleration { factor: 0.06 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                rotation: Rotation { degrees: 0.0 },
                ..Default::default()
            },
        },
    );
    // Right-pad click holds the mode-shift layer (left stick → right stick).
    profile.action_sets[0].bindings.insert(
        InputSource::RightPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![Action::HoldLayer(LayerRef("aim_stick_left".into()))],
                settings: Default::default(),
            }],
        },
    );

    // ----- LAYERS -----

    // Mode-shift layer: while the right pad is clicked, the left stick drives the RIGHT stick,
    // and the right pad itself is nullified (so holding it for the mode-shift doesn't jitter the
    // mouse). `None` overrides the base AsMouse binding for the duration of the layer.
    let aim_stick_left = Layer {
        name: "aim_stick_left".into(),
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
            (
                InputSource::RightPad,
                SourceBinding::None
            ),
        ]),
    };
    let aim_stick_right = Layer {
        name: "aim_stick_right".into(),
        bindings: BTreeMap::from([
            (
                InputSource::RightGrip2,
                SourceBinding::Button {
                    commands: vec![Command {
                        activator: Activator::Regular { interruptible: true },
                        actions: vec![Action::RemoveLayer(LayerRef("aim_stick_right".into()))],
                        settings: Default::default(),
                    }],
                },
            ),
            (
                InputSource::RightStick,
                SourceBinding::Joystick {
                    settings: JoystickSettings {
                        output: StickOutput::Right,
                        ..Default::default()
                    },
                    outer_ring: vec![],
                },
            ),
        ]),
    };

    profile.action_sets[0].layers =
        vec![system_keys_layer(), aim_stick_left, aim_stick_right];

    profile
}

// --- Xbox + Mouse/Gyro mixed controls profile --------------------------------------------------

pub fn xbox_mouse_gyro_profile() -> ConfigDoc {
    let mut profile = xbox_mouse_profile();
    profile.name = "Xbox+Mouse/Gyro".into();

    // Gyro → mouse (vertical inverted, as in the bridge), gated by the left full-pull.
    profile.action_sets[0].bindings.insert(
        InputSource::Gyro,
        SourceBinding::GyroToMouse {
            settings: GyroToMouseSettings {
                output: MouseOutput::Cursor,
                space: GyroSpace::PlayerSpace,
                sensitivity: Sensitivity { x: 0.5, y: 0.5 },
                acceleration: Acceleration { factor: 0.02 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                deadzone: Deadzone { inner: 0.01 },
                invert: Invert { x: false, y: true },
                activation: Activation {
                    mode: ActivationMode::HoldToEnable,
                    gaters: vec![Button::LT],
                },
                ..Default::default()
            },
        },
    );

    profile
}

// --- Cyberpunk 2077 profile --------------------------------------------------

pub fn cp2077_profile() -> ConfigDoc {
    let mut profile = xbox_mouse_gyro_profile();
    profile.name = "Cyberpunk 2077".into();

    // Left-stick click → key L.
    profile.action_sets[0].bindings.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::L)],
                settings: Default::default(),
            }],
        },
    );

    // Right-pad mouse sensitivity
    if let Some(SourceBinding::AsMouse { settings }) =
        profile.action_sets[0].bindings.get_mut(&InputSource::RightPad) {
            settings.sensitivity.x = 1.5;
            settings.sensitivity.y = 1.5;
        };

    // Right-stick mouse sensitivity
    if let Some(SourceBinding::JoystickMouse { settings }) =
        profile.action_sets[0].bindings.get_mut(&InputSource::RightStick) {
            settings.sensitivity.x = 2.0;
            settings.sensitivity.y = 2.0;
        };

    // cp2077 specific: quickhack set 1, reuse system_keys layer
    profile.action_sets[0].layers[0].bindings.insert(
        InputSource::LeftBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftBrace)],
                settings: Default::default(),
            }],
        },
    );

    // cp2077 specific: quickhack set 2, reuse system_keys layer
    profile.action_sets[0].layers[0].bindings.insert(
        InputSource::RightBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::RightBrace)],
                settings: Default::default(),
            }],
        },
    );

    profile
}

pub fn system_shock_profile() -> ConfigDoc {
    let mut profile = xbox_mouse_gyro_profile();
    profile.name = "System Shock".into();

    // Left-stick click → key L.
    profile.action_sets[0].bindings.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::L)],
                settings: Default::default(),
            }],
        },
    );

    profile
}

// --- Control profile ------------------------------------------------

pub fn control_profile() -> ConfigDoc {
    let mut base: BTreeMap<InputSource, SourceBinding> = BTreeMap::new();

    // ----- BUTTONS -----

    base.insert(
        InputSource::FaceButtons,
        SourceBinding::ButtonPad {
            down: vec![
                Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::Space)],
                    settings: Default::default(),
                },
                Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::Enter)],
                    settings: Default::default(),
                }
            ],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftAlt)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::F)],
                settings: Default::default(),
            }],
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::V)],
                settings: Default::default()
            }],
        },
    );

    base.insert(
        InputSource::DPad,
        SourceBinding::ButtonPad {
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Tab)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::R)],
                settings: Default::default(),
            }],
            left: vec![
                Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::G)],
                    settings: Default::default(),
                },
                Command {
                    activator: Activator::Long { hold_ms: 450 },
                    actions: vec![key(Key::M)],
                    settings: CommandSettings {
                        haptics: Haptics {
                            on: HapticEdge::OnPress,
                            strength: HapticStrength::Medium,
                        },
                        ..Default::default()
                    },
                },
            ],
            right: vec![
                Command {
                    activator: Activator::Regular { interruptible: true },
                    actions: vec![key(Key::I)],
                    settings: Default::default(),
                },
                Command {
                    activator: Activator::Long { hold_ms: 450 },
                    actions: vec![key(Key::N)],
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

    base.insert(
        InputSource::LeftBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Q)],
                settings: Default::default(),
            }],
        },
    );
    base.insert(
        InputSource::RightBumper,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::E)],
                settings: Default::default(),
            }],
        },
    );

    base.insert(
        InputSource::LeftGrip,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftShift)],
                settings: Default::default()
            }],
        },
    );
    base.insert(
        InputSource::RightGrip,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftCtrl)],
                settings: Default::default()
            }],
        },
    );

    base.insert(
        InputSource::View,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Esc)],
                settings: Default::default(),
            }],
        },
    );
    base.insert(
        InputSource::Menu,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Esc)],
                settings: Default::default(),
            }],
        },
    );
    base.insert(
        InputSource::Steam,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![Action::HoldLayer(LayerRef("system_keys".into()))],
                settings: Default::default(),
            }],
        },
    );
    base.insert(
        InputSource::QuickAccess,
        SourceBinding::None,
    );

    // ----- TRIGGERS -----

    base.insert(
        InputSource::RightTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                soft_pull: SoftPull { threshold: 0.3 },
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
    base.insert(
        InputSource::RightTriggerFull,
        SourceBinding::None,
    );

    base.insert(
        InputSource::LeftTrigger,
        SourceBinding::Trigger {
            settings: TriggerSettings {
                output: TriggerOutput::None,
                soft_pull: SoftPull { threshold: 0.3 },
                ..Default::default()
            },
            soft_pull: vec![Command {
                activator: Activator::Regular { interruptible: true },
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
    base.insert(
        InputSource::LeftTriggerFull,
        SourceBinding::None,
    );

    // ----- JOYSTICKS -----

    base.insert(
        InputSource::LeftStick,
        SourceBinding::DirectionalPad {
            settings: DirectionalPadSettings {
                layout: DpadLayout::EightWay,
                deadzone: Deadzone { inner: 0.3 },
                ..Default::default()
            },
            up: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::W)],
                settings: Default::default(),
            }],
            down: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::S)],
                settings: Default::default(),
            }],
            left: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::A)],
                settings: Default::default(),
            }],
            right: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::D)],
                settings: Default::default(),
            }],
            outer_ring: vec![],
        },
    );
    base.insert(
        InputSource::LeftStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::Slash)],
                settings: Default::default(),
            }],
        },
    );

    base.insert(
        InputSource::RightStick,
        SourceBinding::JoystickMouse {
            settings: JoystickMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.2, y: 1.2 },
                curve: Curve::Power(2.0),
                deadzone: Deadzone { inner: 0.05 },
                ..Default::default()
            },
        },
    );
    base.insert(
        InputSource::RightStickClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftCtrl)],
                settings: Default::default(),
            }],
        },
    );

    // ----- TRACKPADS -----

    base.insert(
        InputSource::RightPad,
        SourceBinding::AsMouse {
            settings: AsMouseSettings {
                output: MouseOutput::Cursor,
                sensitivity: Sensitivity { x: 1.0, y: 1.0 },
                acceleration: Acceleration { factor: 0.06 },
                smoothing: Some(OneEuroFilter {
                    min_cutoff: 3.0,
                    beta: 0.5,
                }),
                rotation: Rotation { degrees: 0.0 },
                ..Default::default()
            },
        },
    );
    base.insert(
        InputSource::RightPadClick,
        SourceBinding::Button {
            commands: vec![Command {
                activator: Activator::Regular { interruptible: true },
                actions: vec![key(Key::LeftCtrl)],
                settings: Default::default(),
            }],
        },
    );

    base.insert(
        InputSource::LeftPad,
        SourceBinding::None,
    );
    base.insert(
        InputSource::LeftPadClick,
        SourceBinding::None,
    );

    // ----- GYRO -----

    base.insert(
        InputSource::Gyro,
        SourceBinding::GyroToMouse {
            settings: GyroToMouseSettings {
                output: MouseOutput::Cursor,
                space: GyroSpace::PlayerSpace,
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
                    gaters: vec![Button::LT],
                },
                ..Default::default()
            },
        },
    );

    // ----- CONFIG -----

    ConfigDoc {
        version: 0,
        name: "Control".into(),
        rumble: RumbleSettings::default(),
        action_sets: vec![ActionSet {
            name: "base".into(),
            bindings: base,
            layers: vec![system_keys_layer()],
        }],
    }
}

// --- chords + device config -------------------------------------------------------------------

pub fn chords() -> Chords {
    Chords {
        chords: vec![
            Chord {
                buttons: vec![Button::Steam, Button::LGrip],
                action: ChordAction::SwitchProfile {
                    mode: SwitchMode::SetMain,
                },
            },
            Chord {
                buttons: vec![Button::Steam, Button::RGrip],
                action: ChordAction::SwitchProfile {
                    mode: SwitchMode::SetFallback,
                },
            },
            Chord {
                buttons: vec![Button::QuickAccess],
                action: ChordAction::SwitchProfile {
                    mode: SwitchMode::Toggle,
                },
            },
            // Chord {
            //     buttons: vec![Button::Steam, Button::LGrip],
            //     action: ChordAction::CommandExecute {
            //         command: "ls".into(),
            //         args: vec!["-l".into(), "/home/havner/Documents/Steam-Claude".into()],
            //     },
            // },
        ],
    }
}

pub fn device_config() -> DeviceConfig {
    // The defaults already reproduce the stock feel; this just illustrates the per-device rumble
    // shape (Gordon duty + hz, Neptune/Triton speed + gain).
    DeviceConfig::default()
}

fn main() -> std::io::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let pretty = ron::ser::PrettyConfig::default();

    // Mirror the UI's on-disk layout ($XDG_CONFIG_HOME/deckhand): the profiles live under a
    // `profiles/` subdirectory, `devcfg.ron` sits directly in the passed directory. So pointing the
    // example at your deckhand config dir drops everything into the right place.
    let profiles_dir = dir.join("profiles");
    std::fs::create_dir_all(&profiles_dir)?;

    let write = |path: std::path::PathBuf, ron: String| -> std::io::Result<()> {
        std::fs::write(&path, ron)?;
        println!("wrote {}", path.display());
        Ok(())
    };

    for (name, doc) in [
        ("desktop.ron", desktop_profile()),
        ("desktop_gordon.ron", desktop_gordon_profile()),
        ("xbox.ron", xbox_profile()),
        ("xbox_mouse.ron", xbox_mouse_profile()),
        ("xbox_mouse_gyro.ron", xbox_mouse_gyro_profile()),
        ("cp2077.ron", cp2077_profile()),
        ("system_shock.ron", system_shock_profile()),
        ("control.ron", control_profile()),
    ] {
        write(profiles_dir.join(name), ron::ser::to_string_pretty(&doc, pretty.clone()).unwrap())?;
    }
    write(dir.join("chords.ron"), ron::ser::to_string_pretty(&chords(), pretty.clone()).unwrap())?;
    write(dir.join("devcfg.ron"), ron::ser::to_string_pretty(&device_config(), pretty).unwrap())?;
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
        assert_valid_and_round_trips(&desktop_profile());
        assert_valid_and_round_trips(&desktop_gordon_profile());
        assert_valid_and_round_trips(&xbox_profile());
        assert_valid_and_round_trips(&xbox_mouse_profile());
        assert_valid_and_round_trips(&cp2077_profile());
        assert_valid_and_round_trips(&control_profile());
        assert_valid_and_round_trips(&system_shock_profile());

        let no_errors = |ds: Vec<config::Diagnostic>| ds.iter().all(|d| d.severity != config::Severity::Error);

        let c = chords();
        assert!(no_errors(c.validate()));
        assert_eq!(ron::from_str::<Chords>(&ron::to_string(&c).unwrap()).unwrap(), c);

        let d = device_config();
        assert!(no_errors(d.validate()));
        assert_eq!(ron::from_str::<DeviceConfig>(&ron::to_string(&d).unwrap()).unwrap(), d);
    }
}
