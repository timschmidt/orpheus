//! Single-precision functions for coefficient design (no `libm`: its `f32`
//! functions widen to `f64` internally, which is soft-float on the M4).
//!
//! These run only when a parameter changes or a ramp step is designed, never
//! per sample. Accuracy (checked against `f64` in the tests): `sin`/`cos`
//! within 2·10⁻⁷ absolute on [0, π]; `exp2` within 3·10⁻⁷ relative;
//! `log2` within 2·10⁻⁵ absolute.

use core::f32::consts::{FRAC_PI_2, LN_2, LOG2_10, PI};

/// |x|.
#[inline]
#[must_use]
pub fn abs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

/// sin(x) for x in [−π/2, π/2]: Taylor series to x¹¹ (truncation error
/// < 6·10⁻⁸ at the interval ends).
#[must_use]
fn sin_core(x: f32) -> f32 {
    let x2 = x * x;
    let p = 1.0 / 39_916_800.0;
    let p = p * -x2 + 1.0 / 362_880.0;
    let p = p * x2 - 1.0 / 5040.0;
    let p = p * x2 + 1.0 / 120.0;
    let p = p * x2 - 1.0 / 6.0;
    let p = p * x2 + 1.0;
    p * x
}

/// sin(x) for x in [0, π] (the range of a normalised angular frequency).
/// Values outside are clamped.
#[must_use]
pub fn sin(x: f32) -> f32 {
    let x = x.clamp(0.0, PI);
    if x > FRAC_PI_2 { sin_core(PI - x) } else { sin_core(x) }
}

/// cos(x) for x in [0, π]. Values outside are clamped.
#[must_use]
pub fn cos(x: f32) -> f32 {
    let x = x.clamp(0.0, PI);
    sin_core(FRAC_PI_2 - x)
}

/// 2^x. Saturates to 0 below 2^−126 and to 2^127 above.
#[must_use]
pub fn exp2(x: f32) -> f32 {
    if x.is_nan() {
        return 1.0;
    }
    let x = x.clamp(-126.0, 127.0);
    // Nearest integer i and remainder f in [−0.5, 0.5].
    let i = if x >= 0.0 { (x + 0.5) as i32 } else { (x - 0.5) as i32 };
    let f = x - i as f32;
    // e^(f ln 2), Taylor to the 7th power: error < 1.2·10⁻⁷·… on |f| ≤ 0.5.
    let t = f * LN_2;
    let p = 1.0 / 5040.0;
    let p = p * t + 1.0 / 720.0;
    let p = p * t + 1.0 / 120.0;
    let p = p * t + 1.0 / 24.0;
    let p = p * t + 1.0 / 6.0;
    let p = p * t + 0.5;
    let p = p * t + 1.0;
    let p = p * t + 1.0;
    let e = (i + 127).clamp(1, 254) as u32;
    p * f32::from_bits(e << 23)
}

/// 10^x.
#[must_use]
pub fn pow10(x: f32) -> f32 {
    exp2(x * LOG2_10)
}

/// Linear amplitude of `db` decibels.
#[must_use]
pub fn db_to_lin(db: f32) -> f32 {
    pow10(db * 0.05)
}

/// log2(x) for x > 0 (returns −127 for x ≤ 0).
#[must_use]
pub fn log2(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return -127.0;
    }
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xFF) as i32 - 127;
    // Mantissa m in [1, 2); use m' = m/√2 in [0.707, 1.414) and the series
    // of ln((1+u)/(1−u)) with u = (m'−1)/(m'+1), |u| ≤ 0.172.
    let m = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000);
    let (m, e) = if m > core::f32::consts::SQRT_2 { (m * 0.5, e + 1) } else { (m, e) };
    let u = (m - 1.0) / (m + 1.0);
    let u2 = u * u;
    let s = u * (2.0 + u2 * (2.0 / 3.0 + u2 * (2.0 / 5.0 + u2 * (2.0 / 7.0))));
    e as f32 + s * core::f32::consts::LOG2_E
}

/// 20·log10(x) for x > 0.
#[must_use]
pub fn lin_to_db(x: f32) -> f32 {
    log2(x) * (20.0 / LOG2_10)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn trig_accuracy() {
        let mut worst = 0.0f64;
        for i in 0..=10_000 {
            let x = f64::from(i) * core::f64::consts::PI / 10_000.0;
            let xf = x as f32;
            let s = f64::from(sin(xf)) - f64::from(xf).sin();
            let c = f64::from(cos(xf)) - f64::from(xf).cos();
            worst = worst.max(s.abs()).max(c.abs());
        }
        assert!(worst < 2e-7, "worst {worst}");
    }

    #[test]
    fn exp_log_accuracy() {
        let mut worst = 0.0f64;
        for i in -4000..=4000 {
            let x = i as f32 * 0.01;
            let want = (f64::from(x)).exp2();
            let rel = (f64::from(exp2(x)) - want).abs() / want;
            worst = worst.max(rel);
        }
        assert!(worst < 3e-7, "exp2 worst {worst}");
        let mut worst = 0.0f64;
        for i in 1..=10_000 {
            let x = i as f32 * 0.0137;
            let e = (f64::from(log2(x)) - f64::from(x).log2()).abs();
            worst = worst.max(e);
        }
        assert!(worst < 2e-5, "log2 worst {worst}");
        assert!((db_to_lin(-6.0206) - 0.5).abs() < 1e-6);
        assert!((lin_to_db(0.5) + 6.0206).abs() < 1e-3);
    }
}
