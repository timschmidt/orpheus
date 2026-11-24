//! Fades for pause, seek, skip and underrun (04 §5.1: 5–60 ms linear
//! ramps). The last gain before quantisation, so a fade-out reaches true
//! digital silence.

use crate::fixed::{Ramp, UNITY};

#[derive(Debug, Clone)]
pub struct Fade {
    ramp: Ramp<1>,
    rate: u32,
}

impl Default for Fade {
    fn default() -> Self {
        Self::new()
    }
}

impl Fade {
    #[must_use]
    pub const fn new() -> Self {
        Self { ramp: Ramp::new([UNITY]), rate: 44100 }
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
    }

    /// Ramp linearly to `level` (Q3.28, 0 = silence, [`UNITY`] = full) over
    /// `ms` milliseconds.
    pub fn to(&mut self, level: i32, ms: u32) {
        let frames = self.rate.saturating_mul(ms) / 1000;
        self.ramp.to([level.clamp(0, UNITY)], frames);
    }

    /// The current level, Q3.28.
    #[must_use]
    pub fn level(&self) -> i32 {
        self.ramp.cur[0]
    }

    /// The fade has finished.
    #[must_use]
    pub fn done(&self) -> bool {
        !self.ramp.ramping()
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.ramp.ramping() || self.ramp.cur[0] != UNITY
    }

    pub fn process(&mut self, buf: &mut [i32], channels: usize) {
        let ch = channels.max(1);
        if !self.ramp.ramping() {
            let g = i64::from(self.ramp.cur[0]);
            if g == 0 {
                buf.fill(0);
                return;
            }
            for x in buf.iter_mut() {
                *x = ((i64::from(*x) * g) >> 28) as i32;
            }
            return;
        }
        for f in buf.chunks_exact_mut(ch) {
            let g = i64::from(self.ramp.tick()[0]);
            for x in f {
                *x = ((i64::from(*x) * g) >> 28) as i32;
            }
        }
    }

    /// Finish the fade at once.
    pub fn reset(&mut self) {
        let t = self.ramp.target;
        self.ramp.jump(t);
    }
}
