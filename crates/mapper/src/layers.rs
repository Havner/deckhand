//! Layer / action-set change plumbing (PLAN 4 steps 1/6, 4.2 S8).
//!
//! Layer and set changes are **collected during a tick** and applied to the stack for the
//! **next** tick - the layer state a tick resolves against is frozen at tick start, keeping the
//! per-tick pass a single acyclic walk (PLAN 4). A `HoldLayer` latches to its firing **node's
//! held-state** (via [`NodeHeld`]), not to the winning binding, so it survives self-shadowing
//! (the layer re-binding the very node that triggered it) without flicker.

use std::collections::{BTreeMap, HashMap};

use config::{Feedback, InputSource, Side};

use crate::mapper::FeedbackReq;
use crate::logical::{Dir, LogicalFrame};
use crate::program::{LayerId, SetId};

/// How to re-derive a `HoldLayer` trigger node's held-state on later ticks. Frame-local triggers
/// (physical bits, group members, soft-pull) re-derive exactly and are robust; a layer-dependent
/// virtual button (outer-ring, dpad direction) can't be re-derived independent of its (possibly
/// now-shadowed) binding, so it reads released - it may flicker but never sticks (PLAN 4).
#[derive(Debug, Clone)]
pub(super) enum NodeHeld {
    Button(InputSource),
    Group(InputSource, Dir),
    SoftPull(InputSource, f32),
    Virtual,
}

impl NodeHeld {
    /// Whether the trigger node is held in `frame`.
    pub(super) fn held(&self, frame: &LogicalFrame) -> bool {
        match self {
            NodeHeld::Button(s) => frame.button(s),
            NodeHeld::Group(s, d) => frame.group_member(s, d),
            NodeHeld::SoftPull(s, t) => frame.trigger(s) >= *t,
            NodeHeld::Virtual => false,
        }
    }

    /// This node's hashable identity, stable across a binding swap on the same physical node - the
    /// key for [`ArmedNodes`]. The `SoftPull` threshold is dropped (two soft-pull thresholds both
    /// carrying persistent ops is not a real config).
    pub(super) fn key(&self) -> NodeKey {
        match self {
            NodeHeld::Button(s) => NodeKey::Button(s.clone()),
            NodeHeld::Group(s, d) => NodeKey::Group(s.clone(), d.clone()),
            NodeHeld::SoftPull(s, _) => NodeKey::SoftPull(s.clone()),
            NodeHeld::Virtual => NodeKey::Virtual,
        }
    }

    /// Whether this node is free to fire a persistent OpSet - i.e. it hasn't already armed one during
    /// its current engagement. The per-press dedup predicate: an armed node's further OpSets (any
    /// kind) are suppressed until it releases.
    pub(super) fn may_fire(&self, armed: &ArmedNodes) -> bool {
        !armed.contains_key(&self.key())
    }
}

/// A node's identity independent of its binding - the key for [`ArmedNodes`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum NodeKey {
    Button(InputSource),
    Group(InputSource, Dir),
    SoftPull(InputSource),
    Virtual,
}

/// Nodes that have already fired their one-shot persistent op (`AddLayer`/`RemoveLayer`/
/// `ChangeActionSet`) during their **current continuous engagement** - the anti-oscillation latch
/// (PLAN 4). Keyed by the physical node (so the binding swap the op *itself* causes - base-to-layer
/// or set-to-set - can't re-fire it); the [`NodeHeld`] value lets the mapper drop an entry the tick
/// its node releases (`retain(|_, n| n.held(frame))`), freeing the next press to fire again. Same
/// "keep it while `node.held`" idea `held_layers` uses for holds, anchored to the raw frame not to
/// whether a command re-fired.
pub(super) type ArmedNodes = HashMap<NodeKey, NodeHeld>;

/// A `HoldLayer` command's deferred feedback - the whole [`Feedback`] plus the triggering side.
/// Stored with the held layer (not fired from the command's output level) so `reconcile_held_layers`
/// can play the `on_press` effect on the layer's real **engage** and `on_release` on **disengage** -
/// the latter surviving the self-shadow that hides the base command on the release tick (PLAN 7.3).
#[derive(Debug, Clone)]
pub(super) struct HoldFeedback {
    pub(super) feedback: Feedback,
    pub(super) side: Side,
}

/// A layer held live by a `HoldLayer`, with how to re-derive its trigger-node held-state and its
/// deferred engage/disengage feedback (if any). Held in `Mapper::held_layers` for the whole hold.
#[derive(Debug, Clone)]
pub(super) struct HeldLayer {
    pub(super) node: NodeHeld,
    pub(super) feedback: Option<HoldFeedback>,
}

/// Layer/set changes a tick's firing commands requested, applied to the stack for the next tick.
/// Every entry carries its **trigger node**, so `reconcile_layer_ops` treats all node-anchored effects
/// uniformly: every OpSet (`set_change`/`adds`/`removes` **and** `holds`) is deduped per node against
/// [`ArmedNodes`] so a self-toggling button can't strobe. Last `set_change` wins.
///
/// The collection per field matches its semantics, not the stage it was written in:
/// - `set_change` - one active set, so 0-or-1 winner per tick -> `Option`.
/// - `holds` - a **set of held layers** merged into `held_layers`; a layer is held-or-not, so dedup
///   *by layer* is right and one representative [`HeldLayer`] (its trigger node + deferred lifecycle
///   haptic) per layer suffices for the keep-while-held check -> `BTreeMap<LayerId, _>`.
/// - `adds`/`removes` - **lists of one-shot `(layer, node)` ops**, not a set of layers. Each op is
///   deduped against its *own* node, and every firing node must be armed - so two different buttons
///   adding the *same* layer on one tick must stay two entries. A map keyed by `LayerId` would merge
///   them and drop a node from both the dedup and the arming -> `Vec`. (Can't be a `BTreeSet<LayerId>`
///   like the pre-`armed_nodes` shape either - it now has to carry `NodeHeld`, which holds an `f32`.)
/// - `pending_feedback` - the **deferred feedback of persistent-OpSet commands** (`ChangeActionSet`/
///   `AddLayer`/`RemoveLayer`). A persistent op has no held level and is dedup-gated, so its feedback
///   can't ride an output edge like an ordinary command's - it must fire iff the op *actually lands*.
///   `reconcile_layer_ops` emits each entry whose node changed the persistent state this tick, keyed
///   by node against the same verdict the ops use, so an op that was deduped, superseded by a
///   `set_change`, or a no-op against the stack stays silent (PLAN 4).
#[derive(Default)]
pub(super) struct LayerOps {
    pub(super) set_change: Option<(SetId, NodeHeld)>,
    pub(super) adds: Vec<(LayerId, NodeHeld)>,
    pub(super) removes: Vec<(LayerId, NodeHeld)>,
    pub(super) holds: BTreeMap<LayerId, HeldLayer>,
    pub(super) pending_feedback: Vec<(NodeHeld, FeedbackReq)>,
}
