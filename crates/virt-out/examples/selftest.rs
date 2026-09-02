//! `selftest` - drive the virtual devices directly to verify the Linux backend.
//!
//! Creates the keyboard/mouse/gamepad, then loops a demo pattern: nudge the mouse,
//! tap gamepad A, sweep the left stick, pull the right trigger, and tap a key. Also
//! polls for rumble each cycle (run `fftest` against the gamepad's event node to see
//! it). Verify with `evtest` (keyboard/mouse/gamepad) and `jstest`/SDL (the pad should
//! show up as "Microsoft X-Box 360 pad"). Ctrl-C to stop.
//!
//! Run: `cargo run -p virt-out --example selftest`.

use std::thread::sleep;
use std::time::Duration;

use virt_out::{GamepadAxis, GamepadButton, OutputEvent, Sink};

fn main() -> virt_out::Result<()> {
    let mut sink = Sink::new()?;
    println!("virtual devices created (keyboard / mouse / 'Microsoft X-Box 360 pad').");
    println!("Open evtest / jstest to watch. Ctrl-C to stop.");
    // Give userspace time to notice the new devices, and you time to open a tester.
    for n in (1..=3).rev() {
        println!("starting in {n}s...");
        sleep(Duration::from_secs(1));
    }
    println!();

    let step = Duration::from_millis(300);
    loop {
        // Gamepad-only demo (keyboard/mouse dropped so fftest's terminal stays clean).
        // Gamepad button A tap.
        sink.emit(&[OutputEvent::GamepadButton(GamepadButton::A, true)])?;
        sleep(step);
        sink.emit(&[OutputEvent::GamepadButton(GamepadButton::A, false)])?;

        // Left stick sweep left -> right -> center.
        for v in [-1.0, 1.0, 0.0] {
            sink.emit(&[OutputEvent::GamepadAxis(GamepadAxis::LeftStickX, v)])?;
            sleep(step);
        }

        // Right trigger pull -> release.
        sink.emit(&[OutputEvent::GamepadAxis(GamepadAxis::RightTrigger, 1.0)])?;
        sleep(step);
        sink.emit(&[OutputEvent::GamepadAxis(GamepadAxis::RightTrigger, 0.0)])?;

        // Rumble back-channel: print anything a consumer sent us.
        let r = sink.poll_rumble()?;
        if !r.is_zero() {
            println!("rumble in: strong={} weak={}", r.strong, r.weak);
        }
        sleep(step);
    }
}
