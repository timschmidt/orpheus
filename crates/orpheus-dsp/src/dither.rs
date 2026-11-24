//! Word-length reduction to 16 bits: rounding, TPDF dither, noise shaping.
//!
//! * TPDF: the sum of two independent uniform values of ±½ LSB each,
//!   from one 32-bit xorshift output split into two 16-bit halves:
//!   `d = (a + b − 65535) >> 4` in Q4.27 units, i.e. −1 … +1 LSB (16-bit LSB
//!   = 2^12 in Q4.27). Deterministic for a given seed.
//! * Noise shaping by error feedback: `v = x − Σ cᵢ·e[n−i]`,
//!   `q = round(v + d)`, `e = q − v`. The output noise is the dither and
//!   rounding error filtered by `1 − Σ cᵢ z^−i`. Coefficients
//!   `c = [1.623, −0.982, 0.109]` (Wannamaker's three-tap F-weighted
//!   filter, published for 44.1 kHz) in Q12; the chain uses it at 44.1 and
//!   48 kHz and plain TPDF below. The error is clamped to ±4 LSB so a
//!   clipped sample cannot make the loop run away.

/// What the 16-bit output conversion does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DitherMode {
    /// Round to nearest.
    Off,
    /// TPDF dither, flat spectrum (the default: 04 §5.1 asks for TPDF, and
    /// it costs ≈ 20 cycles per sample against ≈ 48 for shaping).
    #[default]
    Tpdf,
    /// TPDF dither with noise shaping (at 44.1 kHz and above; flat below):
    /// about 13 dB less noise below 4 kHz, more above 10 kHz.
    Shaped,
}

/// Shaping coefficients, Q12.
pub const SHAPE_Q12: [i32; 3] = [6648, -4022, 446];

/// Default seed of the dither generator.
pub const SEED: u32 = 0x1234_5678;

const LSB: i32 = 1 << 12;
const ERR_MAX: i32 = 4 * LSB;

#[derive(Debug, Clone)]
pub struct Ditherer {
    mode: DitherMode,
    shaping: bool,
    rng: u32,
    /// Per channel e[n−1], e[n−2], e[n−3] in Q4.27.
    err: [[i32; 3]; 2],
}

impl Default for Ditherer {
    fn default() -> Self {
        Self::new(DitherMode::default())
    }
}

/// xorshift32.
#[inline(always)]
fn next(s: &mut u32) -> u32 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *s = x;
    x
}

impl Ditherer {
    #[must_use]
    pub const fn new(mode: DitherMode) -> Self {
        Self { mode, shaping: matches!(mode, DitherMode::Shaped), rng: SEED, err: [[0; 3]; 2] }
    }

    #[must_use]
    pub fn mode(&self) -> DitherMode {
        self.mode
    }

    /// Mode and output rate (shaping only at ≥ 44.1 kHz).
    pub fn set(&mut self, mode: DitherMode, rate: u32) {
        self.mode = mode;
        self.shaping = mode == DitherMode::Shaped && rate >= 44100;
        self.err = [[0; 3]; 2];
    }

    /// Restart the generator (tests; a fixed seed gives fixed output).
    pub fn seed(&mut self, seed: u32) {
        self.rng = if seed == 0 { SEED } else { seed };
        self.err = [[0; 3]; 2];
    }

    pub fn reset(&mut self) {
        self.err = [[0; 3]; 2];
    }

    /// Quantise `buf` (Q4.27, interleaved) to 16-bit integers, written back
    /// into `buf` as plain integers (−32768 … 32767).
    pub fn quantize(&mut self, buf: &mut [i32], channels: usize) {
        match (self.mode, self.shaping, channels) {
            (DitherMode::Off, _, _) => {
                for x in buf.iter_mut() {
                    *x = round16(*x);
                }
            }
            (_, false, _) => {
                let mut s = self.rng;
                for x in buf.iter_mut() {
                    let d = tpdf(next(&mut s));
                    *x = round16(x.saturating_add(d));
                }
                self.rng = s;
            }
            (_, true, 1) => self.shaped::<1>(buf),
            (_, true, _) => self.shaped::<2>(buf),
        }
    }

    fn shaped<const CH: usize>(&mut self, buf: &mut [i32]) {
        let mut s = self.rng;
        let (frames, _) = buf.as_chunks_mut::<CH>();
        for f in frames.iter_mut() {
            for (x, e) in f.iter_mut().zip(self.err.iter_mut()) {
                let fb = (SHAPE_Q12[0] * e[0] + SHAPE_Q12[1] * e[1] + SHAPE_Q12[2] * e[2]) >> 12;
                let v = x.saturating_sub(fb);
                let d = tpdf(next(&mut s));
                let q = round16(v.saturating_add(d));
                let err = ((q << 12) - v).clamp(-ERR_MAX, ERR_MAX);
                *e = [err, e[0], e[1]];
                *x = q;
            }
        }
        self.rng = s;
    }
}

/// TPDF value from one random word, Q4.27 units (±1 LSB of 16 bits).
#[inline(always)]
#[must_use]
pub fn tpdf(r: u32) -> i32 {
    ((r & 0xFFFF) as i32 + (r >> 16) as i32 - 0xFFFF) >> 4
}

/// Round Q4.27 to a 16-bit integer, saturated.
#[inline(always)]
#[must_use]
pub fn round16(x: i32) -> i32 {
    (x.saturating_add(LSB / 2) >> 12).clamp(-32768, 32767)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tpdf_range() {
        let mut s = SEED;
        let (mut lo, mut hi) = (0, 0);
        for _ in 0..100_000 {
            let d = tpdf(next(&mut s));
            lo = lo.min(d);
            hi = hi.max(d);
        }
        assert!(lo >= -LSB && hi < LSB && lo < -1900 && hi > 1900);
    }

    #[test]
    fn rounding() {
        assert_eq!(round16(0), 0);
        assert_eq!(round16(2047), 0);
        assert_eq!(round16(2048), 1);
        assert_eq!(round16(-2049), -1);
        assert_eq!(round16(i32::MAX), 32767);
        assert_eq!(round16(i32::MIN), -32768);
    }
}
