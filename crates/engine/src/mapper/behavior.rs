//! Behavior evaluation — a source's binding → gamepad axes + virtual-button levels (S6).
//!
//! Each rich source (Pad/Stick/Trigger/ButtonGroup) runs a behavior that produces an **output
//! target** (a gamepad stick/trigger axis chosen in settings) and/or **virtual buttons**
//! (soft-pull, outer-ring, dpad directions, button-pad members) whose commands run through
//! [`eval_commands`] exactly like a physical button. Activation **gaters** are physical buttons
//! read from the raw frame up front (PLAN §4 evaluation order: gates resolve before behaviors,
//! keeping the tick one acyclic pass).
//!
//! **S6a — the non-relative producers below** (pure functions of the current frame). The three
//! relative mouse behaviors (`AsMouse`/`JoystickMouse`/`GyroToMouse`) are stubbed here and land
//! in **S6b**: they integrate deltas over the injected clock and the previous frame, so they
//! need `Tick` + prev state the level-based behaviors don't.
//!
//! Tuning math (deadzone rescale, curve, anti-deadzone, dpad sectoring) is straightforward and
//! **golden-tested for logic**; absolute axis sign / up-vs-down parity is confirmed against the
//! bridge at HW validation (S10).

use config::{
    Activation, ActivationMode, Curve, DirectionalPadSettings, DpadLayout, InputSource,
    JoystickSettings, StickOutput, TriggerOutput, TriggerSettings,
};
use steam_hid::Vec2;
use vocab::GamepadAxis;

use super::command::eval_commands;
use super::reconcile::DesiredLevels;
use crate::logical::{Dir, LogicalFrame};
use crate::program::{CompiledBinding, CompiledCommand};

/// Evaluate one resolved binding into the desired output levels.
pub(super) fn eval_binding(
    binding: &CompiledBinding,
    source: &InputSource,
    frame: &LogicalFrame,
    desired: &mut DesiredLevels,
) {
    match binding {
        CompiledBinding::Button { commands } => {
            eval_commands(commands, frame.button(source), desired);
        }
        CompiledBinding::ButtonPad { up, down, left, right } => {
            eval_commands(up, frame.group_member(source, &Dir::Up), desired);
            eval_commands(down, frame.group_member(source, &Dir::Down), desired);
            eval_commands(left, frame.group_member(source, &Dir::Left), desired);
            eval_commands(right, frame.group_member(source, &Dir::Right), desired);
        }
        CompiledBinding::Joystick { settings, outer_ring } => {
            eval_joystick(source, settings, outer_ring, frame, desired);
        }
        CompiledBinding::DirectionalPad { settings, up, down, left, right, outer_ring } => {
            eval_directional_pad(source, settings, up, down, left, right, outer_ring, frame, desired);
        }
        CompiledBinding::Trigger { settings, soft_pull } => {
            eval_trigger(source, settings, soft_pull, frame, desired);
        }
        // Relative mouse behaviors — S6b (integrate over the injected clock + previous frame).
        CompiledBinding::AsMouse { .. }
        | CompiledBinding::JoystickMouse { .. }
        | CompiledBinding::GyroToMouse { .. } => {}
    }
}

// --- Joystick (Pad/Stick → gamepad stick + outer-ring button) ---------------------------

fn eval_joystick(
    source: &InputSource,
    s: &JoystickSettings,
    outer_ring: &[CompiledCommand],
    frame: &LogicalFrame,
    desired: &mut DesiredLevels,
) {
    if !is_active(&s.activation, frame) {
        return;
    }
    let Some(pos) = source_pos(source, frame) else { return };
    let (ox, oy) = process_joystick(&pos, s);
    let (ax, ay) = match s.output {
        StickOutput::Left => (GamepadAxis::LeftStickX, GamepadAxis::LeftStickY),
        StickOutput::Right => (GamepadAxis::RightStickX, GamepadAxis::RightStickY),
    };
    desired.set_axis(ax, ox);
    desired.set_axis(ay, oy);
    // Outer ring fires on raw input deflection, not the processed output.
    eval_commands(outer_ring, magnitude(&pos) >= s.outer_ring.radius, desired);
}

/// Deadzone-rescale → curve → anti-deadzone, direction preserved, per-axis invert.
fn process_joystick(pos: &Vec2, s: &JoystickSettings) -> (f32, f32) {
    let (rx, ry) = rotate(pos.x, pos.y, s.rotation.degrees);
    let mag = (rx * rx + ry * ry).sqrt();
    if mag <= s.deadzone.inner || mag < 1e-6 {
        return (0.0, 0.0);
    }
    let scaled = ((mag - s.deadzone.inner) / (1.0 - s.deadzone.inner)).clamp(0.0, 1.0);
    let curved = apply_curve(scaled, &s.curve);
    let ad = s.anti_deadzone.amount;
    let out_mag = if curved > 0.0 { ad + (1.0 - ad) * curved } else { 0.0 };
    let (ux, uy) = (rx / mag, ry / mag);
    let mut ox = (ux * out_mag).clamp(-1.0, 1.0);
    let mut oy = (uy * out_mag).clamp(-1.0, 1.0);
    if s.invert.x {
        ox = -ox;
    }
    if s.invert.y {
        oy = -oy;
    }
    (ox, oy)
}

// --- DirectionalPad (Pad/Stick → 4 direction + outer-ring buttons) ----------------------

#[allow(clippy::too_many_arguments)] // one param per virtual button; a struct would not help.
fn eval_directional_pad(
    source: &InputSource,
    s: &DirectionalPadSettings,
    up: &[CompiledCommand],
    down: &[CompiledCommand],
    left: &[CompiledCommand],
    right: &[CompiledCommand],
    outer_ring: &[CompiledCommand],
    frame: &LogicalFrame,
    desired: &mut DesiredLevels,
) {
    if !is_active(&s.activation, frame) {
        return;
    }
    let Some(pos) = source_pos(source, frame) else { return };
    let mag = magnitude(&pos);
    if mag >= s.deadzone.inner && mag > 1e-6 {
        let (rx, ry) = rotate(pos.x, pos.y, s.rotation.degrees);
        let [u, d, l, r] = dpad_dirs(rx, ry, &s.layout);
        eval_commands(up, u, desired);
        eval_commands(down, d, desired);
        eval_commands(left, l, desired);
        eval_commands(right, r, desired);
    }
    eval_commands(outer_ring, mag >= s.outer_ring.radius, desired);
}

/// Which of `[up, down, left, right]` fire. Convention: `+x` = Right, `+y` = Up (final
/// up/down parity confirmed at HW validation). 8-way diagonals fire two adjacent cardinals.
fn dpad_dirs(rx: f32, ry: f32, layout: &DpadLayout) -> [bool; 4] {
    let ang = (ry.atan2(rx).to_degrees() + 360.0) % 360.0;
    let mut d = [false; 4]; // [up, down, left, right]
    match layout {
        DpadLayout::FourWay => {
            if (45.0..135.0).contains(&ang) {
                d[0] = true;
            } else if (135.0..225.0).contains(&ang) {
                d[2] = true;
            } else if (225.0..315.0).contains(&ang) {
                d[1] = true;
            } else {
                d[3] = true;
            }
        }
        DpadLayout::EightWay => match (((ang + 22.5) % 360.0) / 45.0) as usize {
            0 => d[3] = true,                    // E
            1 => (d[0], d[3]) = (true, true),    // NE
            2 => d[0] = true,                    // N
            3 => (d[0], d[2]) = (true, true),    // NW
            4 => d[2] = true,                    // W
            5 => (d[1], d[2]) = (true, true),    // SW
            6 => d[1] = true,                    // S
            _ => (d[1], d[3]) = (true, true),    // SE
        },
    }
    d
}

// --- Trigger (analog output + soft-pull button) -----------------------------------------

fn eval_trigger(
    source: &InputSource,
    s: &TriggerSettings,
    soft_pull: &[CompiledCommand],
    frame: &LogicalFrame,
    desired: &mut DesiredLevels,
) {
    let pull = frame.trigger(source); // 0.0..=1.0
    let axis = match s.output {
        TriggerOutput::Left => GamepadAxis::LeftTrigger,
        TriggerOutput::Right => GamepadAxis::RightTrigger,
    };
    desired.set_axis(axis, process_trigger(pull, s));
    // Soft-pull virtual button fires on the raw analog pull vs its threshold.
    eval_commands(soft_pull, pull >= s.soft_pull.threshold, desired);
}

fn process_trigger(pull: f32, s: &TriggerSettings) -> f32 {
    if pull <= s.deadzone.inner {
        return 0.0;
    }
    let scaled = ((pull - s.deadzone.inner) / (1.0 - s.deadzone.inner)).clamp(0.0, 1.0);
    apply_curve(scaled, &s.curve)
}

// --- shared helpers ---------------------------------------------------------------------

/// The 2D reading for a source, or `None` when it's a **pad that isn't touched** (a
/// pad-as-joystick/dpad must not deflect from a stale resting position). Sticks always read.
fn source_pos(source: &InputSource, frame: &LogicalFrame) -> Option<Vec2> {
    if let Some(pad) = frame.pad(source) {
        return pad.touched.then(|| pad.pos.clone());
    }
    Some(frame.pos(source))
}

fn magnitude(v: &Vec2) -> f32 {
    (v.x * v.x + v.y * v.y).sqrt()
}

fn rotate(x: f32, y: f32, degrees: f32) -> (f32, f32) {
    if degrees == 0.0 {
        return (x, y);
    }
    let (s, c) = degrees.to_radians().sin_cos();
    (x * c - y * s, x * s + y * c)
}

fn apply_curve(v: f32, curve: &Curve) -> f32 {
    match curve {
        Curve::Linear => v,
        Curve::Power(e) => v.powf(*e),
    }
}

/// Whether a behavior is live given its activation gaters (physical buttons, OR-combined).
/// `HoldToDisable` + no gater = always (the default); `HoldToEnable` + no gater = never.
fn is_active(a: &Activation, frame: &LogicalFrame) -> bool {
    let gater_held = a.gaters.iter().any(|g| frame.button(g));
    match a.mode {
        ActivationMode::HoldToEnable => gater_held,
        ActivationMode::HoldToDisable => !gater_held,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{CompiledAction, CompiledCommand};
    use config::{
        Activator, CommandSettings, Deadzone, Invert, OuterRing, SoftPull,
    };
    use steam_hid::{Buttons, ControllerState, TrackPad};
    use vocab::Key;

    fn regular(key: Key) -> Vec<CompiledCommand> {
        vec![CompiledCommand {
            activator: Activator::Regular,
            actions: vec![CompiledAction::Key(key)],
            settings: CommandSettings::default(),
        }]
    }

    fn desired_of(binding: &CompiledBinding, source: &InputSource, state: ControllerState) -> DesiredLevels {
        let frame = LogicalFrame::new(state);
        let mut d = DesiredLevels::default();
        eval_binding(binding, source, &frame, &mut d);
        d
    }

    #[test]
    fn button_pad_members_fire_by_group_bit() {
        let binding = CompiledBinding::ButtonPad {
            up: regular(Key::W),
            down: regular(Key::S),
            left: regular(Key::A),
            right: regular(Key::D),
        };
        // FaceButtons: up = Y bit.
        let d = desired_of(
            &binding,
            &InputSource::FaceButtons,
            ControllerState { buttons: Buttons::Y, ..Default::default() },
        );
        let mut want = DesiredLevels::default();
        want.press_key(Key::W);
        assert_eq!(d, want);
    }

    #[test]
    fn joystick_deadzone_and_output_axis() {
        let binding = CompiledBinding::Joystick {
            settings: JoystickSettings {
                deadzone: Deadzone { inner: 0.2 },
                ..Default::default()
            },
            outer_ring: Vec::new(),
        };
        // Inside the deadzone → neutral (both axes 0.0).
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 0.1, y: 0.0 }, ..Default::default() },
        );
        let mut neutral = DesiredLevels::default();
        neutral.set_axis(GamepadAxis::LeftStickX, 0.0);
        neutral.set_axis(GamepadAxis::LeftStickY, 0.0);
        assert_eq!(d, neutral);

        // Full right deflection → near +1 on X (default output = Left stick).
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 1.0, y: 0.0 }, ..Default::default() },
        );
        assert!(d.axis(&GamepadAxis::LeftStickX).unwrap() > 0.99);
        assert_eq!(d.axis(&GamepadAxis::LeftStickY), Some(0.0));
    }

    #[test]
    fn joystick_invert_x() {
        let binding = CompiledBinding::Joystick {
            settings: JoystickSettings { invert: Invert { x: true, y: false }, ..Default::default() },
            outer_ring: Vec::new(),
        };
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 1.0, y: 0.0 }, ..Default::default() },
        );
        assert!(d.axis(&GamepadAxis::LeftStickX).unwrap() < -0.99);
    }

    #[test]
    fn joystick_outer_ring_fires_past_radius() {
        let binding = CompiledBinding::Joystick {
            settings: JoystickSettings { outer_ring: OuterRing { radius: 0.9 }, ..Default::default() },
            outer_ring: regular(Key::Space),
        };
        // Below the ring radius → no ring button.
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 0.5, y: 0.0 }, ..Default::default() },
        );
        assert!(!d.has_key(&Key::Space));
        // Past it → fires.
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 1.0, y: 0.0 }, ..Default::default() },
        );
        assert!(d.has_key(&Key::Space));
    }

    #[test]
    fn directional_pad_four_way_cardinal() {
        let binding = CompiledBinding::DirectionalPad {
            settings: DirectionalPadSettings { deadzone: Deadzone { inner: 0.2 }, ..Default::default() },
            up: regular(Key::Up),
            down: regular(Key::Down),
            left: regular(Key::Left),
            right: regular(Key::Right),
            outer_ring: Vec::new(),
        };
        // +y = Up.
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 0.0, y: 1.0 }, ..Default::default() },
        );
        assert!(d.has_key(&Key::Up));
        assert!(!d.has_key(&Key::Down) && !d.has_key(&Key::Left) && !d.has_key(&Key::Right));

        // Inside the deadzone → nothing.
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 0.1, y: 0.05 }, ..Default::default() },
        );
        assert!(!d.has_key(&Key::Up) && !d.has_key(&Key::Down));
    }

    #[test]
    fn directional_pad_eight_way_diagonal_fires_two() {
        let binding = CompiledBinding::DirectionalPad {
            settings: DirectionalPadSettings {
                deadzone: Deadzone { inner: 0.2 },
                layout: DpadLayout::EightWay,
                ..Default::default()
            },
            up: regular(Key::Up),
            down: regular(Key::Down),
            left: regular(Key::Left),
            right: regular(Key::Right),
            outer_ring: Vec::new(),
        };
        // Up-right diagonal → Up + Right.
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 0.7, y: 0.7 }, ..Default::default() },
        );
        assert!(d.has_key(&Key::Up) && d.has_key(&Key::Right));
        assert!(!d.has_key(&Key::Down) && !d.has_key(&Key::Left));
    }

    #[test]
    fn trigger_axis_and_soft_pull() {
        let binding = CompiledBinding::Trigger {
            settings: TriggerSettings { soft_pull: SoftPull { threshold: 0.5 }, ..Default::default() },
            soft_pull: regular(Key::F),
        };
        // Light pull below threshold → axis set, no soft-pull button.
        let d = desired_of(
            &binding,
            &InputSource::LeftTrigger,
            ControllerState { left_trigger: 0.3, ..Default::default() },
        );
        assert_eq!(d.axis(&GamepadAxis::LeftTrigger), Some(0.3));
        assert!(!d.has_key(&Key::F));
        // Past threshold → soft-pull fires.
        let d = desired_of(
            &binding,
            &InputSource::LeftTrigger,
            ControllerState { left_trigger: 0.8, ..Default::default() },
        );
        assert!(d.has_key(&Key::F));
    }

    #[test]
    fn pad_joystick_ignored_when_not_touched() {
        let binding = CompiledBinding::Joystick {
            settings: JoystickSettings::default(),
            outer_ring: Vec::new(),
        };
        // Untouched pad with a stale position → no deflection at all.
        let d = desired_of(
            &binding,
            &InputSource::LeftPad,
            ControllerState {
                left_pad: TrackPad { pos: Vec2 { x: 0.9, y: 0.0 }, pressure: 0.0, touched: false },
                ..Default::default()
            },
        );
        assert_eq!(d, DesiredLevels::default());
    }

    #[test]
    fn activation_gater_disables_behavior() {
        // HoldToEnable with L4 as gater: no output unless L4 is held.
        let binding = CompiledBinding::Joystick {
            settings: JoystickSettings {
                activation: Activation {
                    mode: ActivationMode::HoldToEnable,
                    gaters: vec![InputSource::LeftGrip],
                },
                ..Default::default()
            },
            outer_ring: Vec::new(),
        };
        // Gater released → behavior off (no axes even at full deflection).
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 1.0, y: 0.0 }, ..Default::default() },
        );
        assert_eq!(d, DesiredLevels::default());
        // Gater held → behavior live.
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState {
                buttons: Buttons::L4,
                left_stick: Vec2 { x: 1.0, y: 0.0 },
                ..Default::default()
            },
        );
        assert!(d.axis(&GamepadAxis::LeftStickX).unwrap() > 0.99);
    }
}
