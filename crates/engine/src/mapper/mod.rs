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

mod activator;
mod behavior;
mod command;
mod gyro;
mod layers;
mod reconcile;
mod smooth;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use config::{HapticStrength, InputSource, Side};
use virt_out::OutputEvent;

use crate::logical::LogicalFrame;
use crate::program::{CompiledBinding, CompiledSet, LayerId, Program, SetId};

use activator::{Activators, BindingKey};
use gyro::GravityEst;
use layers::{ArmedNodes, LayerOps, NodeHeld};
use reconcile::{AppliedLevels, DesiredLevels, RelAccum};
use smooth::OneEuro2;

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
/// S5/S6 populate the subset the skeleton and behaviors need; later steps add the activator
/// table (S7) and drive the layer stack (S8).
pub struct Mapper {
    /// The action set currently active (`ChangeActionSet` retargets it, S8).
    active_set: SetId,
    /// The layers active this tick, **frozen at tick start** (persistent Add/Remove + held
    /// HoldLayer, S8). Precedence is *declared order* (`LayerId` index), not activation order,
    /// so resolution walks them highest-index-first. Empty until S8 populates it.
    active_layers: Vec<LayerId>,
    /// Last-applied output levels — reconciled against each tick's desired set (S4).
    applied: AppliedLevels,
    /// Sub-pixel remainders for relative outputs (mouse/scroll); fed by the relative behaviors
    /// (S6b) and flushed each tick as integer `MouseMove`/`Scroll` events.
    rel: RelAccum,
    /// Previous logical frame — the pad-delta source for `AsMouse` (S6b) and digital edge
    /// detection (S7).
    prev: Option<LogicalFrame>,
    /// The previous tick's clock stamp, for the per-tick `dt` the rate-based behaviors need.
    last_tick: Option<Tick>,
    /// Per-source activator state (edges/timers/latches), reset when a source's winning binding
    /// changes (PLAN §4 — no long-press bleed across bindings).
    activators: Activators,
    /// Persistent layers from `AddLayer`/`RemoveLayer` (stay until removed or a set change).
    persistent_layers: BTreeSet<LayerId>,
    /// Active `HoldLayer`s and how to re-derive each's trigger-node held-state — a hold persists
    /// while its node stays held, latched to the node (not the binding) so it survives
    /// self-shadowing (PLAN §4).
    held_layers: BTreeMap<LayerId, NodeHeld>,
    /// Nodes that have already fired their one-shot persistent op (`AddLayer`/`RemoveLayer`/
    /// `ChangeActionSet`) this engagement — the anti-oscillation dedup latch (PLAN §4). Keyed by the
    /// physical node so the binding swap the op *itself* causes can't re-fire it; each entry is
    /// dropped (in [`Self::reconcile_layer_ops`]) the tick its node releases.
    armed_nodes: ArmedNodes,
    /// Per-source One-Euro filter state for the smoothed relative behaviors (`AsMouse`/
    /// `GyroToMouse`); only populated for sources that carry one.
    smoothers: HashMap<InputSource, OneEuro2>,
    /// Per-source gravity estimate for player-space gyro; only populated for gyro sources.
    gravity: HashMap<InputSource, GravityEst>,
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
            last_tick: None,
            activators: Activators::default(),
            persistent_layers: BTreeSet::new(),
            held_layers: BTreeMap::new(),
            armed_nodes: ArmedNodes::new(),
            smoothers: HashMap::new(),
            gravity: HashMap::new(),
        }
    }

    /// Re-seed the mapper for a (possibly different) `program` — an Main↔Fallback switch or a
    /// hot-apply of a new program to the current role. Resets the mapping state (active set,
    /// layers, activators) to the new program's defaults, since its `SetId`/`LayerId`s are its
    /// own, but **keeps** the applied output levels + relative remainders: the next tick's
    /// reconcile then releases the outgoing program's outputs and applies the incoming one's, so
    /// nothing sticks across the swap.
    pub fn switch_program(&mut self, program: &Program) {
        self.active_set = program.default_set.clone();
        self.active_layers.clear();
        self.persistent_layers.clear();
        self.held_layers.clear();
        self.armed_nodes.clear();
        self.activators = Activators::default();
        // applied / rel / prev / last_tick deliberately retained.
    }

    /// Run one mapping pass: resolve the winning binding for every bound input, compute the
    /// desired output levels + relative nudges, and reconcile them into `out` (emitting only
    /// diffs). `haptics` is the pulse channel (unused in the first cut, decision C).
    pub fn tick(
        &mut self,
        frame: &LogicalFrame,
        tick: Tick,
        program: &Program,
        out: &mut Vec<OutputEvent>,
        haptics: &mut Vec<HapticReq>,
    ) {
        let set = program.set(&self.active_set);
        let mut desired = DesiredLevels::default();

        // Resolve every bound input first (releases the `&self` borrow before the retained
        // activator/rel state is mutated in the loop below).
        let resolved: Vec<(&InputSource, &CompiledBinding, BindingKey)> = self
            .bound_sources(set)
            .into_iter()
            .filter_map(|s| self.resolve(set, s).map(|(b, key)| (s, b, key)))
            .collect();

        let ctx =
            behavior::Ctx { cur: frame, prev: self.prev.as_ref(), dt: self.dt(&tick), now: tick.clone() };
        let mut ops = LayerOps::default();
        let mut sinks = behavior::Sinks { desired: &mut desired, ops: &mut ops, haptics };
        for (source, binding, key) in resolved {
            let slots = self.activators.for_binding(source, key);
            // Only the smoothed relative behaviors (pad/gyro → mouse) carry a One-Euro filter.
            let smoother = matches!(
                binding,
                CompiledBinding::AsMouse { .. } | CompiledBinding::GyroToMouse { .. }
            )
            .then(|| self.smoothers.entry(source.clone()).or_default());
            // Player-space gyro needs a gravity estimate; only gyro sources carry one.
            let gravity = matches!(binding, CompiledBinding::GyroToMouse { .. })
                .then(|| self.gravity.entry(source.clone()).or_default());
            behavior::eval_binding(
                binding, source, &ctx, slots, &mut sinks, &mut self.rel, smoother, gravity,
            );
        }

        self.applied.reconcile(&desired, out);
        self.rel.flush(out);
        // Apply this tick's collected layer/set changes for the *next* tick (frozen-state rule).
        self.reconcile_layer_ops(frame, ops);
        self.prev = Some(frame.clone());
        self.last_tick = Some(tick);
    }

    /// Release every currently-applied output (the bound device went away → nothing must stick).
    /// Reconciles the applied levels against an empty desired set, so it drops *all* held keys/
    /// buttons/axes regardless of what held them (toggles, latches, layers) — unlike feeding a
    /// neutral input frame, which wouldn't undo a toggle. Relative accumulators hold no output.
    pub(super) fn release_all(&mut self, out: &mut Vec<OutputEvent>) {
        self.applied.reconcile(&DesiredLevels::default(), out);
    }

    /// Fold a tick's collected [`LayerOps`] into the layer stack for the **next** tick. A set
    /// change is a full swap (layers are per-set) that clears the stack; otherwise adds/removes
    /// update the persistent set and hold-layers persist while their trigger node stays held.
    /// The resulting `active_layers` is sorted by declared-order precedence (`LayerId` index).
    ///
    /// **Persistent mutations dedup per node** (PLAN §4): a `set_change`/`add`/`remove` applies only
    /// if its trigger node isn't already armed from an earlier tick of the same press, so a
    /// self-toggling button (base `AddLayer` ↔ layer `RemoveLayer`, or a `ChangeActionSet` cycle)
    /// fires once per press instead of strobing as the op flips its own winning binding. The armed
    /// set is `armed_nodes`, rebuilt at the end **exactly like `held_layers`** — keep entries whose
    /// node is still held, then add this tick's requesting nodes — so a node stays armed for the
    /// whole press and disarms the tick it releases. `contains_key` is read against the pre-tick set,
    /// so every mutation from one fire sees the same snapshot and lands together (`Add(X)+Remove(Y)`).
    fn reconcile_layer_ops(&mut self, frame: &LogicalFrame, ops: LayerOps) {
        let fires = |armed: &ArmedNodes, node: &NodeHeld| !armed.contains_key(&node.key());

        // set_change wins (a full swap clears the per-set stacks); adds/removes otherwise.
        let set_change =
            ops.set_change.as_ref().filter(|(_, n)| fires(&self.armed_nodes, n)).map(|(s, _)| s.clone());
        if let Some(set) = set_change {
            self.active_set = set;
            self.persistent_layers.clear();
            self.held_layers.clear();
        } else {
            for (l, node) in &ops.removes {
                if fires(&self.armed_nodes, node) {
                    self.persistent_layers.remove(l);
                }
            }
            for (l, node) in &ops.adds {
                if fires(&self.armed_nodes, node) {
                    self.persistent_layers.insert(l.clone());
                }
            }
            // A hold persists while its trigger node stays held; this tick's holds refresh it.
            let mut next = BTreeMap::new();
            for (l, node) in std::mem::take(&mut self.held_layers) {
                if node.held(frame) {
                    next.insert(l, node);
                }
            }
            for (l, node) in ops.holds {
                if node.held(frame) {
                    next.insert(l, node);
                }
            }
            self.held_layers = next;
        }

        // Rebuild `armed_nodes` like `held_layers`: keep entries whose node is still held, then arm
        // every node that requested a persistent op this tick (applied or deduped away). So a node
        // stays armed the whole press — its self-caused binding flip can't re-fire the opposing op —
        // and disarms the tick it releases. Node-keyed, so it survives a `ChangeActionSet`.
        let mut next_armed = ArmedNodes::new();
        for (key, node) in std::mem::take(&mut self.armed_nodes) {
            if node.held(frame) {
                next_armed.insert(key, node);
            }
        }
        let requesting = ops
            .set_change
            .into_iter()
            .map(|(_, n)| n)
            .chain(ops.adds.into_iter().map(|(_, n)| n))
            .chain(ops.removes.into_iter().map(|(_, n)| n));
        for node in requesting {
            if node.held(frame) {
                next_armed.insert(node.key(), node);
            }
        }
        self.armed_nodes = next_armed;

        let mut active: Vec<LayerId> = self
            .persistent_layers
            .iter()
            .cloned()
            .chain(self.held_layers.keys().cloned())
            .collect();
        active.sort_unstable();
        active.dedup();
        self.active_layers = active;
    }

    /// Seconds elapsed since the previous tick (`0.0` on the first tick and if the clock did not
    /// advance — so rate-based behaviors emit nothing rather than a spurious jump).
    fn dt(&self, now: &Tick) -> f32 {
        match &self.last_tick {
            Some(prev) if now.0 > prev.0 => (now.0 - prev.0) as f32 / 1000.0,
            _ => 0.0,
        }
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

    /// The winning binding for `source` **and its identity** ([`BindingKey`]): the
    /// highest-precedence active layer that binds it (precedence = `LayerId` index, declared
    /// order), else the base binding, else `None` (PLAN §4 — silent layers fall through). The
    /// key lets the activator table reset when the winner changes.
    fn resolve<'p>(
        &self,
        set: &'p CompiledSet,
        source: &InputSource,
    ) -> Option<(&'p CompiledBinding, BindingKey)> {
        let mut layers: Vec<&LayerId> = self.active_layers.iter().collect();
        layers.sort_by_key(|id| std::cmp::Reverse(id.index()));
        for id in layers {
            if let Some(binding) = set.layer(id).bindings.get(source) {
                return Some((binding, BindingKey::Layer(id.clone())));
            }
        }
        set.base.get(source).map(|b| (b, BindingKey::Base))
    }

    /// Test-only: force a layer active as if a persistent `AddLayer` had fired — both frozen for
    /// the current tick and kept by [`Self::reconcile_layer_ops`] on subsequent ticks.
    #[cfg(test)]
    fn force_layer(&mut self, id: LayerId) {
        self.persistent_layers.insert(id.clone());
        if !self.active_layers.contains(&id) {
            self.active_layers.push(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{
        CompiledAction, CompiledCommand, CompiledLayer, CompiledSet, ProgramMeta, Role, SourceMap,
    };
    use config::{Activator, CommandSettings};
    use vocab_out::{GamepadButton, Key};

    /// A `Regular` command firing the given actions.
    fn regular(actions: Vec<CompiledAction>) -> CompiledCommand {
        CompiledCommand { activator: Activator::Regular { interruptible: false }, actions, settings: CommandSettings::default() }
    }

    /// A `Button` binding firing `action` on a `Regular` press.
    fn btn(action: CompiledAction) -> CompiledBinding {
        CompiledBinding::Button { commands: vec![regular(vec![action])] }
    }

    /// A `Button` binding firing `action` once on the press edge (`Start`).
    fn start_btn(action: CompiledAction) -> CompiledBinding {
        CompiledBinding::Button {
            commands: vec![CompiledCommand {
                activator: Activator::Start,
                actions: vec![action],
                settings: CommandSettings::default(),
            }],
        }
    }

    /// A command firing `action` on a `Long` after `hold_ms`.
    fn long_cmd(hold_ms: u32, action: CompiledAction) -> CompiledCommand {
        CompiledCommand {
            activator: Activator::Long { hold_ms },
            actions: vec![action],
            settings: CommandSettings::default(),
        }
    }

    /// A command firing `action` on an *interruptible* `Regular`.
    fn ireg_cmd(action: CompiledAction) -> CompiledCommand {
        CompiledCommand {
            activator: Activator::Regular { interruptible: true },
            actions: vec![action],
            settings: CommandSettings::default(),
        }
    }

    fn layer(name: &str, bindings: impl IntoIterator<Item = (InputSource, CompiledBinding)>) -> CompiledLayer {
        CompiledLayer { name: name.into(), bindings: SourceMap::from_iter(bindings) }
    }

    /// A program of one or more `(base, layers)` sets.
    fn program_of(sets: Vec<(SourceMap<CompiledBinding>, Vec<CompiledLayer>)>) -> Program {
        Program {
            meta: ProgramMeta { name: "test".into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
            sets: sets
                .into_iter()
                .enumerate()
                .map(|(i, (base, layers))| CompiledSet { name: format!("set{i}"), base, layers })
                .collect(),
        }
    }

    fn down(events: &[OutputEvent], key: Key) -> bool {
        events.contains(&OutputEvent::Key(key, true))
    }

    /// A single-set program from an iterator of `(source, binding)` base bindings.
    fn program_with(base: impl IntoIterator<Item = (InputSource, CompiledBinding)>) -> Program {
        Program {
            meta: ProgramMeta { name: "test".into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
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
        assert!(haptics.is_empty(), "command haptics are deferred (decision C)");
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Hold → no new events.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 1);
        assert!(out.is_empty());

        // Release → key up.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 2);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);
    }

    #[test]
    fn release_all_drops_held_outputs() {
        // The transport-lost path (D5): release everything so nothing sticks when the device dies.
        let program = program_with([(
            InputSource::LeftBumper,
            CompiledBinding::Button { commands: vec![regular(vec![CompiledAction::Key(Key::A)])] },
        )]);
        let mut m = Mapper::new(&program);

        // Hold L1 → key A is applied (down).
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Device gone → release_all emits the release without any input change.
        let mut out = Vec::new();
        m.release_all(&mut out);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);

        // Applied state is now clean: a second release_all emits nothing.
        let mut out2 = Vec::new();
        m.release_all(&mut out2);
        assert!(out2.is_empty());
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::RB), 0);
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LGRIP), 0);
        assert_eq!(out, vec![OutputEvent::GamepadButton(GamepadButton::A, true)]);
    }

    #[test]
    fn long_activator_does_not_fire_on_the_press_tick() {
        // A freshly-pressed Long activator (threshold not yet met) produces nothing this tick.
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn activator_state_resets_when_winning_binding_changes() {
        // Base binds L1 to a 250 ms Long → A. A layer rebinds L1 to Long → B. If we hold L1,
        // let the base Long arm, then swap in the layer, the layer's Long must NOT inherit the
        // base's elapsed hold — it starts timing fresh (no long-press bleed across bindings).
        let long = |key: Key| CompiledBinding::Button {
            commands: vec![CompiledCommand {
                activator: Activator::Long { hold_ms: 250 },
                actions: vec![CompiledAction::Key(key)],
                settings: CommandSettings::default(),
            }],
        };
        let program = Program {
            meta: ProgramMeta { name: "test".into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
            sets: vec![CompiledSet {
                name: "Game".into(),
                base: SourceMap::from_iter([(InputSource::LeftBumper, long(Key::A))]),
                layers: vec![CompiledLayer {
                    name: "alt".into(),
                    bindings: SourceMap::from_iter([(InputSource::LeftBumper, long(Key::B))]),
                }],
            }],
        };
        let mut m = Mapper::new(&program);

        // Hold L1 on base until the Long arms → A down at t=300.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 300);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Swap to the layer (still holding L1). Winning binding changed → fresh activator state:
        // A releases, and the layer's Long has NOT armed yet (press_start reset to now).
        m.force_layer(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 320);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);

        // Only after another 250 ms of holding does the layer's Long fire → B.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 600);
        assert_eq!(out, vec![OutputEvent::Key(Key::B, true)]);
    }

    #[test]
    fn layer_binding_wins_over_base() {
        // Base binds L1→A; a layer rebinds L1→B. With the layer active, B wins.
        let program = Program {
            meta: ProgramMeta { name: "test".into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Activate the layer (a real AddLayer does this in S8's tests; here force it for the
        // resolution check): release first so applied levels are clean, then B should win.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 1);
        m.force_layer(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 2);
        assert_eq!(out, vec![OutputEvent::Key(Key::B, true)]);
    }

    #[test]
    fn switch_program_releases_old_outputs_and_applies_new() {
        // Program A: L1 → A. Program B: L1 → B. Holding L1 across a switch must release A and
        // press B (no stuck key), which is how the loop hot-swaps Main↔Fallback (S9).
        let prog_a = program_with([(InputSource::LeftBumper, btn(CompiledAction::Key(Key::A)))]);
        let prog_b = program_with([(InputSource::LeftBumper, btn(CompiledAction::Key(Key::B)))]);
        let mut m = Mapper::new(&prog_a);

        let out = run(&mut m, &prog_a, &frame(steam_hid::Buttons::LB), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Switch to program B (still holding L1) and tick B: A up, B down in one reconcile.
        m.switch_program(&prog_b);
        let out = run(&mut m, &prog_b, &frame(steam_hid::Buttons::LB), 4);
        assert!(down(&out, Key::B) && out.contains(&OutputEvent::Key(Key::A, false)));
    }

    // --- S8: layer / action-set actions -------------------------------------------------

    #[test]
    fn hold_layer_activates_next_tick_and_releases_on_node_release() {
        // Base: L4 → HoldLayer(aim). aim: L1 → Key(A).
        let program = program_of(vec![(
            SourceMap::from_iter([(
                InputSource::LeftGrip,
                btn(CompiledAction::HoldLayer(LayerId::new(0))),
            )]),
            vec![layer("aim", [(InputSource::LeftBumper, btn(CompiledAction::Key(Key::A)))])],
        )]);
        let mut m = Mapper::new(&program);

        // Press L4 + L1 together: this tick the layer isn't active yet, so L1 does nothing.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LGRIP | steam_hid::Buttons::LB), 0);
        assert!(out.is_empty());
        // Next tick the layer is active → L1 → A.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LGRIP | steam_hid::Buttons::LB), 4);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);
        // Release L4 (the hold's node) but keep L1: the layer drops next tick, A releases.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 8);
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 12);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);
    }

    #[test]
    fn hold_layer_survives_self_shadowing() {
        // Base: L1 → HoldLayer(aim). aim rebinds the very same L1 → Key(X). Holding L1 must keep
        // the layer active (latched to L1's held-state) though the HoldLayer command is shadowed.
        let program = program_of(vec![(
            SourceMap::from_iter([(
                InputSource::LeftBumper,
                btn(CompiledAction::HoldLayer(LayerId::new(0))),
            )]),
            vec![layer("aim", [(InputSource::LeftBumper, btn(CompiledAction::Key(Key::X)))])],
        )]);
        let mut m = Mapper::new(&program);

        // Press L1: layer arms, no output yet.
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0).is_empty());
        // Now L1 is shadowed by the layer → X down, and it stays down while L1 is held (stable,
        // no flicker) — the HoldLayer command no longer fires but the hold latches to L1.
        assert_eq!(
            run(&mut m, &program, &frame(steam_hid::Buttons::LB), 4),
            vec![OutputEvent::Key(Key::X, true)]
        );
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::LB), 8).is_empty()); // held, stable
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::LB), 12).is_empty());
        // Release L1 → X releases, layer drops.
        assert_eq!(
            run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 16),
            vec![OutputEvent::Key(Key::X, false)]
        );
    }

    #[test]
    fn layer_none_binding_nullifies_base() {
        // Base binds L1 → A. A layer binds L1 → None. With the layer active, L1 does nothing and
        // any held base output is released (the right-pad-in-mode-shift case).
        let program = program_of(vec![(
            SourceMap::from_iter([(InputSource::LeftBumper, btn(CompiledAction::Key(Key::A)))]),
            vec![layer("off", [(InputSource::LeftBumper, CompiledBinding::None)])],
        )]);
        let mut m = Mapper::new(&program);

        // Base active: L1 → A.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Activate the None layer while still holding L1 → A is released, nothing new.
        m.force_layer(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 4);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);
    }

    #[test]
    fn non_naming_layer_falls_through_to_base() {
        // aim binds only RightBumper; L1 is unbound in the layer → falls through to base's A.
        let program = program_of(vec![(
            SourceMap::from_iter([(InputSource::LeftBumper, btn(CompiledAction::Key(Key::A)))]),
            vec![layer("aim", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::B)))])],
        )]);
        let mut m = Mapper::new(&program);
        m.force_layer(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]); // base A, not shadowed
    }

    #[test]
    fn two_hold_layers_stack_with_declared_order_precedence() {
        // Base: L4 → HoldLayer(0), L5 → HoldLayer(1). Layer 0 binds R1→A and R4→C; layer 1 binds
        // R1→B. Both held: R1 resolves to the higher-index layer 1 (B), R4 only layer 0 (C).
        let program = program_of(vec![(
            SourceMap::from_iter([
                (InputSource::LeftGrip, btn(CompiledAction::HoldLayer(LayerId::new(0)))),
                (InputSource::LeftGrip2, btn(CompiledAction::HoldLayer(LayerId::new(1)))),
            ]),
            vec![
                layer(
                    "l0",
                    [
                        (InputSource::RightBumper, btn(CompiledAction::Key(Key::A))),
                        (InputSource::RightGrip, btn(CompiledAction::Key(Key::C))),
                    ],
                ),
                layer("l1", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::B)))]),
            ],
        )]);
        let mut m = Mapper::new(&program);

        // Arm both holds (press L4 + L5).
        let held = steam_hid::Buttons::LGRIP | steam_hid::Buttons::LGRIP2;
        let _ = run(&mut m, &program, &frame(held.clone()), 0);
        // Now press R1 + R4 too: layer 1 wins R1 (B), layer 0 supplies R4 (C), no A.
        let out = run(
            &mut m,
            &program,
            &frame(held | steam_hid::Buttons::RB | steam_hid::Buttons::RGRIP),
            4,
        );
        assert!(down(&out, Key::B) && down(&out, Key::C));
        assert!(!down(&out, Key::A));
    }

    #[test]
    fn add_and_remove_layer_are_persistent() {
        // Base: L4 → AddLayer(0) [Start], R4 → RemoveLayer(0) [Start]. Layer 0: L1 → A.
        let program = program_of(vec![(
            SourceMap::from_iter([
                (InputSource::LeftGrip, start_btn(CompiledAction::AddLayer(LayerId::new(0)))),
                (InputSource::RightGrip, start_btn(CompiledAction::RemoveLayer(LayerId::new(0)))),
            ]),
            vec![layer("l0", [(InputSource::LeftBumper, btn(CompiledAction::Key(Key::A)))])],
        )]);
        let mut m = Mapper::new(&program);

        // Tap L4 → AddLayer. Layer persists after release.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LGRIP), 0);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 100);
        // L1 now → A (layer active though L4 long released).
        assert_eq!(
            run(&mut m, &program, &frame(steam_hid::Buttons::LB), 200),
            vec![OutputEvent::Key(Key::A, true)]
        );
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 300);

        // Tap R4 → RemoveLayer. Now L1 is unbound (base has no L1) → nothing.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::RGRIP), 400);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 500);
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::LB), 600).is_empty());
    }

    #[test]
    fn change_action_set_swaps_and_clears_layers() {
        // set0 base: L4 → AddLayer(0) [Start], Menu → ChangeActionSet(1) [Start]. set0 layer0:
        // L1 → A. set1 base: L1 → B.
        let program = program_of(vec![
            (
                SourceMap::from_iter([
                    (InputSource::LeftGrip, start_btn(CompiledAction::AddLayer(LayerId::new(0)))),
                    (InputSource::Menu, start_btn(CompiledAction::ChangeActionSet(SetId::new(1)))),
                ]),
                vec![layer("l0", [(InputSource::LeftBumper, btn(CompiledAction::Key(Key::A)))])],
            ),
            (SourceMap::from_iter([(InputSource::LeftBumper, btn(CompiledAction::Key(Key::B)))]), vec![]),
        ]);
        let mut m = Mapper::new(&program);

        // In set0, add layer0, confirm L1 → A.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LGRIP), 0);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 100);
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::LB), 200), Key::A));
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 300);

        // Tap Menu → ChangeActionSet(1): next tick we're on set1, layer stack cleared.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::MENU), 400);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 500);
        // L1 → B now (set1), and the old layer0's A is gone.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 600);
        assert!(down(&out, Key::B) && !down(&out, Key::A));
    }

    // --- Persistent-op dedup: one flip per press, no self-toggle strobing (PLAN §4) ------

    #[test]
    fn same_button_add_remove_layer_toggles_once_per_press_regardless_of_hold_length() {
        // The cp2077 shape: base L1 → AddLayer(0); layer0 rebinds the very same L1 → RemoveLayer(0)
        // (self-shadowing toggle), plus R1 → Key(X) as a marker (X fires on an R1 press only while
        // the layer is active). Pre-fix, a held L1 strobed the layer every tick, so the landing
        // state depended on how many ticks you held (the "needs 2–5 presses" bug). With the dedup it
        // must flip exactly once per press, whatever the hold length.
        let program = program_of(vec![(
            SourceMap::from_iter([(
                InputSource::LeftBumper,
                btn(CompiledAction::AddLayer(LayerId::new(0))),
            )]),
            vec![layer(
                "aim",
                [
                    (InputSource::LeftBumper, btn(CompiledAction::RemoveLayer(LayerId::new(0)))),
                    (InputSource::RightBumper, btn(CompiledAction::Key(Key::X))),
                ],
            )],
        )]);
        let mut m = Mapper::new(&program);
        let lb = steam_hid::Buttons::LB;
        let rb = steam_hid::Buttons::RB;
        let mut t = 0u64;

        // Probe layer state: pulse R1 up→down and report whether X fired (R1 is bound only in the
        // layer, so X ⇒ layer active). Leaves all inputs released. R1 ≠ L1, so it can't disturb L1's
        // dedup state, and Key(X) is not a persistent op.
        macro_rules! layer_active {
            () => {{
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                t += 4;
                let out = run(&mut m, &program, &frame(rb.clone()), t);
                t += 4;
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                t += 4;
                down(&out, Key::X)
            }};
        }
        // Hold L1 for `n` ticks, then release.
        macro_rules! press_l1 {
            ($n:expr) => {{
                for _ in 0..$n {
                    let _ = run(&mut m, &program, &frame(lb.clone()), t);
                    t += 4;
                }
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                t += 4;
            }};
        }

        assert!(!layer_active!(), "starts off");
        press_l1!(1); // shortest possible press
        assert!(layer_active!(), "one short press → on");
        press_l1!(50); // long hold — pre-fix this would strobe
        assert!(!layer_active!(), "one long press → off (no strobe)");
        press_l1!(37); // a different odd length — proves it's not hold-length parity
        assert!(layer_active!(), "another long press → on");
        let _ = t; // last macro bumps `t` without a further read
    }

    /// The xbox_mouse_profile self-toggle shape, shared by the short-press and long-hold latch
    /// tests: base L1 = {Regular(interruptible): AddLayer(0), Long(450): HoldLayer(0)}; layer0
    /// rebinds L1 → RemoveLayer(0) and binds R1 → X as a "layer active" marker.
    fn xbox_self_toggle_program() -> Program {
        program_of(vec![(
            SourceMap::from_iter([(
                InputSource::LeftBumper,
                CompiledBinding::Button {
                    commands: vec![
                        ireg_cmd(CompiledAction::AddLayer(LayerId::new(0))),
                        long_cmd(450, CompiledAction::HoldLayer(LayerId::new(0))),
                    ],
                },
            )]),
            vec![layer(
                "aim",
                [
                    (
                        InputSource::LeftBumper,
                        CompiledBinding::Button {
                            commands: vec![ireg_cmd(CompiledAction::RemoveLayer(LayerId::new(0)))],
                        },
                    ),
                    (InputSource::RightBumper, btn(CompiledAction::Key(Key::X))),
                ],
            )],
        )])
    }

    #[test]
    fn self_removing_layer_with_interruptible_regular_removes_on_the_second_press() {
        // Exact xbox_mouse_profile shape (see `xbox_self_toggle_program`). A short press adds the
        // layer; a *second* short press must remove it. The bug: on the second press the layer's
        // RemoveLayer fires (arming the node), the winner flips back to base, and base's interruptible-
        // Regular commits a tap whose AddLayer — when *level*-fired — kept firing through the TAP_MS
        // tail, after armed_nodes disarmed at release, re-adding the layer. Edge-firing the persistent
        // ops makes AddLayer fire once, at the tap's commit tick, where the node is still armed →
        // suppressed → no re-add. Continuous ticking (no time jumps) is required to expose it — the
        // re-add was on the empty tap-tail ticks.
        let program = xbox_self_toggle_program();
        let mut m = Mapper::new(&program);
        let lb = steam_hid::Buttons::LB;
        let rb = steam_hid::Buttons::RB;
        let mut t = 0u64;
        // Tick `buttons` continuously every 4ms for `ms` (like the real reader loop — no time jumps).
        macro_rules! hold {
            ($buttons:expr, $ms:expr) => {{
                let mut last = Vec::new();
                let end = t + $ms;
                while t < end {
                    last = run(&mut m, &program, &frame($buttons), t);
                    t += 4;
                }
                last
            }};
        }
        macro_rules! layer_active {
            () => {{
                let _ = hold!(steam_hid::Buttons::empty(), 20);
                let out = run(&mut m, &program, &frame(rb.clone()), t); // single rising-edge tick
                t += 4;
                let _ = hold!(steam_hid::Buttons::empty(), 80);
                down(&out, Key::X)
            }};
        }
        macro_rules! short_press {
            () => {{
                let _ = hold!(lb.clone(), 80); // ~80ms click, well under Long(450)
                let _ = hold!(steam_hid::Buttons::empty(), 80);
            }};
        }
        assert!(!layer_active!(), "starts off");
        short_press!();
        assert!(layer_active!(), "short press adds the layer");
        short_press!();
        assert!(!layer_active!(), "second short press must remove it");
        let _ = t; // last macro bumps `t` without a further read
    }

    #[test]
    fn self_removing_layer_long_hold_activates_only_while_held() {
        // Same xbox_mouse_profile shape. Held *long* (past the 450 ms Long), L1's base fires
        // HoldLayer(0): the layer is active only while L1 is held and drops on release (the Long
        // interrupts the interruptible AddLayer, so there's no persistent add). The layer rebinds
        // L1 → RemoveLayer and L1 does self-shadow to it, but HoldLayer holds the layer in
        // `held_layers` while RemoveLayer only touches the persistent set — so the removal is a no-op
        // and the held layer stays up for the whole hold.
        let program = xbox_self_toggle_program();
        let mut m = Mapper::new(&program);
        let lb = steam_hid::Buttons::LB;
        let rb = steam_hid::Buttons::RB;
        let mut t = 0u64;
        macro_rules! hold {
            ($buttons:expr, $ms:expr) => {{
                let end = t + $ms;
                while t < end {
                    let _ = run(&mut m, &program, &frame($buttons), t);
                    t += 4;
                }
            }};
        }

        // Hold L1 well past the 450 ms Long → HoldLayer activates the layer.
        hold!(lb.clone(), 600);
        // Still holding L1, tap R1: it resolves to the layer's X ⇒ the layer is active mid-hold, and
        // the layer's RemoveLayer never self-fired to tear it down.
        let out = run(&mut m, &program, &frame(lb.clone() | rb.clone()), t);
        t += 4;
        assert!(down(&out, Key::X), "layer active while L1 held past the Long");
        hold!(lb.clone(), 40); // release R1, keep holding L1

        // Release L1 → HoldLayer drops the layer.
        hold!(steam_hid::Buttons::empty(), 40);
        // Tap R1 again: no X ⇒ the hold was temporary, not a persistent add.
        let out = run(&mut m, &program, &frame(rb.clone()), t);
        assert!(!down(&out, Key::X), "layer dropped after releasing the long hold");
    }


    #[test]
    fn a_self_added_layer_that_rebinds_the_node_adds_one_layer_per_press() {
        // base L1 → AddLayer(0); layer 0 rebinds L1 → AddLayer(1). A node makes at most ONE persistent
        // change per press (armed_nodes): the first press adds layer 0, then the flip to layer 0
        // re-binds L1 to AddLayer(1) — but that fires on the *next* tick of the *same* press on the
        // *same* physical node, so it's deduped. A second press adds layer 1. Markers: R1 → A (layer 0
        // active), R4 → B (layer 1 active).
        let program = program_of(vec![(
            SourceMap::from_iter([(
                InputSource::LeftBumper,
                btn(CompiledAction::AddLayer(LayerId::new(0))),
            )]),
            vec![
                layer(
                    "l0",
                    [
                        (InputSource::LeftBumper, btn(CompiledAction::AddLayer(LayerId::new(1)))),
                        (InputSource::RightBumper, btn(CompiledAction::Key(Key::A))),
                    ],
                ),
                layer("l1", [(InputSource::RightGrip, btn(CompiledAction::Key(Key::B)))]),
            ],
        )]);
        let mut m = Mapper::new(&program);
        let lb = steam_hid::Buttons::LB;
        let mut t = 0u64;
        // Press L1 ~40 ms then release, ticking continuously.
        macro_rules! press_l1 {
            () => {{
                let end = t + 40;
                while t < end {
                    let _ = run(&mut m, &program, &frame(lb.clone()), t);
                    t += 4;
                }
                let end = t + 80;
                while t < end {
                    let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                    t += 4;
                }
            }};
        }
        // Pulse a marker button once and report whether its key fired ⇒ that layer is active.
        macro_rules! active {
            ($btn:expr, $key:expr) => {{
                let out = run(&mut m, &program, &frame($btn), t);
                t += 4;
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                t += 4;
                down(&out, $key)
            }};
        }

        assert!(!active!(steam_hid::Buttons::RB, Key::A), "layer 0 off at start");
        press_l1!();
        assert!(active!(steam_hid::Buttons::RB, Key::A), "one press → layer 0");
        assert!(!active!(steam_hid::Buttons::RGRIP, Key::B), "layer 1 still off — the chained add is deduped");
        press_l1!();
        assert!(active!(steam_hid::Buttons::RGRIP, Key::B), "second press → layer 1");
        let _ = t; // last macro bumps `t` without a further read
    }

    #[test]
    fn one_button_cycles_action_sets_once_per_press() {
        // Three sets, each binding Menu → ChangeActionSet(next) and L1 → a per-set marker key. With a
        // plain (held) Regular, pre-fix this re-fired every tick and cycled ~250×/s to a random set;
        // the dedup must advance exactly one set per Menu press, whatever the hold length.
        let set = |next: usize, mark: Key| {
            (
                SourceMap::from_iter([
                    (InputSource::Menu, btn(CompiledAction::ChangeActionSet(SetId::new(next)))),
                    (InputSource::LeftBumper, btn(CompiledAction::Key(mark))),
                ]),
                vec![],
            )
        };
        let program = program_of(vec![set(1, Key::A), set(2, Key::B), set(0, Key::C)]);
        let mut m = Mapper::new(&program);
        let menu = steam_hid::Buttons::MENU;
        let lb = steam_hid::Buttons::LB;
        let mut t = 0u64;

        // Probe the active set: pulse L1 and read which marker key fired.
        macro_rules! active_marker {
            () => {{
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                t += 4;
                let out = run(&mut m, &program, &frame(lb.clone()), t);
                t += 4;
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                t += 4;
                out
            }};
        }
        // Hold Menu for `n` ticks, then release.
        macro_rules! press_menu {
            ($n:expr) => {{
                for _ in 0..$n {
                    let _ = run(&mut m, &program, &frame(menu.clone()), t);
                    t += 4;
                }
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), t);
                t += 4;
            }};
        }

        assert!(down(&active_marker!(), Key::A), "starts on set0");
        press_menu!(30);
        assert!(down(&active_marker!(), Key::B), "one press → set1");
        press_menu!(30);
        assert!(down(&active_marker!(), Key::C), "another press → set2");
        press_menu!(30);
        assert!(down(&active_marker!(), Key::A), "wraps back to set0");
        let _ = t; // last macro bumps `t` without a further read
    }

    #[test]
    fn interruptible_regular_and_long_fire_the_right_layer_op_once() {
        // One button carries BOTH an interruptible `Regular` → AddLayer(0) (the "tap") and a
        // `Long(300)` → AddLayer(1) (the "hold"). They're mutually exclusive: a short press taps the
        // Regular (once, on release), a hold past 300 ms fires the Long and interrupts the Regular.
        // Each persistent op must land exactly once — the Regular's tap holds its output for TAP_MS,
        // and the Long's stays high every tick it's held; the dedup keeps both to one apply. Probed
        // via R1, bound to a distinct marker in each layer (only one layer is ever active here).
        let program = || {
            program_of(vec![(
                SourceMap::from_iter([(
                    InputSource::LeftBumper,
                    CompiledBinding::Button {
                        commands: vec![
                            ireg_cmd(CompiledAction::AddLayer(LayerId::new(0))),
                            long_cmd(300, CompiledAction::AddLayer(LayerId::new(1))),
                        ],
                    },
                )]),
                vec![
                    layer("tap", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::A)))]),
                    layer("hold", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::B)))]),
                ],
            )])
        };
        let lb = steam_hid::Buttons::LB;
        let rb = steam_hid::Buttons::RB;

        // Which marker fires on an R1 pulse → which layer is active. `release_at` < 300 taps, >= 300
        // holds. Fresh mapper each call. Returns (layer0_A, layer1_B).
        let outcome = |hold_to: u64, release_at: u64| {
            let program = program();
            let mut m = Mapper::new(&program);
            let mut t = 0;
            while t <= hold_to {
                let _ = run(&mut m, &program, &frame(lb.clone()), t);
                t += 4;
            }
            // Release, then idle continuously past TAP_MS so a deferred Regular tap resolves + expires
            // (ticking every tick, not sampling — a re-fire in the tap tail would show up).
            let mut rt = release_at;
            while rt <= release_at + 100 {
                let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), rt);
                rt += 4;
            }
            let out = run(&mut m, &program, &frame(rb.clone()), release_at + 200);
            (down(&out, Key::A), down(&out, Key::B))
        };

        // Taps at a couple of pre-threshold release times → layer 0 (Regular), never layer 1.
        assert_eq!(outcome(48, 52), (true, false), "quick tap → layer 0");
        assert_eq!(outcome(292, 296), (true, false), "tap just before threshold → layer 0");
        // Holds past the threshold → layer 1 (Long), Regular interrupted → never layer 0.
        assert_eq!(outcome(300, 320), (false, true), "hold to threshold → layer 1");
        assert_eq!(outcome(1000, 1020), (false, true), "long hold → layer 1");
    }

    #[test]
    fn one_command_applies_all_its_persistent_ops_atomically() {
        // A single command can carry several persistent ops; they all fire on the one press (the
        // dedup snapshots "armed" before applying, so an `Add(X) + Remove(Y)` lands together, not
        // just the first). Base L1 fires [AddLayer(0), RemoveLayer(1)] in one command; layer 1 is
        // pre-added (base L4 → AddLayer(1)). Pressing L1 must end with layer 0 in and layer 1 out.
        let program = program_of(vec![(
            SourceMap::from_iter([
                (InputSource::LeftGrip, start_btn(CompiledAction::AddLayer(LayerId::new(1)))),
                (
                    InputSource::LeftBumper,
                    CompiledBinding::Button {
                        commands: vec![regular(vec![
                            CompiledAction::AddLayer(LayerId::new(0)),
                            CompiledAction::RemoveLayer(LayerId::new(1)),
                        ])],
                    },
                ),
            ]),
            vec![
                layer("l0", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::A)))]),
                layer("l1", [(InputSource::RightGrip, btn(CompiledAction::Key(Key::B)))]),
            ],
        )]);
        let mut m = Mapper::new(&program);
        // Spacing >TAP_MS between actions so the L4 `Start` tap window can't overlap the L1 press.
        let empty = steam_hid::Buttons::empty();

        // Pre-add layer 1 (tap L4). Confirm it's active: R4 → B.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LGRIP), 0);
        let _ = run(&mut m, &program, &frame(empty.clone()), 100);
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::RGRIP), 200), Key::B));
        let _ = run(&mut m, &program, &frame(empty.clone()), 300);

        // Press L1 → the one command adds layer 0 AND removes layer 1, both this press.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 400);
        let _ = run(&mut m, &program, &frame(empty.clone()), 500);
        // Layer 0 now active (R1 → A); layer 1 gone (R4 → nothing).
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::RB), 600), Key::A));
        assert!(!down(&run(&mut m, &program, &frame(steam_hid::Buttons::RGRIP), 700), Key::B));
    }

    #[test]
    fn one_command_adds_two_distinct_layers_atomically() {
        // Sibling of the add+remove atomicity test, but two ADDs of *different* layers: proves each
        // co-fired op actually takes effect (both layers pushed), not merely that some net state
        // flip occurred. Base L1 fires [AddLayer(0), AddLayer(1)] in one command; markers R1 → A
        // (layer 0), R4 → B (layer 1). A single press must leave BOTH layers active — if only the
        // first op applied, layer 1 is never added and the R4 → B assertion fails.
        let program = program_of(vec![(
            SourceMap::from_iter([(
                InputSource::LeftBumper,
                CompiledBinding::Button {
                    commands: vec![regular(vec![
                        CompiledAction::AddLayer(LayerId::new(0)),
                        CompiledAction::AddLayer(LayerId::new(1)),
                    ])],
                },
            )]),
            vec![
                layer("l0", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::A)))]),
                layer("l1", [(InputSource::RightGrip, btn(CompiledAction::Key(Key::B)))]),
            ],
        )]);
        let mut m = Mapper::new(&program);
        let empty = steam_hid::Buttons::empty();

        // Press L1 once → the single command adds layer 0 AND layer 1 this press.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        let _ = run(&mut m, &program, &frame(empty.clone()), 100);
        // Both layers now active: R1 → A and R4 → B.
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::RB), 200), Key::A));
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::RGRIP), 300), Key::B));
    }

    #[test]
    fn persistent_op_dedup_is_per_node_not_global() {
        // The dedup is keyed per node, so two different buttons each toggle their own layer
        // independently — arming one must not suppress the other, even held simultaneously. L1
        // toggles layer 0 (base Add / layer Remove); L4 toggles layer 1 the same way. Markers: R1→A
        // (layer 0), R4→B (layer 1).
        let program = program_of(vec![(
            SourceMap::from_iter([
                (InputSource::LeftBumper, btn(CompiledAction::AddLayer(LayerId::new(0)))),
                (InputSource::LeftGrip, btn(CompiledAction::AddLayer(LayerId::new(1)))),
            ]),
            vec![
                layer(
                    "l0",
                    [
                        (InputSource::LeftBumper, btn(CompiledAction::RemoveLayer(LayerId::new(0)))),
                        (InputSource::RightBumper, btn(CompiledAction::Key(Key::A))),
                    ],
                ),
                layer(
                    "l1",
                    [
                        (InputSource::LeftGrip, btn(CompiledAction::RemoveLayer(LayerId::new(1)))),
                        (InputSource::RightGrip, btn(CompiledAction::Key(Key::B))),
                    ],
                ),
            ],
        )]);
        let mut m = Mapper::new(&program);
        let both = steam_hid::Buttons::LB | steam_hid::Buttons::LGRIP;

        let l0 = |m: &mut Mapper, t| down(&run(m, &program, &frame(steam_hid::Buttons::RB), t), Key::A);
        let l1 = |m: &mut Mapper, t| down(&run(m, &program, &frame(steam_hid::Buttons::RGRIP), t), Key::B);

        // Press BOTH together and hold a while → each adds its own layer once (no cross-suppression).
        for t in (0..40).step_by(4) {
            let _ = run(&mut m, &program, &frame(both.clone()), t);
        }
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 40);
        assert!(l0(&mut m, 44) && l1(&mut m, 48), "both layers on after one shared press");

        // Press both again → each removes its own layer once → both off.
        for t in (52..92).step_by(4) {
            let _ = run(&mut m, &program, &frame(both.clone()), t);
        }
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 92);
        assert!(!l0(&mut m, 96) && !l1(&mut m, 100), "both layers off after the next shared press");
    }

    #[test]
    fn a_node_makes_at_most_one_persistent_change_per_press() {
        // Deliberate rule (not a bug): a single button changes layer/set state **at most once per
        // press**. Two independent hold-takers on one node — long(300) → AddLayer(0) and long(500) →
        // AddLayer(1) — both go high in the same press, but only the first to fire applies; the
        // second is deduped (the node is already armed). This is what keeps a self-toggle from
        // strobing; the cost is that stacking two persistent ops on one button is one-per-press.
        let program = program_of(vec![(
            SourceMap::from_iter([(
                InputSource::LeftBumper,
                CompiledBinding::Button {
                    commands: vec![
                        long_cmd(300, CompiledAction::AddLayer(LayerId::new(0))),
                        long_cmd(500, CompiledAction::AddLayer(LayerId::new(1))),
                    ],
                },
            )]),
            vec![
                layer("l0", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::A)))]),
                layer("l1", [(InputSource::RightGrip, btn(CompiledAction::Key(Key::B)))]),
            ],
        )]);
        let mut m = Mapper::new(&program);
        let lb = steam_hid::Buttons::LB;

        // Hold well past both thresholds, then release.
        for t in (0..700).step_by(4) {
            let _ = run(&mut m, &program, &frame(lb.clone()), t);
        }
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 700);

        // Layer 0 (first long) applied; layer 1 (second long) suppressed — one change per press.
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::RB), 704), Key::A));
        assert!(!down(&run(&mut m, &program, &frame(steam_hid::Buttons::RGRIP), 708), Key::B));
    }

    // --- Persistent-op dedup: layer ops mixed with action-set changes -------------------

    #[test]
    fn tap_adds_a_layer_while_hold_changes_action_set() {
        // One button mixes a layer op and a set op across activators: interruptible `Regular` →
        // AddLayer(0) (tap) and `Long(300)` → ChangeActionSet(1) (hold). They're mutually exclusive,
        // so a tap adds layer 0 in set 0, and a hold switches to set 1 (which also clears layers) —
        // each fires exactly once. Marker: RB → A in set 0's layer 0, RB → C in set 1's base.
        let program = program_of(vec![
            (
                SourceMap::from_iter([(
                    InputSource::LeftBumper,
                    CompiledBinding::Button {
                        commands: vec![
                            ireg_cmd(CompiledAction::AddLayer(LayerId::new(0))),
                            long_cmd(300, CompiledAction::ChangeActionSet(SetId::new(1))),
                        ],
                    },
                )]),
                vec![layer("l0", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::A)))])],
            ),
            (
                SourceMap::from_iter([(InputSource::RightBumper, btn(CompiledAction::Key(Key::C)))]),
                vec![],
            ),
        ]);
        let lb = steam_hid::Buttons::LB;
        let rb = steam_hid::Buttons::RB;
        let empty = steam_hid::Buttons::empty();
        // Tick `buttons` continuously (every 4 ms) over `[from, to)` — no time jumps, like the reader.
        let run_span = |m: &mut Mapper, buttons: &steam_hid::Buttons, from: u64, to: u64| {
            let mut t = from;
            while t < to {
                let _ = run(m, &program, &frame(buttons.clone()), t);
                t += 4;
            }
        };

        // Tap: held ~150 ms (< 300) then released → still set 0, layer 0 added (RB → A), not set 1 (C).
        let mut m = Mapper::new(&program);
        run_span(&mut m, &lb, 0, 150);
        run_span(&mut m, &empty, 150, 300); // ride through the tap tail released
        let out = run(&mut m, &program, &frame(rb.clone()), 300);
        assert!(down(&out, Key::A) && !down(&out, Key::C), "tap → layer 0 in set 0");

        // Hold: held past 300 ms → switched to set 1 (RB → C); layer 0 gone with the swap (no A).
        let mut m = Mapper::new(&program);
        run_span(&mut m, &lb, 0, 400);
        run_span(&mut m, &empty, 400, 480);
        let out = run(&mut m, &program, &frame(rb.clone()), 500);
        assert!(down(&out, Key::C) && !down(&out, Key::A), "hold → action set 1");
    }

    #[test]
    fn set_change_in_a_command_overrides_a_same_command_add_layer() {
        // A single command carrying both AddLayer(0) and ChangeActionSet(1): the set change wins (a
        // full swap clears the per-set stacks), so the layer add is dropped. Documents the precedence
        // when the two collide in one fire. Marker: RB → A in set 0's layer 0, RB → C in set 1.
        let program = program_of(vec![
            (
                SourceMap::from_iter([(
                    InputSource::LeftBumper,
                    CompiledBinding::Button {
                        commands: vec![regular(vec![
                            CompiledAction::AddLayer(LayerId::new(0)),
                            CompiledAction::ChangeActionSet(SetId::new(1)),
                        ])],
                    },
                )]),
                vec![layer("l0", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::A)))])],
            ),
            (
                SourceMap::from_iter([(InputSource::RightBumper, btn(CompiledAction::Key(Key::C)))]),
                vec![],
            ),
        ]);
        let mut m = Mapper::new(&program);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::LB), 0);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 4);
        // In set 1 (RB → C); the layer add was swept by the set change (A absent).
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::RB), 8);
        assert!(down(&out, Key::C) && !down(&out, Key::A), "set change wins; layer add dropped");
    }

    #[test]
    fn a_node_that_changes_set_does_not_also_fire_the_new_sets_op_until_released() {
        // The dedup crosses the set boundary: Menu is a ChangeActionSet(1) in set 0 and an
        // AddLayer(0) in set 1. Holding Menu switches to set 1 once — `armed_nodes` is node-keyed and
        // survives the set change — and does NOT then add layer 0 while still held (one persistent op
        // per press). Releasing and pressing again fires the new set's AddLayer(0). Markers: R4 → C
        // (set 1 base), R1 → A (set 1's layer 0).
        let program = program_of(vec![
            (
                SourceMap::from_iter([(
                    InputSource::Menu,
                    btn(CompiledAction::ChangeActionSet(SetId::new(1))),
                )]),
                vec![],
            ),
            (
                SourceMap::from_iter([
                    (InputSource::Menu, btn(CompiledAction::AddLayer(LayerId::new(0)))),
                    (InputSource::RightGrip, btn(CompiledAction::Key(Key::C))),
                ]),
                vec![layer("l0", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::A)))])],
            ),
        ]);
        let mut m = Mapper::new(&program);
        let menu = steam_hid::Buttons::MENU;

        // Hold Menu across the set change: switches to set 1, but layer 0 is NOT added while held.
        for t in (0..40).step_by(4) {
            let _ = run(&mut m, &program, &frame(menu.clone()), t);
        }
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 40);
        assert!(
            down(&run(&mut m, &program, &frame(steam_hid::Buttons::RGRIP), 44), Key::C),
            "switched to set 1"
        );
        assert!(
            !down(&run(&mut m, &program, &frame(steam_hid::Buttons::RB), 48), Key::A),
            "layer 0 not added while Menu stayed held through the swap"
        );

        // Release and press Menu again (now in set 1) → AddLayer(0) fires.
        let _ = run(&mut m, &program, &frame(menu.clone()), 100);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 104);
        assert!(
            down(&run(&mut m, &program, &frame(steam_hid::Buttons::RB), 108), Key::A),
            "second press adds layer 0 in set 1"
        );
    }

    #[test]
    fn a_set_change_bleeds_the_new_sets_binding_through_the_same_press() {
        // Steam-like bleed-through (no latch): pressing a button that changes to a set where the same
        // button is bound fires the new set's binding immediately, from the same press. set0: Menu →
        // ChangeActionSet(1). set1: Menu → Key(K). Holding Menu switches to set1 and then fires K.
        let program = program_of(vec![
            (
                SourceMap::from_iter([(
                    InputSource::Menu,
                    btn(CompiledAction::ChangeActionSet(SetId::new(1))),
                )]),
                vec![],
            ),
            (
                SourceMap::from_iter([(InputSource::Menu, btn(CompiledAction::Key(Key::K)))]),
                vec![],
            ),
        ]);
        let mut m = Mapper::new(&program);
        let menu = steam_hid::Buttons::MENU;
        // Press edge fires the set change; next tick, still held, set1's Menu → K bleeds through.
        let _ = run(&mut m, &program, &frame(menu.clone()), 0);
        let out = run(&mut m, &program, &frame(menu.clone()), 4);
        assert!(down(&out, Key::K), "the new set's Menu binding bleeds through the same press");
    }

    #[test]
    fn a_held_button_unbound_in_the_new_set_goes_silent() {
        // Bleed-through the other way: a held button re-resolves in the new set, so if the new set
        // doesn't bind it, it goes silent — it does NOT keep its old-set binding. set0: L1 → Key(A),
        // Menu → ChangeActionSet(1). set1: RB → Key(M) (L1 unbound). Holding L1 across the swap
        // releases A.
        let program = program_of(vec![
            (
                SourceMap::from_iter([
                    (InputSource::LeftBumper, btn(CompiledAction::Key(Key::A))),
                    (InputSource::Menu, btn(CompiledAction::ChangeActionSet(SetId::new(1)))),
                ]),
                vec![],
            ),
            (
                SourceMap::from_iter([(InputSource::RightBumper, btn(CompiledAction::Key(Key::M)))]),
                vec![],
            ),
        ]);
        let mut m = Mapper::new(&program);
        let lb = steam_hid::Buttons::LB;
        // Hold L1 → A.
        assert_eq!(run(&mut m, &program, &frame(lb.clone()), 0), vec![OutputEvent::Key(Key::A, true)]);
        // Add Menu → the set change fires (swap queued for next tick); A still held this tick.
        let _ = run(&mut m, &program, &frame(lb.clone() | steam_hid::Buttons::MENU), 4);
        // Next tick we're in set1, where L1 is unbound → A releases.
        let out = run(&mut m, &program, &frame(lb.clone()), 8);
        assert!(out.contains(&OutputEvent::Key(Key::A, false)), "held L1 goes silent — unbound in set1");
    }

    #[test]
    fn a_cross_set_add_after_a_change_set_never_fires() {
        // The scenario that started the whole discussion, on the armed_nodes model. One button:
        // Long(300) → ChangeActionSet(1), Long(500) → AddLayer(0). set0 has layer 0; set1 has none.
        // After the swap the button bleeds through to set1's binding (Menu → D), so set0's Long(500)
        // is never evaluated — AddLayer(0) never fires against the layerless set1. No panic, no stray
        // layer. Markers: LB → C (set1 base), RB → X (the set-0 layer that must never activate).
        let program = program_of(vec![
            (
                SourceMap::from_iter([(
                    InputSource::Menu,
                    CompiledBinding::Button {
                        commands: vec![
                            long_cmd(300, CompiledAction::ChangeActionSet(SetId::new(1))),
                            long_cmd(500, CompiledAction::AddLayer(LayerId::new(0))),
                        ],
                    },
                )]),
                vec![layer("some", [(InputSource::RightBumper, btn(CompiledAction::Key(Key::X)))])],
            ),
            (
                SourceMap::from_iter([
                    (InputSource::Menu, btn(CompiledAction::Key(Key::D))),
                    (InputSource::LeftBumper, btn(CompiledAction::Key(Key::C))),
                ]),
                vec![],
            ),
        ]);
        let mut m = Mapper::new(&program);
        let menu = steam_hid::Buttons::MENU;
        let mut t = 0u64;
        while t < 600 {
            let _ = run(&mut m, &program, &frame(menu.clone()), t);
            t += 4;
        }
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::LB), 600), Key::C), "in set1");
        assert!(!down(&run(&mut m, &program, &frame(steam_hid::Buttons::RB), 608), Key::X), "no cross-set layer added");
    }
}
