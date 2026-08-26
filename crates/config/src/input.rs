//! Logical input vocabulary + per-device shapes (PLAN §3, Round A).
//!
//! [`InputSource`] is the hardware-independent identity a binding attaches to — the
//! device-agnostic **superset** across Gordon + Neptune (and future controllers). A given
//! device provides a subset, described by its [`Shape`]. Profiles reference these logical
//! inputs and stay device-independent; the engine resolves each device's raw report onto
//! them, and ignores bindings for inputs a device lacks.

use serde::{Deserialize, Serialize};

/// A logical, hardware-independent input control (the superset; see [`Shape`] for what a
/// given device provides). `Ord` (by declaration order) is derived so it can key a
/// deterministic `BTreeMap` in a profile.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum InputSource {
    // --- button groups (4-button clusters; ButtonPad in v1) ---
    /// A/B/X/Y cluster.
    FaceButtons,
    /// D-pad cluster (Gordon: left-pad quadrant classifiers; Neptune: physical).
    DPad,
    // --- standalone buttons ---
    LeftBumper,       // L1
    RightBumper,      // R1
    LeftGrip,         // L4
    RightGrip,        // R4
    LeftGrip2,        // L5 (Neptune)
    RightGrip2,       // R5 (Neptune)
    LeftGripTouch,    // capacitive left handle (Triton)
    RightGripTouch,   // capacitive right handle (Triton)
    View,             // Back / Deck ⧉ / Gordon '<'
    Menu,             // Start / Deck ☰ / Gordon '>'
    Steam,            // Guide
    QuickAccess,      // Deck '⋯' (Neptune)
    // --- triggers ---
    LeftTrigger,
    LeftTriggerFull,  // L2 full-pull hardware bit
    RightTrigger,
    RightTriggerFull, // R2 full-pull hardware bit
    // --- sticks ---
    LeftStick,
    LeftStickClick,   // L3
    LeftStickTouch,   // Neptune — sticks are capacitive (Gordon's stick isn't touch-sensitive)
    RightStick,
    RightStickClick,  // R3, Neptune (no right stick on Gordon)
    RightStickTouch,  // Neptune
    // --- trackpads ---
    LeftPad,
    LeftPadClick,     //
    LeftPadTouch,     //
    RightPad,
    RightPadClick,    //
    RightPadTouch,    //
    // --- motion ---
    Gyro,
}

/// The behavioural kind of a control — determines which behaviors/bindings are valid for
/// it (a `Pad` can be `AsMouse`/`Joystick`/`DirectionalPad`, a `Button` is just commands).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceKind {
    /// A standalone digital button (bumpers, grips, system buttons, clicks, touches,
    /// full-pulls).
    Button,
    /// A 4-button cluster (`FaceButtons`, `DPad`).
    ButtonGroup,
    /// A trackpad.
    Pad,
    /// An analog stick.
    Stick,
    /// An analog trigger (+ a virtual soft-pull button).
    Trigger,
    /// The IMU (gyro/accel).
    Gyro,
}

/// Which controller half a control sits on — used to pick the haptic actuator (Round C:
/// left inputs buzz the left pad, right the right).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

impl InputSource {
    /// The behavioural kind of this control.
    pub fn kind(&self) -> SourceKind {
        use InputSource::*;
        match self {
            FaceButtons | DPad => SourceKind::ButtonGroup,
            LeftPad | RightPad => SourceKind::Pad,
            LeftStick | RightStick => SourceKind::Stick,
            LeftTrigger | RightTrigger => SourceKind::Trigger,
            Gyro => SourceKind::Gyro,
            // Everything else is a standalone button.
            LeftBumper | RightBumper | LeftTriggerFull | RightTriggerFull | LeftGrip | RightGrip
            | LeftGrip2 | RightGrip2 | LeftGripTouch | RightGripTouch | View | Menu | Steam
            | QuickAccess | LeftStickClick | RightStickClick | LeftStickTouch | RightStickTouch
            | LeftPadClick | RightPadClick | LeftPadTouch | RightPadTouch => {
                SourceKind::Button
            }
        }
    }

    /// The controller half this input sits on (haptic actuator selection, Round C).
    /// Central buttons: `View`/`Steam` → Left, `Menu`/`QuickAccess` → Right. `Gyro` has no
    /// natural side (no haptic-firing bindings) — defaulted to Left.
    pub fn side(&self) -> Side {
        use InputSource::*;
        match self {
            FaceButtons | RightPad | RightStick | RightTrigger | RightBumper | RightTriggerFull
            | RightGrip | RightGrip2 | RightGripTouch | RightStickClick | RightStickTouch
            | RightPadClick | RightPadTouch | Menu | QuickAccess => Side::Right,
            // DPad, all Left*, View, Steam, Gyro → Left.
            DPad | LeftPad | LeftStick | LeftTrigger | Gyro | LeftBumper | LeftTriggerFull
            | LeftGrip | LeftGrip2 | LeftGripTouch | View | Steam | LeftStickClick
            | LeftStickTouch | LeftPadClick | LeftPadTouch => {
                Side::Left
            }
        }
    }

    /// Every logical input, for UI listings / iterating.
    pub const ALL: &'static [InputSource] = &[
        InputSource::FaceButtons,
        InputSource::DPad,
        InputSource::LeftBumper,
        InputSource::RightBumper,
        InputSource::LeftGrip,
        InputSource::RightGrip,
        InputSource::LeftGrip2,
        InputSource::RightGrip2,
        InputSource::LeftGripTouch,
        InputSource::RightGripTouch,
        InputSource::View,
        InputSource::Menu,
        InputSource::Steam,
        InputSource::QuickAccess,
        InputSource::LeftTrigger,
        InputSource::LeftTriggerFull,
        InputSource::RightTrigger,
        InputSource::RightTriggerFull,
        InputSource::LeftStick,
        InputSource::LeftStickClick,
        InputSource::LeftStickTouch,
        InputSource::RightStick,
        InputSource::RightStickClick,
        InputSource::RightStickTouch,
        InputSource::LeftPad,
        InputSource::LeftPadClick,
        InputSource::LeftPadTouch,
        InputSource::RightPad,
        InputSource::RightPadClick,
        InputSource::RightPadTouch,
        InputSource::Gyro,
    ];
}

/// A device's input-layout / capability descriptor (Round A). Lets the UI author
/// **offline** (grey out absent inputs) and the engine ignore bindings for inputs a device
/// lacks. This is reference data — a profile ([`crate`]'s `ConfigDoc`) stays
/// device-independent and never embeds a `Shape`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Shape {
    /// Original Steam Controller.
    Gordon,
    /// Steam Deck.
    Neptune,
    /// New Steam Controller (2026; SDL codename "Triton").
    Triton,
}

impl Shape {
    /// Whether this device provides the given logical input. Triton is a **superset of
    /// Neptune** (all the Deck's inputs) plus the two capacitive grip-touch sensors; Neptune
    /// adds a handful over Gordon (right stick, stick-touch, grip-2, quick-access).
    pub fn has(&self, input: &InputSource) -> bool {
        use InputSource::*;
        let neptune_up = matches!(
            input,
            RightStick
                | RightStickClick
                | LeftStickTouch
                | RightStickTouch
                | LeftGrip2
                | RightGrip2
                | QuickAccess
        );
        let triton_only = matches!(input, LeftGripTouch | RightGripTouch);
        match self {
            Shape::Gordon => !neptune_up && !triton_only,
            Shape::Neptune => !triton_only,
            Shape::Triton => true,
        }
    }

    /// Whether the trackpads report pressure (Neptune and Triton).
    pub fn pad_pressure(&self) -> bool {
        matches!(self, Shape::Neptune | Shape::Triton)
    }

    /// The logical inputs this device provides.
    pub fn inputs(&self) -> impl Iterator<Item = &'static InputSource> {
        InputSource::ALL.iter().filter(|i| self.has(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gordon_lacks_the_neptune_only_inputs() {
        let g = Shape::Gordon;
        assert!(!g.has(&InputSource::RightStick));
        assert!(!g.has(&InputSource::QuickAccess));
        assert!(!g.has(&InputSource::LeftGrip2));
        assert!(g.has(&InputSource::LeftStick));
        assert!(g.has(&InputSource::DPad));
        assert!(Shape::Neptune.has(&InputSource::RightStick));
        assert!(!g.has(&InputSource::LeftGripTouch));
        assert!(!Shape::Neptune.has(&InputSource::LeftGripTouch));
        assert!(Shape::Triton.has(&InputSource::LeftGripTouch));
        // Gordon lacks the 7 Neptune-ups (5 + both stick touches) and the 2 Triton grip-touches.
        assert_eq!(g.inputs().count(), InputSource::ALL.len() - 9);
        // Neptune lacks only the 2 Triton grip-touches; Triton has everything.
        assert_eq!(Shape::Neptune.inputs().count(), InputSource::ALL.len() - 2);
        assert_eq!(Shape::Triton.inputs().count(), InputSource::ALL.len());
    }

    #[test]
    fn sides_and_kinds() {
        assert_eq!(InputSource::FaceButtons.kind(), SourceKind::ButtonGroup);
        assert_eq!(InputSource::LeftPad.kind(), SourceKind::Pad);
        assert_eq!(InputSource::RightTriggerFull.kind(), SourceKind::Button);
        assert_eq!(InputSource::FaceButtons.side(), Side::Right);
        assert_eq!(InputSource::DPad.side(), Side::Left);
        assert_eq!(InputSource::Steam.side(), Side::Left);
        assert_eq!(InputSource::Menu.side(), Side::Right);
    }

    #[test]
    fn input_source_round_trips_ron() {
        let src = InputSource::LeftTrigger;
        let s = ron::to_string(&src).unwrap();
        let back: InputSource = ron::from_str(&s).unwrap();
        assert_eq!(src, back);
    }
}
