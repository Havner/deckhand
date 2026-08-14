//! Behavior settings palette (PLAN §3, Round B).
//!
//! Shared setting types (deadzone, curve, sensitivity, …) plus the per-behavior settings
//! structs. These carry `f32` tuning values, so they are `PartialEq` but **not** `Eq`.
//! Each has a sensible `Default` and struct-level `#[serde(default)]`, so RON stays
//! forgiving (omit what you don't override).

use serde::{Deserialize, Serialize};

// --- shared setting types ---------------------------------------------------------------

/// Inner **radial** deadzone (`0..=1`): input magnitude below this maps to neutral.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Deadzone {
    pub inner: f32,
}

/// Output anti-deadzone (`0..=1`): pushes small outputs past a game's own stick deadzone.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AntiDeadzone {
    pub amount: f32,
}

/// Response curve. `Power(e)` raises the normalized magnitude to `e` (`e>1` = slow centre,
/// `e<1` = twitchy). Named/custom presets are deferred.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum Curve {
    #[default]
    Linear,
    Power(f32),
}

impl Curve {
    /// Remap a normalized magnitude `v` (`0..=1`, endpoints pinned) through the curve.
    pub fn apply(&self, v: f32) -> f32 {
        match self {
            Curve::Linear => v,
            Curve::Power(e) => v.powf(*e),
        }
    }
}

/// Per-axis sensitivity multiplier (for gyro, `x` = yaw, `y` = pitch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sensitivity {
    pub x: f32,
    pub y: f32,
}

impl Default for Sensitivity {
    fn default() -> Self {
        Sensitivity { x: 1.0, y: 1.0 }
    }
}

/// Simplest acceleration: output scaled by `1 + speed·factor` (`0` = off). Only the two **velocity**
/// mouse behaviors carry it — **`AsMouse`** (pad-swipe velocity, pad-units/s) and **`GyroToMouse`**
/// (angular velocity, deg/s) — both wanting small values (~0.02–0.05). Deflection behaviors
/// (`Joystick`/`JoystickMouse`/`Trigger`) shape their response with `Curve` instead (a held
/// deflection has no velocity to accelerate on; accel there would just be a narrower curve).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Acceleration {
    pub factor: f32,
}

/// Input-frame rotation, in degrees.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rotation {
    pub degrees: f32,
}

/// Per-axis inversion.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Invert {
    pub x: bool,
    pub y: bool,
}

/// One-Euro smoothing filter (V1). Raise `min_cutoff` to reduce lag at rest; raise `beta`
/// to reduce lag at speed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OneEuroFilter {
    pub min_cutoff: f32,
    pub beta: f32,
}

impl Default for OneEuroFilter {
    fn default() -> Self {
        OneEuroFilter { min_cutoff: 1.0, beta: 0.0 }
    }
}

/// The radius (`0..=1`) at which an outer-ring virtual button fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OuterRing {
    pub radius: f32,
}

impl Default for OuterRing {
    fn default() -> Self {
        OuterRing { radius: 0.9 }
    }
}

/// The analog threshold (`0..=1`) at which a trigger's soft-pull virtual button fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoftPull {
    pub threshold: f32,
}

impl Default for SoftPull {
    fn default() -> Self {
        SoftPull { threshold: 0.5 }
    }
}

// --- activation (general per-behavior gate; Round B, provisional) ------------------------

/// Behavior activation (Round B): whether the behavior is live, gated by held **physical**
/// buttons (`gaters`, OR-combined; no thresholds). Default = always active.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Activation {
    pub mode: ActivationMode,
    /// Held hardware buttons that gate the behaviour (OR-combined). Raw controller button bits —
    /// any of them, including face buttons / dpad directions (`vocab_hid::Button`), not just the
    /// standalone `SourceKind::Button` inputs.
    pub gaters: Vec<vocab_hid::Button>,
}

/// Activation polarity. `HoldToEnable` + no gater = never; `HoldToDisable` + no gater =
/// always (the default).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ActivationMode {
    HoldToEnable,
    #[default]
    HoldToDisable,
}

// --- output-target choices --------------------------------------------------------------

/// Which gamepad stick a `Joystick` drives — or `None` to drive no stick axis (keeping only the
/// outer-ring virtual button).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StickOutput {
    #[default]
    Left,
    Right,
    /// No stick axis output (outer-ring button still fires).
    None,
}

/// Which gamepad trigger a `Trigger` drives — or `None` to drive no trigger axis (keeping only the
/// soft-pull virtual button, e.g. a trigger bound purely to a mouse click on the desktop).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerOutput {
    #[default]
    Left,
    Right,
    /// No trigger axis output (soft-pull button still fires).
    None,
}

/// A mouse behavior's output — cursor or scroll (scroll is our extension over Steam).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MouseOutput {
    #[default]
    Cursor,
    /// Discrete wheel notches (`REL_WHEEL`) — compatible with every app.
    Scroll,
    /// High-resolution smooth scroll (`REL_WHEEL_HI_RES`; Windows sub-`WHEEL_DELTA`) — pixel-smooth
    /// in apps that support it, with a legacy notch fallback so others still scroll.
    SmoothScroll,
}

/// Directional-pad layout: 4-way (cardinals only) or 8-way (diagonals fire two).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DpadLayout {
    #[default]
    FourWay,
    EightWay,
}

/// How gyro rotation maps to cursor motion (Round B; more spaces — World/Laser — later). Vertical
/// is local pitch in every variant here; they differ in what drives **horizontal**.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GyroSpace {
    /// Local preset — horizontal from **yaw** (turn around the controller's vertical axis).
    Yaw,
    /// Local preset — horizontal from **roll** (lean around the controller's forward axis).
    Roll,
    /// Local preset — horizontal from **yaw + roll** (turn and lean combined). The robust local
    /// default — works whether you turn the controller or lean it.
    #[default]
    YawRoll,
    /// Player space — horizontal from yaw + roll **around the gravity axis** (from the
    /// accelerometer), vertical from local pitch. "Turning around real-world vertical" maps to
    /// horizontal regardless of how the controller is tilted/rolled.
    PlayerSpace,
}

// --- per-behavior settings --------------------------------------------------------------

/// `Joystick` (Pad + Stick → gamepad stick + an outer-ring button).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JoystickSettings {
    pub output: StickOutput,
    pub outer_ring: OuterRing,
    pub curve: Curve,
    pub deadzone: Deadzone,
    pub anti_deadzone: AntiDeadzone,
    pub invert: Invert,
    pub rotation: Rotation,
    pub activation: Activation,
}

/// `DirectionalPad` (Pad + Stick → 4 direction + outer-ring buttons).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectionalPadSettings {
    pub layout: DpadLayout,
    pub outer_ring: OuterRing,
    /// Register deadzone: how far from centre before a direction activates.
    pub deadzone: Deadzone,
    pub rotation: Rotation,
    pub activation: Activation,
}

/// `AsMouse` (Pad → cursor/scroll). No deadzone (relative delta, Round B). A **velocity** behavior
/// (finger-swipe speed), so it carries `acceleration`, NOT `curve` — a curve remaps a held
/// deflection, which a relative pad delta doesn't have (matches Steam; PLAN §3 Round B).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AsMouseSettings {
    pub output: MouseOutput,
    pub sensitivity: Sensitivity,
    pub acceleration: Acceleration,
    pub smoothing: Option<OneEuroFilter>,
    pub invert: Invert,
    pub rotation: Rotation,
    pub activation: Activation,
}

/// `JoystickMouse` (Stick → cursor/scroll via deflection→rate). A **deflection** behavior, so it
/// carries `curve` (which shapes the deflection→rate response — precision near center, fast at the
/// edge), NOT `acceleration`: accel on a stick would read *deflection* as its speed, which is just a
/// narrower curve (matches Steam). No `smoothing`: a stick is already a smooth analog signal, so a
/// 1€ filter would only add lag — only `AsMouse` (pad delta) and `GyroToMouse` (noisy IMU) carry one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JoystickMouseSettings {
    pub output: MouseOutput,
    pub sensitivity: Sensitivity,
    pub curve: Curve,
    pub deadzone: Deadzone,
    pub invert: Invert,
    pub rotation: Rotation,
    pub activation: Activation,
}

/// `GyroToMouse` (Gyro → cursor/scroll). Player-space + 1€ filter are V1.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GyroToMouseSettings {
    pub output: MouseOutput,
    pub space: GyroSpace,
    pub sensitivity: Sensitivity,
    pub acceleration: Acceleration,
    pub smoothing: Option<OneEuroFilter>,
    /// Radial deadzone — kills the resting-bias drift (Round B / bridge).
    pub deadzone: Deadzone,
    pub invert: Invert,
    pub rotation: Rotation,
    pub activation: Activation,
}

/// `Trigger` (analog output + a virtual soft-pull button). Full-pull is a standalone
/// `Button` input (Round A), not here.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerSettings {
    pub output: TriggerOutput,
    pub soft_pull: SoftPull,
    pub curve: Curve,
    pub deadzone: Deadzone,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        assert_eq!(Sensitivity::default(), Sensitivity { x: 1.0, y: 1.0 });
        assert_eq!(OuterRing::default().radius, 0.9);
        assert_eq!(SoftPull::default().threshold, 0.5);
        assert_eq!(Curve::default(), Curve::Linear);
        let a = Activation::default();
        assert_eq!(a.mode, ActivationMode::HoldToDisable); // always active
        assert!(a.gaters.is_empty());
        assert!(AsMouseSettings::default().smoothing.is_none());
    }

    #[test]
    fn gyro_settings_round_trip_ron() {
        let g = GyroToMouseSettings {
            space: GyroSpace::PlayerSpace,
            sensitivity: Sensitivity { x: 0.7, y: 0.9 },
            smoothing: Some(OneEuroFilter { min_cutoff: 1.0, beta: 0.5 }),
            activation: Activation {
                mode: ActivationMode::HoldToEnable,
                gaters: vec![vocab_hid::Button::LT],
            },
            ..Default::default()
        };
        let s = ron::to_string(&g).unwrap();
        assert_eq!(ron::from_str::<GyroToMouseSettings>(&s).unwrap(), g);
    }
}
