//! Look-ahead-free soft limiter.
//!
//! A peak envelope with instant attack and exponential release
//! (`env = max(|x|, env·r)`, stereo-linked) drives a soft-knee gain
//! computer:
//!
//! ```text
//! f(e) = e                                   e ≤ T
//! f(e) = T + D·(e − T) / (D + (e − T))       e > T,   D = C − T
//! g    = f(env) / env
//! ```
//!
//! `f` has slope 1 at the knee T and approaches the ceiling C, so
//! `|y| = |x|·g ≤ env·g = f(env) < C` for every sample: no overshoot without
//! look-ahead. The price is that the attack is instantaneous (some
//! distortion on hard transients); the release is smooth. Below the knee the
//! stage costs one compare per frame.
//!
//! The gain computer runs in `f32` (two divisions, ≈ 40 cycles on the M4
//! FPU) only for frames whose envelope is above the knee.

use crate::fixed::{ONE, q28};
use crate::math;

/// Knee, dBFS.
pub const KNEE_DB: f32 = -2.0;
/// Ceiling, dBFS.
pub const CEILING_DB: f32 = -0.1;
/// Release time constant.
pub const RELEASE_MS: f32 = 50.0;

#[derive(Debug, Clone)]
pub struct Limiter {
    /// Envelope, Q4.27.
    env: i32,
    /// Per-frame release factor, Q3.28.
    rel: i32,
    thr: i32,
    knee: f32,
    ceil: f32,
    /// Frames in which the gain was below 1 since the last `take_engaged`.
    engaged: u32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    #[must_use]
    pub fn new() -> Self {
        let knee = math::db_to_lin(KNEE_DB);
        let mut l = Self {
            env: 0,
            rel: 0,
            thr: (knee * ONE as f32) as i32,
            knee,
            ceil: math::db_to_lin(CEILING_DB),
            engaged: 0,
        };
        l.set_rate(44100);
        l
    }

    pub fn set_rate(&mut self, rate: u32) {
        // r = e^(−1/(τ·fs)) = 2^(−1/(τ·fs·ln 2)).
        let n = RELEASE_MS * 0.001 * rate.max(1) as f32;
        self.rel = q28(math::exp2(-1.0 / (n * core::f32::consts::LN_2)));
    }

    /// The envelope has decayed below the knee: the stage passes the signal
    /// unchanged and can be dropped without a step.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.env <= self.thr
    }

    /// Frames limited since the last call.
    pub fn take_engaged(&mut self) -> u32 {
        core::mem::take(&mut self.engaged)
    }

    fn gain(&self, env: i32) -> i32 {
        let e = env as f32 / ONE as f32;
        let r = e - self.knee;
        let d = self.ceil - self.knee;
        let f = self.knee + d * r / (d + r);
        q28(f / e)
    }

    pub fn process(&mut self, buf: &mut [i32], channels: usize) {
        let ch = channels.max(1);
        let mut env = self.env;
        let rel = i64::from(self.rel);
        for f in buf.chunks_exact_mut(ch) {
            let mut p = 0u32;
            for x in f.iter() {
                p = p.max(x.unsigned_abs());
            }
            let p = p.min(i32::MAX as u32) as i32;
            let decayed = ((i64::from(env) * rel) >> 28) as i32;
            env = decayed.max(p);
            if env > self.thr {
                let g = i64::from(self.gain(env));
                for x in f.iter_mut() {
                    *x = ((i64::from(*x) * g) >> 28) as i32;
                }
                self.engaged = self.engaged.saturating_add(1);
            }
        }
        self.env = env;
    }

    pub fn reset(&mut self) {
        self.env = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_exceeds_ceiling() {
        let mut l = Limiter::new();
        let ceil = (math::db_to_lin(CEILING_DB) * ONE as f32) as i32 + 2;
        let mut buf = [0i32; 128];
        for k in 0..200 {
            for (i, x) in buf.iter_mut().enumerate() {
                // Up to 15× full scale, alternating signs.
                let a = i64::from(ONE) * ((k * 128 + i) % 15 + 1) as i64;
                let v = if i % 3 == 0 { -a } else { a };
                *x = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            }
            l.process(&mut buf, 2);
            for x in buf {
                assert!(x.abs() <= ceil, "{x} > {ceil}");
            }
        }
        // Quiet signal after release passes unchanged.
        for _ in 0..200 {
            buf = [ONE / 10; 128];
            l.process(&mut buf, 2);
        }
        assert!(l.is_idle());
        assert_eq!(buf, [ONE / 10; 128]);
    }
}
