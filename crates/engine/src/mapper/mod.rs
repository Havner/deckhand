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
use layers::{LayerOps, NodeHeld};
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
        self.reconcile_layers(frame, ops);
        self.prev = Some(frame.clone());
        self.last_tick = Some(tick);
    }

    /// Fold a tick's collected [`LayerOps`] into the layer stack for the **next** tick. A set
    /// change is a full swap (layers are per-set) that clears the stack; otherwise adds/removes
    /// update the persistent set and hold-layers persist while their trigger node stays held.
    /// The resulting `active_layers` is sorted by declared-order precedence (`LayerId` index).
    fn reconcile_layers(&mut self, frame: &LogicalFrame, ops: LayerOps) {
        if let Some(set) = ops.set_change {
            self.active_set = set;
            self.persistent_layers.clear();
            self.held_layers.clear();
        } else {
            for l in ops.removes {
                self.persistent_layers.remove(&l);
            }
            for l in ops.adds {
                self.persistent_layers.insert(l);
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
    /// the current tick and kept by [`Self::reconcile_layers`] on subsequent ticks.
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
    use vocab::{GamepadButton, Key};

    /// A `Regular` command firing the given actions.
    fn regular(actions: Vec<CompiledAction>) -> CompiledCommand {
        CompiledCommand { activator: Activator::Regular, actions, settings: CommandSettings::default() }
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
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
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 300);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Swap to the layer (still holding L1). Winning binding changed → fresh activator state:
        // A releases, and the layer's Long has NOT armed yet (press_start reset to now).
        m.force_layer(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 320);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);

        // Only after another 250 ms of holding does the layer's Long fire → B.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 600);
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Activate the layer (a real AddLayer does this in S8's tests; here force it for the
        // resolution check): release first so applied levels are clean, then B should win.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 1);
        m.force_layer(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 2);
        assert_eq!(out, vec![OutputEvent::Key(Key::B, true)]);
    }

    #[test]
    fn switch_program_releases_old_outputs_and_applies_new() {
        // Program A: L1 → A. Program B: L1 → B. Holding L1 across a switch must release A and
        // press B (no stuck key), which is how the loop hot-swaps Main↔Fallback (S9).
        let prog_a = program_with([(InputSource::LeftBumper, btn(CompiledAction::Key(Key::A)))]);
        let prog_b = program_with([(InputSource::LeftBumper, btn(CompiledAction::Key(Key::B)))]);
        let mut m = Mapper::new(&prog_a);

        let out = run(&mut m, &prog_a, &frame(steam_hid::Buttons::L1), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Switch to program B (still holding L1) and tick B: A up, B down in one reconcile.
        m.switch_program(&prog_b);
        let out = run(&mut m, &prog_b, &frame(steam_hid::Buttons::L1), 4);
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L4 | steam_hid::Buttons::L1), 0);
        assert!(out.is_empty());
        // Next tick the layer is active → L1 → A.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L4 | steam_hid::Buttons::L1), 4);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);
        // Release L4 (the hold's node) but keep L1: the layer drops next tick, A releases.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 8);
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 12);
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
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0).is_empty());
        // Now L1 is shadowed by the layer → X down, and it stays down while L1 is held (stable,
        // no flicker) — the HoldLayer command no longer fires but the hold latches to L1.
        assert_eq!(
            run(&mut m, &program, &frame(steam_hid::Buttons::L1), 4),
            vec![OutputEvent::Key(Key::X, true)]
        );
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::L1), 8).is_empty()); // held, stable
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::L1), 12).is_empty());
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Activate the None layer while still holding L1 → A is released, nothing new.
        m.force_layer(LayerId::new(0));
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 4);
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
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 0);
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
        let held = steam_hid::Buttons::L4 | steam_hid::Buttons::L5;
        let _ = run(&mut m, &program, &frame(held.clone()), 0);
        // Now press R1 + R4 too: layer 1 wins R1 (B), layer 0 supplies R4 (C), no A.
        let out = run(
            &mut m,
            &program,
            &frame(held | steam_hid::Buttons::R1 | steam_hid::Buttons::R4),
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
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::L4), 0);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 100);
        // L1 now → A (layer active though L4 long released).
        assert_eq!(
            run(&mut m, &program, &frame(steam_hid::Buttons::L1), 200),
            vec![OutputEvent::Key(Key::A, true)]
        );
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 300);

        // Tap R4 → RemoveLayer. Now L1 is unbound (base has no L1) → nothing.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::R4), 400);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 500);
        assert!(run(&mut m, &program, &frame(steam_hid::Buttons::L1), 600).is_empty());
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
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::L4), 0);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 100);
        assert!(down(&run(&mut m, &program, &frame(steam_hid::Buttons::L1), 200), Key::A));
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 300);

        // Tap Menu → ChangeActionSet(1): next tick we're on set1, layer stack cleared.
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::MENU), 400);
        let _ = run(&mut m, &program, &frame(steam_hid::Buttons::empty()), 500);
        // L1 → B now (set1), and the old layer0's A is gone.
        let out = run(&mut m, &program, &frame(steam_hid::Buttons::L1), 600);
        assert!(down(&out, Key::B) && !down(&out, Key::A));
    }
}
