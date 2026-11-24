//! Second-order sections: design (RBJ "Audio EQ Cookbook" formulas) and the
//! cascade that runs them.
//!
//! [`BiquadCascade`] is the interface the equaliser and tone stages drive. The
//! software implementation [`SoftCascade`] is direct form I in Q4.27 samples
//! and Q3.28 coefficients with a 64-bit accumulator (one `smull` and four
//! `smlal` per sample on the M4) and per-frame linear coefficient ramps. The
//! hardware FFT/IIR engine (WP DSP-1) is meant to implement the same trait;
//! an engine that cannot ramp per frame may apply each new coefficient set at
//! the start of the next block, which the 64-frame design steps of the
//! equaliser keep small.
//!
//! Direct form I is used because its state is the actual input and output
//! history: a coefficient change produces only a proportional change of the
//! output, never the state-dependent transients of the transposed forms.

use crate::fixed::{COEF_BITS, UNITY, q28, ramp_step};
use crate::math;

/// Normalised biquad coefficients (a0 = 1), Q3.28:
/// `y = b0·x + b1·x[−1] + b2·x[−2] − a1·y[−1] − a2·y[−2]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coeffs {
    pub b0: i32,
    pub b1: i32,
    pub b2: i32,
    pub a1: i32,
    pub a2: i32,
}

impl Coeffs {
    /// The section that passes its input unchanged (bit-exact).
    pub const IDENTITY: Self = Self { b0: UNITY, b1: 0, b2: 0, a1: 0, a2: 0 };

    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// From real-valued normalised coefficients `[b0, b1, b2, a1, a2]`.
    #[must_use]
    pub fn from_f32(c: [f32; 5]) -> Self {
        Self { b0: q28(c[0]), b1: q28(c[1]), b2: q28(c[2]), a1: q28(c[3]), a2: q28(c[4]) }
    }

    /// Magnitude of the quantised section at `freq` Hz (rate `rate` Hz), as a
    /// linear factor. Design-time only (UI curves, headroom estimate).
    #[must_use]
    pub fn magnitude(&self, freq: f32, rate: u32) -> f32 {
        let w = 2.0 * core::f32::consts::PI * freq / rate as f32;
        let (c1, s1) = (math::cos(w), math::sin(w));
        // cos 2w, sin 2w from the double-angle formulas (2w can exceed π).
        let (c2, s2) = (c1 * c1 - s1 * s1, 2.0 * s1 * c1);
        let f = |v: i32| crate::fixed::from_q28(v);
        let (b0, b1, b2, a1, a2) = (f(self.b0), f(self.b1), f(self.b2), f(self.a1), f(self.a2));
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -(b1 * s1 + b2 * s2);
        let dr = 1.0 + a1 * c1 + a2 * c2;
        let di = -(a1 * s1 + a2 * s2);
        let num = nr * nr + ni * ni;
        let den = dr * dr + di * di;
        if den > 0.0 { sqrt(num / den) } else { 1.0 }
    }
}

/// √x for x ≥ 0 (Newton from an exponent-halving guess). Design-time only.
#[must_use]
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    let mut g = f32::from_bits((x.to_bits() >> 1) + 0x1FC0_0000);
    for _ in 0..4 {
        g = 0.5 * (g + x / g);
    }
    g
}

/// Shape of a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    /// Bell around the centre frequency; `q` sets the bandwidth.
    #[default]
    Peaking,
    /// Gain below the corner frequency; `q` = 0.707 is the cookbook's
    /// shelf slope S = 1 (no overshoot).
    LowShelf,
    /// Gain above the corner frequency.
    HighShelf,
}

/// Design one section. `freq` and `rate` in Hz, `q` > 0, `gain_db` any.
/// A gain of exactly 0 dB gives [`Coeffs::IDENTITY`] (bit-exact pass), as
/// does a frequency at or above 0.49·`rate` (the band does not exist at this
/// rate).
#[must_use]
pub fn design(shape: Shape, freq: f32, q: f32, gain_db: f32, rate: u32) -> Coeffs {
    let Some((c, sb, sa)) = design_dc(shape, freq, q, gain_db, rate) else {
        return Coeffs::IDENTITY;
    };
    // b1 and a1 are chosen so that the quantised sums b0+b1+b2 and
    // 1+a1+a2 equal the exactly computed DC sums: at low frequencies these
    // sums are ~10⁻⁴ and would otherwise inherit the f32 rounding of the
    // individual coefficients (≈ 0.03 dB of DC-gain error at 100 Hz).
    let (b0, b2, a2) = (q28(c[0]), q28(c[2]), q28(c[4]));
    let a1 = q28(sa).wrapping_sub(UNITY).wrapping_sub(a2);
    let b1 = q28(sb).wrapping_sub(b0).wrapping_sub(b2);
    Coeffs { b0, b1, b2, a1, a2 }
}

/// The real-valued coefficients `[b0, b1, b2, a1, a2]`, or `None` when the
/// section is the identity.
#[must_use]
pub fn design_f32(shape: Shape, freq: f32, q: f32, gain_db: f32, rate: u32) -> Option<[f32; 5]> {
    design_dc(shape, freq, q, gain_db, rate).map(|d| d.0)
}

/// Coefficients plus the DC sums `b0 + b1 + b2` and `1 + a1 + a2`,
/// computed without cancellation.
fn design_dc(shape: Shape, freq: f32, q: f32, gain_db: f32, rate: u32) -> Option<([f32; 5], f32, f32)> {
    let fs = rate as f32;
    if gain_db == 0.0 || freq.is_nan() || freq <= 0.0 || freq >= 0.49 * fs || q.is_nan() || q <= 0.0 {
        return None;
    }
    let w0 = 2.0 * core::f32::consts::PI * freq / fs;
    // cos w0 = 1 − 2 sin²(w0/2): exact near w0 = 0 where 1 − cos would
    // cancel (low bands at high rates).
    let sh = math::sin(0.5 * w0);
    let omc = 2.0 * sh * sh;
    let cw = 1.0 - omc;
    let sw = math::sin(w0);
    let alpha = sw / (2.0 * q);
    // A = 10^(dB/40); √A = 10^(dB/80).
    let a = math::pow10(gain_db / 40.0);
    let r = match shape {
        Shape::Peaking => {
            let a0 = 1.0 + alpha / a;
            let s = 2.0 * omc / a0;
            let c = [
                (1.0 + alpha * a) / a0,
                -2.0 * cw / a0,
                (1.0 - alpha * a) / a0,
                -2.0 * cw / a0,
                (1.0 - alpha / a) / a0,
            ];
            (c, s, s)
        }
        Shape::LowShelf | Shape::HighShelf => {
            let sa = math::pow10(gain_db / 80.0);
            let k = 2.0 * sa * alpha;
            let ap1 = a + 1.0;
            let am1 = a - 1.0;
            let lo = 2.0 + am1 * omc; // (A+1) − (A−1)cos
            let hi = 2.0 * a - am1 * omc; // (A+1) + (A−1)cos
            if shape == Shape::LowShelf {
                let a0 = hi + k;
                let c = [
                    a * (lo + k) / a0,
                    2.0 * a * (ap1 * omc - 2.0) / a0,
                    a * (lo - k) / a0,
                    -2.0 * (2.0 * a - ap1 * omc) / a0,
                    (hi - k) / a0,
                ];
                let s_a = 4.0 * omc / a0;
                (c, a * a * s_a, s_a)
            } else {
                let a0 = lo + k;
                let c = [
                    a * (hi + k) / a0,
                    -2.0 * a * (2.0 * a - ap1 * omc) / a0,
                    a * (hi - k) / a0,
                    2.0 * (ap1 * omc - 2.0) / a0,
                    (lo - k) / a0,
                ];
                let s_a = 4.0 * a * omc / a0;
                (c, s_a, s_a)
            }
        }
    };
    Some(r)
}

/// What the equaliser and tone stages need from whatever runs the sections.
///
/// Samples are interleaved Q4.27, `channels` per frame (1 or 2), at most
/// [`crate::CHUNK`] frames per call. Sections are applied in index order.
pub trait BiquadCascade {
    /// Sections this engine runs.
    fn capacity(&self) -> usize;
    /// Let section `i` reach `c`, moving linearly over the next `frames`
    /// frames processed (at once when `frames` is 0). A section that is the
    /// identity and not moving costs nothing.
    fn set(&mut self, i: usize, c: Coeffs, frames: u32);
    /// Coefficients section `i` will have once its ramp ends.
    fn target(&self, i: usize) -> Coeffs;
    /// Whether section `i` is still moving.
    fn ramping(&self, i: usize) -> bool;
    /// Run the cascade over `buf` in place.
    fn process(&mut self, buf: &mut [i32], channels: usize);
    /// Forget the signal history (seek, new stream). Coefficients stay.
    fn reset(&mut self);
}

/// One section of [`SoftCascade`]: `k = [b0, b1, b2, −a1, −a2]` so every
/// term is a multiply-accumulate.
#[derive(Debug, Clone, Copy)]
struct Section {
    k: [i32; 5],
    step: [i32; 5],
    target: [i32; 5],
    left: u32,
    /// Per channel `[x1, x2, y1, y2]`.
    st: [[i32; 4]; 2],
    /// The history is stale (the section was skipped as an identity).
    cold: bool,
}

const ID_K: [i32; 5] = [UNITY, 0, 0, 0, 0];

impl Section {
    const IDLE: Self = Self { k: ID_K, step: [0; 5], target: ID_K, left: 0, st: [[0; 4]; 2], cold: true };
}

fn to_k(c: Coeffs) -> [i32; 5] {
    [c.b0, c.b1, c.b2, c.a1.wrapping_neg(), c.a2.wrapping_neg()]
}

/// The software cascade: up to `N` direct-form-I sections.
///
/// RAM: 100 bytes per section (coefficients, ramp, two channels of history).
#[derive(Debug, Clone)]
pub struct SoftCascade<const N: usize> {
    sec: [Section; N],
}

impl<const N: usize> Default for SoftCascade<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> SoftCascade<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self { sec: [Section::IDLE; N] }
    }

    /// Sections that cost cycles in the next call (not an idle identity).
    #[must_use]
    pub fn busy_sections(&self) -> usize {
        self.sec.iter().filter(|s| s.left != 0 || s.k != ID_K).count()
    }
}

/// `a·b` widened, written so LLVM sees two sign-extended 32-bit operands
/// (`smull`/`smlal`).
#[inline(always)]
fn wm(a: i32, b: i32) -> i64 {
    i64::from(a) * i64::from(b)
}

/// One section over `buf` with fixed coefficients.
#[inline(always)]
fn run_fixed<const CH: usize>(k: &[i32; 5], st: &mut [[i32; 4]; 2], buf: &mut [i32]) {
    let (frames, _) = buf.as_chunks_mut::<CH>();
    let [b0, b1, b2, a1, a2] = *k;
    for (c, s) in st.iter_mut().enumerate().take(CH) {
        let [mut x1, mut x2, mut y1, mut y2] = *s;
        for f in frames.iter_mut() {
            let x = f[c];
            let acc =
                (1i64 << (COEF_BITS - 1)) + wm(b0, x) + wm(b1, x1) + wm(b2, x2) + wm(a1, y1) + wm(a2, y2);
            let y = (acc >> COEF_BITS) as i32;
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
            f[c] = y;
        }
        *s = [x1, x2, y1, y2];
    }
}

/// One section over `buf` with the coefficients moving by `step` per frame.
fn run_ramp<const CH: usize>(k0: &[i32; 5], step: &[i32; 5], st: &mut [[i32; 4]; 2], buf: &mut [i32]) {
    let (frames, _) = buf.as_chunks_mut::<CH>();
    for (c, s) in st.iter_mut().enumerate().take(CH) {
        let mut k = *k0;
        let [mut x1, mut x2, mut y1, mut y2] = *s;
        for f in frames.iter_mut() {
            for j in 0..5 {
                k[j] = k[j].wrapping_add(step[j]);
            }
            let x = f[c];
            let acc = (1i64 << (COEF_BITS - 1))
                + wm(k[0], x)
                + wm(k[1], x1)
                + wm(k[2], x2)
                + wm(k[3], y1)
                + wm(k[4], y2);
            let y = (acc >> COEF_BITS) as i32;
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
            f[c] = y;
        }
        *s = [x1, x2, y1, y2];
    }
}

impl<const N: usize> SoftCascade<N> {
    fn process_ch<const CH: usize>(&mut self, buf: &mut [i32]) {
        let frames = (buf.len() / CH) as u32;
        let used = &mut buf[..frames as usize * CH];
        for s in &mut self.sec {
            if s.left == 0 && s.k == ID_K {
                s.cold = true;
                continue;
            }
            if s.cold {
                // Waking from a skipped identity: pretend the signal was
                // constant at its first value (the coefficients start at the
                // identity, so the error is one ramp step times a sample
                // difference).
                for c in 0..CH {
                    let x = used.get(c).copied().unwrap_or(0);
                    s.st[c] = [x; 4];
                }
                s.cold = false;
            }
            if s.left == 0 {
                run_fixed::<CH>(&s.k, &mut s.st, used);
                continue;
            }
            let r = s.left.min(frames);
            let (head, tail) = used.split_at_mut(r as usize * CH);
            run_ramp::<CH>(&s.k, &s.step, &mut s.st, head);
            if r == s.left {
                s.k = s.target;
                s.step = [0; 5];
                s.left = 0;
            } else {
                for j in 0..5 {
                    s.k[j] = s.k[j].wrapping_add(s.step[j].wrapping_mul(r as i32));
                }
                s.left -= r;
            }
            if !tail.is_empty() {
                run_fixed::<CH>(&s.k, &mut s.st, tail);
            }
        }
    }
}

impl<const N: usize> BiquadCascade for SoftCascade<N> {
    fn capacity(&self) -> usize {
        N
    }

    fn set(&mut self, i: usize, c: Coeffs, frames: u32) {
        let Some(s) = self.sec.get_mut(i) else { return };
        let t = to_k(c);
        s.target = t;
        if frames == 0 || s.k == t {
            s.k = t;
            s.step = [0; 5];
            s.left = 0;
            return;
        }
        for ((st, &k), &t) in s.step.iter_mut().zip(&s.k).zip(&t) {
            *st = ramp_step(k, t, frames);
        }
        s.left = frames;
    }

    fn target(&self, i: usize) -> Coeffs {
        self.sec.get(i).map_or(Coeffs::IDENTITY, |s| {
            let t = s.target;
            Coeffs { b0: t[0], b1: t[1], b2: t[2], a1: t[3].wrapping_neg(), a2: t[4].wrapping_neg() }
        })
    }

    fn ramping(&self, i: usize) -> bool {
        self.sec.get(i).is_some_and(|s| s.left != 0)
    }

    fn process(&mut self, buf: &mut [i32], channels: usize) {
        match channels {
            1 => self.process_ch::<1>(buf),
            2 => self.process_ch::<2>(buf),
            _ => {}
        }
    }

    fn reset(&mut self) {
        for s in &mut self.sec {
            s.st = [[0; 4]; 2];
            s.cold = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed::ONE;

    #[test]
    fn identity_is_bit_exact() {
        let mut c = SoftCascade::<3>::new();
        let mut buf: [i32; 64] = core::array::from_fn(|i| (i as i32 * 7919 - 200_000) << 8);
        let orig = buf;
        c.process(&mut buf, 2);
        assert_eq!(buf, orig);
        // A section that has ramped back to the identity is bit-exact too.
        let d = design(Shape::Peaking, 1000.0, 1.0, 6.0, 44100);
        c.set(1, d, 32);
        c.process(&mut buf, 2);
        c.set(1, Coeffs::IDENTITY, 32);
        c.process(&mut buf, 2);
        let mut b2 = orig;
        c.process(&mut b2, 2);
        assert_eq!(b2, orig);
    }

    #[test]
    fn zero_db_designs_identity() {
        assert!(design(Shape::LowShelf, 100.0, 0.7, 0.0, 48000).is_identity());
        assert!(design(Shape::Peaking, 30000.0, 0.7, 6.0, 48000).is_identity());
    }

    #[test]
    fn dc_gain_of_low_shelf() {
        let c = design(Shape::LowShelf, 100.0, 0.707, 6.0, 44100);
        let mut cas = SoftCascade::<1>::new();
        cas.set(0, c, 0);
        let mut buf = [ONE / 4; 64];
        for _ in 0..400 {
            buf = [ONE / 4; 64];
            cas.process(&mut buf, 1);
        }
        let g = buf[63] as f32 / (ONE / 4) as f32;
        assert!((g - math::db_to_lin(6.0)).abs() < 1e-3, "{g}");
    }
}
