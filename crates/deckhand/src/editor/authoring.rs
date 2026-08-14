//! Behaviour taxonomy + authoring defaults for the profile editor.
//!
//! [`Behavior`] is a lightweight tag mirroring the [`SourceBinding`] variants (plus an explicit
//! `Unbound`). It's the value type the behaviour picker selects, and the seam for constructing a
//! fresh binding when the user picks a behaviour: [`Behavior::default_binding`] builds the right
//! variant with UI-authored **starting values** — tuned per behaviour, and per input where it
//! matters (e.g. a stick's larger inner deadzone). `config` owns the data model + neutral `Default`;
//! these *authoring* defaults live here in the UI, since no non-UI path ever needs them (this
//! session's decision).
//!
//! UI **ranges** (slider min/max) deliberately aren't here: they vary only by behaviour, so they'll
//! live inline in each behaviour's settings view — no lookup table needed.
//!
//! This is the initial scaffold: the taxonomy is wired into the behaviour picker; `of` /
//! `default_binding` are ready for when the editor reflects and constructs bindings.

use config::{
    Acceleration, AsMouseSettings, Deadzone, GyroToMouseSettings, InputSource, JoystickSettings,
    OneEuroFilter, Sensitivity, SourceBinding, SourceKind,
};

/// A behaviour choice — the picker's value type, one per [`SourceBinding`] variant (+ `Unbound`).
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
    /// An explicit `SourceBinding::None` — layer-only picker choice "Disabled" (nullifies the base).
    Disabled,
}

impl Behavior {
    /// The label shown in the behaviour picker (the set label; the caller relabels `Unbound` →
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

    /// Which behaviour a binding currently is. Exhaustive over `SourceBinding` — the drift guard. An
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

    /// Whether this is a real behaviour (settings/virtual buttons) vs a pseudo (`Unbound`/`Disabled`).
    pub(crate) fn is_real(self) -> bool {
        !matches!(self, Behavior::Unbound | Behavior::Disabled)
    }

    /// Build a fresh binding of this behaviour for `input`, with UI-authored starting values. The
    /// tuned values differ by behaviour (and, for `Joystick`, by pad-vs-stick); every other field
    /// is config's neutral `Default`. This is the *authoring* default — distinct from serde's
    /// `Default`, which stays the minimal/neutral on-disk fallback.
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
                settings: JoystickSettings {
                    // A stick rests off-centre (mechanical jitter), so it wants a larger inner
                    // deadzone than an absolute-touch pad — the one genuinely per-input default.
                    deadzone: Deadzone {
                        inner: if input.kind() == SourceKind::Stick { 0.15 } else { 0.0 },
                    },
                    ..Default::default()
                },
                outer_ring: Vec::new(),
            },
            Behavior::DirectionalPad => SourceBinding::DirectionalPad {
                settings: Default::default(),
                up: Vec::new(),
                down: Vec::new(),
                left: Vec::new(),
                right: Vec::new(),
                outer_ring: Vec::new(),
            },
            // Pad-velocity mouse — sensitivity/accel/1€ tuned for pad-units/s (see the example
            // profiles; the difference from gyro is a per-behaviour thing, absorbed here).
            Behavior::AsMouse => SourceBinding::AsMouse {
                settings: AsMouseSettings {
                    sensitivity: Sensitivity { x: 0.5, y: 0.5 },
                    acceleration: Acceleration { factor: 0.05 },
                    smoothing: Some(OneEuroFilter { min_cutoff: 3.0, beta: 0.5 }),
                    ..Default::default()
                },
            },
            Behavior::JoystickMouse => SourceBinding::JoystickMouse { settings: Default::default() },
            // Gyro-velocity mouse — the same knobs tuned for deg/s (smaller accel, lower cutoff).
            Behavior::GyroToMouse => SourceBinding::GyroToMouse {
                settings: GyroToMouseSettings {
                    sensitivity: Sensitivity { x: 0.5, y: 0.5 },
                    acceleration: Acceleration { factor: 0.02 },
                    smoothing: Some(OneEuroFilter { min_cutoff: 1.0, beta: 0.5 }),
                    ..Default::default()
                },
            },
            Behavior::Trigger => {
                SourceBinding::Trigger { settings: Default::default(), soft_pull: Vec::new() }
            }
            // `Unbound` is "no entry" so this is never inserted (SetBehavior removes instead);
            // `Disabled` is the explicit nullifying `None`.
            Behavior::Unbound | Behavior::Disabled => SourceBinding::None,
        }
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

    #[test]
    fn stick_joystick_default_deadzone_exceeds_pad() {
        let inner = |input: InputSource| match Behavior::Joystick.default_binding(&input) {
            SourceBinding::Joystick { settings, .. } => settings.deadzone.inner,
            _ => unreachable!(),
        };
        assert!(inner(InputSource::LeftStick) > inner(InputSource::LeftPad));
    }
}
