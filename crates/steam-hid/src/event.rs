//! Events: the streaming, change-driven view over the frame stream (PLAN §1.5).
//!
//! [`Event`]s are a convenience for change-log consumers (the `dump` example, a
//! future UI binding-capture). The engine does **not** use this path — it reads
//! snapshots and does its own exact button-edge detection (PLAN §1.5, §4).

use std::collections::VecDeque;

use crate::buttons::{Axis, Button, button_flag};
use crate::device::Device;
use crate::state::{Battery, ControllerState, Report};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Deadband applied to analog axes before an [`Event::AxisChanged`] is emitted,
/// so a change-log doesn't spew at the full stream rate on sensor noise. This is
/// cosmetic and analog-only — it never touches the snapshot path (PLAN §1.5).
const AXIS_DEADBAND: f32 = 0.005;

/// A change-driven event derived from the frame stream (PLAN §1.5).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum Event {
    ButtonPressed(Button),
    ButtonReleased(Button),
    /// Analog axis crossed the deadband; carries the new normalized value.
    AxisChanged(Axis, f32),
    Connected,
    Disconnected,
    Battery(Battery),
}

impl ControllerState {
    /// Stateless diff: events for the transition from `prev` to `self` (PLAN §1.5).
    ///
    /// Digital buttons are exact bit flips; analog axes apply [`AXIS_DEADBAND`].
    pub fn diff(&self, prev: &Self) -> impl Iterator<Item = Event> {
        let mut out = Vec::new();

        let (cur_b, prev_b) = (self.buttons.bits(), prev.buttons.bits());
        if cur_b != prev_b {
            for btn in Button::ALL {
                let bit = button_flag(&btn).bits();
                let now = cur_b & bit != 0;
                let was = prev_b & bit != 0;
                if now && !was {
                    out.push(Event::ButtonPressed(btn));
                } else if !now && was {
                    out.push(Event::ButtonReleased(btn));
                }
            }
        }

        for axis in Axis::ALL {
            let now = self.axis(axis.clone());
            let was = prev.axis(axis.clone());
            if (now - was).abs() >= AXIS_DEADBAND {
                out.push(Event::AxisChanged(axis, now));
            }
        }

        out.into_iter()
    }
}

/// Streaming iterator of [`Event`]s over a [`Device`] (PLAN §1.5).
///
/// Holds an internal previous snapshot and yields diffs for input frames plus
/// passthrough for lifecycle frames. Ends (returns `None`) on a read error.
pub struct Events<'a> {
    device: &'a mut Device,
    prev: Option<ControllerState>,
    pending: VecDeque<Event>,
}

impl<'a> Events<'a> {
    pub(crate) fn new(device: &'a mut Device) -> Self {
        Events {
            device,
            prev: None,
            pending: VecDeque::new(),
        }
    }
}

impl Iterator for Events<'_> {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(e) = self.pending.pop_front() {
                return Some(e);
            }
            match self.device.read() {
                Ok(Report::State(cur)) => {
                    if let Some(prev) = &self.prev {
                        self.pending.extend(cur.diff(prev));
                    }
                    self.prev = Some(cur);
                    // Loop again to drain pending (or read the next frame).
                }
                Ok(Report::Connected) => return Some(Event::Connected),
                Ok(Report::Disconnected) => return Some(Event::Disconnected),
                Ok(Report::Battery(b)) => return Some(Event::Battery(b)),
                Err(_) => return None,
            }
        }
    }
}
