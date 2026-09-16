//! Fixed-duration interpolation for small numeric vectors.

#[derive(Clone, Copy, Debug, PartialEq)]
/// Tracks a clamped linear transition between two points.
pub struct LinearInterpolation<const DIMENSIONS: usize> {
    p0: [f32; DIMENSIONS],
    p1: [f32; DIMENSIONS],
    elapsed_secs: f32,
    duration_secs: f32,
}

impl<const DIMENSIONS: usize> LinearInterpolation<DIMENSIONS> {
    /// Creates a completed transition fixed at one point.
    pub fn stationary(point: [f32; DIMENSIONS], duration_secs: f32) -> Self {
        Self {
            p0: point,
            p1: point,
            elapsed_secs: duration_secs,
            duration_secs,
        }
    }

    /// Starts a new transition from an explicit visual origin.
    pub fn retarget(&mut self, p0: [f32; DIMENSIONS], p1: [f32; DIMENSIONS]) {
        self.p0 = p0;
        self.p1 = p1;
        self.elapsed_secs = 0.0;
    }

    /// Completes the transition immediately at a new point.
    pub fn reset(&mut self, point: [f32; DIMENSIONS]) {
        self.p0 = point;
        self.p1 = point;
        self.elapsed_secs = self.duration_secs;
    }

    /// Advances by finite positive time and returns the current value.
    pub fn advance(&mut self, delta_secs: f32) -> [f32; DIMENSIONS] {
        if delta_secs.is_finite() && delta_secs > 0.0 {
            self.elapsed_secs += delta_secs;
        }
        self.value()
    }

    /// Evaluates the transition without advancing elapsed time.
    pub fn value(&self) -> [f32; DIMENSIONS] {
        let factor = if self.duration_secs.is_finite() && self.duration_secs > 0.0 {
            (self.elapsed_secs / self.duration_secs).clamp(0.0, 1.0)
        } else {
            1.0
        };
        std::array::from_fn(|index| self.p0[index] + (self.p1[index] - self.p0[index]) * factor)
    }
}

#[cfg(test)]
mod tests {
    use super::LinearInterpolation;

    #[test]
    fn advances_linearly_between_points_and_clamps_at_p1() {
        let mut interpolation = LinearInterpolation::stationary([0.0, 0.0, 0.0], 0.05);
        interpolation.retarget([0.0, 2.0, 4.0], [10.0, 4.0, 0.0]);

        assert_eq!(interpolation.advance(0.025), [5.0, 3.0, 2.0]);
        assert_eq!(interpolation.advance(0.1), [10.0, 4.0, 0.0]);
    }

    #[test]
    fn retargeting_starts_from_the_current_visual_point() {
        let mut interpolation = LinearInterpolation::stationary([0.0], 1.0);
        interpolation.retarget([0.0], [10.0]);
        let current = interpolation.advance(0.4);
        interpolation.retarget(current, [20.0]);

        assert_eq!(interpolation.value(), [4.0]);
        assert_eq!(interpolation.advance(0.5), [12.0]);
    }
}
