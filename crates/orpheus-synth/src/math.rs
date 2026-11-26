//! Single-precision helpers for the sample path.
//!
//! Only IEEE-754 basic operations (`+ − × ÷`, comparisons, conversions) are
//! used, never `mul_add` or a `libm` function, so the result of every
//! expression is defined bit for bit by the language and is the same on the
//! host (x86-64 SSE), in the browser (wasm32) and on the player (Cortex-M4F
//! FPv4-SP). Rust does not contract `a * b + c` into a fused multiply-add
//! unless asked to, at any optimisation level.
//!
//! Requirement on the player: the FPU's flush-to-zero bit (`FPSCR.FZ`) stays
//! at its reset value 0, like x86's MXCSR under Rust. The engine keeps its
//! own state out of the subnormal range ([`flush`]), so FZ buys nothing.
//!
//! Accuracy: [`sin_turns`] is within 2.5·10⁻⁷ (two units in the last place
//! at 1.0) of sin(2πp) (the `sin_turns`
//! unit test measures it against `f64`). [`exp2`] and [`log2`] are
//! `orpheus-dsp`'s (3·10⁻⁷ relative, 2·10⁻⁵ absolute).

use core::f32::consts::{PI, TAU};

pub use orpheus_dsp::math::{exp2, log2};

/// State variables below this magnitude are set to zero (−400 dB: far below
/// anything audible, far above the subnormal range at 1.2·10⁻³⁸).
pub const FLUSH: f32 = 1e-20;

/// `x`, or 0 if it is tiny.
#[inline]
#[must_use]
pub fn flush(x: f32) -> f32 {
    if x.abs() < FLUSH { 0.0 } else { x }
}

/// `x` clamped to `[lo, hi]`; NaN becomes `lo`.
#[inline]
#[must_use]
pub fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    if x >= lo { if x <= hi { x } else { hi } } else { lo }
}

/// sin(t) for t in [−π/2, π/2]: Taylor series to t¹¹.
#[inline]
fn sin_core(t: f32) -> f32 {
    let t2 = t * t;
    let p = 1.0 / 39_916_800.0;
    let p = p * -t2 + 1.0 / 362_880.0;
    let p = p * t2 - 1.0 / 5040.0;
    let p = p * t2 + 1.0 / 120.0;
    let p = p * t2 - 1.0 / 6.0;
    let p = p * t2 + 1.0;
    p * t
}

/// sin(2πp) for a phase `p` in turns (any value with |p| < 2²⁴; others give 0).
#[inline]
#[must_use]
pub fn sin_turns(p: f32) -> f32 {
    if p.is_nan() || p.abs() >= 16_777_216.0 {
        return 0.0;
    }
    let mut f = p - (p as i32) as f32;
    if f < 0.0 {
        f += 1.0;
    }
    let x = if f < 0.25 {
        f
    } else if f < 0.75 {
        0.5 - f
    } else {
        f - 1.0
    };
    sin_core(x * TAU)
}

/// sin(2π·phase/2³²) for a 32-bit phase accumulator: the quadrant fold is
/// integer arithmetic (the top two bits), then the same polynomial as
/// [`sin_turns`]. The oscillators' and the FM operators' sine.
#[inline]
#[must_use]
pub fn sin_phase(phase: u32) -> f32 {
    // As i32 the phase is [−½, ½) turn; fold |x| > ¼ turn onto ±(½ − |x|).
    let x = phase as i32;
    let x = if (x ^ (x << 1)) < 0 { i32::MIN.wrapping_sub(x) } else { x };
    sin_core(x as f32 * (TAU / 4_294_967_296.0))
}

/// A phase offset in turns (|t| < 128) as a 32-bit phase increment.
#[inline]
#[must_use]
pub fn turns_to_phase(t: f32) -> u32 {
    ((t * 16_777_216.0) as i32 as u32) << 8
}

/// tan(w) for w in [0, 0.49π] (the SVF's prewarped cutoff).
#[must_use]
pub fn tan_lo(w: f32) -> f32 {
    let w = clampf(w, 0.0, 0.49 * PI);
    orpheus_dsp::math::sin(w) / orpheus_dsp::math::cos(w)
}

/// Frequency in Hz of a (fractional) MIDI note number, A4 = 69 = 440 Hz.
#[must_use]
pub fn note_hz(note: f32) -> f32 {
    440.0 * exp2((note - 69.0) * (1.0 / 12.0))
}

/// The per-sample multiplier of an exponential decay with time constant
/// `tau` seconds (e^(−1/(τ·fs))).
#[must_use]
pub fn decay_coef(tau: f32, rate: f32) -> f32 {
    let n = clampf(tau * rate, 1.0, 1.0e7);
    exp2(-core::f32::consts::LOG2_E / n)
}

/// Soft clip with a knee: identity up to |x| = 0.8, then a Padé tanh
/// approximation that reaches ±1 at |x| = 1.4 and stays there. Monotonic,
/// slope 1 at the knee.
#[inline]
#[must_use]
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    const SPAN: f32 = 0.2;
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    // tanh(u) ≈ u(27 + u²)/(27 + 9u²) for |u| ≤ 3, 1 beyond.
    let u = (a - KNEE) * (1.0 / SPAN);
    let t = if u >= 3.0 { 1.0 } else { u * (27.0 + u * u) / (27.0 + 9.0 * u * u) };
    let y = KNEE + SPAN * t;
    if x < 0.0 { -y } else { y }
}

/// f32 in [−1, 1] to i16 with rounding (half away from zero) and saturation.
#[inline]
#[must_use]
pub fn to_i16(x: f32) -> i16 {
    let v = x * 32767.0;
    // Through i32 (the same values as `as i16`): one vcvt and one ssat on the M4.
    ((if v >= 0.0 { v + 0.5 } else { v - 0.5 }) as i32).clamp(-32_768, 32_767) as i16
}

/// Marsaglia's xorshift32: the deterministic noise and random source. One
/// per voice (seeded from the engine's at note-on) and one in the engine.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rng(u32);

impl Rng {
    /// A generator; seed 0 is replaced by a fixed non-zero value.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    /// The next 32 random bits.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// A uniform value in [−1, 1).
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn sin_turns_error() {
        let mut worst = 0.0f64;
        for i in -40_000..40_000 {
            let p = i as f32 / 10_000.0 + 0.000_037;
            let e = (f64::from(sin_turns(p)) - (f64::from(p) * core::f64::consts::TAU).sin()).abs();
            worst = worst.max(e);
        }
        assert!(worst < 2.5e-7, "sin_turns error {worst}");
        let mut worst = 0.0f64;
        for i in 0..200_000u32 {
            let ph = i.wrapping_mul(21_474_836).wrapping_add(i >> 3);
            let e = (f64::from(sin_phase(ph))
                - (f64::from(ph) / 4_294_967_296.0 * core::f64::consts::TAU).sin())
            .abs();
            worst = worst.max(e);
        }
        assert!(worst < 2.5e-7, "sin_phase error {worst}");
        assert_eq!(sin_phase(0), 0.0);
        assert!((sin_phase(1 << 30) - 1.0).abs() < 2e-7);
        assert_eq!(turns_to_phase(0.25), 1 << 30);
        assert_eq!(turns_to_phase(-0.25), 3 << 30);
        assert_eq!(turns_to_phase(2.5), 1 << 31);
        assert_eq!(sin_turns(f32::NAN), 0.0);
        assert_eq!(sin_turns(f32::INFINITY), 0.0);
    }

    #[test]
    fn soft_clip_is_monotonic_and_bounded() {
        let mut prev = -2.0;
        for i in -3000..=3000 {
            let x = i as f32 / 1000.0;
            let y = soft_clip(x);
            assert!(y >= prev && y.abs() <= 1.0, "{x} -> {y}");
            prev = y;
        }
        assert_eq!(soft_clip(0.5), 0.5);
    }

    #[test]
    fn helpers() {
        assert!((note_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((note_hz(81.0) - 880.0).abs() < 1e-3);
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32767);
        assert_eq!(to_i16(5.0), 32767);
        assert_eq!(to_i16(f32::NAN), 0);
        assert_eq!(clampf(f32::NAN, 1.0, 2.0), 1.0);
        assert!((tan_lo(0.25 * PI) - 1.0).abs() < 1e-5);
        let c = decay_coef(0.01, 48_000.0);
        assert!((f64::from(c) - (-1.0f64 / 480.0).exp()).abs() < 1e-6);
    }
}
