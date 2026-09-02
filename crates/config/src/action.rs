//! Actions - what a command/subcommand fires (PLAN 3, Round D).

use serde::{Deserialize, Serialize};
use vocab_out::{GamepadButton, Key, MouseButton};

/// Reference to an action set by name (validated against the profile's action sets; the
/// compile step later resolves it to an index). Serializes as a bare string in RON.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionSetRef(pub String);

/// Reference to a layer by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayerRef(pub String);

/// What a command - or one of its subcommands - fires (Round D).
///
/// **Output** actions realize on the OS via `virt-out` (they name [`vocab`] targets);
/// **mode** actions (`ChangeActionSet` / `*Layer`) are handled by the engine. Discrete
/// scroll is a `MouseButton` pseudo-button (backend -> wheel tick) and the dpad directions
/// are `GamepadButton`s (backend -> hat). Continuous outputs (mouse move, gamepad axes) come
/// from *behaviors*, never an action. A timed-sequence `Macro` is deferred (distinct from
/// the simultaneous subcommand combo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// No-op - explicitly unbound / blocks a fall-through to a lower layer or the set.
    None,
    Key(Key),
    MouseButton(MouseButton),
    GamepadButton(GamepadButton),
    /// Switch the active action set (a full-controller mode swap).
    ChangeActionSet(ActionSetRef),
    /// Apply a layer while the firing input is held (released when it releases).
    HoldLayer(LayerRef),
    /// Add a layer to the active stack (persists until removed).
    AddLayer(LayerRef),
    /// Remove a layer from the active stack.
    RemoveLayer(LayerRef),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_round_trips_ron() {
        for a in [
            Action::None,
            Action::Key(Key::C),
            Action::MouseButton(MouseButton::ScrollUp),
            Action::GamepadButton(GamepadButton::DpadUp),
            Action::HoldLayer(LayerRef("aim".into())),
            Action::ChangeActionSet(ActionSetRef("driving".into())),
        ] {
            let s = ron::to_string(&a).unwrap();
            assert_eq!(ron::from_str::<Action>(&s).unwrap(), a);
        }
    }
}
