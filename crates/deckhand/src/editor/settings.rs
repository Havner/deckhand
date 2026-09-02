//! The behaviour-settings **edit path** - the state-side twin of [`crate::view::editor`]'s settings
//! form. The view builds the per-behaviour settings page and emits one [`SettingEdit`]; this applies
//! that edit to whichever field the current binding carries, via small `&mut` accessors over
//! [`SourceBinding`] shared across every behaviour that has that field. The per-type outputs
//! (Stick/Trigger/Mouse) and the singletons (layout/space/soft_pull/anti_deadzone) resolve inline;
//! everything shared goes through an accessor so the logic isn't duplicated.

use config::{
    Acceleration, Activation, Axis, Curve, Deadzone, Invert, OneEuroFilter, OuterRing, Rotation,
    Sensitivity, SourceBinding,
};

use super::SettingEdit;

/// Apply one behaviour-settings field edit to a binding. A no-op when the binding's behaviour lacks
/// that field (the page only ever offers a behaviour's real fields, so that never happens in
/// practice - but it keeps the mapping total and safe).
pub(super) fn apply_setting(binding: &mut SourceBinding, edit: SettingEdit) {
    use SettingEdit as E;
    use SourceBinding as B;
    match edit {
        E::StickOutput(o) => {
            if let B::Joystick { settings, .. } = binding {
                settings.output = o;
            }
        }
        E::TriggerOutput(o) => {
            if let B::Trigger { settings, .. } = binding {
                settings.output = o;
            }
        }
        E::MouseOutput(o) => match binding {
            B::AsMouse { settings } => settings.output = o,
            B::JoystickMouse { settings } => settings.output = o,
            B::GyroToMouse { settings } => settings.output = o,
            _ => {}
        },
        E::Axis(a) => {
            if let Some(x) = axis_mut(binding) {
                *x = a;
            }
        }
        E::OuterRing(r) => {
            if let Some(x) = outer_ring_mut(binding) {
                x.radius = r;
            }
        }
        E::SoftPull(t) => {
            if let B::Trigger { settings, .. } = binding {
                settings.soft_pull.threshold = t;
            }
        }
        E::Layout(l) => {
            if let B::DirectionalPad { settings, .. } = binding {
                settings.layout = l;
            }
        }
        E::Space(s) => {
            if let B::GyroToMouse { settings } = binding {
                settings.space = s;
            }
        }
        E::SensitivityX(v) => {
            if let Some(s) = sensitivity_mut(binding) {
                s.x = v;
            }
        }
        E::SensitivityY(v) => {
            if let Some(s) = sensitivity_mut(binding) {
                s.y = v;
            }
        }
        E::Curve(c) => {
            if let Some(x) = curve_mut(binding) {
                *x = c;
            }
        }
        E::Acceleration(f) => {
            if let Some(a) = acceleration_mut(binding) {
                a.factor = f;
            }
        }
        E::SmoothingEnabled(on) => {
            if let Some(s) = smoothing_mut(binding) {
                *s = on.then(OneEuroFilter::default);
            }
        }
        E::SmoothingMinCutoff(v) => {
            if let Some(f) = smoothing_mut(binding).and_then(|s| s.as_mut()) {
                f.min_cutoff = v;
            }
        }
        E::SmoothingBeta(v) => {
            if let Some(f) = smoothing_mut(binding).and_then(|s| s.as_mut()) {
                f.beta = v;
            }
        }
        E::Deadzone(v) => {
            if let Some(d) = deadzone_mut(binding) {
                d.inner = v;
            }
        }
        E::AntiDeadzone(v) => {
            if let B::Joystick { settings, .. } = binding {
                settings.anti_deadzone.amount = v;
            }
        }
        E::InvertX(b) => {
            if let Some(i) = invert_mut(binding) {
                i.x = b;
            }
        }
        E::InvertY(b) => {
            if let Some(i) = invert_mut(binding) {
                i.y = b;
            }
        }
        E::Rotation(d) => {
            if let Some(r) = rotation_mut(binding) {
                r.degrees = d;
            }
        }
        E::ActivationMode(m) => {
            if let Some(a) = activation_mut(binding) {
                a.mode = m;
            }
        }
        E::AddGater(b) => {
            if let Some(a) = activation_mut(binding)
                && !a.gaters.contains(&b)
            {
                a.gaters.push(b);
            }
        }
        E::RemoveGater(i) => {
            if let Some(a) = activation_mut(binding)
                && i < a.gaters.len()
            {
                a.gaters.remove(i);
            }
        }
    }
}

fn deadzone_mut(b: &mut SourceBinding) -> Option<&mut Deadzone> {
    use SourceBinding as B;
    match b {
        B::Joystick { settings, .. } => Some(&mut settings.deadzone),
        B::DirectionalPad { settings, .. } => Some(&mut settings.deadzone),
        B::JoystickMouse { settings } => Some(&mut settings.deadzone),
        B::GyroToMouse { settings } => Some(&mut settings.deadzone),
        B::Trigger { settings, .. } => Some(&mut settings.deadzone),
        _ => None,
    }
}

fn sensitivity_mut(b: &mut SourceBinding) -> Option<&mut Sensitivity> {
    use SourceBinding as B;
    match b {
        B::AsMouse { settings } => Some(&mut settings.sensitivity),
        B::JoystickMouse { settings } => Some(&mut settings.sensitivity),
        B::GyroToMouse { settings } => Some(&mut settings.sensitivity),
        _ => None,
    }
}

fn curve_mut(b: &mut SourceBinding) -> Option<&mut Curve> {
    use SourceBinding as B;
    match b {
        B::Joystick { settings, .. } => Some(&mut settings.curve),
        B::JoystickMouse { settings } => Some(&mut settings.curve),
        B::Trigger { settings, .. } => Some(&mut settings.curve),
        _ => None,
    }
}

fn acceleration_mut(b: &mut SourceBinding) -> Option<&mut Acceleration> {
    use SourceBinding as B;
    match b {
        B::AsMouse { settings } => Some(&mut settings.acceleration),
        B::GyroToMouse { settings } => Some(&mut settings.acceleration),
        _ => None,
    }
}

fn smoothing_mut(b: &mut SourceBinding) -> Option<&mut Option<OneEuroFilter>> {
    use SourceBinding as B;
    match b {
        B::AsMouse { settings } => Some(&mut settings.smoothing),
        B::GyroToMouse { settings } => Some(&mut settings.smoothing),
        _ => None,
    }
}

fn invert_mut(b: &mut SourceBinding) -> Option<&mut Invert> {
    use SourceBinding as B;
    match b {
        B::Joystick { settings, .. } => Some(&mut settings.invert),
        B::JoystickMouse { settings } => Some(&mut settings.invert),
        B::AsMouse { settings } => Some(&mut settings.invert),
        B::GyroToMouse { settings } => Some(&mut settings.invert),
        _ => None,
    }
}

fn rotation_mut(b: &mut SourceBinding) -> Option<&mut Rotation> {
    use SourceBinding as B;
    match b {
        B::Joystick { settings, .. } => Some(&mut settings.rotation),
        B::DirectionalPad { settings, .. } => Some(&mut settings.rotation),
        B::JoystickMouse { settings } => Some(&mut settings.rotation),
        B::AsMouse { settings } => Some(&mut settings.rotation),
        B::GyroToMouse { settings } => Some(&mut settings.rotation),
        _ => None,
    }
}

fn activation_mut(b: &mut SourceBinding) -> Option<&mut Activation> {
    use SourceBinding as B;
    match b {
        B::Joystick { settings, .. } => Some(&mut settings.activation),
        B::DirectionalPad { settings, .. } => Some(&mut settings.activation),
        B::AsMouse { settings } => Some(&mut settings.activation),
        B::JoystickMouse { settings } => Some(&mut settings.activation),
        B::GyroToMouse { settings } => Some(&mut settings.activation),
        _ => None,
    }
}

fn outer_ring_mut(b: &mut SourceBinding) -> Option<&mut OuterRing> {
    use SourceBinding as B;
    match b {
        B::Joystick { settings, .. } => Some(&mut settings.outer_ring),
        B::DirectionalPad { settings, .. } => Some(&mut settings.outer_ring),
        _ => None,
    }
}

fn axis_mut(b: &mut SourceBinding) -> Option<&mut Axis> {
    use SourceBinding as B;
    match b {
        B::Joystick { settings, .. } => Some(&mut settings.axis),
        B::AsMouse { settings } => Some(&mut settings.axis),
        B::JoystickMouse { settings } => Some(&mut settings.axis),
        B::GyroToMouse { settings } => Some(&mut settings.axis),
        _ => None,
    }
}
