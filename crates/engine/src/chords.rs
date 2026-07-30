//! Global chords — the above-profile switch layer (PLAN §3 Round E / §4, §4.2 S9).
//!
//! Global chords are evaluated **before** any profile binding and **consume** their buttons
//! (masked out of the frame the Mapper sees), so a desktop escape hatch works even with the UI
//! down. A `SwitchFallback` chord flips the engine between its **Active** and **Fallback**
//! programs — `Hold` while the chord is held, `Toggle` latched on each engage. `CommandExecute`
//! is deferred (decision B); its buttons are still consumed when the chord is held.
//!
//! Pure and golden-testable — the threaded loop (S9b) owns a [`Chords`] and calls [`Chords::eval`]
//! each frame, then masks the frame and selects the program for the returned [`Role`].

use config::{GlobalAction, GlobalChord, InputSource, SwitchMode};

use crate::logical::LogicalFrame;
use crate::program::Role;

/// Retained per-chord runtime state (toggle latch + engage edge).
#[derive(Default)]
pub(crate) struct Chords {
    /// The persistent base "fallback engaged?" state — seeded from `GlobalConfig::start_profile`
    /// and flipped by each `SwitchFallback` **Toggle**. Survives releases (unlike a hold), so a
    /// start-in-Fallback boot sticks until toggled. A **Hold** chord forces Fallback *on top* of
    /// this while held.
    persistent_fallback: bool,
    states: Vec<ChordState>,
}

#[derive(Default, Clone)]
struct ChordState {
    prev_active: bool,
}

/// The outcome of evaluating the chords for one frame.
pub(crate) struct ChordOutcome {
    /// Buttons an active chord consumed — mask these out before the Mapper.
    pub consumed: Vec<InputSource>,
    /// Which program role should drive this frame.
    pub role: Role,
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
    /// buttons); a `SwitchFallback` Toggle flips the persistent base on its rising edge, a Hold
    /// forces Fallback while held. Role = base OR any hold.
    pub fn eval(&mut self, chords: &[GlobalChord], frame: &LogicalFrame) -> ChordOutcome {
        if self.states.len() != chords.len() {
            self.states = vec![ChordState::default(); chords.len()];
        }
        let mut consumed = Vec::new();
        let mut hold_fallback = false;
        let mut base = self.persistent_fallback;

        for (chord, st) in chords.iter().zip(self.states.iter_mut()) {
            // AND-combined physical buttons; an empty chord never fires.
            let active = !chord.buttons.is_empty() && chord.buttons.iter().all(|b| frame.button(b));
            if active {
                consumed.extend(chord.buttons.iter().cloned());
            }
            match &chord.action {
                GlobalAction::SwitchFallback { mode } => match mode {
                    SwitchMode::Hold => hold_fallback |= active,
                    SwitchMode::Toggle => {
                        if active && !st.prev_active {
                            base = !base;
                        }
                    }
                },
                // Deferred (decision B): buttons still consumed; subprocess spawn lands later.
                GlobalAction::CommandExecute { .. } => {}
            }
            st.prev_active = active;
        }

        self.persistent_fallback = base;
        let role = if base || hold_fallback { Role::Fallback } else { Role::Active };
        ChordOutcome { consumed, role }
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
        GlobalChord { buttons, action: GlobalAction::SwitchFallback { mode: SwitchMode::Hold } }
    }

    fn toggle_chord(buttons: Vec<InputSource>) -> GlobalChord {
        GlobalChord { buttons, action: GlobalAction::SwitchFallback { mode: SwitchMode::Toggle } }
    }

    #[test]
    fn hold_chord_switches_and_consumes_while_held() {
        let chords = vec![hold_chord(vec![InputSource::Steam, InputSource::RightGrip])];
        let mut c = Chords::new(&chords, false);

        // Both held → Fallback, both consumed.
        let out = c.eval(&chords, &frame(Buttons::STEAM | Buttons::R4));
        assert_eq!(out.role, Role::Fallback);
        assert!(out.consumed.contains(&InputSource::Steam) && out.consumed.contains(&InputSource::RightGrip));

        // Only one held → Active, nothing consumed.
        let out = c.eval(&chords, &frame(Buttons::STEAM));
        assert_eq!(out.role, Role::Active);
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
        assert_eq!(c.eval(&chords, &both()).role, Role::Active); // press again → latch off
        assert_eq!(c.eval(&chords, &none()).role, Role::Active);
    }

    #[test]
    fn no_chords_is_always_active() {
        let chords: Vec<GlobalChord> = vec![];
        let mut c = Chords::new(&chords, false);
        let out = c.eval(&chords, &frame(Buttons::STEAM | Buttons::R4));
        assert_eq!(out.role, Role::Active);
        assert!(out.consumed.is_empty());
    }

    #[test]
    fn start_in_fallback_persists_then_toggles() {
        // Seeded start-in-Fallback: the first (idle) frame is already Fallback, and a Toggle chord
        // switches to Active — no spurious flip to Active on frame 1.
        let chords = vec![toggle_chord(vec![InputSource::Steam, InputSource::RightGrip])];
        let mut c = Chords::new(&chords, true);
        assert_eq!(c.eval(&chords, &frame(Buttons::empty())).role, Role::Fallback); // idle → stays
        assert!(c.fallback_base());
        assert_eq!(c.eval(&chords, &frame(Buttons::STEAM | Buttons::R4)).role, Role::Active); // toggle
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
