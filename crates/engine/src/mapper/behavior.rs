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
    Activation, ActivationMode, AsMouseSettings, Curve, DirectionalPadSettings, DpadLayout,
    GyroToMouseSettings, InputSource, Invert, JoystickMouseSettings, JoystickSettings, MouseOutput,
    Sensitivity, StickOutput, TriggerOutput, TriggerSettings,
};
use steam_hid::Vec2;
use vocab::GamepadAxis;

use super::Tick;
use super::activator::{SlotState, SourceActivators};
use super::command::eval_commands;
use super::layers::{LayerOps, NodeHeld};
use super::reconcile::{DesiredLevels, RelAccum};
use crate::logical::{Dir, LogicalFrame};
use crate::program::{CompiledBinding, CompiledCommand};

/// Raw gyro units per degree/second (steam-hid `GYRO_RES_PER_DPS`, PLAN §1.9).
const GYRO_RES_PER_DPS: f32 = 16.0;
/// Behavior output gains — reasonable starting points; final feel is tuned against the bridge
/// at HW validation (S10). Pixels per normalized-pad-delta / per stick-rate·second / per degree.
const PAD_MOUSE_GAIN: f32 = 400.0;
const JOY_MOUSE_RATE: f32 = 800.0;
const GYRO_MOUSE_GAIN: f32 = 20.0;

/// Per-tick context for behaviors: the current frame, the previous frame (the pad-delta source
/// for `AsMouse`), `dt` in seconds (for the rate-based stick/gyro behaviors), and the injected
/// clock `now` (for activator timing).
pub(super) struct Ctx<'a> {
    pub cur: &'a LogicalFrame,
    pub prev: Option<&'a LogicalFrame>,
    pub dt: f32,
    pub now: Tick,
}

/// Evaluate one resolved binding into the desired output levels and/or relative accumulators.
/// `slots` is this source's activator state; the behavior owns the fixed slot numbering below.
#[allow(clippy::too_many_arguments)] // the pure mapping core threads all its state explicitly.
pub(super) fn eval_binding(
    binding: &CompiledBinding,
    source: &InputSource,
    ctx: &Ctx,
    slots: &mut SourceActivators,
    desired: &mut DesiredLevels,
    rel: &mut RelAccum,
    ops: &mut LayerOps,
) {
    let frame = ctx.cur;
    let now = &ctx.now;
    match binding {
        CompiledBinding::Button { commands } => {
            let node = NodeHeld::Button(source.clone());
            eval_commands(commands, frame.button(source), &node, slots.slot(0), now, desired, ops);
        }
        CompiledBinding::ButtonPad { up, down, left, right } => {
            let member = |dir| NodeHeld::Group(source.clone(), dir);
            eval_commands(up, frame.group_member(source, &Dir::Up), &member(Dir::Up), slots.slot(0), now, desired, ops);
            eval_commands(down, frame.group_member(source, &Dir::Down), &member(Dir::Down), slots.slot(1), now, desired, ops);
            eval_commands(left, frame.group_member(source, &Dir::Left), &member(Dir::Left), slots.slot(2), now, desired, ops);
            eval_commands(right, frame.group_member(source, &Dir::Right), &member(Dir::Right), slots.slot(3), now, desired, ops);
        }
        CompiledBinding::Joystick { settings, outer_ring } => {
            eval_joystick(source, settings, outer_ring, frame, slots.slot(0), now, desired, ops);
        }
        CompiledBinding::DirectionalPad { settings, up, down, left, right, outer_ring } => {
            eval_directional_pad(
                source, settings, up, down, left, right, outer_ring, frame, slots, now, desired, ops,
            );
        }
        CompiledBinding::Trigger { settings, soft_pull } => {
            eval_trigger(source, settings, soft_pull, frame, slots.slot(0), now, desired, ops);
        }
        CompiledBinding::AsMouse { settings } => eval_as_mouse(source, settings, ctx, rel),
        CompiledBinding::JoystickMouse { settings } => {
            eval_joystick_mouse(source, settings, ctx, rel)
        }
        CompiledBinding::GyroToMouse { settings } => eval_gyro_to_mouse(settings, ctx, rel),
    }
}

// --- Joystick (Pad/Stick → gamepad stick + outer-ring button) ---------------------------

#[allow(clippy::too_many_arguments)]
fn eval_joystick(
    source: &InputSource,
    s: &JoystickSettings,
    outer_ring: &[CompiledCommand],
    frame: &LogicalFrame,
    slot: &mut SlotState,
    now: &Tick,
    desired: &mut DesiredLevels,
    ops: &mut LayerOps,
) {
    // When the behavior is gated off (or a pad is untouched) it produces no axes (they
    // reconcile to neutral) and its outer ring reads un-held — but we still advance the slot so
    // edge-based activators stay correct across the gap.
    let pos = is_active(&s.activation, frame).then(|| source_pos(source, frame)).flatten();
    let ring_held = if let Some(pos) = pos {
        let (ox, oy) = process_joystick(&pos, s);
        let (ax, ay) = match s.output {
            StickOutput::Left => (GamepadAxis::LeftStickX, GamepadAxis::LeftStickY),
            StickOutput::Right => (GamepadAxis::RightStickX, GamepadAxis::RightStickY),
        };
        desired.set_axis(ax, ox);
        desired.set_axis(ay, oy);
        // Outer ring fires on raw input deflection, not the processed output.
        magnitude(&pos) >= s.outer_ring.radius
    } else {
        false
    };
    eval_commands(outer_ring, ring_held, &NodeHeld::Virtual, slot, now, desired, ops);
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
    slots: &mut SourceActivators,
    now: &Tick,
    desired: &mut DesiredLevels,
    ops: &mut LayerOps,
) {
    // Slots: 0=up 1=down 2=left 3=right 4=outer-ring. Gated off / untouched / inside the
    // deadzone → all virtual buttons un-held, but every slot is still advanced (edges stay
    // correct; reconcile releases any held output). Directions are layer-dependent virtual
    // nodes, so a HoldLayer on one is not robustly re-derivable → NodeHeld::Virtual.
    let pos = is_active(&s.activation, frame).then(|| source_pos(source, frame)).flatten();
    let mag = pos.as_ref().map_or(0.0, magnitude);
    let (mut u, mut d, mut l, mut r) = (false, false, false, false);
    if let Some(pos) = &pos
        && mag >= s.deadzone.inner
        && mag > 1e-6
    {
        let (rx, ry) = rotate(pos.x, pos.y, s.rotation.degrees);
        [u, d, l, r] = dpad_dirs(rx, ry, &s.layout);
    }
    let v = &NodeHeld::Virtual;
    eval_commands(up, u, v, slots.slot(0), now, desired, ops);
    eval_commands(down, d, v, slots.slot(1), now, desired, ops);
    eval_commands(left, l, v, slots.slot(2), now, desired, ops);
    eval_commands(right, r, v, slots.slot(3), now, desired, ops);
    eval_commands(outer_ring, mag >= s.outer_ring.radius, v, slots.slot(4), now, desired, ops);
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

#[allow(clippy::too_many_arguments)]
fn eval_trigger(
    source: &InputSource,
    s: &TriggerSettings,
    soft_pull: &[CompiledCommand],
    frame: &LogicalFrame,
    slot: &mut SlotState,
    now: &Tick,
    desired: &mut DesiredLevels,
    ops: &mut LayerOps,
) {
    let pull = frame.trigger(source); // 0.0..=1.0
    let axis = match s.output {
        TriggerOutput::Left => GamepadAxis::LeftTrigger,
        TriggerOutput::Right => GamepadAxis::RightTrigger,
    };
    desired.set_axis(axis, process_trigger(pull, s));
    // Soft-pull virtual button fires on the raw analog pull vs its threshold — a frame-local
    // trigger, so a HoldLayer on it re-derives exactly (NodeHeld::SoftPull).
    let node = NodeHeld::SoftPull(source.clone(), s.soft_pull.threshold);
    eval_commands(soft_pull, pull >= s.soft_pull.threshold, &node, slot, now, desired, ops);
}

fn process_trigger(pull: f32, s: &TriggerSettings) -> f32 {
    if pull <= s.deadzone.inner {
        return 0.0;
    }
    let scaled = ((pull - s.deadzone.inner) / (1.0 - s.deadzone.inner)).clamp(0.0, 1.0);
    apply_curve(scaled, &s.curve)
}

// --- AsMouse (Pad → cursor/scroll via frame-to-frame delta) -----------------------------

fn eval_as_mouse(source: &InputSource, s: &AsMouseSettings, ctx: &Ctx, rel: &mut RelAccum) {
    if !is_active(&s.activation, ctx.cur) {
        return;
    }
    // Positional delta of a pad, only while touched on *both* this frame and the last — so a
    // touch-down (or lift) never injects a jump. dt-independent (a trackpad reports position).
    let (Some(cur), Some(prev)) = (ctx.cur.pad(source), ctx.prev.and_then(|p| p.pad(source)))
    else {
        return;
    };
    if !cur.touched || !prev.touched {
        return;
    }
    let (mx, my) = process_relative(
        cur.pos.x - prev.pos.x,
        cur.pos.y - prev.pos.y,
        &s.sensitivity,
        &s.invert,
        s.rotation.degrees,
        PAD_MOUSE_GAIN,
    );
    emit_relative(&s.output, mx, my, rel);
}

// --- JoystickMouse (Stick → cursor/scroll via deflection→rate·dt) -----------------------

fn eval_joystick_mouse(source: &InputSource, s: &JoystickMouseSettings, ctx: &Ctx, rel: &mut RelAccum) {
    if !is_active(&s.activation, ctx.cur) {
        return;
    }
    let pos = ctx.cur.pos(source);
    let (rx, ry) = rotate(pos.x, pos.y, s.rotation.degrees);
    let mag = (rx * rx + ry * ry).sqrt();
    if mag <= s.deadzone.inner || mag < 1e-6 {
        return;
    }
    // Deflection past the deadzone → speed; integrated over dt into a pixel delta.
    let scaled = ((mag - s.deadzone.inner) / (1.0 - s.deadzone.inner)).clamp(0.0, 1.0);
    let speed = apply_curve(scaled, &s.curve) * JOY_MOUSE_RATE * ctx.dt;
    let (ux, uy) = (rx / mag, ry / mag);
    let mut mx = ux * speed * s.sensitivity.x;
    let mut my = uy * speed * s.sensitivity.y;
    if s.invert.x {
        mx = -mx;
    }
    if s.invert.y {
        my = -my;
    }
    emit_relative(&s.output, mx, my, rel);
}

// --- GyroToMouse (angular velocity → pixel delta, crude local space) --------------------

fn eval_gyro_to_mouse(s: &GyroToMouseSettings, ctx: &Ctx, rel: &mut RelAccum) {
    if !is_active(&s.activation, ctx.cur) {
        return;
    }
    // Local space (crude, decision D): yaw (z) → horizontal, pitch (x) → vertical, deg/s.
    let g = ctx.cur.gyro();
    let (yaw, pitch) = rotate(
        g.z as f32 / GYRO_RES_PER_DPS,
        g.x as f32 / GYRO_RES_PER_DPS,
        s.rotation.degrees,
    );
    let mut mx = yaw * s.sensitivity.x * GYRO_MOUSE_GAIN * ctx.dt;
    let mut my = pitch * s.sensitivity.y * GYRO_MOUSE_GAIN * ctx.dt;
    if s.invert.x {
        mx = -mx;
    }
    if s.invert.y {
        my = -my;
    }
    // Radial pixel deadzone kills the resting DC-bias drift (bridge finding; PLAN decision D).
    if (mx * mx + my * my).sqrt() < s.deadzone.inner {
        return;
    }
    emit_relative(&s.output, mx, my, rel);
}

// --- shared helpers ---------------------------------------------------------------------

/// Apply rotation, per-axis sensitivity, gain, and invert to a raw relative delta.
fn process_relative(
    dx: f32,
    dy: f32,
    sens: &Sensitivity,
    invert: &Invert,
    rotation_deg: f32,
    gain: f32,
) -> (f32, f32) {
    let (rx, ry) = rotate(dx, dy, rotation_deg);
    let mut mx = rx * sens.x * gain;
    let mut my = ry * sens.y * gain;
    if invert.x {
        mx = -mx;
    }
    if invert.y {
        my = -my;
    }
    (mx, my)
}

/// Route a relative delta to the chosen mouse output (cursor motion or scroll).
fn emit_relative(output: &MouseOutput, dx: f32, dy: f32, rel: &mut RelAccum) {
    match output {
        MouseOutput::Cursor => rel.add_mouse(dx, dy),
        MouseOutput::Scroll => rel.add_scroll(dx, dy),
    }
}

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
        Activator, CommandSettings, Deadzone, GyroToMouseSettings, Invert, JoystickMouseSettings,
        OuterRing, SoftPull,
    };
    use steam_hid::{Buttons, ControllerState, TrackPad, Vec3i};
    use virt_out::OutputEvent;
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
        let ctx = Ctx { cur: &frame, prev: None, dt: 0.0, now: Tick(0) };
        let mut d = DesiredLevels::default();
        let mut rel = RelAccum::default();
        let mut slots = SourceActivators::default();
        let mut ops = LayerOps::default();
        eval_binding(binding, source, &ctx, &mut slots, &mut d, &mut rel, &mut ops);
        d
    }

    /// Run a relative behavior and return the flushed `MouseMove`/`Scroll` events.
    fn relative_of(
        binding: &CompiledBinding,
        source: &InputSource,
        prev: Option<ControllerState>,
        cur: ControllerState,
        dt: f32,
    ) -> Vec<OutputEvent> {
        let prev_frame = prev.map(LogicalFrame::new);
        let cur_frame = LogicalFrame::new(cur);
        let ctx = Ctx { cur: &cur_frame, prev: prev_frame.as_ref(), dt, now: Tick(0) };
        let mut d = DesiredLevels::default();
        let mut rel = RelAccum::default();
        let mut slots = SourceActivators::default();
        let mut ops = LayerOps::default();
        eval_binding(binding, source, &ctx, &mut slots, &mut d, &mut rel, &mut ops);
        let mut out = Vec::new();
        rel.flush(&mut out);
        out
    }

    fn touched(x: f32, y: f32) -> TrackPad {
        TrackPad { pos: Vec2 { x, y }, pressure: 0.0, touched: true }
    }

    fn mouse_dx(events: &[OutputEvent]) -> i32 {
        events.iter().find_map(|e| match e {
            OutputEvent::MouseMove { dx, .. } => Some(*dx),
            _ => None,
        }).unwrap_or(0)
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

    // --- relative behaviors (S6b) -------------------------------------------------------

    #[test]
    fn as_mouse_pad_delta_moves_cursor() {
        let binding = CompiledBinding::AsMouse { settings: AsMouseSettings::default() };
        // Rightward swipe while touched across two frames → positive dx.
        let prev = ControllerState { left_pad: touched(0.0, 0.0), ..Default::default() };
        let cur = ControllerState { left_pad: touched(0.5, 0.0), ..Default::default() };
        let out = relative_of(&binding, &InputSource::LeftPad, Some(prev), cur, 0.016);
        assert!(mouse_dx(&out) > 0);
    }

    #[test]
    fn as_mouse_needs_touch_on_both_frames() {
        let binding = CompiledBinding::AsMouse { settings: AsMouseSettings::default() };
        // Touch-down this frame (prev not touched) → no jump.
        let prev = ControllerState {
            left_pad: TrackPad { pos: Vec2 { x: 0.0, y: 0.0 }, pressure: 0.0, touched: false },
            ..Default::default()
        };
        let cur = ControllerState { left_pad: touched(0.5, 0.0), ..Default::default() };
        let out = relative_of(&binding, &InputSource::LeftPad, Some(prev), cur, 0.016);
        assert!(out.is_empty());
        // No previous frame at all → nothing.
        let cur = ControllerState { left_pad: touched(0.5, 0.0), ..Default::default() };
        let out = relative_of(&binding, &InputSource::LeftPad, None, cur, 0.016);
        assert!(out.is_empty());
    }

    #[test]
    fn joystick_mouse_rate_scales_with_dt() {
        let binding =
            CompiledBinding::JoystickMouse { settings: JoystickMouseSettings::default() };
        let cur = || ControllerState { left_stick: Vec2 { x: 1.0, y: 0.0 }, ..Default::default() };
        // dt = 0 (first tick) → no motion despite full deflection.
        let out = relative_of(&binding, &InputSource::LeftStick, None, cur(), 0.0);
        assert!(out.is_empty());
        // dt > 0 → moves right.
        let out = relative_of(&binding, &InputSource::LeftStick, None, cur(), 0.1);
        assert!(mouse_dx(&out) > 0);
    }

    #[test]
    fn gyro_to_mouse_yaw_moves_horizontally_and_deadzones() {
        // Default deadzone 0 → a yaw rate moves the cursor horizontally.
        let binding = CompiledBinding::GyroToMouse { settings: GyroToMouseSettings::default() };
        let yaw = ControllerState {
            gyro: Vec3i { x: 0, y: 0, z: (10.0 * GYRO_RES_PER_DPS) as i16 }, // 10 deg/s yaw
            ..Default::default()
        };
        let out = relative_of(&binding, &InputSource::Gyro, None, yaw, 0.1);
        assert!(mouse_dx(&out) > 0);

        // With a large radial deadzone, a tiny rate is dropped (kills resting drift).
        let binding = CompiledBinding::GyroToMouse {
            settings: GyroToMouseSettings { deadzone: Deadzone { inner: 5.0 }, ..Default::default() },
        };
        let tiny = ControllerState {
            gyro: Vec3i { x: 0, y: 0, z: (0.5 * GYRO_RES_PER_DPS) as i16 },
            ..Default::default()
        };
        let out = relative_of(&binding, &InputSource::Gyro, None, tiny, 0.1);
        assert!(out.is_empty());
    }
}
