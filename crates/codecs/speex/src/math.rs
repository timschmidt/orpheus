//! `f32` helpers `core` lacks: square root (correctly rounded, integer
//! arithmetic), cosine on `[0, π]`, floor, libspeex's float-to-`i16`
//! conversion and its pseudo-random generator.

/// Integer square root of a 64-bit value.
fn isqrt(n: u64) -> u64 {
    let mut rem = n;
    let mut res = 0u64;
    let mut bit = 1u64 << 62;
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if rem >= res + bit {
            rem -= res + bit;
            res = (res >> 1) + bit;
        } else {
            res >>= 1;
        }
        bit >>= 2;
    }
    res
}

/// `2^n` for `n` in the normal exponent range.
fn pow2(n: i32) -> f32 {
    f32::from_bits(((n.clamp(-126, 127) + 127) as u32) << 23)
}

/// Correctly rounded square root (what C's `sqrt` on a `float` argument
/// gives once the result is stored back in a `float`). Negative and NaN
/// arguments give NaN.
#[must_use]
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x < 0.0 {
        return f32::NAN;
    }
    if x == 0.0 || x == f32::INFINITY {
        return x;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 23) & 0xFF) as i32;
    let mut m = u64::from(bits & 0x7F_FFFF);
    if e == 0 {
        // Subnormal: normalise.
        e = 1;
        while m & 0x80_0000 == 0 {
            m <<= 1;
            e -= 1;
        }
    } else {
        m |= 0x80_0000;
    }
    // x = m * 2^k with m in [2^23, 2^24).
    let mut k = e - 127 - 23;
    if k & 1 != 0 {
        m <<= 1;
        k -= 1;
    }
    // sqrt(m << 26) has 25 or 26 bits.
    let big = m << 26;
    let q = isqrt(big);
    let sticky = q * q != big;
    let nb = 64 - q.leading_zeros() as i32;
    let d = nb - 24;
    let low = q & ((1u64 << d) - 1);
    let half = 1u64 << (d - 1);
    let mut qh = q >> d;
    if low > half || (low == half && (sticky || qh & 1 == 1)) {
        qh += 1;
    }
    (qh as f32) * pow2(d + k / 2 - 13)
}

/// `floor` for values within ±2^31.
#[must_use]
pub fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

/// libspeex's `spx_cos` of the float build (`math_approx.h`): a quartic
/// in `x²` on `[0, π/2)`, mirrored on `[π/2, π]`. LSP frequencies lie in
/// `(0, π)`. The C code computes `π − x` in double precision; here
/// `π_hi − x` is exact (Sterbenz) and adding `π_lo` rounds once, as the
/// conversion of the double difference to `float` does.
#[must_use]
pub fn spx_cos(x: f32) -> f32 {
    const C1: f32 = 0.999_993_3;
    const C2: f32 = -0.499_912_44;
    const C3: f32 = 0.041_487_747;
    const C4: f32 = -0.001_271_209_5;
    const PI_HI: f32 = core::f32::consts::PI;
    const PI_LO: f32 = -8.742_278e-8;
    // `x < 1.5707963268` (a double) for a float `x`.
    if x < 1.570_796_4 {
        let x = x * x;
        C1 + x * (C2 + x * (C3 + C4 * x))
    } else {
        let x = (PI_HI - x) + PI_LO;
        let x = x * x;
        -(C1 + x * (C2 + x * (C3 + C4 * x)))
    }
}

/// libspeex's conversion of decoded `float` samples to `i16`
/// (`speex_decode_int`): saturate, then `floor(0.5 + x)` evaluated exactly.
#[must_use]
pub fn to_i16(x: f32) -> i16 {
    if x > 32767.0 {
        32767
    } else if x < -32768.0 {
        -32768
    } else if x.is_nan() {
        0
    } else {
        let fl = floor(x);
        let r = if x >= fl + 0.5 { fl + 1.0 } else { fl };
        r as i16
    }
}

/// Advance the `speex_rand` seed by `n` draws.
pub fn skip_rand(seed: &mut u32, n: usize) {
    for _ in 0..n {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
}

/// `speex_rand`: a uniform deviate with standard deviation `std`.
#[must_use]
pub fn rand(std: f32, seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let r = f32::from_bits(0x3F80_0000 | (*seed & 0x007F_FFFF)) - 1.5;
    3.4642 * std * r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_is_correctly_rounded() {
        let mut s = 0x1234_5678u32;
        for i in 0..200_000u32 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let x = f32::from_bits(s & 0x7FFF_FFFF);
            if !x.is_finite() {
                continue;
            }
            let want = (f64::from(x)).sqrt() as f32;
            assert_eq!(sqrt(x).to_bits(), want.to_bits(), "sqrt({x:e}) #{i}");
        }
        for x in [0.0f32, 1.0, 2.0, 4.0, 1000.0, 1e-40, 3.4e38, 0.1, 1.0 + f32::EPSILON] {
            assert_eq!(sqrt(x).to_bits(), (f64::from(x).sqrt() as f32).to_bits(), "{x:e}");
        }
        assert!(sqrt(-1.0).is_nan());
    }

    #[test]
    fn spx_cos_is_close() {
        for i in 0..=1000 {
            let x = 3.14f32 * i as f32 / 1000.0;
            assert!((f64::from(spx_cos(x)) - f64::from(x).cos()).abs() < 2e-5, "{x}");
        }
    }

    #[test]
    fn rounding() {
        assert_eq!(to_i16(0.499_999_97), 0);
        assert_eq!(to_i16(0.5), 1);
        assert_eq!(to_i16(-0.5), 0);
        assert_eq!(to_i16(-0.500_000_06), -1);
        assert_eq!(to_i16(-1.5), -1);
        assert_eq!(to_i16(40000.0), 32767);
        assert_eq!(to_i16(-40000.0), -32768);
        assert_eq!(to_i16(32767.0), 32767);
        assert_eq!(to_i16(f32::NAN), 0);
        assert_eq!(floor(-0.25), -1.0);
        assert_eq!(floor(2.0), 2.0);
    }
}
