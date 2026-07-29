//! Command evaluation — a button-like node's commands → output levels, through the activator
//! state machine (PLAN §4 step 5 / Round C, §4.2 S7).
//!
//! Shared by physical buttons and every behavior virtual button (soft-pull, outer-ring, dpad
//! directions, button-pad members): each computes its held-state and calls [`eval_commands`]
//! with that node's [`SlotState`], so there are no special cases (Round A's uniform digital
//! model).
//!
//! Activator timing runs off the injected [`Tick`] (deterministic, golden-testable). **S7a**
//! implements the five activator *types* (`Regular`/`Start`/`Long`/`Double`/`Release`); the
//! `toggle`/`turbo`/`interruptible` **settings** are S7b, and layer/set action firing is S8
//! (their output leaves are ignored here for now).

use config::Activator;

use super::activator::SlotState;
use super::reconcile::DesiredLevels;
use super::Tick;
use crate::program::{CompiledAction, CompiledCommand};

/// How long a tap-style activator (`Start`/`Release`) holds its action from the trigger edge,
/// in ms — long enough for a game to register, short enough to feel like a tap. HW-tuned later.
const TAP_MS: u64 = 40;

/// Advance a node's commands one tick and fold the firing ones' output into `desired`.
///
/// `held` is the node's digital level this tick; `slot` carries its retained timing/latches.
pub(super) fn eval_commands(
    commands: &[CompiledCommand],
    held: bool,
    slot: &mut SlotState,
    now: &Tick,
    desired: &mut DesiredLevels,
) {
    let pressed = held && !slot.prev_held;
    let released = !held && slot.prev_held;

    // Press timing must be visible to the commands this tick; release timing updates *after*
    // (so `Double` sees the *previous* release, not this one).
    if pressed {
        slot.press_start = Some(now.clone());
    }
    let press_start = slot.press_start.clone();
    let last_release = slot.last_release.clone();

    for (i, cmd) in commands.iter().enumerate() {
        let cs = slot.command(i);
        if fires(&cmd.activator, held, pressed, released, &press_start, &last_release, cs, now) {
            // The whole ordered combo is held while the command fires (subcommands = modifiers).
            for action in &cmd.actions {
                apply_output(action, desired);
            }
        }
    }

    if released {
        slot.last_release = Some(now.clone());
        slot.press_start = None;
    }
    slot.prev_held = held;
}

/// Whether a command's actions should be held **this tick**, per its activator type (S7a).
#[allow(clippy::too_many_arguments)] // an activator reads the whole per-node edge/timing frame.
fn fires(
    activator: &Activator,
    held: bool,
    pressed: bool,
    released: bool,
    press_start: &Option<Tick>,
    last_release: &Option<Tick>,
    cs: &mut super::activator::CmdState,
    now: &Tick,
) -> bool {
    match activator {
        // Held while the node is held.
        Activator::Regular => held,
        // One-shot tap from the press edge.
        Activator::Start => {
            if pressed {
                cs.tap_until = Some(Tick(now.0 + TAP_MS));
            }
            tap_active(cs, now)
        }
        // Held once the press has lasted `hold_ms`, until release.
        Activator::Long { hold_ms } => {
            held && press_start.as_ref().is_some_and(|ps| now.0.saturating_sub(ps.0) >= *hold_ms as u64)
        }
        // Held while a press that landed within `window_ms` of the previous release is held.
        Activator::Double { window_ms } => {
            if pressed {
                cs.double_active = last_release
                    .as_ref()
                    .is_some_and(|lr| now.0.saturating_sub(lr.0) <= *window_ms as u64);
            }
            if released {
                cs.double_active = false;
            }
            held && cs.double_active
        }
        // One-shot tap from the release edge.
        Activator::Release => {
            if released {
                cs.tap_until = Some(Tick(now.0 + TAP_MS));
            }
            tap_active(cs, now)
        }
    }
}

fn tap_active(cs: &super::activator::CmdState, now: &Tick) -> bool {
    cs.tap_until.as_ref().is_some_and(|tu| now.0 < tu.0)
}

/// Fold one output-leaf action into the desired levels. Scroll impulses (S7b), layer/set
/// actions (S8), and `None` produce no *level* here.
fn apply_output(action: &CompiledAction, desired: &mut DesiredLevels) {
    match action {
        CompiledAction::Key(k) => desired.press_key(k.clone()),
        CompiledAction::MouseButton(b) if !b.is_scroll() => desired.press_mouse(b.clone()),
        CompiledAction::GamepadButton(b) => desired.press_pad(b.clone()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::CompiledAction;
    use config::CommandSettings;
    use vocab::Key;

    fn cmd(activator: Activator) -> CompiledCommand {
        CompiledCommand {
            activator,
            actions: vec![CompiledAction::Key(Key::A)],
            settings: CommandSettings::default(),
        }
    }

    /// Run one tick over a single command; return whether its key is desired-held.
    fn step(command: &CompiledCommand, slot: &mut SlotState, held: bool, now: u64) -> bool {
        let mut d = DesiredLevels::default();
        eval_commands(std::slice::from_ref(command), held, slot, &Tick(now), &mut d);
        d.has_key(&Key::A)
    }

    #[test]
    fn regular_holds_while_held() {
        let c = cmd(Activator::Regular);
        let mut s = SlotState::default();
        assert!(step(&c, &mut s, true, 0)); // press → held
        assert!(step(&c, &mut s, true, 4)); // still held
        assert!(!step(&c, &mut s, false, 8)); // release → up
    }

    #[test]
    fn start_taps_on_press_then_releases_even_if_held() {
        let c = cmd(Activator::Start);
        let mut s = SlotState::default();
        assert!(step(&c, &mut s, true, 0)); // fires on the press edge
        assert!(step(&c, &mut s, true, 20)); // within TAP_MS, still down
        assert!(!step(&c, &mut s, true, 60)); // past TAP_MS (40) → up, even though still held
    }

    #[test]
    fn long_fires_only_after_threshold() {
        let c = cmd(Activator::Long { hold_ms: 100 });
        let mut s = SlotState::default();
        assert!(!step(&c, &mut s, true, 0)); // just pressed
        assert!(!step(&c, &mut s, true, 50)); // not yet
        assert!(step(&c, &mut s, true, 100)); // threshold reached
        assert!(step(&c, &mut s, true, 150)); // stays held
        assert!(!step(&c, &mut s, false, 160)); // released
    }

    #[test]
    fn double_fires_on_second_press_within_window() {
        let c = cmd(Activator::Double { window_ms: 200 });
        let mut s = SlotState::default();
        assert!(!step(&c, &mut s, true, 0)); // first press → not a double
        assert!(!step(&c, &mut s, false, 20)); // release
        assert!(step(&c, &mut s, true, 60)); // second press within 200ms → fires
        assert!(step(&c, &mut s, true, 64)); // held
        assert!(!step(&c, &mut s, false, 70)); // release → clears

        // A second press *outside* the window does not fire.
        let mut s = SlotState::default();
        assert!(!step(&c, &mut s, true, 0));
        assert!(!step(&c, &mut s, false, 20));
        assert!(!step(&c, &mut s, true, 500)); // 480ms later → no double
    }

    #[test]
    fn release_taps_on_release_edge() {
        let c = cmd(Activator::Release);
        let mut s = SlotState::default();
        assert!(!step(&c, &mut s, true, 0)); // press → nothing
        assert!(!step(&c, &mut s, true, 10)); // held → nothing
        assert!(step(&c, &mut s, false, 20)); // release → taps
        assert!(step(&c, &mut s, false, 40)); // within TAP_MS
        assert!(!step(&c, &mut s, false, 80)); // past TAP_MS → up
    }

    #[test]
    fn short_press_long_activator_never_fires() {
        // A short tap on a Long activator: pressed then released before the threshold → nothing.
        let c = cmd(Activator::Long { hold_ms: 100 });
        let mut s = SlotState::default();
        assert!(!step(&c, &mut s, true, 0));
        assert!(!step(&c, &mut s, true, 30));
        assert!(!step(&c, &mut s, false, 50)); // released early
    }
}
