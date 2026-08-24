//! Layer / action-set change plumbing (PLAN §4 steps 1/6, §4.2 S8).
//!
//! Layer and set changes are **collected during a tick** and applied to the stack for the
//! **next** tick — the layer state a tick resolves against is frozen at tick start, keeping the
//! per-tick pass a single acyclic walk (PLAN §4). A `HoldLayer` latches to its firing **node's
//! held-state** (via [`NodeHeld`]), not to the winning binding, so it survives self-shadowing
//! (the layer re-binding the very node that triggered it) without flicker.

use std::collections::{BTreeMap, HashMap};

use config::InputSource;

use crate::logical::{Dir, LogicalFrame};
use crate::program::{LayerId, SetId};

/// How to re-derive a `HoldLayer` trigger node's held-state on later ticks. Frame-local triggers
/// (physical bits, group members, soft-pull) re-derive exactly and are robust; a layer-dependent
/// virtual button (outer-ring, dpad direction) can't be re-derived independent of its (possibly
/// now-shadowed) binding, so it reads released — it may flicker but never sticks (PLAN §4).
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

    /// This node's hashable identity, stable across a binding swap on the same physical node — the
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
}

/// A node's identity independent of its binding — the key for [`ArmedNodes`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum NodeKey {
    Button(InputSource),
    Group(InputSource, Dir),
    SoftPull(InputSource),
    Virtual,
}

/// Nodes that have already fired their one-shot persistent op (`AddLayer`/`RemoveLayer`/
/// `ChangeActionSet`) during their **current continuous engagement** — the anti-oscillation latch
/// (PLAN §4). Keyed by the physical node (so the binding swap the op *itself* causes — base-to-layer
/// or set-to-set — can't re-fire it); the [`NodeHeld`] value lets the mapper drop an entry the tick
/// its node releases (`retain(|_, n| n.held(frame))`), freeing the next press to fire again. Same
/// "keep it while `node.held`" idea `held_layers` uses for holds, anchored to the raw frame not to
/// whether a command re-fired.
pub(super) type ArmedNodes = HashMap<NodeKey, NodeHeld>;

/// Layer/set changes a tick's firing commands requested, applied to the stack for the next tick.
/// Every entry carries its **trigger node**, so `reconcile_layer_ops` treats all node-anchored effects
/// uniformly: `holds` latch to `node.held` ([`HoldLayer`](crate::program::CompiledAction::HoldLayer)),
/// and the persistent mutations (`set_change`/`adds`/`removes`) are deduped per node against
/// [`ArmedNodes`] so a self-toggling button can't strobe. Last `set_change` wins.
#[derive(Default)]
pub(super) struct LayerOps {
    pub(super) set_change: Option<(SetId, NodeHeld)>,
    pub(super) adds: Vec<(LayerId, NodeHeld)>,
    pub(super) removes: Vec<(LayerId, NodeHeld)>,
    pub(super) holds: BTreeMap<LayerId, NodeHeld>,
}
