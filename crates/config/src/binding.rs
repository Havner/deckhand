//! The per-source binding — what an action set / layer attaches to an [`InputSource`]
//! (PLAN §3, rounds A–C tied together).
//!
//! One [`SourceBinding`] enum captures every binding shape: a standalone button's commands,
//! a button-group's four members, and each rich behavior (its settings + the commands on
//! its synthesized virtual buttons). Which variant is valid for a source depends on the
//! source's [`SourceKind`] — checked by [`SourceBinding::is_valid_for`] (and validation).

use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::input::SourceKind;
use crate::settings::{
    Activation, AsMouseSettings, DirectionalPadSettings, GyroToMouseSettings, JoystickMouseSettings,
    JoystickSettings, TriggerSettings,
};

/// What is bound to one top-level control. Contains `f32` settings, so `PartialEq` only.
/// `Vec<Command>` fields default to empty (unbound) so RON can omit them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SourceBinding {
    /// A standalone button: the commands fired by it (multi-activator).
    Button {
        #[serde(default)]
        commands: Vec<Command>,
    },
    /// A button group in `ButtonPad` mode: its four member buttons. For `FaceButtons` these
    /// are the diamond positions (`up` = top, `down` = bottom, `left`/`right` = sides); for
    /// `DPad` the literal directions. (Directional/Joystick emulation modes deferred.)
    ButtonPad {
        #[serde(default)]
        up: Vec<Command>,
        #[serde(default)]
        down: Vec<Command>,
        #[serde(default)]
        left: Vec<Command>,
        #[serde(default)]
        right: Vec<Command>,
    },
    /// Pad/Stick → gamepad stick, plus an outer-ring virtual button.
    Joystick {
        #[serde(default)]
        settings: JoystickSettings,
        #[serde(default)]
        outer_ring: Vec<Command>,
    },
    /// Pad/Stick → four direction + outer-ring virtual buttons.
    DirectionalPad {
        #[serde(default)]
        settings: DirectionalPadSettings,
        #[serde(default)]
        up: Vec<Command>,
        #[serde(default)]
        down: Vec<Command>,
        #[serde(default)]
        left: Vec<Command>,
        #[serde(default)]
        right: Vec<Command>,
        #[serde(default)]
        outer_ring: Vec<Command>,
    },
    /// Pad → cursor/scroll (no virtual buttons).
    AsMouse {
        #[serde(default)]
        settings: AsMouseSettings,
    },
    /// Stick → cursor/scroll (no virtual buttons).
    JoystickMouse {
        #[serde(default)]
        settings: JoystickMouseSettings,
    },
    /// Gyro → cursor/scroll (no virtual buttons).
    GyroToMouse {
        #[serde(default)]
        settings: GyroToMouseSettings,
    },
    /// Trigger → gamepad trigger, plus a soft-pull virtual button.
    Trigger {
        #[serde(default)]
        settings: TriggerSettings,
        #[serde(default)]
        soft_pull: Vec<Command>,
    },
    /// Explicitly **unbound** — the input does nothing. Valid on any source; used mainly in a
    /// layer to *nullify* a base binding that would otherwise fall through (e.g. suppress a pad's
    /// mouse while a mode-shift layer is held). In a base action set it is the same as omitting
    /// the input. (To drop only a behavior's axis while keeping its virtual buttons, use the
    /// output target `None` instead — see [`TriggerOutput`](crate::TriggerOutput) etc.)
    None,
}

impl SourceBinding {
    /// Whether this binding shape is valid on a source of the given [`SourceKind`]
    /// (`Joystick`/`DirectionalPad` are shared by `Pad` + `Stick`).
    pub fn is_valid_for(&self, kind: &SourceKind) -> bool {
        use SourceBinding as B;
        use SourceKind as K;
        // Explicit unbind is valid on any source (nullify a fall-through, mainly in layers).
        if matches!(self, B::None) {
            return true;
        }
        matches!(
            (self, kind),
            (B::Button { .. }, K::Button)
                | (B::ButtonPad { .. }, K::ButtonGroup)
                | (B::Joystick { .. } | B::DirectionalPad { .. }, K::Pad | K::Stick)
                | (B::AsMouse { .. }, K::Pad)
                | (B::JoystickMouse { .. }, K::Stick)
                | (B::GyroToMouse { .. }, K::Gyro)
                | (B::Trigger { .. }, K::Trigger)
        )
    }

    /// All commands across this binding's button/virtual-button slots (for validation and
    /// later compilation).
    pub fn commands(&self) -> impl Iterator<Item = &Command> {
        use SourceBinding as B;
        let slots: Vec<&Vec<Command>> = match self {
            B::Button { commands } => vec![commands],
            B::ButtonPad { up, down, left, right } => vec![up, down, left, right],
            B::Joystick { outer_ring, .. } => vec![outer_ring],
            B::DirectionalPad { up, down, left, right, outer_ring, .. } => {
                vec![up, down, left, right, outer_ring]
            }
            B::Trigger { soft_pull, .. } => vec![soft_pull],
            B::AsMouse { .. } | B::JoystickMouse { .. } | B::GyroToMouse { .. } | B::None => vec![],
        };
        slots.into_iter().flatten()
    }

    /// This behavior's activation gate, if it has one (the continuous behaviors do; a plain
    /// button / button-pad / trigger doesn't).
    pub fn activation(&self) -> Option<&Activation> {
        use SourceBinding as B;
        match self {
            B::Joystick { settings, .. } => Some(&settings.activation),
            B::DirectionalPad { settings, .. } => Some(&settings.activation),
            B::AsMouse { settings } => Some(&settings.activation),
            B::JoystickMouse { settings } => Some(&settings.activation),
            B::GyroToMouse { settings } => Some(&settings.activation),
            B::Button { .. } | B::ButtonPad { .. } | B::Trigger { .. } | B::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::command::{Activator, Command};
    use crate::input::InputSource;
    use crate::settings::MouseOutput;
    use vocab::Key;

    fn press(action: Action) -> Command {
        Command {
            activator: Activator::Regular,
            actions: vec![action],
            settings: Default::default(),
        }
    }

    #[test]
    fn kind_validation() {
        let btn = SourceBinding::Button { commands: vec![press(Action::Key(Key::Space))] };
        assert!(btn.is_valid_for(&InputSource::LeftBumper.kind())); // Button
        assert!(!btn.is_valid_for(&InputSource::LeftPad.kind())); // Pad

        let mouse = SourceBinding::AsMouse { settings: Default::default() };
        assert!(mouse.is_valid_for(&InputSource::RightPad.kind())); // Pad
        assert!(!mouse.is_valid_for(&InputSource::LeftStick.kind())); // Stick

        // Joystick is shared by Pad and Stick.
        let joy = SourceBinding::Joystick { settings: Default::default(), outer_ring: vec![] };
        assert!(joy.is_valid_for(&InputSource::LeftPad.kind()));
        assert!(joy.is_valid_for(&InputSource::LeftStick.kind()));
        assert!(!joy.is_valid_for(&InputSource::Gyro.kind()));
    }

    #[test]
    fn binding_round_trips_ron() {
        let b = SourceBinding::AsMouse {
            settings: AsMouseSettings { output: MouseOutput::Cursor, ..Default::default() },
        };
        let s = ron::to_string(&b).unwrap();
        assert_eq!(ron::from_str::<SourceBinding>(&s).unwrap(), b);

        // Omitted Vec fields default to empty (forgiving RON).
        let dpad: SourceBinding = ron::from_str("DirectionalPad()").unwrap();
        assert!(matches!(dpad, SourceBinding::DirectionalPad { up, .. } if up.is_empty()));
    }

    #[test]
    fn none_is_valid_on_any_source_and_round_trips() {
        // Explicit unbind is valid regardless of the source's kind.
        assert!(SourceBinding::None.is_valid_for(&InputSource::RightPad.kind()));
        assert!(SourceBinding::None.is_valid_for(&InputSource::LeftBumper.kind()));
        assert!(SourceBinding::None.is_valid_for(&InputSource::Gyro.kind()));
        // No commands, no activation.
        assert_eq!(SourceBinding::None.commands().count(), 0);
        assert!(SourceBinding::None.activation().is_none());
        // Round-trips.
        assert_eq!(ron::from_str::<SourceBinding>("None").unwrap(), SourceBinding::None);
    }
}
