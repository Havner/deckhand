//! Behaviour taxonomy + authoring defaults for the profile editor.
//!
//! [`Behavior`] is a lightweight tag mirroring the [`SourceBinding`] variants (plus an explicit
//! `Unbound`). It's the value type the behaviour picker selects, and the seam for constructing a
//! fresh binding when the user picks a behaviour: [`Behavior::default_binding`] builds the right
//! variant with UI-authored **starting values** - tuned per behaviour, and per input where it
//! matters (side-aware output; a trackpad DirectionalPad gated on that pad's click). Deadzones
//! default to 0.0 (neutral), like `config`. `config` owns the data model + neutral `Default`;
//! these *authoring* defaults live here in the UI, since no non-UI path ever needs them (this
//! session's decision).
//!
//! UI **ranges** (slider min/max) deliberately aren't here: they vary only by behaviour, so they'll
//! live inline in each behaviour's settings view - no lookup table needed.
//!
//! This is the initial scaffold: the taxonomy is wired into the behaviour picker; `of` /
//! `default_binding` are ready for when the editor reflects and constructs bindings.

use config::{
    Activation, ActivationMode, AsMouseSettings, DirectionalPadSettings, GyroToMouseSettings,
    InputSource, JoystickMouseSettings, JoystickSettings, MouseOutput, Side, SourceBinding,
    SourceKind, StickOutput, TriggerOutput, TriggerSettings,
};

/// A behaviour choice - the picker's value type, one per [`SourceBinding`] variant (+ `Unbound`).
/// Kept in lock-step with `SourceBinding` by [`Behavior::of`]'s exhaustive match: adding a
/// `SourceBinding` variant is a compile error until it's handled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behavior {
    Button,
    ButtonPad,
    Joystick,
    DirectionalPad,
    AsMouse,
    JoystickMouse,
    GyroToMouse,
    Trigger,
    /// **No map entry.** On a set = "None"; on a layer = "Inherited" (falls through to the base).
    Unbound,
    /// An explicit `SourceBinding::None` - layer-only picker choice "Disabled" (nullifies the base).
    Disabled,
}

impl Behavior {
    /// The label shown in the behaviour picker (the set label; the caller relabels `Unbound` ->
    /// "Inherited" on a layer, since it knows the context).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Behavior::Button => "Button",
            Behavior::ButtonPad => "Button Pad",
            Behavior::Joystick => "Joystick",
            Behavior::DirectionalPad => "Directional Pad",
            Behavior::AsMouse => "As Mouse",
            Behavior::JoystickMouse => "Joystick Mouse",
            Behavior::GyroToMouse => "Gyro to Mouse",
            Behavior::Trigger => "Trigger",
            Behavior::Unbound => "None",
            Behavior::Disabled => "Disabled",
        }
    }

    /// The behaviours offered for a source kind, in picker order. On a **layer** a `Disabled`
    /// (explicit `None`) choice comes next; `Unbound` (no entry = "Inherited") is always last.
    pub(crate) fn valid_for(kind: SourceKind, on_layer: bool) -> Vec<Behavior> {
        use Behavior::*;
        let mut v = match kind {
            SourceKind::Button => vec![Button],
            SourceKind::ButtonGroup => vec![ButtonPad],
            SourceKind::Pad => vec![Joystick, DirectionalPad, AsMouse],
            SourceKind::Stick => vec![Joystick, DirectionalPad, JoystickMouse],
            SourceKind::Trigger => vec![Trigger],
            SourceKind::Gyro => vec![GyroToMouse],
        };
        if on_layer {
            v.push(Disabled);
        }
        v.push(Unbound);
        v
    }

    /// Which behaviour a binding currently is. Exhaustive over `SourceBinding` - the drift guard. An
    /// explicit `SourceBinding::None` reads as `Disabled`; a *missing* entry reads as `Unbound` (the
    /// caller supplies that, since `of` only sees present bindings).
    pub(crate) fn of(binding: &SourceBinding) -> Behavior {
        match binding {
            SourceBinding::Button { .. } => Behavior::Button,
            SourceBinding::ButtonPad { .. } => Behavior::ButtonPad,
            SourceBinding::Joystick { .. } => Behavior::Joystick,
            SourceBinding::DirectionalPad { .. } => Behavior::DirectionalPad,
            SourceBinding::AsMouse { .. } => Behavior::AsMouse,
            SourceBinding::JoystickMouse { .. } => Behavior::JoystickMouse,
            SourceBinding::GyroToMouse { .. } => Behavior::GyroToMouse,
            SourceBinding::Trigger { .. } => Behavior::Trigger,
            SourceBinding::None => Behavior::Disabled,
        }
    }

    /// Whether this behaviour has a per-behaviour **settings page** - i.e. carries a settings struct
    /// (the analog/rich behaviours). `Button`/`ButtonPad` have none (nor do the pseudo-behaviours),
    /// so their behaviour-row gear is disabled. NOTE: this governs only the **behaviour-row** gear;
    /// a plain Button's own gear is a command context menu, a separate path this doesn't touch.
    pub(crate) fn has_settings(self) -> bool {
        matches!(
            self,
            Behavior::Joystick
                | Behavior::DirectionalPad
                | Behavior::AsMouse
                | Behavior::JoystickMouse
                | Behavior::GyroToMouse
                | Behavior::Trigger
        )
    }

    /// Build a fresh binding of this behaviour for `input`. The only per-input tuning is **side-aware
    /// output** (left/right stick/trigger; left pad/stick -> scroll, right -> cursor) and the
    /// **trackpad DirectionalPad** gated on that pad's click; every value field is config's neutral
    /// `Default` (sensitivity 1.0, acceleration 0.0, deadzones 0.0, smoothing off). This is the
    /// *authoring* default - distinct from serde's `Default`, which is the on-disk fallback.
    #[allow(dead_code)] // wired when the behaviour picker constructs a binding
    pub(crate) fn default_binding(self, input: &InputSource) -> SourceBinding {
        match self {
            Behavior::Button => SourceBinding::Button { commands: Vec::new() },
            Behavior::ButtonPad => SourceBinding::ButtonPad {
                up: Vec::new(),
                down: Vec::new(),
                left: Vec::new(),
                right: Vec::new(),
            },
            Behavior::Joystick => SourceBinding::Joystick {
                // Drive the gamepad stick on the input's own side (left input -> left stick).
                // Deadzone defaults to 0.0 (neutral) like every other behaviour - tune per profile.
                settings: JoystickSettings { output: stick_output(input), ..Default::default() },
                outer_ring: Vec::new(),
            },
            Behavior::DirectionalPad => SourceBinding::DirectionalPad {
                // On a trackpad, gate directions on that pad's click so a resting finger doesn't fire
                // one (a stick self-centres, so it stays always-on).
                settings: DirectionalPadSettings {
                    activation: pad_click_activation(input),
                    ..Default::default()
                },
                up: Vec::new(),
                down: Vec::new(),
                left: Vec::new(),
                right: Vec::new(),
                outer_ring: Vec::new(),
            },
            // Pad-velocity mouse. Left side scrolls, right side moves the cursor; everything else
            // (sensitivity 1.0, acceleration 0.0, smoothing off) is config's neutral default.
            Behavior::AsMouse => SourceBinding::AsMouse {
                settings: AsMouseSettings { output: mouse_output(input), ..Default::default() },
            },
            Behavior::JoystickMouse => SourceBinding::JoystickMouse {
                settings: JoystickMouseSettings { output: mouse_output(input), ..Default::default() },
            },
            // Gyro-velocity mouse - neutral value defaults (sensitivity 1.0, acceleration 0.0,
            // smoothing off), cursor output; but **hold-to-enable** so gyro stays off until the user
            // adds a gater (the usual hold-to-aim pattern), rather than always-on.
            Behavior::GyroToMouse => SourceBinding::GyroToMouse {
                settings: GyroToMouseSettings {
                    activation: Activation { mode: ActivationMode::HoldToEnable, ..Default::default() },
                    ..Default::default()
                },
            },
            // Drive the gamepad trigger on the input's own side (left trigger -> left trigger axis).
            Behavior::Trigger => SourceBinding::Trigger {
                settings: TriggerSettings { output: trigger_output(input), ..Default::default() },
                soft_pull: Vec::new(),
            },
            // `Unbound` is "no entry" so this is never inserted (SetBehavior removes instead);
            // `Disabled` is the explicit nullifying `None`.
            Behavior::Unbound | Behavior::Disabled => SourceBinding::None,
        }
    }
}

// --- side-aware authoring defaults ----------------------------------------------------------

/// Default stick-axis output for a Joystick binding - the input's own side.
fn stick_output(input: &InputSource) -> StickOutput {
    match input.side() {
        Side::Left => StickOutput::Left,
        Side::Right => StickOutput::Right,
    }
}

/// Default trigger-axis output for a Trigger binding - the input's own side.
fn trigger_output(input: &InputSource) -> TriggerOutput {
    match input.side() {
        Side::Left => TriggerOutput::Left,
        Side::Right => TriggerOutput::Right,
    }
}

/// Default mouse output for a pad/stick mouse behaviour: left side -> scroll, right side -> cursor.
fn mouse_output(input: &InputSource) -> MouseOutput {
    match input.side() {
        Side::Left => MouseOutput::Scroll,
        Side::Right => MouseOutput::Cursor,
    }
}

/// Default activation for a DirectionalPad: on a trackpad, hold-to-enable gated on that pad's own
/// click; anything else (a stick) stays always-on.
fn pad_click_activation(input: &InputSource) -> Activation {
    let click = match input {
        InputSource::LeftPad => Some(vocab_hid::Button::LPadPress),
        InputSource::RightPad => Some(vocab_hid::Button::RPadPress),
        _ => None,
    };
    match click {
        Some(c) => Activation { mode: ActivationMode::HoldToEnable, gaters: vec![c] },
        None => Activation::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: &[SourceKind] = &[
        SourceKind::Button,
        SourceKind::ButtonGroup,
        SourceKind::Pad,
        SourceKind::Stick,
        SourceKind::Trigger,
        SourceKind::Gyro,
    ];

    fn sample_input(kind: SourceKind) -> InputSource {
        match kind {
            SourceKind::Button => InputSource::LeftBumper,
            SourceKind::ButtonGroup => InputSource::FaceButtons,
            SourceKind::Pad => InputSource::LeftPad,
            SourceKind::Stick => InputSource::LeftStick,
            SourceKind::Trigger => InputSource::LeftTrigger,
            SourceKind::Gyro => InputSource::Gyro,
        }
    }

    #[test]
    fn every_offered_behaviour_builds_a_valid_binding_that_round_trips() {
        for kind in KINDS {
            let kind = kind.clone();
            let input = sample_input(kind.clone());
            // `on_layer=true` offers every choice including `Disabled`; skip the `Unbound` sentinel
            // (it's "no entry", not a constructable binding).
            for b in Behavior::valid_for(kind.clone(), true) {
                if b == Behavior::Unbound {
                    continue;
                }
                let binding = b.default_binding(&input);
                assert_eq!(Behavior::of(&binding), b, "{b:?} should read back as itself");
                assert!(binding.is_valid_for(&kind), "{b:?} should be valid for {kind:?}");
            }
        }
    }
}
