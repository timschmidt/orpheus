//! Step detector (03 §15 "Step detector", an improvement on the stock
//! algorithm of fm-i2c-imu.md §3.4, which stays the reference).
//!
//! Per sample, integer arithmetic only (it runs on the player once a
//! second over a batch of samples):
//!
//! 1. **Magnitude** `|a| = √(x² + y² + z²)` in counts: independent of how
//!    the player is held or carried.
//! 2. **Band-pass**: gravity (a slow average of the magnitude, ≈ 1.3 s) is
//!    subtracted (high-pass), then a short average removes vibration above
//!    ≈ 4 Hz (low-pass).
//! 3. **Adaptive threshold**: envelopes follow the signal's running maximum
//!    and minimum (decaying over ≈ 1.3 s). A step candidate is a rise
//!    through the upper hysteresis level (mid + ¼ of the swing) after the
//!    signal was below the lower one (mid − ¼), while the swing is at least
//!    the sensitivity's minimum (0.06–0.16 g peak to peak: the table's
//!    noise is a tenth of that).
//! 4. **Cadence window**: candidates closer than 270 ms are not steps
//!    (above 220 steps a minute), nor are those whose interval holds two
//!    rises of the unfiltered signal (shaking at 4–7 Hz, which the
//!    low-pass shows as one rise every other cycle, a walking rhythm);
//!    a second such candidate without a clean step between them (shaking,
//!    tapping) drops everything pending. More than 2 s without a
//!    candidate ends a walk. Shaking slower than ≈ 3.7 Hz is a running
//!    cadence and is counted.
//! 5. **Regularity gate**: counting starts after four candidates in a row
//!    whose intervals agree within 40 %; the four are then credited at
//!    once. Single bumps, a bus over a pothole, putting the player down
//!    never count.
//!
//! Output: the number of new steps (0, 1, or the four credited) per
//! sample.

use crate::COUNTS_PER_G;

/// Minimum swing for a candidate (05 §7.7 Sensitivity).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sensitivity {
    /// 0.16 g peak to peak: firm walking only.
    Low,
    /// 0.10 g.
    #[default]
    Medium,
    /// 0.06 g: slow or soft walking (more false steps from handling).
    High,
}

impl Sensitivity {
    /// From the setting's value (0 Low, 1 Medium, 2 High).
    #[must_use]
    pub const fn from_index(i: i16) -> Sensitivity {
        match i {
            0 => Sensitivity::Low,
            2 => Sensitivity::High,
            _ => Sensitivity::Medium,
        }
    }

    /// Minimum peak-to-peak swing in counts.
    #[must_use]
    pub const fn min_swing(self) -> i32 {
        match self {
            Sensitivity::Low => COUNTS_PER_G * 16 / 100,
            Sensitivity::Medium => COUNTS_PER_G / 10,
            Sensitivity::High => COUNTS_PER_G * 6 / 100,
        }
    }
}

/// Candidates in a row before counting starts.
pub const REGULAR: u8 = 4;
/// Shortest step interval (ms).
pub const MIN_STEP_MS: u32 = 270;
/// Longest step interval (ms).
pub const MAX_STEP_MS: u32 = 2000;

/// Fixed-point shift of the filter state.
const FP: u32 = 8;

/// The detector's whole state (≈ 64 bytes; no buffers).
#[derive(Clone, Copy, Debug)]
pub struct Detector {
    /// Sample rate (Hz) and the constants derived from it.
    rate: u16,
    k_grav: u32,
    k_lp: u32,
    k_env: u32,
    min_s: u32,
    max_s: u32,
    min_swing: i32,
    started: bool,
    /// Gravity and low-pass filter state, envelopes (counts << FP).
    grav: i32,
    lp: i32,
    hi: i32,
    lo: i32,
    /// Above the upper level since the last fall below the lower one.
    high: bool,
    /// Sample counter and the last accepted candidate.
    n: u32,
    last: Option<u32>,
    prev_dt: u32,
    /// Candidates pending before the walk is confirmed.
    pending: u8,
    walking: bool,
    /// Too-fast candidates not yet forgiven.
    fast: u8,
    /// A too-fast candidate came since the last accepted one.
    fast_since: bool,
    /// The unfiltered signal above the upper level, and its rises since
    /// the last candidate.
    raw_high: bool,
    raw_ups: u8,
    /// Steps counted since creation (for tests and `steps status`).
    pub total: u32,
}

/// `⌊√v⌋`.
#[must_use]
pub const fn isqrt(v: u32) -> u32 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = x.div_ceil(2);
    while y < x && y > 0 {
        x = y;
        // `x` > 0 here; written without a division panic site.
        y = (x + match v.checked_div(x) {
            Some(q) => q,
            None => 0,
        }) / 2;
    }
    x
}

/// `2^k ≈ v` (k ≥ 1).
const fn log2_round(v: u32) -> u32 {
    let mut k = 1;
    while (1u32 << (k + 1)) <= v + (v >> 1) {
        k += 1;
    }
    k
}

impl Detector {
    /// A detector for samples arriving at `rate` Hz (10–100; the player
    /// counts at 25 Hz).
    #[must_use]
    pub const fn new(rate: u16, s: Sensitivity) -> Detector {
        let r = if rate < 10 {
            10
        } else if rate > 100 {
            100
        } else {
            rate
        } as u32;
        Detector {
            rate: r as u16,
            // ≈ 1.3 s averages: 2^k / rate.
            k_grav: log2_round(r * 13 / 10),
            // Low-pass: ½ at 25 Hz, ¼ at 50 Hz (≈ 4 Hz).
            k_lp: if r <= 32 { 1 } else { 2 },
            k_env: log2_round(r * 13 / 10),
            min_s: (MIN_STEP_MS * r).div_ceil(1000),
            max_s: MAX_STEP_MS * r / 1000,
            min_swing: s.min_swing() << FP,
            started: false,
            grav: 0,
            lp: 0,
            hi: 0,
            lo: 0,
            high: false,
            n: 0,
            last: None,
            prev_dt: 0,
            pending: 0,
            walking: false,
            fast: 0,
            fast_since: false,
            raw_high: false,
            raw_ups: 0,
            total: 0,
        }
    }

    #[must_use]
    pub const fn rate(&self) -> u16 {
        self.rate
    }

    /// Change the sensitivity (keeps the filter state).
    pub fn set_sensitivity(&mut self, s: Sensitivity) {
        self.min_swing = s.min_swing() << FP;
    }

    /// A walk is being counted (four regular candidates seen).
    #[must_use]
    pub const fn walking(&self) -> bool {
        self.walking
    }

    /// Forget the walk in progress (after a gap in the samples).
    pub fn restart(&mut self) {
        let (rate, swing, total) = (self.rate, self.min_swing, self.total);
        *self = Detector::new(rate, Sensitivity::Medium);
        self.min_swing = swing;
        self.total = total;
    }

    /// One sample (counts); returns the steps it completes.
    pub fn feed(&mut self, x: i16, y: i16, z: i16) -> u32 {
        let sq = |v: i16| {
            let v = i32::from(v).unsigned_abs();
            v * v
        };
        let m = isqrt(sq(x).saturating_add(sq(y)).saturating_add(sq(z))).min(1 << 20) as i32;
        self.feed_magnitude(m)
    }

    /// One sample given as its magnitude (counts).
    pub fn feed_magnitude(&mut self, m: i32) -> u32 {
        let m = m << FP;
        if !self.started {
            self.started = true;
            self.grav = m;
            self.lp = 0;
            self.hi = 0;
            self.lo = 0;
            return 0;
        }
        self.n = self.n.wrapping_add(1);
        self.grav += (m - self.grav) >> self.k_grav;
        let a = m - self.grav;
        self.lp += (a - self.lp) >> self.k_lp;
        let f = self.lp;
        // Envelopes: jump to a new extreme, otherwise decay towards it.
        if f > self.hi {
            self.hi = f;
        } else {
            self.hi -= (self.hi - f) >> self.k_env;
        }
        if f < self.lo {
            self.lo = f;
        } else {
            self.lo += (f - self.lo) >> self.k_env;
        }
        // A walk ends after the longest step interval without a step.
        if let Some(last) = self.last
            && self.n.wrapping_sub(last) > self.max_s
        {
            self.last = None;
            self.walking = false;
            self.pending = 0;
            self.fast = 0;
            self.fast_since = false;
            self.prev_dt = 0;
        }
        let swing = self.hi - self.lo;
        let mid = self.lo + swing / 2;
        let band = swing / 4;
        // The same levels on the unfiltered signal: shaking that the
        // low-pass turns into one rise every other cycle still rises
        // every cycle here.
        if self.raw_high {
            if a < mid - band {
                self.raw_high = false;
            }
        } else if a > mid + band {
            self.raw_high = true;
            self.raw_ups = self.raw_ups.saturating_add(1);
        }
        if self.high {
            if f < mid - band {
                self.high = false;
            }
            return 0;
        }
        if f <= mid + band {
            return 0;
        }
        self.high = true;
        if swing < self.min_swing {
            return 0;
        }
        self.candidate()
    }

    /// A rise through the upper level with enough swing.
    fn candidate(&mut self) -> u32 {
        let n = self.n;
        let ups = core::mem::take(&mut self.raw_ups);
        let Some(last) = self.last else {
            self.start_at(n);
            return 0;
        };
        let dt = n.wrapping_sub(last);
        if dt < self.min_s || ups >= 2 {
            // Faster than any walk: a bounce within a step now and then;
            // a second one before a step without one between is shaking
            // or tapping (counted every other cycle, it would look like a
            // walk).
            self.fast += 1;
            self.fast_since = true;
            if self.fast >= 2 {
                self.last = None;
                self.walking = false;
                self.pending = 0;
                self.fast = 0;
                self.fast_since = false;
                self.prev_dt = 0;
            }
            return 0;
        }
        // A clean step interval forgives one earlier bounce.
        if !core::mem::take(&mut self.fast_since) {
            self.fast = self.fast.saturating_sub(1);
        }
        if self.walking {
            self.last = Some(n);
            self.prev_dt = dt;
            self.total += 1;
            return 1;
        }
        // Not yet walking: the intervals must agree within 40 %.
        if self.prev_dt != 0 && dt.abs_diff(self.prev_dt) * 10 > self.prev_dt * 4 {
            // Irregular: the walk (if it is one) starts at the last
            // candidate.
            self.pending = 2;
        } else {
            self.pending += 1;
        }
        self.prev_dt = dt;
        self.last = Some(n);
        if self.pending >= REGULAR {
            self.walking = true;
            let credited = u32::from(self.pending);
            self.pending = 0;
            self.total += credited;
            return credited;
        }
        0
    }

    fn start_at(&mut self, n: u32) {
        self.last = Some(n);
        self.pending = 1;
        self.walking = false;
        self.fast = 0;
        self.fast_since = false;
        self.prev_dt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_roots() {
        for v in [0u32, 1, 2, 3, 4, 15, 16, 17, 1_048_576, 12_582_912, u32::MAX] {
            let r = isqrt(v);
            assert!(u64::from(r) * u64::from(r) <= u64::from(v), "{v}");
            assert!(u64::from(r + 1) * u64::from(r + 1) > u64::from(v), "{v}");
        }
    }

    #[test]
    fn constants_per_rate() {
        let d = Detector::new(25, Sensitivity::Medium);
        assert_eq!((d.k_grav, d.k_lp, d.k_env, d.min_s, d.max_s), (5, 1, 5, 7, 50));
        let d = Detector::new(50, Sensitivity::Medium);
        assert_eq!((d.k_grav, d.k_lp, d.k_env, d.min_s, d.max_s), (6, 2, 6, 14, 100));
        assert_eq!(Detector::new(1, Sensitivity::Low).rate(), 10);
        assert_eq!(Sensitivity::from_index(2), Sensitivity::High);
        assert!(Sensitivity::High.min_swing() < Sensitivity::Low.min_swing());
    }

    #[test]
    fn constant_magnitude_never_counts() {
        let mut d = Detector::new(25, Sensitivity::High);
        for i in 0..100_000 {
            // ± 1 count of noise.
            let n = [0i16, 1, -1, 0, 1][i % 5];
            assert_eq!(d.feed(n, 1024 + n, 3), 0);
        }
        assert_eq!(d.total, 0);
    }

    #[test]
    fn regular_square_wave_counts_after_four() {
        // 2 steps/s at 25 Hz: 12.5 samples per step; ± 0.2 g.
        let mut d = Detector::new(25, Sensitivity::Medium);
        let mut got = std::vec::Vec::new();
        for i in 0..250u32 {
            let phase = (i * 2 * 1000 / 25) % 1000;
            let m = if phase < 500 { 1024 + 205 } else { 1024 - 205 };
            let s = d.feed_magnitude(m);
            if s > 0 {
                got.push((i, s));
            }
        }
        assert_eq!(got.first().map(|g| g.1), Some(4), "four credited at once");
        let total: u32 = got.iter().map(|g| g.1).sum();
        // 10 s at 2 steps/s; the first rise only primes the envelopes.
        assert!((18..=20).contains(&total), "{total}");
    }
}
