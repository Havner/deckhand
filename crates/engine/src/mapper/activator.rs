//! Activator state — the retained per-command latches + node timing the activator state
//! machine runs on (PLAN §4 step 5, §4.2 S7).
//!
//! The table is keyed by **source → winning binding**: a source's activator state is reset the
//! moment its winning binding changes (a layer swap), so a long-press started under one binding
//! can't bleed into another (PLAN §4). Within a source, one [`SlotState`] tracks each
//! button-like node the binding exposes — the button itself, or a behavior's virtual buttons
//! (soft-pull, outer-ring, dpad directions, button-pad members) — indexed by a fixed slot order
//! the behavior code owns.

use std::collections::HashMap;

use config::InputSource;

use super::Tick;
use crate::program::LayerId;

/// Which binding won for a source — part of the activator identity, so a change resets state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum BindingKey {
    #[default]
    Base,
    Layer(LayerId),
}

/// The activator table: per-source retained state, reset when a source's winning binding changes.
#[derive(Default)]
pub(super) struct Activators {
    sources: HashMap<InputSource, SourceActivators>,
}

impl Activators {
    /// The activator state for `source` under its current winning binding `key`, **reset to
    /// fresh** if that key differs from last tick's (the no-bleed-across-bindings rule).
    pub(super) fn for_binding(&mut self, source: &InputSource, key: BindingKey) -> &mut SourceActivators {
        let sa = self.sources.entry(source.clone()).or_default();
        if sa.key != key {
            sa.slots.clear();
            sa.key = key;
        }
        sa
    }
}

/// A source's activator state — one [`SlotState`] per button-like node, grown lazily by index.
#[derive(Default)]
pub(super) struct SourceActivators {
    key: BindingKey,
    slots: Vec<SlotState>,
}

impl SourceActivators {
    /// The state for slot `i` (the behavior fixes the slot numbering per binding kind).
    pub(super) fn slot(&mut self, i: usize) -> &mut SlotState {
        if self.slots.len() <= i {
            self.slots.resize_with(i + 1, SlotState::default);
        }
        &mut self.slots[i]
    }
}

/// One button-like node's timing (shared by its commands) + per-command latches.
#[derive(Default)]
pub(super) struct SlotState {
    /// This node's held-state last tick, for digital edge detection (`cur & !prev`).
    pub(super) prev_held: bool,
    /// When the current press began (`None` while released) — the `Long` timer base.
    pub(super) press_start: Option<Tick>,
    /// The **previous** press's start, retained across release — `Double`'s press-to-press window
    /// base (a double = second press within `window_ms` of the *first press*, not its release).
    pub(super) last_press: Option<Tick>,
    commands: Vec<CmdState>,
}

impl SlotState {
    /// The latch state for command `i` on this node (grown lazily).
    pub(super) fn command(&mut self, i: usize) -> &mut CmdState {
        if self.commands.len() <= i {
            self.commands.resize(i + 1, CmdState::default());
        }
        &mut self.commands[i]
    }
}

/// Per-command latch state carried across ticks.
#[derive(Default, Clone)]
pub(super) struct CmdState {
    /// A tap-style output window — one-shot `Start`/`Release` taps, and the interruptible-`Regular`
    /// tap once it commits — held until this stamp.
    pub(super) tap_until: Option<Tick>,
    /// When this hold-taker (`Long`/`Double`) last became active — its **fire time**, used for the
    /// fire-time takeover (latest fire wins the hold) and the minimum-click floor. `None` while
    /// inactive.
    pub(super) hold_since: Option<Tick>,
    /// `Double`: the current press qualified as the second-within-window and is held.
    pub(super) double_active: bool,
    /// Interruptible-`Regular` deferral state, and its interaction's first-press time.
    pub(super) deferred: Deferred,
    pub(super) deferred_start: Option<Tick>,
    /// `toggle`: the latch state, and the previous raw activation (to flip on its rising edge).
    pub(super) toggle_on: bool,
    pub(super) raw_prev: bool,
    /// `turbo`: when the current pulse train started (`None` while inactive).
    pub(super) turbo_start: Option<Tick>,
    /// The command's previous-tick output level, for firing command-haptic pulses on its edges.
    pub(super) haptic_prev: bool,
}

/// The state of an interruptible `Regular` command that shares its node with an interrupter
/// (`Long`/`Double`). It presses-and-holds as soon as it is *safe* from interruption; until then it
/// is deferred. See [`super::command`]'s module docs for the full rule.
#[derive(Default, Clone, PartialEq, Eq)]
pub(super) enum Deferred {
    /// No interaction in progress (or one that has fully resolved with nothing pending).
    #[default]
    Idle,
    /// Pressed, not yet resolved — could still be interrupted, or commit to a hold or a tap.
    Pending,
    /// Committed to a real press-and-hold (safe while still held); outputs until release.
    Holding,
    /// Resolved for this interaction — interrupted (no output) or a committed tap playing out.
    Done,
}
