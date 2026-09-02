//! Null controller backend: used when no virtual-gamepad feature is enabled (neither `vigem`
//! nor, later, `viiper`). Keyboard/mouse still work fully; only gamepad output is dropped -
//! with a single warning the first time any controller event arrives, so a controller-bound
//! profile isn't silently doing nothing while the log stays quiet under the ~250 Hz axis stream.

use crate::event::Rumble;
use vocab_out::{GamepadAxis, GamepadButton};

/// A controller backend that realizes nothing. `warned` gates the one-time drop warning.
pub(crate) struct NoController {
    warned: bool,
}

impl NoController {
    /// Warn once, on the first dropped gamepad event.
    fn warn_once(&mut self) {
        if !self.warned {
            log::warn!(
                "virt-out(win): gamepad output requested but no controller backend is compiled \
                 in - rebuild with the `vigem` feature; dropping all controller output"
            );
            self.warned = true;
        }
    }

    fn new() -> crate::Result<Self> {
        log::info!(
            "virt-out(win): controller backend = none (no `vigem` feature; gamepad output dropped)"
        );
        Ok(Self { warned: false })
    }

    fn set_button(&mut self, _b: &GamepadButton, _down: bool) {
        self.warn_once();
    }

    fn set_axis(&mut self, _a: &GamepadAxis, _v: f32) {
        self.warn_once();
    }

    fn flush(&mut self) -> crate::Result<()> {
        Ok(())
    }

    fn poll_rumble(&mut self) -> crate::Result<Rumble> {
        Ok(Rumble::default())
    }
}
