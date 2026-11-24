//! Headphone crossfeed in the manner of Bauer's (1961) circuit: each ear
//! also hears the other channel, low-passed (which also delays it by roughly
//! the interaural time) and lower in level, while the direct path loses as
//! much bass as the cross path adds.
//!
//! With a first-order low-pass `LP` at the cut frequency, a cross level
//! `c = k / (1 + k)` (`k` the feed level as a linear factor, the cross path
//! `feed` dB below the direct path at low frequencies), the direct path
//! `1 − c·LP` and the cross path `c·LP`:
//!
//! ```text
//! L' = L + c·LP(R − L)        R' = R − c·LP(R − L)
//! ```
//!
//! One low-pass on the side signal per frame. A mono signal (L = R) passes
//! unchanged at every frequency; at low frequencies L' = (1 − c)L + cR with
//! (cR)/((1 − c)L) = k. The magnitude of each path is ≤ 1; the sum of both
//! magnitudes peaks at about +0.6 dB (for feed 4.5 dB) in the transition
//! band, so the stage reports that it may boost.

use crate::fixed::{Ramp, UNITY, mul, q28};
use crate::math;

/// Ramp length of an on/off or parameter change.
pub const XFEED_RAMP_MS: u32 = 40;

/// Cut frequency and feed level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossfeedParams {
    pub cutoff_hz: u16,
    /// How far the crossfed signal is below the direct one at low
    /// frequencies, mB (positive).
    pub feed_mb: u16,
}

impl CrossfeedParams {
    /// A common default (700 Hz, 4.5 dB).
    pub const DEFAULT: Self = Self { cutoff_hz: 700, feed_mb: 450 };
    /// Chu Moy's values (700 Hz, 6 dB).
    pub const CHU_MOY: Self = Self { cutoff_hz: 700, feed_mb: 600 };
    /// Jan Meier's values (650 Hz, 9.5 dB).
    pub const MEIER: Self = Self { cutoff_hz: 650, feed_mb: 950 };
}

/// `[c, b, −a1]` of the stage: cross level and the low-pass
/// `y = b·(s + s1) − a1·y1` (bilinear transform of ω/(s + ω), prewarped).
fn coeffs(p: CrossfeedParams, rate: u32) -> [i32; 3] {
    let k = math::db_to_lin(-f32::from(p.feed_mb) / 100.0);
    let c = k / (1.0 + k);
    let w = core::f32::consts::PI * f32::from(p.cutoff_hz.max(10)) / rate as f32;
    let w = w.min(1.5);
    let t = math::sin(w) / math::cos(w);
    let b = t / (1.0 + t);
    let na1 = (1.0 - t) / (1.0 + t);
    [q28(c), q28(b), q28(na1)]
}

#[derive(Debug, Clone)]
pub struct Crossfeed {
    params: Option<CrossfeedParams>,
    rate: u32,
    ramp: Ramp<3>,
    /// Low-pass history: previous side input and output.
    s1: i32,
    y1: i32,
}

impl Default for Crossfeed {
    fn default() -> Self {
        Self::new()
    }
}

impl Crossfeed {
    #[must_use]
    pub const fn new() -> Self {
        Self { params: None, rate: 44100, ramp: Ramp::new([0, 0, 0]), s1: 0, y1: 0 }
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
        let t = match self.params {
            Some(p) => coeffs(p, self.rate),
            None => [0; 3],
        };
        self.ramp.jump(t);
        self.s1 = 0;
        self.y1 = 0;
    }

    /// Turn on (with these parameters) or off; ramped.
    pub fn set(&mut self, params: Option<CrossfeedParams>) {
        if params == self.params {
            return;
        }
        self.params = params;
        let frames = self.rate * XFEED_RAMP_MS / 1000;
        match params {
            Some(p) => {
                let t = coeffs(p, self.rate);
                if self.ramp.cur[0] == 0 && !self.ramp.ramping() {
                    // From off: filter at its target, only the level ramps.
                    self.ramp.jump([0, t[1], t[2]]);
                    self.s1 = 0;
                    self.y1 = 0;
                }
                self.ramp.to(t, frames);
            }
            None => {
                let cur = self.ramp.cur;
                self.ramp.to([0, cur[1], cur[2]], frames);
            }
        }
    }

    #[must_use]
    pub fn params(&self) -> Option<CrossfeedParams> {
        self.params
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.ramp.ramping() || self.ramp.cur[0] != 0
    }

    /// Stereo only (`buf` interleaved L, R).
    pub fn process(&mut self, buf: &mut [i32]) {
        let (mut s1, mut y1) = (self.s1, self.y1);
        let (frames, _) = buf.as_chunks_mut::<2>();
        if self.ramp.ramping() {
            for f in frames.iter_mut() {
                let [c, b, na1] = self.ramp.tick();
                let s = f[1].wrapping_sub(f[0]);
                let y = mul(s.wrapping_add(s1), b).wrapping_add(mul(y1, na1));
                s1 = s;
                y1 = y;
                let d = mul(y, c);
                f[0] = f[0].wrapping_add(d);
                f[1] = f[1].wrapping_sub(d);
            }
        } else {
            let [c, b, na1] = self.ramp.cur;
            let (c, b, na1) = (i64::from(c), i64::from(b), i64::from(na1));
            for f in frames.iter_mut() {
                let s = f[1].wrapping_sub(f[0]);
                let y = ((i64::from(s.wrapping_add(s1)) * b + i64::from(y1) * na1) >> 28) as i32;
                s1 = s;
                y1 = y;
                let d = ((i64::from(y) * c) >> 28) as i32;
                f[0] = f[0].wrapping_add(d);
                f[1] = f[1].wrapping_sub(d);
            }
        }
        self.s1 = s1;
        self.y1 = y1;
        if !self.is_active() {
            self.s1 = 0;
            self.y1 = 0;
        }
    }

    pub fn reset(&mut self) {
        let t = self.ramp.target;
        self.ramp.jump(t);
        self.s1 = 0;
        self.y1 = 0;
    }
}

/// The cross level `c` (for tests and the UI), as a linear factor.
#[must_use]
pub fn cross_level(p: CrossfeedParams) -> f32 {
    let k = math::db_to_lin(-f32::from(p.feed_mb) / 100.0);
    k / (1.0 + k)
}

const _: () = assert!(UNITY == 1 << 28);
