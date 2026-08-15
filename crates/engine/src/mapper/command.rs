//! Command evaluation — a button-like node's commands → output levels, through the activator
//! state machine (PLAN §4 step 5 / Round C, §4.2 S7).
//!
//! Shared by physical buttons and every behavior virtual button (soft-pull, outer-ring, dpad
//! directions, button-pad members): each computes its held-state and calls [`eval_commands`]
//! with that node's [`SlotState`], so there are no special cases (Round A's uniform digital
//! model). Timing runs off the injected [`Tick`] (deterministic, golden-testable).
//!
//! # The activator model (the rules a node's commands obey together)
//!
//! Five activators, in two roles:
//!
//! - **`Start` / `Release`** — independent one-shot taps (on the press / release edge). They never
//!   interrupt and are never interrupted; they don't participate in anything below.
//! - **`Long` / `Double`** — the *interrupters* and *hold-takers*. Each fires when its condition
//!   holds (`Long`: held past `hold_ms`; `Double`: a second press within `window_ms` **of the first
//!   press**) and then **holds from that point until release**. They **never contest each other** —
//!   several may be active at once and they all stay held (`double(200),long(300),long(500)` held
//!   long → all three down until you release). While any of them is active, an interruptible
//!   `Regular` on the node is killed.
//! - **`Regular`** — the only interruptible command, and the whole model collapses to one rule:
//!
//!   > **A `Regular` presses-and-holds the moment it becomes *safe* from interruption** — where
//!   > "safe" = no `Long`/`Double` on the node can still fire from this interaction. If it's safe at
//!   > press (no interrupters present at all) it holds from press; if it becomes safe later *while
//!   > still held* it holds from that moment; if it only becomes safe after release (or the button
//!   > is already up) it's a `TAP_MS` tap; if a `Long`/`Double` fires before it's safe, it is
//!   > interrupted and outputs nothing.
//!
//!   Consequences (all fall out of the one rule):
//!   - No interrupters on the node → safe at press → **plain press/release hold** (`interruptible`
//!     is a no-op).
//!   - `Long` present, short press → released before the `Long` fired → safe on release, button up
//!     → **tap**. Hold past the threshold → the `Long` fires → **interrupted**.
//!   - `Double` present → safe only when the window has closed (measured from the first press). A
//!     quick press → **tap after the window**; a **press-and-hold** past the window (no `Long` to
//!     interrupt) → a **real, delayed press-and-hold**; a genuine second press → `Double` fires,
//!     `Regular` **interrupted**.
//!   - Why "no `Long` ⇒ can real-hold": a `Long` either fires (interrupt) or, while you hold below
//!     its threshold, keeps threatening — there is no "safe while held" with a `Long` present, so a
//!     committed hold only happens with `Double`(s) and no `Long`.
//!
//! `toggle`/`turbo`/haptics are per-command post-processing on the resolved level (S7b); they are
//! meaningful on holds — on the one-shot taps they are effectively inert (and the editor hides the
//! ones that don't apply).

use config::{Activator, HapticEdge, Haptics, Side, Turbo};

use super::activator::{CmdState, Deferred, SlotState};
use super::behavior::Sinks;
use super::layers::{LayerOps, NodeHeld};
use super::reconcile::DesiredLevels;
use super::{HapticReq, Tick};
use crate::program::{CompiledAction, CompiledCommand};

/// How long a tap-style output holds from its trigger edge, in ms — one-shot `Start`/`Release`
/// taps and the committed interruptible-`Regular` tap. Long enough for a game to register, short
/// enough to feel like a tap. HW-tuned later.
const TAP_MS: u64 = 40;

/// Advance a node's commands one tick and apply the firing ones' actions — output leaves into
/// `sinks.desired`, layer/set actions into `sinks.ops` (for the next tick), command-haptic
/// pulses into `sinks.haptics`.
///
/// `held` is the node's digital level this tick; `node` describes how to re-derive that held
/// state later (for `HoldLayer` latching); `slot` carries its retained timing/latches.
///
/// Two passes: pass 1 computes each `Long`/`Double`'s raw activity (and thus whether an interrupter
/// is active); pass 2 resolves every command's output (hold-takers hold, taps tap, the deferred-
/// `Regular` state machine) and applies it. See the module docs for the rules.
pub(super) fn eval_commands(
    commands: &[CompiledCommand],
    held: bool,
    node: &NodeHeld,
    side: &Side,
    slot: &mut SlotState,
    now: &Tick,
    sinks: &mut Sinks,
) {
    let pressed = held && !slot.prev_held;
    let released = !held && slot.prev_held;

    // Node facts: the interrupters present, and the widest double window (the latest a `Double` on
    // this node can still form → the point an interruptible `Regular` is finally safe).
    let has_long = commands.iter().any(|c| matches!(c.activator, Activator::Long { .. }));
    let max_double_window = commands
        .iter()
        .filter_map(|c| match &c.activator {
            Activator::Double { window_ms } => Some(*window_ms as u64),
            _ => None,
        })
        .max();
    let has_interrupter = has_long || max_double_window.is_some();

    // Press timing. `last_press` retains the *previous* press's start for `Double`'s press-to-press
    // window; capture it before this tick's press overwrites it.
    let prev_press = slot.last_press.clone();
    if pressed {
        slot.press_start = Some(now.clone());
        slot.last_press = Some(now.clone());
    } else if released {
        slot.press_start = None;
    }
    let press_start = slot.press_start.clone();

    // --- Pass 1: is each hold-taker (Long/Double) active this tick? They never contest, so this is
    // the whole story for them; the OR is the signal that interrupts a Regular. ---
    let mut hold_active = vec![false; commands.len()];
    for (i, cmd) in commands.iter().enumerate() {
        hold_active[i] = match &cmd.activator {
            Activator::Long { hold_ms } => {
                held && press_start.as_ref().is_some_and(|ps| now.0.saturating_sub(ps.0) >= *hold_ms as u64)
            }
            Activator::Double { window_ms } => {
                let cs = slot.command(i);
                if pressed {
                    cs.double_active =
                        prev_press.as_ref().is_some_and(|pp| now.0.saturating_sub(pp.0) <= *window_ms as u64);
                } else if released {
                    cs.double_active = false;
                }
                held && cs.double_active
            }
            _ => false,
        };
    }
    // Any hold-taker active this tick = the signal that an interruptible `Regular` is interrupted.
    let interrupter_active = hold_active.iter().any(|&a| a);

    // --- Pass 2: resolve each command's output level and apply it. ---
    for (i, cmd) in commands.iter().enumerate() {
        let active = match &cmd.activator {
            // A hold-taker holds from its fire until release; all active ones coexist.
            Activator::Long { .. } | Activator::Double { .. } => hold_active[i],
            // One-shot taps: arm a `TAP_MS` window on the press / release edge, output while it runs.
            Activator::Start => tap_on_edge(slot.command(i), pressed, now),
            Activator::Release => tap_on_edge(slot.command(i), released, now),
            Activator::Regular { interruptible } => {
                if !interruptible || !has_interrupter {
                    held // plain press/release hold: non-interruptible, or nothing to interrupt it
                } else {
                    let cs = slot.command(i);
                    regular_deferred(cs, held, pressed, interrupter_active, has_long, max_double_window, now)
                }
            }
        };

        // `toggle`/`turbo`/haptics on the resolved level (meaningful on holds; inert on the taps).
        let cs = slot.command(i);
        let toggled = apply_toggle(active, cmd.settings.toggle, cs);
        let out = apply_turbo(toggled, cmd.settings.turbo.as_ref(), cs, now);
        emit_haptic(&cmd.settings.haptics, cs, out, side, sinks.haptics);
        if out {
            // The whole ordered combo fires while the command fires (subcommands = modifiers).
            for action in &cmd.actions {
                apply_action(action, node, sinks.desired, sinks.ops);
            }
        }
    }

    slot.prev_held = held;
}

/// The interruptible-`Regular` state machine (only reached when the node has an interrupter). It
/// presses-and-holds as soon as it is safe from interruption, else taps, else nothing. See the
/// module docs for the rule this implements.
fn regular_deferred(
    cs: &mut CmdState,
    held: bool,
    pressed: bool,
    interrupter_active: bool,
    has_long: bool,
    max_double_window: Option<u64>,
    now: &Tick,
) -> bool {
    if pressed {
        cs.deferred = Deferred::Pending; // a fresh interaction (a double-forming press is caught below)
        cs.deferred_start = Some(now.clone());
    }
    match cs.deferred {
        Deferred::Pending => {
            if interrupter_active {
                cs.deferred = Deferred::Idle; // a Long fired (or a Double formed) → interrupted
                return false;
            }
            let start = cs.deferred_start.as_ref().map_or(now.0, |t| t.0);
            let window_closed = now.0.saturating_sub(start) >= max_double_window.unwrap_or(0);
            if held && !has_long && window_closed {
                // Safe while still held (no `Long` to fire, every `Double` window closed) → the real,
                // delayed press-and-hold.
                cs.deferred = Deferred::Holding;
                true
            } else if !held && window_closed {
                // Safe on release (window closed with no second press) → a delayed tap.
                cs.tap_until = Some(Tick(now.0 + TAP_MS));
                cs.deferred = Deferred::Idle;
                true
            } else {
                false // still threatened (a `Long` may fire, or the double window is still open) — wait
            }
        }
        Deferred::Holding => {
            if !held {
                cs.deferred = Deferred::Idle; // physical release ends the hold
            }
            held
        }
        Deferred::Idle => tap_active(cs, now), // a committed tap plays out; otherwise silent
    }
}

/// Push a command-haptic pulse when the command's output crosses the configured edge, tracking the
/// previous level in `cs.haptic_prev`. `Off` never fires (but still advances the edge state).
fn emit_haptic(
    h: &Haptics,
    cs: &mut CmdState,
    out: bool,
    side: &Side,
    haptics: &mut Vec<HapticReq>,
) {
    let rising = out && !cs.haptic_prev;
    let falling = !out && cs.haptic_prev;
    let fire = match h.on {
        HapticEdge::Off => false,
        HapticEdge::OnPress => rising,
        HapticEdge::OnRelease => falling,
        HapticEdge::Both => rising || falling,
    };
    if fire {
        haptics.push(HapticReq { side: side.clone(), strength: h.strength.clone() });
    }
    cs.haptic_prev = out;
}

/// `toggle`: flip a latch on each **rising edge** of the raw activation; output the latch.
fn apply_toggle(raw: bool, toggle: bool, cs: &mut CmdState) -> bool {
    if !toggle {
        return raw;
    }
    if raw && !cs.raw_prev {
        cs.toggle_on = !cs.toggle_on;
    }
    cs.raw_prev = raw;
    cs.toggle_on
}

/// `turbo`: while `active`, emit a 50%-duty square wave of period `interval_ms` (fires at once
/// on activation) so a held output rapid-fires; releases the pulse train when `active` drops.
fn apply_turbo(active: bool, turbo: Option<&Turbo>, cs: &mut CmdState, now: &Tick) -> bool {
    let Some(turbo) = turbo else {
        return active;
    };
    if !active {
        cs.turbo_start = None;
        return false;
    }
    if cs.turbo_start.is_none() {
        cs.turbo_start = Some(now.clone());
    }
    let start = cs.turbo_start.as_ref().map_or(now.0, |t| t.0);
    let half = (turbo.interval_ms as u64 / 2).max(1);
    (now.0.saturating_sub(start) / half).is_multiple_of(2)
}

fn tap_active(cs: &CmdState, now: &Tick) -> bool {
    cs.tap_until.as_ref().is_some_and(|tu| now.0 < tu.0)
}

/// Arm a `TAP_MS` output window when `fire` (the activator's edge), then report whether one is
/// running — the shared body of the `Start`/`Release` one-shot taps.
fn tap_on_edge(cs: &mut CmdState, fire: bool, now: &Tick) -> bool {
    if fire {
        cs.tap_until = Some(Tick(now.0 + TAP_MS));
    }
    tap_active(cs, now)
}

/// Apply one action of a firing command: output leaves become desired levels; layer/set actions
/// are queued into `ops` for the next tick (`HoldLayer` carries its trigger node so it can latch
/// to that node's held-state). `None` does nothing.
///
/// Scroll pseudo-buttons (`MouseButton::Scroll*`) ride the ordinary button-level path: virt-out
/// realizes a scroll button's **press** as one wheel tick and no-ops its release, so a plain press
/// scrolls one notch and a `Turbo` command (which pulses the level on/off) scrolls one notch per
/// pulse — continuous scroll while held. (No held-state to reconcile; the backend collapses it.)
fn apply_action(
    action: &CompiledAction,
    node: &NodeHeld,
    desired: &mut DesiredLevels,
    ops: &mut LayerOps,
) {
    match action {
        CompiledAction::Key(k) => desired.press_key(k.clone()),
        CompiledAction::MouseButton(b) => desired.press_mouse(b.clone()),
        CompiledAction::GamepadButton(b) => desired.press_pad(b.clone()),
        CompiledAction::ChangeActionSet(s) => ops.set_change = Some(s.clone()),
        CompiledAction::AddLayer(l) => {
            ops.adds.insert(l.clone());
        }
        CompiledAction::RemoveLayer(l) => {
            ops.removes.insert(l.clone());
        }
        CompiledAction::HoldLayer(l) => {
            ops.holds.insert(l.clone(), node.clone());
        }
        CompiledAction::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::CompiledAction;
    use config::CommandSettings;
    use vocab_out::Key;

    fn cmd(activator: Activator) -> CompiledCommand {
        cmd_key(activator, Key::A, CommandSettings::default())
    }

    fn cmd_key(activator: Activator, key: Key, settings: CommandSettings) -> CompiledCommand {
        CompiledCommand { activator, actions: vec![CompiledAction::Key(key)], settings }
    }

    fn regular(interruptible: bool) -> Activator {
        Activator::Regular { interruptible }
    }

    /// Run one tick over a single command; return whether its key is desired-held.
    fn step(command: &CompiledCommand, slot: &mut SlotState, held: bool, now: u64) -> bool {
        let mut d = DesiredLevels::default();
        let mut ops = LayerOps::default();
        let mut haptics = Vec::new();
        let mut sinks = Sinks { desired: &mut d, ops: &mut ops, haptics: &mut haptics };
        let node = NodeHeld::Button(config::InputSource::LeftBumper);
        eval_commands(std::slice::from_ref(command), held, &node, &Side::Left, slot, &Tick(now), &mut sinks);
        d.has_key(&Key::A)
    }

    /// Run one tick over several commands on one node; return the desired levels.
    fn step_many(commands: &[CompiledCommand], slot: &mut SlotState, held: bool, now: u64) -> DesiredLevels {
        let mut d = DesiredLevels::default();
        let mut ops = LayerOps::default();
        let mut haptics = Vec::new();
        let mut sinks = Sinks { desired: &mut d, ops: &mut ops, haptics: &mut haptics };
        let node = NodeHeld::Button(config::InputSource::LeftBumper);
        eval_commands(commands, held, &node, &Side::Left, slot, &Tick(now), &mut sinks);
        d
    }

    /// Run one tick and return the haptic pulses produced.
    fn step_haptics(command: &CompiledCommand, slot: &mut SlotState, held: bool, now: u64) -> Vec<HapticReq> {
        let mut d = DesiredLevels::default();
        let mut ops = LayerOps::default();
        let mut haptics = Vec::new();
        let mut sinks = Sinks { desired: &mut d, ops: &mut ops, haptics: &mut haptics };
        let node = NodeHeld::Button(config::InputSource::LeftBumper);
        eval_commands(std::slice::from_ref(command), held, &node, &Side::Right, slot, &Tick(now), &mut sinks);
        haptics
    }

    // --- S7a: single-activator basics ---------------------------------------------------

    #[test]
    fn regular_holds_while_held() {
        let c = cmd(regular(false));
        let mut s = SlotState::default();
        assert!(step(&c, &mut s, true, 0)); // press → held
        assert!(step(&c, &mut s, true, 4)); // still held
        assert!(!step(&c, &mut s, false, 8)); // release → up
    }

    #[test]
    fn interruptible_regular_alone_is_a_plain_hold() {
        // Rule 5: with no Long/Double on the node, `interruptible` is a no-op — press/release hold.
        let c = cmd(regular(true));
        let mut s = SlotState::default();
        assert!(step(&c, &mut s, true, 0)); // holds from press, not a deferred tap
        assert!(step(&c, &mut s, true, 100));
        assert!(!step(&c, &mut s, false, 120));
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
        assert!(!step(&c, &mut s, false, 70)); // release → clears (a user release ends it, no floor)

        // A second press *outside* the window does not fire.
        let mut s = SlotState::default();
        assert!(!step(&c, &mut s, true, 0));
        assert!(!step(&c, &mut s, false, 20));
        assert!(!step(&c, &mut s, true, 500)); // 500ms later → no double
    }

    #[test]
    fn double_window_is_measured_from_the_first_press() {
        // Hold the first press past the window, release, quick re-press → NOT a double, because the
        // window runs from the first *press*, not its release.
        let c = cmd(Activator::Double { window_ms: 200 });
        let mut s = SlotState::default();
        assert!(!step(&c, &mut s, true, 0)); // first press
        assert!(!step(&c, &mut s, true, 300)); // held past 200
        assert!(!step(&c, &mut s, false, 300)); // release
        assert!(!step(&c, &mut s, true, 320)); // re-press: 320 − 0 > 200 → no double
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

    // --- Interruptible Regular + interrupters -------------------------------------------

    #[test]
    fn regular_taps_on_release_when_a_long_is_present_but_unfired() {
        // Example 1 (short press): regular(interruptible) + long(300) + long(500), released early.
        let cmds = [
            cmd_key(regular(true), Key::A, CommandSettings::default()),
            cmd_key(Activator::Long { hold_ms: 300 }, Key::B, CommandSettings::default()),
            cmd_key(Activator::Long { hold_ms: 500 }, Key::C, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        assert!(!step_many(&cmds, &mut s, true, 0).has_key(&Key::A)); // deferred while pressed
        let d = step_many(&cmds, &mut s, false, 100); // released before any long
        assert!(d.has_key(&Key::A) && !d.has_key(&Key::B) && !d.has_key(&Key::C)); // tap on release
    }

    #[test]
    fn two_longs_both_hold_and_interrupt_the_regular() {
        // Example 1 (held to 1000): both longs fire and both hold (no contest); regular interrupted.
        let cmds = [
            cmd_key(regular(true), Key::A, CommandSettings::default()),
            cmd_key(Activator::Long { hold_ms: 300 }, Key::B, CommandSettings::default()),
            cmd_key(Activator::Long { hold_ms: 500 }, Key::C, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        step_many(&cmds, &mut s, true, 0);
        let d = step_many(&cmds, &mut s, true, 300); // long(300) fires
        assert!(d.has_key(&Key::B) && !d.has_key(&Key::C) && !d.has_key(&Key::A));
        let d = step_many(&cmds, &mut s, true, 500); // long(500) fires too → both held
        assert!(d.has_key(&Key::B) && d.has_key(&Key::C) && !d.has_key(&Key::A));
        let d = step_many(&cmds, &mut s, true, 900);
        assert!(d.has_key(&Key::B) && d.has_key(&Key::C));
        let d = step_many(&cmds, &mut s, false, 1000); // release → all up
        assert!(!d.has_key(&Key::B) && !d.has_key(&Key::C) && !d.has_key(&Key::A));
    }

    #[test]
    fn regular_with_double_taps_after_the_window_on_a_single_press() {
        // Example 2 (single press): regular(interruptible) + double(500).
        let cmds = [
            cmd_key(regular(true), Key::A, CommandSettings::default()),
            cmd_key(Activator::Double { window_ms: 500 }, Key::B, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        assert!(!step_many(&cmds, &mut s, true, 0).has_key(&Key::A)); // deferred
        assert!(!step_many(&cmds, &mut s, false, 50).has_key(&Key::A)); // released, still in window
        assert!(!step_many(&cmds, &mut s, false, 400).has_key(&Key::A)); // waiting
        assert!(step_many(&cmds, &mut s, false, 500).has_key(&Key::A)); // window closed → tap
        assert!(step_many(&cmds, &mut s, false, 530).has_key(&Key::A)); // tap still on
        assert!(!step_many(&cmds, &mut s, false, 545).has_key(&Key::A)); // tap over (500 + 40)
    }

    #[test]
    fn regular_with_double_real_holds_when_held_past_the_window() {
        // The key new behaviour: regular(interruptible) + double(500), press-and-hold → after 500 a
        // real (delayed) press-and-hold, not a tap.
        let cmds = [
            cmd_key(regular(true), Key::A, CommandSettings::default()),
            cmd_key(Activator::Double { window_ms: 500 }, Key::B, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        assert!(!step_many(&cmds, &mut s, true, 0).has_key(&Key::A)); // deferred
        assert!(!step_many(&cmds, &mut s, true, 250).has_key(&Key::A)); // window still open
        assert!(step_many(&cmds, &mut s, true, 500).has_key(&Key::A)); // window closed, held → real hold
        assert!(step_many(&cmds, &mut s, true, 900).has_key(&Key::A)); // stays held (not a 40ms tap)
        assert!(!step_many(&cmds, &mut s, false, 1000).has_key(&Key::A)); // release → up
    }

    #[test]
    fn regular_with_double_is_interrupted_by_a_real_double() {
        let cmds = [
            cmd_key(regular(true), Key::A, CommandSettings::default()),
            cmd_key(Activator::Double { window_ms: 500 }, Key::B, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        step_many(&cmds, &mut s, true, 0);
        step_many(&cmds, &mut s, false, 50);
        let d = step_many(&cmds, &mut s, true, 100); // second press within window → double, regular killed
        assert!(d.has_key(&Key::B) && !d.has_key(&Key::A));
        let d = step_many(&cmds, &mut s, true, 150);
        assert!(d.has_key(&Key::B) && !d.has_key(&Key::A));
        assert!(!step_many(&cmds, &mut s, false, 200).has_key(&Key::A)); // no late regular tap
    }

    // --- Hold-takers coexist (Long/Double never contest) --------------------------------

    #[test]
    fn two_longs_both_hold_independently() {
        // long(300), long(500): each holds from its own threshold to release; they overlap.
        let cmds = [
            cmd_key(Activator::Long { hold_ms: 300 }, Key::A, CommandSettings::default()),
            cmd_key(Activator::Long { hold_ms: 500 }, Key::B, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        step_many(&cmds, &mut s, true, 0);
        let d = step_many(&cmds, &mut s, true, 300);
        assert!(d.has_key(&Key::A) && !d.has_key(&Key::B));
        let d = step_many(&cmds, &mut s, true, 500); // both held, no takeover
        assert!(d.has_key(&Key::A) && d.has_key(&Key::B));
        let d = step_many(&cmds, &mut s, false, 600);
        assert!(!d.has_key(&Key::A) && !d.has_key(&Key::B));
    }

    #[test]
    fn two_doubles_both_hold() {
        // Example 3 (corrected): double(500), double(200); second press within 200 → both fire and
        // both stay held until release.
        let cmds = [
            cmd_key(Activator::Double { window_ms: 500 }, Key::A, CommandSettings::default()),
            cmd_key(Activator::Double { window_ms: 200 }, Key::B, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        step_many(&cmds, &mut s, true, 0);
        step_many(&cmds, &mut s, false, 10);
        let d = step_many(&cmds, &mut s, true, 50); // both windows include 50
        assert!(d.has_key(&Key::A) && d.has_key(&Key::B));
        let d = step_many(&cmds, &mut s, true, 100); // still both held (no floor to expire)
        assert!(d.has_key(&Key::A) && d.has_key(&Key::B));
        let d = step_many(&cmds, &mut s, false, 120);
        assert!(!d.has_key(&Key::A) && !d.has_key(&Key::B));
    }

    #[test]
    fn only_the_wider_double_fires_between_the_windows() {
        // Example 4: double(500), double(200); second press at 300 (in 500, out of 200).
        let cmds = [
            cmd_key(Activator::Double { window_ms: 500 }, Key::A, CommandSettings::default()),
            cmd_key(Activator::Double { window_ms: 200 }, Key::B, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        step_many(&cmds, &mut s, true, 0);
        step_many(&cmds, &mut s, false, 10);
        let d = step_many(&cmds, &mut s, true, 300);
        assert!(d.has_key(&Key::A) && !d.has_key(&Key::B));
    }

    #[test]
    fn double_then_two_longs_all_hold_on_the_second_press() {
        // Example 5 (corrected): double(200), long(300), long(600); double then hold → the double
        // and both longs each fire and all three stay held until release.
        let cmds = [
            cmd_key(Activator::Double { window_ms: 200 }, Key::A, CommandSettings::default()),
            cmd_key(Activator::Long { hold_ms: 300 }, Key::B, CommandSettings::default()),
            cmd_key(Activator::Long { hold_ms: 600 }, Key::C, CommandSettings::default()),
        ];
        let mut s = SlotState::default();
        step_many(&cmds, &mut s, true, 0);
        step_many(&cmds, &mut s, false, 10);
        let d = step_many(&cmds, &mut s, true, 50); // double fires (second press within 200 of first)
        assert!(d.has_key(&Key::A) && !d.has_key(&Key::B) && !d.has_key(&Key::C));
        let d = step_many(&cmds, &mut s, true, 350); // long(300) off the 2nd press (50 + 300) adds
        assert!(d.has_key(&Key::A) && d.has_key(&Key::B) && !d.has_key(&Key::C));
        let d = step_many(&cmds, &mut s, true, 650); // long(600) (50 + 600) adds; all three held
        assert!(d.has_key(&Key::A) && d.has_key(&Key::B) && d.has_key(&Key::C));
        let d = step_many(&cmds, &mut s, false, 700);
        assert!(!d.has_key(&Key::A) && !d.has_key(&Key::B) && !d.has_key(&Key::C));
    }

    // --- S7b settings modifiers ---------------------------------------------------------

    #[test]
    fn toggle_latches_on_alternate_presses() {
        let c = cmd_key(regular(false), Key::A, CommandSettings { toggle: true, ..Default::default() });
        let mut s = SlotState::default();
        assert!(step(&c, &mut s, true, 0)); // press → latched on
        assert!(step(&c, &mut s, true, 4)); // held → stays on
        assert!(step(&c, &mut s, false, 8)); // release → STAYS on (that's toggle)
        assert!(!step(&c, &mut s, true, 12)); // next press → off
        assert!(!step(&c, &mut s, false, 16)); // stays off
        assert!(step(&c, &mut s, true, 20)); // press again → on
    }

    #[test]
    fn turbo_pulses_a_square_wave_while_held() {
        let c = cmd_key(
            regular(false),
            Key::A,
            CommandSettings { turbo: Some(Turbo { interval_ms: 100 }), ..Default::default() },
        );
        let mut s = SlotState::default();
        assert!(step(&c, &mut s, true, 0)); // fires immediately
        assert!(!step(&c, &mut s, true, 50)); // half period → off
        assert!(step(&c, &mut s, true, 100)); // full period → on
        assert!(!step(&c, &mut s, true, 150)); // off
        assert!(!step(&c, &mut s, false, 160)); // released → train stops
    }

    #[test]
    fn scroll_button_rides_the_level_path_and_turbo_repeats() {
        use vocab_out::MouseButton;
        let up = |settings| CompiledCommand {
            activator: regular(false),
            actions: vec![CompiledAction::MouseButton(MouseButton::ScrollUp)],
            settings,
        };
        let has = |d: &DesiredLevels| d.has_mouse(&MouseButton::ScrollUp);
        let one = |c: &CompiledCommand, s: &mut SlotState, held, now| {
            has(&step_many(std::slice::from_ref(c), s, held, now))
        };

        // Plain press: ScrollUp is a level while held; the reconcile's press edge is one wheel tick.
        let c = up(CommandSettings::default());
        let mut s = SlotState::default();
        assert!(one(&c, &mut s, true, 0)); // press → member (→ one notch)
        assert!(one(&c, &mut s, true, 4)); // held → still member (backend already ticked, no repeat)
        assert!(!one(&c, &mut s, false, 8)); // release → gone

        // Turbo pulses the level on/off, so a fresh press (another notch) lands each on-phase.
        let c = up(CommandSettings { turbo: Some(Turbo { interval_ms: 100 }), ..Default::default() });
        let mut s = SlotState::default();
        assert!(one(&c, &mut s, true, 0)); // on → notch
        assert!(!one(&c, &mut s, true, 50)); // off phase
        assert!(one(&c, &mut s, true, 100)); // on → next notch
    }

    #[test]
    fn command_haptic_fires_on_configured_edges() {
        use config::{HapticEdge, HapticStrength, Haptics};
        let with = |on| {
            cmd_key(
                regular(false),
                Key::A,
                CommandSettings {
                    haptics: Haptics { on, strength: HapticStrength::High },
                    ..Default::default()
                },
            )
        };

        // OnPress: one pulse on the press edge (the input's side, High), none on release/hold.
        let c = with(HapticEdge::OnPress);
        let mut s = SlotState::default();
        let h = step_haptics(&c, &mut s, true, 0);
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].side.clone(), h[0].strength.clone()), (Side::Right, HapticStrength::High));
        assert!(step_haptics(&c, &mut s, true, 4).is_empty()); // held, no edge
        assert!(step_haptics(&c, &mut s, false, 8).is_empty()); // release, OnPress → quiet

        // OnRelease: quiet on press, fires on release.
        let c = with(HapticEdge::OnRelease);
        let mut s = SlotState::default();
        assert!(step_haptics(&c, &mut s, true, 0).is_empty());
        assert_eq!(step_haptics(&c, &mut s, false, 8).len(), 1);

        // Both: press and release; Off: never.
        let c = with(HapticEdge::Both);
        let mut s = SlotState::default();
        assert_eq!(step_haptics(&c, &mut s, true, 0).len(), 1);
        assert_eq!(step_haptics(&c, &mut s, false, 8).len(), 1);
        let c = with(HapticEdge::Off);
        let mut s = SlotState::default();
        assert!(step_haptics(&c, &mut s, true, 0).is_empty());
        assert!(step_haptics(&c, &mut s, false, 8).is_empty());
    }
}
