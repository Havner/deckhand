//! [`LogicalFrame`] - the engine's view of a controller frame keyed by [`InputSource`]
//! (PLAN 4.1/4.2 S3).
//!
//! The mapper works in the *logical* vocabulary ([`config::InputSource`]), not `steam-hid`'s
//! wire buttons. This is the fixed lens that answers "what is `InputSource` X doing this
//! frame?" over a [`ControllerState`].
//!
//! **It is device-independent** - because `steam-hid` already normalizes each device's raw
//! bits into the unified [`ControllerState`] (the Gordon multiplex, the Neptune separate
//! fields, etc. are resolved upstream). So there's no per-`DeviceKind` branch here; the only
//! device difference is that inputs a device lacks read as zero/unset (Gordon's right stick,
//! say) and their behaviors naturally no-op. `Shape`-based skipping/warnings live in the UI.
//!
//! `steam-hid` and `config` share Valve's on-device labels for the two small top buttons:
//! `View` ([copy], left = select) and `Menu` ([menu], right = start). So the mapping here is the
//! identity `View -> VIEW`, `Menu -> MENU` - no inversion. (`steam-hid` previously carried the
//! C#-inherited `MENU`/`OPTIONS` labels, where `MENU` was actually the *left/select* button;
//! both crates were unified onto Valve's names.)

use config::InputSource;
use vocab_hid::{Button, Buttons, ControllerState, TrackPad, Vec2, Vec3i};

/// A direction within a directional source (button group, dpad, joystick ring). Reused by
/// the `ButtonPad`/`DirectionalPad` behaviors (S6).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// One controller frame, viewed through the logical [`InputSource`] vocabulary. Owns the
/// snapshot so the mapper can retain the previous frame for edge detection.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalFrame {
    state: ControllerState,
}

impl LogicalFrame {
    pub fn new(state: ControllerState) -> Self {
        LogicalFrame { state }
    }

    /// The underlying unified snapshot (for `seq`/`timestamp`/IMU raw access).
    pub fn state(&self) -> &ControllerState {
        &self.state
    }

    /// A copy with the given raw controller buttons cleared - used to **consume** the buttons a
    /// global chord fired on, so profile bindings don't also see them (PLAN 3 Round E / 4).
    pub fn masked(&self, consumed: &[Button]) -> LogicalFrame {
        let mut state = self.state.clone();
        for b in consumed {
            state.buttons.remove(vocab_hid::button_flag(b));
        }
        LogicalFrame::new(state)
    }

    /// Digital level of a **standalone button** input (bumpers, grips, system buttons,
    /// clicks, touches, full-pulls). `false` for non-button sources.
    pub fn button(&self, source: &InputSource) -> bool {
        button_flag(source).is_some_and(|f| self.state.buttons.contains(f))
    }

    /// Whether a raw controller [`Button`] is held. Chords and gaters name hardware buttons
    /// directly (any bit - including face buttons / dpad directions), so they use this rather than
    /// the [`InputSource`]-keyed [`button`](Self::button).
    pub fn button_held(&self, b: &Button) -> bool {
        self.state.buttons.contains(vocab_hid::button_flag(b))
    }

    /// Digital level of one member of a **button group** (`FaceButtons`/`DPad`). `false` for
    /// non-group sources or when the group lacks that direction.
    pub fn group_member(&self, group: &InputSource, dir: &Dir) -> bool {
        group_member_flag(group, dir).is_some_and(|f| self.state.buttons.contains(f))
    }

    /// Position of a **stick or pad** source, `-1.0..=1.0` per axis (pads report their
    /// touch position). Zero for other sources.
    pub fn pos(&self, source: &InputSource) -> Vec2 {
        match source {
            InputSource::LeftStick => self.state.left_stick.clone(),
            InputSource::RightStick => self.state.right_stick.clone(),
            InputSource::LeftPad => self.state.left_pad.pos.clone(),
            InputSource::RightPad => self.state.right_pad.pos.clone(),
            _ => Vec2::default(),
        }
    }

    /// The full trackpad reading (pos + pressure + touched) for a **pad** source.
    pub fn pad(&self, source: &InputSource) -> Option<&TrackPad> {
        match source {
            InputSource::LeftPad => Some(&self.state.left_pad),
            InputSource::RightPad => Some(&self.state.right_pad),
            _ => None,
        }
    }

    /// Analog pull of a **trigger** source, `0.0..=1.0`. Zero for other sources. (The digital
    /// full-pull is a separate `LeftTriggerFull`/`RightTriggerFull` button - see [`Self::button`].)
    pub fn trigger(&self, source: &InputSource) -> f32 {
        match source {
            InputSource::LeftTrigger => self.state.left_trigger,
            InputSource::RightTrigger => self.state.right_trigger,
            _ => 0.0,
        }
    }

    /// Angular velocity (gyro), raw device units (`GYRO_RES_PER_DPS = 16`; PLAN 1.9).
    pub fn gyro(&self) -> &Vec3i {
        &self.state.gyro
    }

    /// Specific force (accel), raw device units (`ACCEL_RES_PER_G = 16384`).
    pub fn accel(&self) -> &Vec3i {
        &self.state.accel
    }
}

/// The unified button bit for a standalone-button [`InputSource`], if it is one.
fn button_flag(source: &InputSource) -> Option<Buttons> {
    use InputSource as I;
    Some(match source {
        I::LeftBumper => Buttons::LB,
        I::RightBumper => Buttons::RB,
        I::LeftTriggerFull => Buttons::LT,
        I::RightTriggerFull => Buttons::RT,
        I::LeftGrip => Buttons::LGRIP,
        I::RightGrip => Buttons::RGRIP,
        I::LeftGrip2 => Buttons::LGRIP2,
        I::RightGrip2 => Buttons::RGRIP2,
        // Valve labels, unified across crates: View = left/select, Menu = right/start.
        I::View => Buttons::VIEW,
        I::Menu => Buttons::MENU,
        I::Steam => Buttons::STEAM,
        I::QuickAccess => Buttons::QUICK_ACCESS,
        I::LeftStickClick => Buttons::LSTICK_PRESS,
        I::RightStickClick => Buttons::RSTICK_PRESS,
        I::LeftStickTouch => Buttons::LSTICK_TOUCH,
        I::RightStickTouch => Buttons::RSTICK_TOUCH,
        I::LeftPadClick => Buttons::LPAD_PRESS,
        I::RightPadClick => Buttons::RPAD_PRESS,
        I::LeftPadTouch => Buttons::LPAD_TOUCH,
        I::RightPadTouch => Buttons::RPAD_TOUCH,
        I::LeftGripTouch => Buttons::LGRIP_TOUCH,
        I::RightGripTouch => Buttons::RGRIP_TOUCH,
        _ => return None,
    })
}

/// The unified button bit for a member of a button group (`FaceButtons` diamond / `DPad`).
fn group_member_flag(group: &InputSource, dir: &Dir) -> Option<Buttons> {
    use InputSource as I;
    Some(match (group, dir) {
        // FaceButtons diamond: top = Y, bottom = A, left = X, right = B.
        (I::FaceButtons, Dir::Up) => Buttons::Y,
        (I::FaceButtons, Dir::Down) => Buttons::A,
        (I::FaceButtons, Dir::Left) => Buttons::X,
        (I::FaceButtons, Dir::Right) => Buttons::B,
        (I::DPad, Dir::Up) => Buttons::DPAD_UP,
        (I::DPad, Dir::Down) => Buttons::DPAD_DOWN,
        (I::DPad, Dir::Left) => Buttons::DPAD_LEFT,
        (I::DPad, Dir::Right) => Buttons::DPAD_RIGHT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_with(buttons: Buttons, f: impl FnOnce(&mut ControllerState)) -> LogicalFrame {
        let mut s = ControllerState { buttons, ..Default::default() };
        f(&mut s);
        LogicalFrame::new(s)
    }

    #[test]
    fn view_menu_naming() {
        // VIEW bit (left/select) = View pressed, Menu not.
        let f = frame_with(Buttons::VIEW, |_| {});
        assert!(f.button(&InputSource::View));
        assert!(!f.button(&InputSource::Menu));

        // MENU bit (right/start) = Menu pressed, View not.
        let f = frame_with(Buttons::MENU, |_| {});
        assert!(f.button(&InputSource::Menu));
        assert!(!f.button(&InputSource::View));
    }

    #[test]
    fn standalone_buttons_and_group_members() {
        let f = frame_with(Buttons::LB | Buttons::Y | Buttons::LT, |_| {});
        assert!(f.button(&InputSource::LeftBumper)); // L1
        assert!(f.button(&InputSource::LeftTriggerFull)); // L2
        assert!(!f.button(&InputSource::RightBumper)); // R1 unset
        assert!(f.group_member(&InputSource::FaceButtons, &Dir::Up)); // Y
        assert!(!f.group_member(&InputSource::FaceButtons, &Dir::Down)); // A unset
        // A ButtonGroup source is not a standalone button.
        assert!(!f.button(&InputSource::FaceButtons));
    }

    #[test]
    fn analog_accessors() {
        let f = frame_with(Buttons::empty(), |s| {
            s.left_stick = Vec2 { x: 0.5, y: -0.5 };
            s.left_trigger = 0.7;
            s.left_pad = TrackPad { pos: Vec2 { x: 0.1, y: 0.2 }, pressure: 0.3, touched: true };
        });
        assert_eq!(f.pos(&InputSource::LeftStick), Vec2 { x: 0.5, y: -0.5 });
        assert_eq!(f.pos(&InputSource::LeftPad), Vec2 { x: 0.1, y: 0.2 });
        assert_eq!(f.trigger(&InputSource::LeftTrigger), 0.7);
        assert!(f.pad(&InputSource::LeftPad).unwrap().touched);
        assert!(f.pad(&InputSource::LeftStick).is_none());
        // Absent input (Gordon right stick) reads zero -> behavior no-ops.
        assert_eq!(f.pos(&InputSource::RightStick), Vec2::default());
    }
}
