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

use config::{Activator, HapticEdge, Haptics, Side, Turbo};

use super::activator::{CmdState, SlotState};
use super::behavior::Sinks;
use super::layers::{LayerOps, NodeHeld};
use super::reconcile::DesiredLevels;
use super::{HapticReq, Tick};
use crate::program::{CompiledAction, CompiledCommand};

/// How long a tap-style activator (`Start`/`Release`) holds its action from the trigger edge,
/// in ms — long enough for a game to register, short enough to feel like a tap. HW-tuned later.
const TAP_MS: u64 = 40;

/// Advance a node's commands one tick and apply the firing ones' actions — output leaves into
/// `sinks.desired`, layer/set actions into `sinks.ops` (for the next tick), command-haptic
/// pulses into `sinks.haptics`.
///
/// `held` is the node's digital level this tick; `node` describes how to re-derive that held
/// state later (for `HoldLayer` latching); `slot` carries its retained timing/latches.
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

    // Press timing must be visible to the commands this tick; release timing updates *after*
    // (so `Double` sees the *previous* release, not this one). `sibling_fired` restarts each press.
    if pressed {
        slot.press_start = Some(now.clone());
        slot.sibling_fired = false;
    }
    let press_start = slot.press_start.clone();
    let last_release = slot.last_release.clone();
    let mut sibling_fired = slot.sibling_fired;

    for (i, cmd) in commands.iter().enumerate() {
        let settings = &cmd.settings;
        let interruptible = settings.interruptible && matches!(cmd.activator, Activator::Regular);
        let cs = slot.command(i);

        let out = if interruptible {
            // Deferred "short press": fire as a tap on release iff no sibling fired this press —
            // so a short tap fires this, a long/double hold fires the sibling and this stays quiet.
            if released && !sibling_fired {
                cs.tap_until = Some(Tick(now.0 + TAP_MS));
            }
            tap_active(cs, now)
        } else {
            let raw = fires(&cmd.activator, held, pressed, released, &press_start, &last_release, cs, now);
            let toggled = apply_toggle(raw, settings.toggle, cs);
            let out = apply_turbo(toggled, settings.turbo.as_ref(), cs, now);
            // A non-interruptible command that fires is a "sibling" for interruptible Regulars.
            sibling_fired |= out;
            out
        };

        // Command haptic: a singular pulse on the input's side at the command's output edges (rising
        // = press, falling = release), gated by its `Haptics` setting. Follows the *output* level so
        // it's uniform across activator types (a Toggle clicks on activate/deactivate; a Turbo would
        // click per pulse — the user disables haptics on turbo if that's unwanted).
        emit_haptic(&settings.haptics, cs, out, side, sinks.haptics);

        if out {
            // The whole ordered combo fires while the command fires (subcommands = modifiers).
            for action in &cmd.actions {
                apply_action(action, node, sinks.desired, sinks.ops);
            }
        }
    }

    slot.sibling_fired = sibling_fired;
    if released {
        slot.last_release = Some(now.clone());
        slot.press_start = None;
    }
    slot.prev_held = held;
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

/// Whether a command's actions should be held **this tick**, per its activator type (S7a).
#[allow(clippy::too_many_arguments)] // an activator reads the whole per-node edge/timing frame.
fn fires(
    activator: &Activator,
    held: bool,
    pressed: bool,
    released: bool,
    press_start: &Option<Tick>,
    last_release: &Option<Tick>,
    cs: &mut CmdState,
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

fn tap_active(cs: &CmdState, now: &Tick) -> bool {
    cs.tap_until.as_ref().is_some_and(|tu| now.0 < tu.0)
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
    use vocab::Key;

    fn cmd(activator: Activator) -> CompiledCommand {
        cmd_key(activator, Key::A, CommandSettings::default())
    }

    fn cmd_key(activator: Activator, key: Key, settings: CommandSettings) -> CompiledCommand {
        CompiledCommand { activator, actions: vec![CompiledAction::Key(key)], settings }
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

    // --- S7b settings modifiers ---------------------------------------------------------

    #[test]
    fn toggle_latches_on_alternate_presses() {
        let c = cmd_key(
            Activator::Regular,
            Key::A,
            CommandSettings { toggle: true, ..Default::default() },
        );
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
            Activator::Regular,
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
        use vocab::MouseButton;
        let up = |settings| CompiledCommand {
            activator: Activator::Regular,
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
                Activator::Regular,
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

    #[test]
    fn interruptible_regular_short_vs_long() {
        // Node: interruptible Regular → A (short), Long{100} → B (long).
        let cmds = [
            cmd_key(
                Activator::Regular,
                Key::A,
                CommandSettings { interruptible: true, ..Default::default() },
            ),
            cmd_key(Activator::Long { hold_ms: 100 }, Key::B, CommandSettings::default()),
        ];

        // Short press: A does NOT fire while held, then taps on release; B never fires.
        let mut s = SlotState::default();
        assert!(!step_many(&cmds, &mut s, true, 0).has_key(&Key::A)); // deferred
        let d = step_many(&cmds, &mut s, false, 50); // release before threshold
        assert!(d.has_key(&Key::A) && !d.has_key(&Key::B));

        // Long press: B fires at the threshold; A is suppressed (a sibling fired) on release.
        let mut s = SlotState::default();
        assert!(!step_many(&cmds, &mut s, true, 0).has_key(&Key::A));
        let d = step_many(&cmds, &mut s, true, 100);
        assert!(d.has_key(&Key::B) && !d.has_key(&Key::A));
        let d = step_many(&cmds, &mut s, false, 110); // release → A stays quiet
        assert!(!d.has_key(&Key::A) && !d.has_key(&Key::B));
    }
}
