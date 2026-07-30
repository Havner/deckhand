//! Player-space gyro support (PLAN §3 Round B). Player space projects the angular velocity onto
//! real-world vertical, so "turning around vertical" maps to horizontal aim no matter how the
//! controller is tilted/rolled. That needs the **gravity direction**, which we recover with a slow
//! low-pass of the accelerometer: the accel reads gravity + linear motion, and while aiming (little
//! sustained linear acceleration) the low-pass settles on gravity. Full gyro/accel sensor fusion is
//! deferred — this is the crude-but-good-enough estimate (like the crude local-space start).

/// Gravity-direction estimate — an EMA of the raw accel vector, kept per gyro source in the
/// [`Mapper`](super::Mapper). [`Self::update`] returns the current **up** unit vector (points up,
/// opposite gravity) — the axis player space projects yaw+roll onto.
#[derive(Debug, Clone, Default)]
pub(super) struct GravityEst {
    v: [f32; 3],
    init: bool,
}

impl GravityEst {
    /// Low-pass time constant (s): slow enough to reject aiming motion, fast enough to track a real
    /// re-orientation of the controller within ~a second.
    const TAU: f32 = 0.5;

    /// Fold in a raw accel sample and return the smoothed **up** unit vector.
    pub(super) fn update(&mut self, accel: [f32; 3], dt: f32) -> [f32; 3] {
        if !self.init || dt <= 0.0 {
            self.v = accel; // seed on the first sample (or a stalled clock)
            self.init = true;
        } else {
            let a = (dt / (Self::TAU + dt)).clamp(0.0, 1.0);
            for (v, &sample) in self.v.iter_mut().zip(accel.iter()) {
                *v += a * (sample - *v);
            }
        }
        let mag = (self.v[0] * self.v[0] + self.v[1] * self.v[1] + self.v[2] * self.v[2]).sqrt();
        if mag < 1e-6 {
            [0.0, 0.0, 1.0] // no reading yet → assume flat (up = +Z)
        } else {
            [self.v[0] / mag, self.v[1] / mag, self.v[2] / mag]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_on_the_gravity_direction_and_normalizes() {
        // A steady accel of (0,0,2g-ish) → up unit vector (0,0,1) after enough samples.
        let mut g = GravityEst::default();
        let mut up = [0.0; 3];
        for _ in 0..500 {
            up = g.update([0.0, 0.0, 16384.0], 0.004);
        }
        assert!((up[2] - 1.0).abs() < 1e-3 && up[0].abs() < 1e-3 && up[1].abs() < 1e-3);
    }

    #[test]
    fn first_sample_seeds_immediately() {
        let mut g = GravityEst::default();
        let up = g.update([16384.0, 0.0, 0.0], 0.004); // right-side down → up = +X
        assert!((up[0] - 1.0).abs() < 1e-3);
    }
}
