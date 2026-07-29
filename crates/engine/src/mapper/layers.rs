//! Layer / action-set change plumbing (PLAN §4 steps 1/6, §4.2 S8).
//!
//! Layer and set changes are **collected during a tick** and applied to the stack for the
//! **next** tick — the layer state a tick resolves against is frozen at tick start, keeping the
//! per-tick pass a single acyclic walk (PLAN §4). A `HoldLayer` latches to its firing **node's
//! held-state** (via [`NodeHeld`]), not to the winning binding, so it survives self-shadowing
//! (the layer re-binding the very node that triggered it) without flicker.

use std::collections::{BTreeMap, BTreeSet};

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
}

/// Layer/set changes a tick's firing commands requested, applied to the stack for the next tick.
/// Set changes and adds/removes dedupe; the last `ChangeActionSet`/`HoldLayer` node wins.
#[derive(Default)]
pub(super) struct LayerOps {
    pub(super) set_change: Option<SetId>,
    pub(super) adds: BTreeSet<LayerId>,
    pub(super) removes: BTreeSet<LayerId>,
    pub(super) holds: BTreeMap<LayerId, NodeHeld>,
}
