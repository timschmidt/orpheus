//! Number formats of the sample path.
//!
//! * **Samples** inside the chain are `i32` in Q4.27: full scale ±1.0 is
//!   ±2^27, which leaves 4 bits (24 dB) of headroom for gains and filter
//!   overshoot before anything can wrap. A 16-bit sample is `s << 12`; a
//!   left-justified 24-bit sample (`s24 << 8`) is `x >> 4`, which keeps all
//!   24 bits. The 4 bits dropped from a 32-bit source are below −168 dBFS.
//! * **Coefficients and gains** are `i32` in Q3.28 (±8.0, resolution
//!   3.7·10⁻⁹). A product `sample × coefficient` is formed in 64 bits and
//!   shifted right by 28, which LLVM lowers to `smull`/`smlal` plus one
//!   shift-or on the Cortex-M4.

/// Fraction bits of a working sample.
pub const SAMPLE_BITS: u32 = 27;
/// Full scale (1.0) of a working sample.
pub const ONE: i32 = 1 << SAMPLE_BITS;
/// Fraction bits of coefficients and gains.
pub const COEF_BITS: u32 = 28;
/// 1.0 as a coefficient or gain.
pub const UNITY: i32 = 1 << COEF_BITS;

/// `x · g` with `g` in Q3.28, truncated. No saturation: callers keep the
/// operands inside the headroom (see `Chain` headroom guard).
#[inline(always)]
#[must_use]
pub fn mul(x: i32, g: i32) -> i32 {
    ((i64::from(x) * i64::from(g)) >> COEF_BITS) as i32
}

/// `x · g` with `g` in Q3.28, saturated to the `i32` range.
#[inline(always)]
#[must_use]
pub fn mul_sat(x: i32, g: i32) -> i32 {
    sat64((i64::from(x) * i64::from(g)) >> COEF_BITS)
}

/// Saturate an `i64` to `i32`.
#[inline(always)]
#[must_use]
pub fn sat64(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// A real number to Q3.28, rounded and saturated. Design-time only.
#[must_use]
pub fn q28(v: f32) -> i32 {
    let s = v * UNITY as f32;
    // `as` saturates (and maps NaN to 0).
    if s >= 0.0 { (s + 0.5) as i32 } else { (s - 0.5) as i32 }
}

/// Q3.28 to a real number. Design-time only.
#[must_use]
pub fn from_q28(v: i32) -> f32 {
    v as f32 / UNITY as f32
}

/// `(target − cur) / n` for a per-frame ramp step, without overflowing for
/// any pair of `i32`s.
#[inline]
#[must_use]
pub fn ramp_step(cur: i32, target: i32, n: u32) -> i32 {
    let d = i64::from(target) - i64::from(cur);
    if n <= 1 {
        return sat64(d);
    }
    if let Ok(d32) = i32::try_from(d) {
        // 32-bit division: `sdiv` on the M4 instead of a libcall.
        d32 / n as i32
    } else {
        sat64(d / i64::from(n))
    }
}

/// Linear interpolation of `K` values from where they are to a target, one
/// step per frame. Used by every stage whose parameters change at run time.
#[derive(Debug, Clone, Copy)]
pub struct Ramp<const K: usize> {
    pub cur: [i32; K],
    pub step: [i32; K],
    pub target: [i32; K],
    /// Frames still to go; 0 when settled.
    pub left: u32,
}

impl<const K: usize> Ramp<K> {
    #[must_use]
    pub const fn new(v: [i32; K]) -> Self {
        Self { cur: v, step: [0; K], target: v, left: 0 }
    }

    /// Move to `target` over `frames` frames (immediately if 0).
    pub fn to(&mut self, target: [i32; K], frames: u32) {
        self.target = target;
        if frames == 0 || target == self.cur {
            self.jump(target);
            return;
        }
        for ((s, &c), &t) in self.step.iter_mut().zip(&self.cur).zip(&target) {
            *s = ramp_step(c, t, frames);
        }
        self.left = frames;
    }

    /// Set the value now.
    pub fn jump(&mut self, v: [i32; K]) {
        self.cur = v;
        self.target = v;
        self.step = [0; K];
        self.left = 0;
    }

    #[must_use]
    pub fn ramping(&self) -> bool {
        self.left != 0
    }

    /// Advance by `n` frames (the caller applied the per-frame steps itself
    /// or calls [`Ramp::tick`] per frame). Snaps to the target at the end.
    pub fn advance(&mut self, n: u32) {
        if self.left == 0 {
            return;
        }
        if n >= self.left {
            self.cur = self.target;
            self.step = [0; K];
            self.left = 0;
        } else {
            for i in 0..K {
                self.cur[i] = self.cur[i].wrapping_add(self.step[i].wrapping_mul(n as i32));
            }
            self.left -= n;
        }
    }

    /// Advance one frame and return the new values.
    #[inline(always)]
    pub fn tick(&mut self) -> [i32; K] {
        if self.left != 0 {
            self.left -= 1;
            if self.left == 0 {
                self.cur = self.target;
                self.step = [0; K];
            } else {
                for i in 0..K {
                    self.cur[i] = self.cur[i].wrapping_add(self.step[i]);
                }
            }
        }
        self.cur
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_is_exact_for_unity_and_half() {
        for x in [-ONE, -12345, 0, 1, 777, ONE - 1, i32::MAX, i32::MIN] {
            assert_eq!(mul(x, UNITY), x);
        }
        assert_eq!(mul(ONE, UNITY / 2), ONE / 2);
        assert_eq!(mul_sat(i32::MAX, 4 * UNITY), i32::MAX);
        assert_eq!(mul_sat(i32::MIN, 4 * UNITY), i32::MIN);
    }

    #[test]
    fn ramp_reaches_target_exactly() {
        let mut r = Ramp::<2>::new([0, UNITY]);
        r.to([UNITY, 0], 100);
        let mut last = r.cur;
        for _ in 0..100 {
            let v = r.tick();
            assert!(v[0] >= last[0] && v[1] <= last[1]);
            last = v;
        }
        assert_eq!(r.cur, [UNITY, 0]);
        assert!(!r.ramping());
        r.to([i32::MAX, i32::MIN], 3);
        r.advance(2);
        r.advance(5);
        assert_eq!(r.cur, [i32::MAX, i32::MIN]);
    }

    #[test]
    fn q28_round_trip() {
        assert_eq!(q28(1.0), UNITY);
        assert_eq!(q28(-0.5), -UNITY / 2);
        assert_eq!(q28(100.0), i32::MAX);
        assert!((from_q28(q28(0.123_456)) - 0.123_456).abs() < 1e-7);
    }
}
