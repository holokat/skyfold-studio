//! Fixed-timestep accumulator (port of `core/frameStepper.ts`, "Fix Your Timestep!").

/// Fraction of a step that still counts as a whole step, absorbing float rounding.
const STEP_TOLERANCE: f64 = 1e-6;

/// Runs whole `fixed_dt` steps out of elapsed real time, at most
/// `max_steps_per_frame` per call; time beyond that is dropped so a slow
/// machine runs slower than real time instead of falling further behind.
#[derive(Clone, Debug)]
pub struct FrameStepper {
    pub fixed_dt: f64,
    pub max_steps_per_frame: u32,
    /// Elapsed time above this counts as a pause: the clock restarts without catching up.
    pub pause_threshold: f64,
    accumulator: f64,
}

/// What one [`FrameStepper::pump`] decided.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StepInfo {
    /// Fixed steps run by this call.
    pub steps: u32,
    /// Elapsed time was dropped because the simulation fell behind.
    pub truncated: bool,
    /// Time carried to the next call, s.
    pub remainder: f64,
}

impl FrameStepper {
    pub fn new(fixed_dt: f64, max_steps_per_frame: u32) -> Self {
        assert!(
            fixed_dt > 0.0 && fixed_dt.is_finite(),
            "FrameStepper: fixed_dt must be positive"
        );
        assert!(
            max_steps_per_frame > 0,
            "FrameStepper: max_steps_per_frame must be positive"
        );
        Self {
            fixed_dt,
            max_steps_per_frame,
            pause_threshold: 0.25,
            accumulator: 0.0,
        }
    }

    /// Add `elapsed` seconds and decide how many fixed steps to run.
    pub fn pump(&mut self, elapsed: f64) -> StepInfo {
        if elapsed.is_nan() || elapsed < 0.0 || elapsed > self.pause_threshold {
            // After a pause, carry on from where we were instead of catching up.
            self.accumulator = 0.0;
            return StepInfo::default();
        }
        self.accumulator += elapsed;
        let available = self.accumulator / self.fixed_dt;
        let max = self.max_steps_per_frame as f64;
        let truncated = available > max + STEP_TOLERANCE;
        let steps = if truncated {
            max
        } else {
            (available + STEP_TOLERANCE).floor().min(max)
        };
        self.accumulator = if truncated {
            0.0
        } else {
            (self.accumulator - steps * self.fixed_dt).max(0.0)
        };
        StepInfo {
            steps: steps as u32,
            truncated,
            remainder: self.accumulator,
        }
    }

    /// Forget accumulated time.
    pub fn reset(&mut self) {
        self.accumulator = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_and_caps() {
        let mut s = FrameStepper::new(1.0 / 60.0, 4);
        assert_eq!(s.pump(1.0 / 120.0).steps, 0);
        assert_eq!(s.pump(1.0 / 120.0).steps, 1);
        let r = s.pump(0.2);
        assert_eq!(r.steps, 4);
        assert!(r.truncated);
        assert_eq!(s.pump(0.3).steps, 0);
    }
}
