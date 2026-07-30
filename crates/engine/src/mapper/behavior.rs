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
//! **golden-tested for logic**.
//!
//! **Vertical axis convention (HW-confirmed).** The controller reports stick/pad Y as **+up**
//! (physical); evdev's cursor `REL_Y` and gamepad `ABS_Y` are both **+down**, and `virt-out`
//! passes values through unchanged — so analog *vertical output* must be flipped at the emission
//! boundary (`emit_relative` cursor path, `eval_joystick` stick Y). X already agrees. Scroll
//! (`REL_WHEEL`) has its own convention (left as-is). `GyroToMouse` additionally negates its
//! **yaw→X** term (yaw-left = +z must move the cursor left) — a behavior-intrinsic handedness,
//! HW-confirmed, distinct from the evdev vertical flip above.

use config::{
    Activation, ActivationMode, AsMouseSettings, Curve, DirectionalPadSettings, DpadLayout,
    GyroToMouseSettings, InputSource, Invert, JoystickMouseSettings, JoystickSettings, MouseOutput,
    Sensitivity, StickOutput, TriggerOutput, TriggerSettings,
};
use steam_hid::{GYRO_RES_PER_DPS, Vec2};
use vocab::GamepadAxis;

use super::{HapticReq, Tick};
use super::activator::{SlotState, SourceActivators};
use super::command::eval_commands;
use super::layers::{LayerOps, NodeHeld};
use super::reconcile::{DesiredLevels, RelAccum};
use super::smooth::OneEuro2;
use crate::logical::{Dir, LogicalFrame};
use crate::program::{CompiledBinding, CompiledCommand};

/// Behavior output gains — reasonable starting points; final feel is tuned against the bridge
/// at HW validation (S10). Pixels per normalized-pad-delta / per stick-rate·second / per degree.
const PAD_MOUSE_GAIN: f32 = 400.0;
const JOY_MOUSE_RATE: f32 = 800.0;
const GYRO_MOUSE_GAIN: f32 = 20.0;
/// Scroll reuses a behavior's pixel-scaled motion but a wheel is far coarser than the cursor
/// (`REL_WHEEL` counts notches, not pixels), so scroll divides the pixel motion down by this many
/// pixels per wheel notch. Starting point, HW-tuned; per-behavior `sensitivity` tunes on top.
const PIXELS_PER_SCROLL_TICK: f32 = 50.0;
/// High-resolution scroll units per wheel detent (evdev `REL_WHEEL_HI_RES` / Windows `WHEEL_DELTA`).
/// `SmoothScroll` covers the same distance as `Scroll` but in these finer units → ~120× smoother.
const SCROLL_HI_RES_PER_TICK: f32 = 120.0;

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
    haptics: &mut Vec<HapticReq>,
    smoother: Option<&mut OneEuro2>,
) {
    let frame = ctx.cur;
    let now = &ctx.now;
    // Command haptics fire on the actuator matching this input's side (device-independent).
    let side = source.side();
    match binding {
        CompiledBinding::Button { commands } => {
            let node = NodeHeld::Button(source.clone());
            eval_commands(commands, frame.button(source), &node, &side, slots.slot(0), now, desired, ops, haptics);
        }
        CompiledBinding::ButtonPad { up, down, left, right } => {
            let member = |dir| NodeHeld::Group(source.clone(), dir);
            eval_commands(up, frame.group_member(source, &Dir::Up), &member(Dir::Up), &side, slots.slot(0), now, desired, ops, haptics);
            eval_commands(down, frame.group_member(source, &Dir::Down), &member(Dir::Down), &side, slots.slot(1), now, desired, ops, haptics);
            eval_commands(left, frame.group_member(source, &Dir::Left), &member(Dir::Left), &side, slots.slot(2), now, desired, ops, haptics);
            eval_commands(right, frame.group_member(source, &Dir::Right), &member(Dir::Right), &side, slots.slot(3), now, desired, ops, haptics);
        }
        CompiledBinding::Joystick { settings, outer_ring } => {
            eval_joystick(source, settings, outer_ring, frame, slots.slot(0), now, desired, ops, haptics);
        }
        CompiledBinding::DirectionalPad { settings, up, down, left, right, outer_ring } => {
            eval_directional_pad(
                source, settings, up, down, left, right, outer_ring, frame, slots, now, desired, ops, haptics,
            );
        }
        CompiledBinding::Trigger { settings, soft_pull } => {
            eval_trigger(source, settings, soft_pull, frame, slots.slot(0), now, desired, ops, haptics);
        }
        CompiledBinding::AsMouse { settings } => eval_as_mouse(source, settings, ctx, rel, smoother),
        CompiledBinding::JoystickMouse { settings } => {
            eval_joystick_mouse(source, settings, ctx, rel)
        }
        CompiledBinding::GyroToMouse { settings } => {
            eval_gyro_to_mouse(settings, ctx, rel, smoother)
        }
        // Explicit unbind: no output. Overrides a base binding when resolved from a layer; the
        // reconcile then releases whatever the base binding was holding (no stuck output).
        CompiledBinding::None => {}
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
    haptics: &mut Vec<HapticReq>,
) {
    // When the behavior is gated off (or a pad is untouched) it produces no axes (they
    // reconcile to neutral) and its outer ring reads un-held — but we still advance the slot so
    // edge-based activators stay correct across the gap.
    let pos = is_active(&s.activation, frame).then(|| source_pos(source, frame)).flatten();
    let ring_held = if let Some(pos) = pos {
        // `None` output drives no stick axis (outer-ring button still fires).
        let axes = match s.output {
            StickOutput::Left => Some((GamepadAxis::LeftStickX, GamepadAxis::LeftStickY)),
            StickOutput::Right => Some((GamepadAxis::RightStickX, GamepadAxis::RightStickY)),
            StickOutput::None => None,
        };
        if let Some((ax, ay)) = axes {
            let (ox, oy) = process_joystick(&pos, s);
            desired.set_axis(ax, ox);
            // evdev stick Y is +down while the controller reports +up, so flip (same
            // screen-orientation fix as the mouse cursor). Per-axis `invert` composes on top.
            desired.set_axis(ay, -oy);
        }
        // Outer ring fires on raw input deflection, not the processed output.
        magnitude(&pos) >= s.outer_ring.radius
    } else {
        false
    };
    eval_commands(outer_ring, ring_held, &NodeHeld::Virtual, &source.side(), slot, now, desired, ops, haptics);
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
    haptics: &mut Vec<HapticReq>,
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
    let side = source.side();
    eval_commands(up, u, v, &side, slots.slot(0), now, desired, ops, haptics);
    eval_commands(down, d, v, &side, slots.slot(1), now, desired, ops, haptics);
    eval_commands(left, l, v, &side, slots.slot(2), now, desired, ops, haptics);
    eval_commands(right, r, v, &side, slots.slot(3), now, desired, ops, haptics);
    eval_commands(outer_ring, mag >= s.outer_ring.radius, v, &side, slots.slot(4), now, desired, ops, haptics);
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
    haptics: &mut Vec<HapticReq>,
) {
    let pull = frame.trigger(source); // 0.0..=1.0
    // `None` output drives no trigger axis (soft-pull button still fires).
    let axis = match s.output {
        TriggerOutput::Left => Some(GamepadAxis::LeftTrigger),
        TriggerOutput::Right => Some(GamepadAxis::RightTrigger),
        TriggerOutput::None => None,
    };
    if let Some(axis) = axis {
        desired.set_axis(axis, process_trigger(pull, s));
    }
    // Soft-pull virtual button fires on the raw analog pull vs its threshold — a frame-local
    // trigger, so a HoldLayer on it re-derives exactly (NodeHeld::SoftPull).
    let node = NodeHeld::SoftPull(source.clone(), s.soft_pull.threshold);
    eval_commands(soft_pull, pull >= s.soft_pull.threshold, &node, &source.side(), slot, now, desired, ops, haptics);
}

fn process_trigger(pull: f32, s: &TriggerSettings) -> f32 {
    if pull <= s.deadzone.inner {
        return 0.0;
    }
    let scaled = ((pull - s.deadzone.inner) / (1.0 - s.deadzone.inner)).clamp(0.0, 1.0);
    apply_curve(scaled, &s.curve)
}

// --- AsMouse (Pad → cursor/scroll via frame-to-frame delta) -----------------------------

fn eval_as_mouse(
    source: &InputSource,
    s: &AsMouseSettings,
    ctx: &Ctx,
    rel: &mut RelAccum,
    smoother: Option<&mut OneEuro2>,
) {
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
    let (mut dx, mut dy) = (cur.pos.x - prev.pos.x, cur.pos.y - prev.pos.y);
    // Optional 1€ smoothing on the *velocity* (delta/dt) — frame-rate-independent — then back to a
    // delta. When off, this is identity.
    if let (Some(cfg), Some(sm)) = (&s.smoothing, smoother)
        && ctx.dt > 0.0
    {
        let (vx, vy) = sm.filter(dx / ctx.dt, dy / ctx.dt, ctx.dt, cfg);
        (dx, dy) = (vx * ctx.dt, vy * ctx.dt);
    }
    // Acceleration scales with finger **speed** (velocity = delta/dt, pad-units per second), not
    // the per-frame delta — so it's poll-rate-independent. `factor = 0` is off. The base motion
    // stays positional (dt-independent); only the accel multiplier reads dt.
    let speed = if ctx.dt > 0.0 { (dx * dx + dy * dy).sqrt() / ctx.dt } else { 0.0 };
    let accel = 1.0 + speed * s.acceleration.factor;
    let (mx, my) =
        process_relative(dx, dy, &s.sensitivity, &s.invert, s.rotation.degrees, PAD_MOUSE_GAIN * accel);
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
    // Deflection past the deadzone → speed; integrated over dt into a pixel delta. Deflection is
    // already an instantaneous speed, so acceleration scales by it directly (poll-rate-independent).
    let scaled = ((mag - s.deadzone.inner) / (1.0 - s.deadzone.inner)).clamp(0.0, 1.0);
    let accel = 1.0 + scaled * s.acceleration.factor;
    let speed = apply_curve(scaled, &s.curve) * JOY_MOUSE_RATE * ctx.dt * accel;
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

fn eval_gyro_to_mouse(
    s: &GyroToMouseSettings,
    ctx: &Ctx,
    rel: &mut RelAccum,
    smoother: Option<&mut OneEuro2>,
) {
    if !is_active(&s.activation, ctx.cur) {
        return;
    }
    // Local space (crude, decision D): yaw (z) → horizontal, pitch (x) → vertical, deg/s.
    // Sign: yaw-left is +z but should move the cursor **left** (−X), so negate yaw. Pitch-up is
    // +x → cursor up (the +up→+down cursor flip in emit_relative supplies that sign).
    let g = ctx.cur.gyro();
    let (mut yaw, mut pitch) = rotate(
        -(g.z as f32) / GYRO_RES_PER_DPS,
        g.x as f32 / GYRO_RES_PER_DPS,
        s.rotation.degrees,
    );
    // Optional 1€ smoothing on the angular velocity (deg/s) — the canonical gyro-aim smoothing.
    if let (Some(cfg), Some(sm)) = (&s.smoothing, smoother) {
        (yaw, pitch) = sm.filter(yaw, pitch, ctx.dt, cfg);
    }
    // Angular velocity (deg/s) is already an instantaneous speed → acceleration scales by its
    // magnitude directly (poll-rate-independent). `factor = 0` is off.
    let accel = 1.0 + (yaw * yaw + pitch * pitch).sqrt() * s.acceleration.factor;
    let mut mx = yaw * s.sensitivity.x * GYRO_MOUSE_GAIN * ctx.dt * accel;
    let mut my = pitch * s.sensitivity.y * GYRO_MOUSE_GAIN * ctx.dt * accel;
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

/// Route a relative delta to the chosen mouse output (cursor motion, discrete scroll, or smooth
/// scroll).
///
/// The controller reports stick/pad Y as **+up** (physical), but evdev's cursor `REL_Y` is
/// **+down**, so the cursor path flips Y to keep "up on the controller" = "up on screen" (X
/// already agrees). Scroll (`REL_WHEEL`) has the opposite polarity (+ = up), so *not* flipping Y
/// there likewise means "controller up = scroll up"; but a wheel is far coarser than the cursor,
/// so scroll divides the pixel-scaled motion down to notches ([`PIXELS_PER_SCROLL_TICK`]).
/// `SmoothScroll` covers the same distance at high resolution — the same notch value scaled up by
/// `SCROLL_HI_RES_PER_TICK` (120 units per detent) so it emits ~120× finer and feels smooth.
fn emit_relative(output: &MouseOutput, dx: f32, dy: f32, rel: &mut RelAccum) {
    match output {
        MouseOutput::Cursor => rel.add_mouse(dx, -dy),
        MouseOutput::Scroll => {
            rel.add_scroll(dx / PIXELS_PER_SCROLL_TICK, dy / PIXELS_PER_SCROLL_TICK)
        }
        MouseOutput::SmoothScroll => {
            let s = SCROLL_HI_RES_PER_TICK / PIXELS_PER_SCROLL_TICK; // hi-res units per pixel
            rel.add_smooth_scroll(dx * s, dy * s)
        }
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
        let mut haptics = Vec::new();
        eval_binding(binding, source, &ctx, &mut slots, &mut d, &mut rel, &mut ops, &mut haptics, None);
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
        let mut haptics = Vec::new();
        eval_binding(binding, source, &ctx, &mut slots, &mut d, &mut rel, &mut ops, &mut haptics, None);
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

    fn mouse_dy(events: &[OutputEvent]) -> i32 {
        events.iter().find_map(|e| match e {
            OutputEvent::MouseMove { dy, .. } => Some(*dy),
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
    fn joystick_up_is_negative_stick_y() {
        // Controller up (+pos.y) must map to evdev "up" = negative ABS_Y (screen orientation).
        let binding = CompiledBinding::Joystick {
            settings: JoystickSettings::default(),
            outer_ring: Vec::new(),
        };
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 0.0, y: 1.0 }, ..Default::default() },
        );
        assert!(d.axis(&GamepadAxis::LeftStickY).unwrap() < -0.99);
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
    fn trigger_output_none_drops_axis_keeps_soft_pull() {
        let binding = CompiledBinding::Trigger {
            settings: TriggerSettings {
                output: config::TriggerOutput::None,
                soft_pull: SoftPull { threshold: 0.5 },
                ..Default::default()
            },
            soft_pull: regular(Key::F),
        };
        let d = desired_of(
            &binding,
            &InputSource::LeftTrigger,
            ControllerState { left_trigger: 0.8, ..Default::default() },
        );
        // No trigger axis is set, but the soft-pull button still fires.
        assert_eq!(d.axis(&GamepadAxis::LeftTrigger), None);
        assert!(d.has_key(&Key::F));
    }

    #[test]
    fn joystick_output_none_drops_axis_keeps_outer_ring() {
        let binding = CompiledBinding::Joystick {
            settings: JoystickSettings {
                output: config::StickOutput::None,
                outer_ring: OuterRing { radius: 0.9 },
                ..Default::default()
            },
            outer_ring: regular(Key::Space),
        };
        let d = desired_of(
            &binding,
            &InputSource::LeftStick,
            ControllerState { left_stick: Vec2 { x: 1.0, y: 0.0 }, ..Default::default() },
        );
        // No stick axis, but the outer-ring virtual button still fires past the radius.
        assert_eq!(d.axis(&GamepadAxis::LeftStickX), None);
        assert_eq!(d.axis(&GamepadAxis::LeftStickY), None);
        assert!(d.has_key(&Key::Space));
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
    fn as_mouse_scroll_is_far_coarser_than_cursor() {
        // The same pad delta yields many cursor pixels but only a few scroll notches (REL_WHEEL
        // counts notches, not pixels), so scroll doesn't fly.
        let prev = ControllerState { left_pad: touched(0.0, 0.0), ..Default::default() };
        let cur = ControllerState { left_pad: touched(0.0, 0.5), ..Default::default() };

        let cursor = CompiledBinding::AsMouse { settings: AsMouseSettings::default() };
        let out = relative_of(&cursor, &InputSource::LeftPad, Some(prev.clone()), cur.clone(), 0.016);
        let cursor_dy = mouse_dy(&out).abs();

        let scroll = CompiledBinding::AsMouse {
            settings: AsMouseSettings { output: MouseOutput::Scroll, ..Default::default() },
        };
        let out = relative_of(&scroll, &InputSource::LeftPad, Some(prev), cur, 0.016);
        let scroll_dy = out
            .iter()
            .find_map(|e| match e {
                OutputEvent::Scroll { dy, .. } => Some(dy.abs()),
                _ => None,
            })
            .unwrap_or(0);

        assert!(cursor_dy > 100, "cursor should move ~200 px, got {cursor_dy}");
        assert!(scroll_dy > 0 && scroll_dy < 10, "scroll should be a few notches, got {scroll_dy}");
    }

    #[test]
    fn as_mouse_acceleration_is_velocity_based() {
        let prev = ControllerState { left_pad: touched(0.0, 0.0), ..Default::default() };
        let cur = ControllerState { left_pad: touched(0.0, 0.5), ..Default::default() };

        // factor = 0: motion is purely positional — identical regardless of dt (poll rate).
        let plain = CompiledBinding::AsMouse { settings: AsMouseSettings::default() };
        let fast = relative_of(&plain, &InputSource::LeftPad, Some(prev.clone()), cur.clone(), 0.004);
        let slow = relative_of(&plain, &InputSource::LeftPad, Some(prev.clone()), cur.clone(), 0.016);
        assert_eq!(mouse_dy(&fast), mouse_dy(&slow));

        // factor > 0: acceleration scales with velocity (delta/dt), so the *same* delta moved
        // faster (smaller dt) travels farther — the poll-rate-independent, proper behavior.
        let accel = CompiledBinding::AsMouse {
            settings: AsMouseSettings {
                acceleration: config::Acceleration { factor: 0.05 },
                ..Default::default()
            },
        };
        let fast = relative_of(&accel, &InputSource::LeftPad, Some(prev.clone()), cur.clone(), 0.004);
        let slow = relative_of(&accel, &InputSource::LeftPad, Some(prev), cur, 0.016);
        assert!(mouse_dy(&fast).abs() > mouse_dy(&slow).abs());
    }

    #[test]
    fn as_mouse_one_euro_attenuates_a_jitter_spike() {
        use config::OneEuroFilter;
        let smoothed = AsMouseSettings {
            smoothing: Some(OneEuroFilter { min_cutoff: 1.0, beta: 0.0 }),
            ..Default::default()
        };
        let plain = AsMouseSettings::default();

        let pair = |prev_y: f32, cur_y: f32| {
            (
                LogicalFrame::new(ControllerState { left_pad: touched(0.0, prev_y), ..Default::default() }),
                LogicalFrame::new(ControllerState { left_pad: touched(0.0, cur_y), ..Default::default() }),
            )
        };
        let run = |s: &AsMouseSettings, p: &LogicalFrame, c: &LogicalFrame, sm: Option<&mut OneEuro2>| {
            let ctx = Ctx { cur: c, prev: Some(p), dt: 0.004, now: Tick(0) };
            let mut rel = RelAccum::default();
            eval_as_mouse(&InputSource::LeftPad, s, &ctx, &mut rel, sm);
            let mut out = Vec::new();
            rel.flush(&mut out);
            mouse_dy(&out).abs()
        };

        // Warm the filter with steady slow motion, then hit it with a sudden spike: the smoothed
        // output is far smaller than the same spike unsmoothed.
        let mut sm = OneEuro2::default();
        let (p, c) = pair(0.0, 0.1);
        run(&smoothed, &p, &c, Some(&mut sm));
        let (p, c) = pair(0.0, 0.5);
        let smoothed_dy = run(&smoothed, &p, &c, Some(&mut sm));
        let raw_dy = run(&plain, &p, &c, None);
        assert!(smoothed_dy * 2 < raw_dy, "smoothed {smoothed_dy} should be << raw {raw_dy}");
    }

    #[test]
    fn joystick_mouse_acceleration_amplifies_deflection() {
        let cur = || ControllerState { left_stick: Vec2 { x: 1.0, y: 0.0 }, ..Default::default() };
        let plain = CompiledBinding::JoystickMouse { settings: JoystickMouseSettings::default() };
        let accel = CompiledBinding::JoystickMouse {
            settings: JoystickMouseSettings {
                acceleration: config::Acceleration { factor: 2.0 },
                ..Default::default()
            },
        };
        let base = relative_of(&plain, &InputSource::LeftStick, None, cur(), 0.1);
        let acc = relative_of(&accel, &InputSource::LeftStick, None, cur(), 0.1);
        assert!(mouse_dx(&acc).abs() > mouse_dx(&base).abs());
    }

    #[test]
    fn as_mouse_smooth_scroll_emits_fine_hi_res_units() {
        // A pad delta that yields ~1 discrete notch yields ~120 hi-res units (1 detent) as a
        // SmoothScroll event — same distance, ~120× finer resolution.
        let prev = ControllerState { left_pad: touched(0.0, 0.0), ..Default::default() };
        let cur = ControllerState { left_pad: touched(0.0, 0.15), ..Default::default() };

        let smooth = CompiledBinding::AsMouse {
            settings: AsMouseSettings { output: MouseOutput::SmoothScroll, ..Default::default() },
        };
        let out = relative_of(&smooth, &InputSource::LeftPad, Some(prev), cur, 0.016);
        let dy = out
            .iter()
            .find_map(|e| match e {
                OutputEvent::SmoothScroll { dy, .. } => Some(*dy),
                _ => None,
            })
            .expect("a SmoothScroll event");
        // 0.15 pad-units * 400 gain / 50 px-per-tick * 120 hi-res = ~144 units — far finer than
        // the 1 notch discrete scroll would give.
        assert!(dy.abs() > 100, "expected hi-res units, got {dy}");
    }

    #[test]
    fn as_mouse_pad_up_moves_cursor_up() {
        // Pad up (+y delta, controller is +up) must move the cursor up = negative evdev REL_Y.
        let binding = CompiledBinding::AsMouse { settings: AsMouseSettings::default() };
        let prev = ControllerState { left_pad: touched(0.0, 0.0), ..Default::default() };
        let cur = ControllerState { left_pad: touched(0.0, 0.5), ..Default::default() };
        let out = relative_of(&binding, &InputSource::LeftPad, Some(prev), cur, 0.016);
        assert!(mouse_dy(&out) < 0);
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
        // Yaw-left is +z and must move the cursor LEFT (negative REL_X).
        let binding = CompiledBinding::GyroToMouse { settings: GyroToMouseSettings::default() };
        let yaw_left = ControllerState {
            gyro: Vec3i { x: 0, y: 0, z: (10.0 * GYRO_RES_PER_DPS) as i16 }, // +z = yaw-left
            ..Default::default()
        };
        let out = relative_of(&binding, &InputSource::Gyro, None, yaw_left, 0.1);
        assert!(mouse_dx(&out) < 0);

        // Pitch-up is +x and must move the cursor UP (negative REL_Y).
        let pitch_up = ControllerState {
            gyro: Vec3i { x: (10.0 * GYRO_RES_PER_DPS) as i16, y: 0, z: 0 },
            ..Default::default()
        };
        let out = relative_of(&binding, &InputSource::Gyro, None, pitch_up, 0.1);
        assert!(mouse_dy(&out) < 0);

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
