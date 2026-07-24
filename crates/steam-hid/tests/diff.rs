//! Public-API tests for the stateless `ControllerState::diff` (PLAN §1.5).

use steam_hid::{Axis, Button, Buttons, ControllerState, Event};

#[test]
fn diff_detects_button_press_and_release() {
    let released = ControllerState::default();
    let mut pressed = released.clone();
    pressed.buttons = Buttons::A | Buttons::B;

    let events: Vec<Event> = pressed.diff(&released).collect();
    assert!(events.contains(&Event::ButtonPressed(Button::A)));
    assert!(events.contains(&Event::ButtonPressed(Button::B)));

    let events: Vec<Event> = released.diff(&pressed).collect();
    assert!(events.contains(&Event::ButtonReleased(Button::A)));
    assert!(events.contains(&Event::ButtonReleased(Button::B)));
}

#[test]
fn diff_axis_respects_deadband() {
    let base = ControllerState::default();

    let mut big = base.clone();
    big.left_trigger = 0.5;
    let events: Vec<Event> = big.diff(&base).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::AxisChanged(Axis::LeftTrigger, _)))
    );

    let mut tiny = base.clone();
    tiny.left_trigger = 0.005; // below the deadband
    let events: Vec<Event> = tiny.diff(&base).collect();
    assert!(!events.iter().any(|e| matches!(e, Event::AxisChanged(..))));
}
