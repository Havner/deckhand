//! Output-side reconciliation + relative accumulation (PLAN §4 / §4.2 S4).
//!
//! The mapper computes, each tick, the **desired output state** — the levels that should be
//! held *now* ([`DesiredLevels`]) — and the relative nudges (mouse/scroll). Reconciliation
//! emits only the **difference** vs the last-applied levels ([`AppliedLevels`]): so a held
//! output stays held with zero events, a released one emits exactly one up, and a layer
//! change can't strand a stuck output (the desired set is recomputed fresh each tick).
//!
//! Relative outputs are the exception ([`RelAccum`]) — they're deltas, not levels, so they
//! **accumulate** a sub-pixel remainder and emit only the integer part, carrying the fraction
//! forward (PLAN §4; a real feel improvement validated in the Phase B bridge).
#![allow(dead_code)] // Wired by Mapper::tick in S5; this allow drops then.

use std::collections::{BTreeMap, BTreeSet};

use virt_out::OutputEvent;
use vocab::{GamepadAxis, GamepadButton, Key, MouseButton};

/// The desired output **levels** for one tick (what should be held now). Keys/buttons are
/// membership; axes carry a position. Scroll pseudo-buttons are **not** levels (they're
/// impulses) and must not appear here.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DesiredLevels {
    keys: BTreeSet<Key>,
    mouse_buttons: BTreeSet<MouseButton>,
    pad_buttons: BTreeSet<GamepadButton>,
    axes: BTreeMap<GamepadAxis, f32>,
}

impl DesiredLevels {
    pub fn press_key(&mut self, key: Key) {
        self.keys.insert(key);
    }
    pub fn press_mouse(&mut self, button: MouseButton) {
        debug_assert!(!button.is_scroll(), "scroll pseudo-buttons are impulses, not levels");
        self.mouse_buttons.insert(button);
    }
    pub fn press_pad(&mut self, button: GamepadButton) {
        self.pad_buttons.insert(button);
    }
    pub fn set_axis(&mut self, axis: GamepadAxis, value: f32) {
        self.axes.insert(axis, value);
    }
}

/// The last-applied output levels. [`Self::reconcile`] diffs a new [`DesiredLevels`] into it,
/// pushing only the changed events, and adopts the new state.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AppliedLevels {
    keys: BTreeSet<Key>,
    mouse_buttons: BTreeSet<MouseButton>,
    pad_buttons: BTreeSet<GamepadButton>,
    axes: BTreeMap<GamepadAxis, f32>,
}

impl AppliedLevels {
    /// Emit the diff (releases before presses, sorted → deterministic) and adopt `desired`.
    pub fn reconcile(&mut self, desired: &DesiredLevels, out: &mut Vec<OutputEvent>) {
        diff_set(&self.keys, &desired.keys, out, |k, down| OutputEvent::Key(k.clone(), down));
        diff_set(&self.mouse_buttons, &desired.mouse_buttons, out, |b, down| {
            OutputEvent::MouseButton(b.clone(), down)
        });
        diff_set(&self.pad_buttons, &desired.pad_buttons, out, |b, down| {
            OutputEvent::GamepadButton(b.clone(), down)
        });

        // Axes over the union of applied+desired: an axis dropped from desired returns to
        // neutral (0.0). Emit only where the value actually changed.
        let touched: BTreeSet<&GamepadAxis> = self.axes.keys().chain(desired.axes.keys()).collect();
        for axis in touched {
            let want = desired.axes.get(axis).copied().unwrap_or(0.0);
            let have = self.axes.get(axis).copied().unwrap_or(0.0);
            if want != have {
                out.push(OutputEvent::GamepadAxis(axis.clone(), want));
            }
        }

        self.keys = desired.keys.clone();
        self.mouse_buttons = desired.mouse_buttons.clone();
        self.pad_buttons = desired.pad_buttons.clone();
        // Keep only non-neutral axes so the applied set stays minimal.
        self.axes =
            desired.axes.iter().filter(|(_, v)| **v != 0.0).map(|(a, v)| (a.clone(), *v)).collect();
    }
}

/// Emit releases (applied − desired) then presses (desired − applied). `BTreeSet::difference`
/// yields sorted order, so the event stream is deterministic (golden-testable).
fn diff_set<T: Ord + Clone>(
    applied: &BTreeSet<T>,
    desired: &BTreeSet<T>,
    out: &mut Vec<OutputEvent>,
    make: impl Fn(&T, bool) -> OutputEvent,
) {
    for t in applied.difference(desired) {
        out.push(make(t, false));
    }
    for t in desired.difference(applied) {
        out.push(make(t, true));
    }
}

/// Relative-output accumulator (mouse move + continuous scroll). Emits the integer part and
/// **carries the fraction forward** so slow/fine motion isn't truncated away (PLAN §4). NOT
/// anti-drift — it faithfully integrates; bias correction is the behavior's job.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RelAccum {
    mouse_x: f32,
    mouse_y: f32,
    scroll_x: f32,
    scroll_y: f32,
}

impl RelAccum {
    pub fn add_mouse(&mut self, dx: f32, dy: f32) {
        self.mouse_x += dx;
        self.mouse_y += dy;
    }
    pub fn add_scroll(&mut self, dx: f32, dy: f32) {
        self.scroll_x += dx;
        self.scroll_y += dy;
    }

    /// Emit accumulated integer motion, keeping the sub-integer remainder for next tick.
    pub fn flush(&mut self, out: &mut Vec<OutputEvent>) {
        let (mx, my) = (self.mouse_x.trunc(), self.mouse_y.trunc());
        if mx != 0.0 || my != 0.0 {
            out.push(OutputEvent::MouseMove { dx: mx as i32, dy: my as i32 });
            self.mouse_x -= mx;
            self.mouse_y -= my;
        }
        let (sx, sy) = (self.scroll_x.trunc(), self.scroll_y.trunc());
        if sx != 0.0 || sy != 0.0 {
            out.push(OutputEvent::Scroll { dx: sx as i32, dy: sy as i32 });
            self.scroll_x -= sx;
            self.scroll_y -= sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_press_hold_release() {
        let mut applied = AppliedLevels::default();
        let mut out = Vec::new();

        // Press A.
        let mut d = DesiredLevels::default();
        d.press_key(Key::A);
        applied.reconcile(&d, &mut out);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, true)]);

        // Hold A → no events.
        out.clear();
        applied.reconcile(&d, &mut out);
        assert!(out.is_empty());

        // Release (empty desired) → one up.
        out.clear();
        applied.reconcile(&DesiredLevels::default(), &mut out);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false)]);
    }

    #[test]
    fn swap_emits_release_before_press() {
        let mut applied = AppliedLevels::default();
        let mut out = Vec::new();
        let mut a = DesiredLevels::default();
        a.press_key(Key::A);
        applied.reconcile(&a, &mut out);

        // Now want B instead of A.
        out.clear();
        let mut b = DesiredLevels::default();
        b.press_key(Key::B);
        applied.reconcile(&b, &mut out);
        assert_eq!(out, vec![OutputEvent::Key(Key::A, false), OutputEvent::Key(Key::B, true)]);
    }

    #[test]
    fn axis_set_and_return_to_neutral() {
        let mut applied = AppliedLevels::default();
        let mut out = Vec::new();

        let mut d = DesiredLevels::default();
        d.set_axis(GamepadAxis::LeftStickX, 0.5);
        applied.reconcile(&d, &mut out);
        assert_eq!(out, vec![OutputEvent::GamepadAxis(GamepadAxis::LeftStickX, 0.5)]);

        // Drop the axis from desired → returns to neutral once.
        out.clear();
        applied.reconcile(&DesiredLevels::default(), &mut out);
        assert_eq!(out, vec![OutputEvent::GamepadAxis(GamepadAxis::LeftStickX, 0.0)]);

        // Stays neutral → no further events.
        out.clear();
        applied.reconcile(&DesiredLevels::default(), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn rel_accum_carries_subpixel_remainder() {
        let mut acc = RelAccum::default();
        let mut out = Vec::new();

        // 0.6 px: below 1, nothing emitted yet.
        acc.add_mouse(0.6, 0.0);
        acc.flush(&mut out);
        assert!(out.is_empty());

        // +0.6 = 1.2 → emit 1, keep 0.2.
        acc.add_mouse(0.6, 0.0);
        acc.flush(&mut out);
        assert_eq!(out, vec![OutputEvent::MouseMove { dx: 1, dy: 0 }]);

        // +0.6 = 0.8 → still nothing.
        out.clear();
        acc.add_mouse(0.6, 0.0);
        acc.flush(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn rel_accum_scroll_independent() {
        let mut acc = RelAccum::default();
        let mut out = Vec::new();
        acc.add_scroll(0.0, 2.5);
        acc.flush(&mut out);
        assert_eq!(out, vec![OutputEvent::Scroll { dx: 0, dy: 2 }]);
        // Remainder 0.5 carried.
        out.clear();
        acc.add_scroll(0.0, 0.5);
        acc.flush(&mut out);
        assert_eq!(out, vec![OutputEvent::Scroll { dx: 0, dy: 1 }]);
    }
}
