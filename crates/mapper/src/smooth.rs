//! One-Euro filter - adaptive low-pass smoothing for the noisy relative behaviors (PLAN 4
//! decision D). Its running state is kept per source in the [`Mapper`](crate::mapper::Mapper) and applied
//! to the behavior's *velocity/rate* signal (frame-rate-independent).
//!
//! Casiez et al.: a low-pass whose cutoff **rises with the signal's speed** - heavy smoothing at
//! rest (kills jitter) but little lag when moving fast. `min_cutoff` is the resting cutoff (Hz;
//! lower = smoother and laggier at rest), `beta` how fast the cutoff opens with speed (higher =
//! less lag when moving). Only `AsMouse` (pad delta) and `GyroToMouse` (noisy IMU) carry one.

use std::f32::consts::PI;

use config::OneEuroFilter;

/// Derivative cutoff (Hz) - the standard fixed value for the One-Euro filter.
const DERIV_CUTOFF: f32 = 1.0;

/// One axis of running filter state. Parameters (`min_cutoff`/`beta`) come from the config each
/// call, so only the running values live here.
#[derive(Debug, Clone, Default)]
struct Axis {
    init: bool,
    x_prev: f32, // last raw input (for the derivative)
    dx_hat: f32, // last filtered derivative
    x_hat: f32,  // last filtered output
}

impl Axis {
    fn filter(&mut self, x: f32, dt: f32, min_cutoff: f32, beta: f32) -> f32 {
        // First sample (or a stalled clock) seeds the state and passes through.
        if !self.init || dt <= 0.0 {
            *self = Axis { init: true, x_prev: x, dx_hat: 0.0, x_hat: x };
            return x;
        }
        let dx = (x - self.x_prev) / dt;
        let edx = lowpass(dx, alpha(DERIV_CUTOFF, dt), self.dx_hat);
        let cutoff = (min_cutoff + beta * edx.abs()).max(f32::MIN_POSITIVE);
        let x_hat = lowpass(x, alpha(cutoff, dt), self.x_hat);
        self.x_prev = x;
        self.dx_hat = edx;
        self.x_hat = x_hat;
        x_hat
    }
}

/// A 2-axis One-Euro filter; its running state is keyed per source in the Mapper.
#[derive(Debug, Clone, Default)]
pub(super) struct OneEuro2 {
    x: Axis,
    y: Axis,
}

impl OneEuro2 {
    /// Smooth `(x, y)` given the tick's `dt` (seconds) and the config parameters.
    pub(super) fn filter(&mut self, x: f32, y: f32, dt: f32, cfg: &OneEuroFilter) -> (f32, f32) {
        (self.x.filter(x, dt, cfg.min_cutoff, cfg.beta), self.y.filter(y, dt, cfg.min_cutoff, cfg.beta))
    }
}

/// Low-pass smoothing factor for a cutoff frequency (Hz) at timestep `dt` (s).
fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (2.0 * PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

fn lowpass(x: f32, alpha: f32, prev: f32) -> f32 {
    alpha * x + (1.0 - alpha) * prev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooths_a_noisy_signal_toward_its_mean() {
        // A signal jittering +/-2 around 10 should filter to stay close to 10 after the first
        // (seed) sample, with min_cutoff low enough to smooth heavily.
        let cfg = OneEuroFilter { min_cutoff: 1.0, beta: 0.0 };
        let mut f = OneEuro2::default();
        let samples = [10.0, 12.0, 8.0, 11.0, 9.0, 10.5, 9.5, 10.0];
        let mut max_dev = 0.0f32;
        for (i, &s) in samples.iter().enumerate() {
            let (out, _) = f.filter(s, s, 0.004, &cfg);
            if i > 0 {
                max_dev = max_dev.max((out - 10.0).abs());
            }
        }
        assert!(max_dev < 1.0, "filtered signal should hug the mean, max dev {max_dev}");
    }

    #[test]
    fn first_sample_passes_through() {
        let cfg = OneEuroFilter::default();
        let mut f = OneEuro2::default();
        assert_eq!(f.filter(3.0, 3.0, 0.004, &cfg), (3.0, 3.0));
    }
}
