//! The pure mapping core (PLAN §4.1/§4.2).
//!
//! [`Mapper`] holds the live runtime state and runs **one acyclic pass per tick**
//! ([`Mapper::tick`]). It is deliberately pure: it takes an input frame plus an injected
//! [`Tick`] clock and produces [`OutputEvent`]s (and, later, [`HapticReq`]s), touching no
//! hardware and no wall clock — so recorded `(frame, Tick)` traces replay identically in
//! golden tests. The manager shell (S9) drives it; the network sink drives the same core.
//!
//! **This is S5 — skeleton + binding resolution.** Retained state, the frozen-layer-state
//! read, and winning-binding resolution (declared-order layer-stack walk, else base) are
//! here. Only [`CompiledBinding::Button`] with a `Regular` activator is wired (held-while-held
//! levels, via [`reconcile`]); behaviors (S6), the full activator set (S7), and layer/set
//! actions (S8) fill in the remaining match arms and state. The per-tick pass will grow into
//! `resolve`/`behavior`/`command` submodules as those steps land.

mod reconcile;

use std::collections::BTreeSet;

use config::{Activator, HapticStrength, InputSource, Side};
use virt_out::OutputEvent;

use crate::logical::LogicalFrame;
use crate::program::{CompiledAction, CompiledBinding, CompiledSet, LayerId, Program, SetId};

use reconcile::{AppliedLevels, DesiredLevels, RelAccum};

/// A monotonic logical clock stamp, injected by the manager loop each tick (PLAN §4.1).
///
/// The [`Mapper`] reads time **only** from the `Tick` handed to [`Mapper::tick`], never the
/// wall clock, so activator timers (`Long`/`Double`/`Turbo`, S7) are deterministic and a
/// recorded trace replays bit-for-bit. Unit is milliseconds since the loop started. No `Copy`
/// (project convention); it is cheap to `clone`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(pub u64);

/// A request for one haptic pulse, produced by the mapping pass and drained by the owning
/// device's reader thread (PLAN §4.1 haptics; the seam kept from the start, decision C).
///
/// Each pulse targets a single actuator — the [`Side`] of the triggering input — at one of
/// three strengths. Command-activation haptics that would fill this in are **deferred**; S5
/// never produces one, but the `tick()` signature carries the channel so wiring it later is a
/// pure addition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HapticReq {
    pub side: Side,
    pub strength: HapticStrength,
}

/// The live mapping state — everything retained across ticks (PLAN §4.1).
///
/// S5 populates the subset the skeleton needs; later steps add the activator table (S7), the
/// gyro integrator and relative remainders' companions (S6), and drive the layer stack (S8).
pub struct Mapper {
    /// The action set currently active (`ChangeActionSet` retargets it, S8).
    active_set: SetId,
    /// The layers active this tick, **frozen at tick start** (persistent Add/Remove + held
    /// HoldLayer, S8). Precedence is *declared order* (`LayerId` index), not activation order,
    /// so resolution walks them highest-index-first. Empty until S8 populates it.
    active_layers: Vec<LayerId>,
    /// Last-applied output levels — reconciled against each tick's desired set (S4).
    applied: AppliedLevels,
    /// Sub-pixel remainders for relative outputs (mouse/scroll); fed by behaviors in S6.
    rel: RelAccum,
    /// Previous logical frame, retained for digital edge detection (`cur & !prev`, S7).
    #[allow(dead_code)] // Read for edge detection once activators land (S7).
    prev: Option<LogicalFrame>,
}

impl Mapper {
    /// A fresh mapper for `program`, starting on its default action set with no active layers.
    pub fn new(program: &Program) -> Self {
        Mapper {
            active_set: program.default_set.clone(),
            active_layers: Vec::new(),
            applied: AppliedLevels::default(),
            rel: RelAccum::default(),
            prev: None,
        }
    }

    /// Run one mapping pass: resolve the winning binding for every bound input, compute the
    /// desired output levels, and reconcile them into `out` (emitting only diffs). `haptics`
    /// is the pulse channel (unused in S5). `_tick` is the injected clock (used from S7).
    pub fn tick(
        &mut self,
        frame: &LogicalFrame,
        _tick: Tick,
        program: &Program,
        out: &mut Vec<OutputEvent>,
        _haptics: &mut Vec<HapticReq>,
    ) {
        let set = program.set(&self.active_set);
        let mut desired = DesiredLevels::default();

        for source in self.bound_sources(set) {
            let Some(binding) = self.resolve(set, source) else { continue };
            // A single real arm today; behaviors add the Joystick/Pad/Trigger/… arms in S6.
            #[allow(clippy::single_match, clippy::collapsible_match)]
            match binding {
                CompiledBinding::Button { commands } => {
                    // S5: only the held-while-held `Regular` activator; richer activators,
                    // toggle/turbo/interruptible, and combos-as-sequences land in S7.
                    if frame.button(source) {
                        for cmd in commands {
                            if !matches!(cmd.activator, Activator::Regular) {
                                continue;
                            }
                            for action in &cmd.actions {
                                apply_output(action, &mut desired);
                            }
                        }
                    }
                }
                // Behaviors (S6) and their virtual buttons produce axes / relative nudges /
                // virtual-button levels; not yet handled.
                _ => {}
            }
        }

        self.applied.reconcile(&desired, out);
        self.rel.flush(out);
        self.prev = Some(frame.clone());
    }

    /// The set of inputs to evaluate this tick: every source bound in the base or in any active
    /// layer (resolution then picks the single winner per source). Deterministic order.
    fn bound_sources<'p>(&self, set: &'p CompiledSet) -> BTreeSet<&'p InputSource> {
        let mut sources: BTreeSet<&InputSource> = set.base.iter().map(|(s, _)| s).collect();
        for id in &self.active_layers {
            sources.extend(set.layer(id).bindings.iter().map(|(s, _)| s));
        }
        sources
    }

    /// The winning binding for `source`: the highest-precedence active layer that binds it
    /// (precedence = `LayerId` index, declared order), else the base binding, else `None`
    /// (PLAN §4 — silent layers fall through).
    fn resolve<'p>(&self, set: &'p CompiledSet, source: &InputSource) -> Option<&'p CompiledBinding> {
        let mut layers: Vec<&LayerId> = self.active_layers.iter().collect();
        layers.sort_by_key(|id| std::cmp::Reverse(id.index()));
        for id in layers {
            if let Some(binding) = set.layer(id).bindings.get(source) {
                return Some(binding);
            }
        }
        set.base.get(source)
    }
}

/// Fold one output-leaf action into the desired levels. Non-output actions (layer/set changes,
/// `None`) and scroll impulses are handled by later steps and ignored here.
fn apply_output(action: &CompiledAction, desired: &mut DesiredLevels) {
    match action {
        CompiledAction::Key(k) => desired.press_key(k.clone()),
        // Scroll pseudo-buttons are impulses, not levels — turbo/impulse handling is S7.
        CompiledAction::MouseButton(b) if !b.is_scroll() => desired.press_mouse(b.clone()),
        CompiledAction::GamepadButton(b) => desired.press_pad(b.clone()),
        // Scroll impulses, layer/set actions (S8), and `None` produce no level here.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{
        CompiledCommand, CompiledLayer, CompiledSet, ProgramMeta, Role, SourceMap,
    };
    use config::CommandSettings;
    use vocab::{GamepadButton, Key};

    /// A `Regular` command firing the given actions.
    fn regular(actions: Vec<CompiledAction>) -> CompiledCommand {
        CompiledCommand { activator: Activator::Regular, actions, settings: CommandSettings::default() }
    }

    /// A single-set program from an iterator of `(source, binding)` base bindings.
    fn program_with(base: impl IntoIterator<Item = (InputSource, CompiledBinding)>) -> Program {
        Program {
            meta: ProgramMeta { name: "test".into(), role: Role::Active },
            default_set: SetId::new(0),
            sets: vec![CompiledSet {
                name: "Game".into(),
                base: SourceMap::from_iter(base),
                layers: Vec::new(),
            }],
        }
    }

    fn frame(buttons: steam_hid::Buttons) -> LogicalFrame {
        LogicalFrame::new(steam_hid::ControllerState { buttons, ..Default::default() })
    }

    fn run(mapper: &mut Mapper, program: &Program, f: &LogicalFrame, t: u64) -> Vec<OutputEvent> {
        let mut out = Vec::new();
        let mut haptics = Vec::new();
        mapper.tick(f, Tick(t), program, &mut out, &mut haptics);
        assert!(haptics.is_empty(), "S5 produces no haptics");
        out
    }

    #[test]
    fn button_maps_to_key_press_and_release() {
        let program = program_with([(
            InputSource::LeftBumper,
            CompiledBinding::Button { commands: vec![regular(vec![CompiledAction::Key(Key::A)])] },
        )]);
        let mut m = Mapper::new(&program);

        // Press L1 → key down.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Hold → no new events.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 1);
        assert!(out.is_empty());

        // Release → key up.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 2);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);
    }

    #[test]
    fn combo_presses_all_actions_while_held() {
        // Ctrl + C as a single held combo.
        let program = program_with([(
            InputSource::RightBumper,
            CompiledBinding::Button {
                commands: vec![regular(vec![
                    CompiledAction::Key(Key::LeftCtrl),
                    CompiledAction::Key(Key::C),
                ])],
            },
        )]);
        let mut m = Mapper::new(&program);
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::R1), 0);
        // Sorted (BTreeSet) → deterministic order; both go down.
        assert_eq!(
            out.iter().filter(|e| matches!(e, OutputEvent::Key(_, true))).count(),
            2
        );
        assert!(out.contains(&OutputEvent::Key(Key::LeftCtrl, true)));
        assert!(out.contains(&OutputEvent::Key(Key::C, true)));
    }

    #[test]
    fn gamepad_button_binding() {
        let program = program_with([(
            InputSource::LeftGrip,
            CompiledBinding::Button {
                commands: vec![regular(vec![CompiledAction::GamepadButton(GamepadButton::A)])],
            },
        )]);
        let mut m = Mapper::new(&program);
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L4), 0);
        assert_eq!(out, vec![OutputEvent::GamepadButton(GamepadButton::A, true)]);
    }

    #[test]
    fn unbound_and_non_regular_produce_nothing() {
        // A Long activator is ignored in S5; an unbound button produces nothing.
        let program = program_with([(
            InputSource::LeftBumper,
            CompiledBinding::Button {
                commands: vec![CompiledCommand {
                    activator: Activator::Long { hold_ms: 250 },
                    actions: vec![CompiledAction::Key(Key::A)],
                    settings: CommandSettings::default(),
                }],
            },
        )]);
        let mut m = Mapper::new(&program);
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn layer_binding_wins_over_base() {
        // Base binds L1→A; a layer rebinds L1→B. With the layer active, B wins.
        let program = Program {
            meta: ProgramMeta { name: "test".into(), role: Role::Active },
            default_set: SetId::new(0),
            sets: vec![CompiledSet {
                name: "Game".into(),
                base: SourceMap::from_iter([(
                    InputSource::LeftBumper,
                    CompiledBinding::Button {
                        commands: vec![regular(vec![CompiledAction::Key(Key::A)])],
                    },
                )]),
                layers: vec![CompiledLayer {
                    name: "alt".into(),
                    bindings: SourceMap::from_iter([(
                        InputSource::LeftBumper,
                        CompiledBinding::Button {
                            commands: vec![regular(vec![CompiledAction::Key(Key::B)])],
                        },
                    )]),
                }],
            }],
        };
        let mut m = Mapper::new(&program);

        // Base only → A.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Activate the layer (S8 does this for real; poke the state for the S5 resolution test):
        // release first so applied levels are clean, then the layer's B should win.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 1);
        m.active_layers.push(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 2);
        assert_eq!(out, vec![OutputEvent::Key(Key::B, true)]);
    }
}
