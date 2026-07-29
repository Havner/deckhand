//! Command evaluation — a button-like node's commands → output levels (PLAN §4 / Round C).
//!
//! Shared by physical buttons and every behavior virtual button (soft-pull, outer-ring, dpad
//! directions, button-pad members): each computes its held-state and calls [`eval_commands`],
//! so there are no special cases (Round A's uniform digital model).
//!
//! **S6:** only the held-while-held `Regular` activator is honoured. S7 replaces this with the
//! full activator state machine (press/release edges, `Long`/`Double` timers, `toggle`/`turbo`/
//! `interruptible`), keyed by resolved-binding id — the signature grows then (prev held-state,
//! the injected [`Tick`](super::Tick), the activator table).

use config::Activator;

use super::reconcile::DesiredLevels;
use crate::program::{CompiledAction, CompiledCommand};

/// Fold a node's commands into `desired` given whether the node is held this tick.
pub(super) fn eval_commands(commands: &[CompiledCommand], held: bool, desired: &mut DesiredLevels) {
    if !held {
        return;
    }
    for cmd in commands {
        // S6: honour only the held-while-held `Regular` activator; others fire in S7.
        if !matches!(cmd.activator, Activator::Regular) {
            continue;
        }
        // The whole ordered combo is held while the node is held (subcommands = modifiers).
        for action in &cmd.actions {
            apply_output(action, desired);
        }
    }
}

/// Fold one output-leaf action into the desired levels. Scroll impulses (S7), layer/set
/// actions (S8), and `None` produce no *level* here.
fn apply_output(action: &CompiledAction, desired: &mut DesiredLevels) {
    match action {
        CompiledAction::Key(k) => desired.press_key(k.clone()),
        CompiledAction::MouseButton(b) if !b.is_scroll() => desired.press_mouse(b.clone()),
        CompiledAction::GamepadButton(b) => desired.press_pad(b.clone()),
        _ => {}
    }
}
