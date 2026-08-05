//! Global chords — the above-profile switch layer (PLAN §3 Round E / §4, §4.2 S9).
//!
//! Global chords are evaluated **before** any profile binding and **consume** their buttons
//! (masked out of the frame the Mapper sees), so a desktop escape hatch works even with the UI
//! down. A `SwitchProfile` chord flips the engine between its **Main** and **Fallback**
//! programs — `HoldFallback` while the chord is held, `Toggle` latched on each engage. A `CommandExecute`
//! chord runs a headless external program on its **engage edge** (buttons still consumed).
//!
//! Pure and golden-testable — the threaded loop (S9b) owns a [`Chords`] and calls [`Chords::eval`]
//! each frame, then masks the frame, spawns any [`ExecReq`], and selects the program for the
//! returned [`Role`]. `eval` itself never spawns (that's the runtime's job), so it stays testable.

use config::{GlobalAction, GlobalChord, InputSource, SwitchMode};

use crate::logical::LogicalFrame;
use crate::program::Role;

/// Retained per-chord runtime state (toggle latch + engage edge).
#[derive(Default)]
pub(crate) struct Chords {
    /// The persistent base "fallback engaged?" state — seeded from `GlobalConfig::start_profile`
    /// and flipped by each `SwitchProfile` **Toggle**. Survives releases (unlike a hold), so a
    /// start-in-Fallback boot sticks until toggled. A **HoldFallback** chord forces Fallback *on top* of
    /// this while held.
    persistent_fallback: bool,
    states: Vec<ChordState>,
}

#[derive(Default, Clone)]
struct ChordState {
    prev_active: bool,
}

/// A `CommandExecute` chord that engaged this frame — the runtime spawns the program (headless).
pub(crate) struct ExecReq {
    pub command: String,
    pub args: Vec<String>,
}

/// The outcome of evaluating the chords for one frame.
pub(crate) struct ChordOutcome {
    /// Buttons an active chord consumed — mask these out before the Mapper.
    pub consumed: Vec<InputSource>,
    /// Which program role should drive this frame.
    pub role: Role,
    /// `CommandExecute` chords that engaged this frame (rising edge) — the runtime runs each.
    pub execute: Vec<ExecReq>,
}

impl Chords {
    /// A fresh runtime for `chords`, starting in `Fallback` if `start_fallback` (from
    /// `GlobalConfig::start_profile`). On a live `GlobalConfig` hot-swap, pass the current base
    /// ([`Self::fallback_base`]) instead — `start_profile` is a start-only setting.
    pub fn new(chords: &[GlobalChord], start_fallback: bool) -> Self {
        Chords { persistent_fallback: start_fallback, states: vec![ChordState::default(); chords.len()] }
    }

    /// The current persistent base role (`true` = Fallback) — used to preserve the role across a
    /// hot-swap of the globals (so `start_profile` doesn't retroactively yank the role).
    pub fn fallback_base(&self) -> bool {
        self.persistent_fallback
    }

    /// Evaluate every chord against `frame`: an all-buttons-held chord is *active* (consumes its
    /// buttons); a `SwitchProfile` Toggle flips the persistent base on its rising edge, a HoldFallback
    /// forces Fallback while held. Role = base OR any hold.
    pub fn eval(&mut self, chords: &[GlobalChord], frame: &LogicalFrame) -> ChordOutcome {
        if self.states.len() != chords.len() {
            self.states = vec![ChordState::default(); chords.len()];
        }
        let mut consumed = Vec::new();
        let mut hold_fallback = false;
        let mut base = self.persistent_fallback;
        let mut execute = Vec::new();

        for (chord, st) in chords.iter().zip(self.states.iter_mut()) {
            // AND-combined physical buttons; an empty chord never fires.
            let active = !chord.buttons.is_empty() && chord.buttons.iter().all(|b| frame.button(b));
            if active {
                consumed.extend(chord.buttons.iter().cloned());
            }
            match &chord.action {
                GlobalAction::SwitchProfile { mode } => match mode {
                    SwitchMode::HoldFallback => hold_fallback |= active,
                    SwitchMode::Toggle => {
                        if active && !st.prev_active {
                            base = !base;
                        }
                    }
                    SwitchMode::SetMain => {
                        if active && !st.prev_active {
                            base = false;
                        }
                    }
                    SwitchMode::SetFallback => {
                        if active && !st.prev_active {
                            base = true;
                        }
                    }
                },
                // Fire once on the engage edge; the runtime spawns it (eval stays side-effect-free).
                GlobalAction::CommandExecute { command, args } => {
                    if active && !st.prev_active {
                        execute.push(ExecReq { command: command.clone(), args: args.clone() });
                    }
                }
            }
            st.prev_active = active;
        }

        self.persistent_fallback = base;
        let role = if base || hold_fallback { Role::Fallback } else { Role::Main };
        ChordOutcome { consumed, role, execute }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steam_hid::{Buttons, ControllerState};

    fn frame(buttons: Buttons) -> LogicalFrame {
        LogicalFrame::new(ControllerState { buttons, ..Default::default() })
    }

    fn hold_chord(buttons: Vec<InputSource>) -> GlobalChord {
        GlobalChord { buttons, action: GlobalAction::SwitchProfile { mode: SwitchMode::HoldFallback } }
    }

    fn toggle_chord(buttons: Vec<InputSource>) -> GlobalChord {
        GlobalChord { buttons, action: GlobalAction::SwitchProfile { mode: SwitchMode::Toggle } }
    }

    fn set_chord(buttons: Vec<InputSource>, mode: SwitchMode) -> GlobalChord {
        GlobalChord { buttons, action: GlobalAction::SwitchProfile { mode } }
    }

    #[test]
    fn hold_chord_switches_and_consumes_while_held() {
        let chords = vec![hold_chord(vec![InputSource::Steam, InputSource::RightGrip])];
        let mut c = Chords::new(&chords, false);

        // Both held → Fallback, both consumed.
        let out = c.eval(&chords, &frame(Buttons::STEAM | Buttons::R4));
        assert_eq!(out.role, Role::Fallback);
        assert!(out.consumed.contains(&InputSource::Steam) && out.consumed.contains(&InputSource::RightGrip));

        // Only one held → Main, nothing consumed.
        let out = c.eval(&chords, &frame(Buttons::STEAM));
        assert_eq!(out.role, Role::Main);
        assert!(out.consumed.is_empty());
    }

    #[test]
    fn toggle_chord_latches_on_each_engage() {
        let chords = vec![toggle_chord(vec![InputSource::Steam, InputSource::RightGrip])];
        let mut c = Chords::new(&chords, false);
        let both = || frame(Buttons::STEAM | Buttons::R4);
        let none = || frame(Buttons::empty());

        assert_eq!(c.eval(&chords, &both()).role, Role::Fallback); // press → latch on
        assert_eq!(c.eval(&chords, &both()).role, Role::Fallback); // held → stays
        assert_eq!(c.eval(&chords, &none()).role, Role::Fallback); // release → stays latched
        assert_eq!(c.eval(&chords, &both()).role, Role::Main); // press again → latch off
        assert_eq!(c.eval(&chords, &none()).role, Role::Main);
    }

    #[test]
    fn command_execute_fires_once_on_engage_and_consumes() {
        let chords = vec![GlobalChord {
            buttons: vec![InputSource::Steam, InputSource::RightGrip],
            action: GlobalAction::CommandExecute { command: "true".into(), args: vec!["x".into()] },
        }];
        let mut c = Chords::new(&chords, false);
        let both = || frame(Buttons::STEAM | Buttons::R4);

        // Engage → one exec req (command + args), buttons consumed, role untouched (stays Main).
        let out = c.eval(&chords, &both());
        assert_eq!(out.execute.len(), 1);
        assert_eq!((out.execute[0].command.as_str(), out.execute[0].args.as_slice()), ("true", &["x".to_string()][..]));
        assert!(out.consumed.contains(&InputSource::Steam));
        assert_eq!(out.role, Role::Main);

        // Held → no repeat (edge only).
        assert!(c.eval(&chords, &both()).execute.is_empty());
        // Release then re-engage → fires again.
        c.eval(&chords, &frame(Buttons::empty()));
        assert_eq!(c.eval(&chords, &both()).execute.len(), 1);
    }

    #[test]
    fn set_main_and_set_fallback_latch_specific_roles_on_engage() {
        // SetFallback and SetMain latch a specific persistent base on the engage edge, unlike
        // Toggle (relative) — engaging the same one twice is idempotent, and each is a no-op if
        // already in the target role.
        let to_fb = set_chord(vec![InputSource::Steam, InputSource::RightGrip], SwitchMode::SetFallback);
        let to_main = set_chord(vec![InputSource::View, InputSource::LeftGrip], SwitchMode::SetMain);
        let chords = vec![to_fb, to_main];
        let mut c = Chords::new(&chords, false); // boot in Main

        let fb = || frame(Buttons::STEAM | Buttons::R4);
        let main = || frame(Buttons::VIEW | Buttons::L4);
        let none = || frame(Buttons::empty());

        assert_eq!(c.eval(&chords, &fb()).role, Role::Fallback); // engage SetFallback → latch
        assert_eq!(c.eval(&chords, &fb()).role, Role::Fallback); // held → idempotent
        assert_eq!(c.eval(&chords, &none()).role, Role::Fallback); // release → stays latched
        assert_eq!(c.eval(&chords, &fb()).role, Role::Fallback); // re-engage same → still Fallback
        assert_eq!(c.eval(&chords, &main()).role, Role::Main); // engage SetMain → latch to Main
        assert_eq!(c.eval(&chords, &none()).role, Role::Main); // release → stays Main
    }

    #[test]
    fn no_chords_is_always_active() {
        let chords: Vec<GlobalChord> = vec![];
        let mut c = Chords::new(&chords, false);
        let out = c.eval(&chords, &frame(Buttons::STEAM | Buttons::R4));
        assert_eq!(out.role, Role::Main);
        assert!(out.consumed.is_empty());
    }

    #[test]
    fn start_in_fallback_persists_then_toggles() {
        // Seeded start-in-Fallback: the first (idle) frame is already Fallback, and a Toggle chord
        // switches to Main — no spurious flip to Main on frame 1.
        let chords = vec![toggle_chord(vec![InputSource::Steam, InputSource::RightGrip])];
        let mut c = Chords::new(&chords, true);
        assert_eq!(c.eval(&chords, &frame(Buttons::empty())).role, Role::Fallback); // idle → stays
        assert!(c.fallback_base());
        assert_eq!(c.eval(&chords, &frame(Buttons::STEAM | Buttons::R4)).role, Role::Main); // toggle
        assert!(!c.fallback_base());
    }

    #[test]
    fn start_in_fallback_sticks_even_without_a_chord() {
        // With no chord there's nothing to flip it, so a Fallback boot simply stays Fallback.
        let chords: Vec<GlobalChord> = vec![];
        let mut c = Chords::new(&chords, true);
        assert_eq!(c.eval(&chords, &frame(Buttons::empty())).role, Role::Fallback);
    }

    #[test]
    fn consumed_buttons_are_masked_from_the_frame() {
        // The loop masks the consumed buttons so profile bindings don't also see them.
        let f = frame(Buttons::STEAM | Buttons::R4 | Buttons::L1);
        let masked = f.masked(&[InputSource::Steam, InputSource::RightGrip]);
        assert!(!masked.button(&InputSource::Steam));
        assert!(!masked.button(&InputSource::RightGrip));
        assert!(masked.button(&InputSource::LeftBumper)); // an unconsumed button survives
    }
}
