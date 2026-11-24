//! `f32` maths for the CELT float path (libopus 1.5.2 `celt/mathops.[ch]`,
//! BSD-3-Clause, see LICENSE-XIPH).
//!
//! `core` has no `sqrt`/`exp`/`cos` without `std`, and libopus's float build
//! takes them from the C library in double precision. These are `f32`
//! versions: `sqrt` is correctly rounded (integer square root of the
//! mantissa), `exp2` and `cos_norm` are within about one unit in the last
//! place, so results differ from libopus only by rounding. No `f64`.

/// Correctly rounded square root (0 for negative or NaN input).
#[must_use]
pub fn sqrt(x: f32) -> f32 {
    if !(x > 0.0) {
        return 0.0;
    }
    if x == f32::INFINITY {
        return x;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 23) & 0xFF) as i32;
    let mut m = u64::from(bits & 0x7F_FFFF);
    if e == 0 {
        // Subnormal: normalise.
        while m & 0x80_0000 == 0 {
            m <<= 1;
            e -= 1;
        }
        e += 1;
    } else {
        m |= 0x80_0000;
    }
    // x = m * 2^(e - 150), m in [2^23, 2^24).
    let mut ex = e - 150;
    // Scale m so the root has exactly 24 bits: M in [2^46, 2^48), even exponent.
    let mut shift = 23;
    if (ex - shift) & 1 != 0 {
        shift += 1;
    }
    let mm = m << shift;
    ex -= shift;
    let mut r = isqrt64(mm);
    let rem = mm - r * r;
    if rem > r {
        r += 1;
    }
    // r in [2^23, 2^24]; result = r * 2^(ex/2).
    let half = ex / 2;
    ldexp(r as f32, half)
}

fn isqrt64(v: u64) -> u64 {
    let mut rem = v;
    let mut root = 0u64;
    let mut bit = 1u64 << 62;
    while bit > v {
        bit >>= 2;
    }
    while bit != 0 {
        if rem >= root + bit {
            rem -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// `x * 2^n` for an `x` that is an exact integer below 2^25 or a normal
/// number, by exact power-of-two scaling.
#[must_use]
pub fn ldexp(mut x: f32, mut n: i32) -> f32 {
    while n > 127 {
        x *= f32::from_bits(0x7F00_0000); // 2^127
        n -= 127;
    }
    while n < -126 {
        x *= f32::from_bits(0x0080_0000); // 2^-126
        n += 126;
    }
    x * f32::from_bits(((n + 127) as u32) << 23)
}

/// `1 / sqrt(x)`.
#[inline]
#[must_use]
pub fn rsqrt(x: f32) -> f32 {
    1.0 / sqrt(x)
}

/// 2^x.
#[must_use]
pub fn exp2(x: f32) -> f32 {
    if x.is_nan() {
        return x;
    }
    if x > 128.0 {
        return f32::INFINITY;
    }
    if x < -150.0 {
        return 0.0;
    }
    // Round to nearest integer without `round()` (not in core).
    let n = if x >= 0.0 { (x + 0.5) as i32 } else { -((-x + 0.5) as i32) };
    let f = x - n as f32;
    // 2^f = e^(f ln 2), f in [-0.5, 0.5]: Taylor series to degree 8.
    const L: [f32; 9] = [
        1.0,
        0.693_147_2,
        0.240_226_5,
        0.055_504_11,
        0.009_618_129,
        0.001_333_355_8,
        0.000_154_035_3,
        0.000_015_252_734,
        0.000_001_321_548_7,
    ];
    let mut p = L[8];
    let mut i = 8;
    while i > 0 {
        i -= 1;
        p = p * f + L[i];
    }
    ldexp(p, n)
}

/// `cos(pi/2 * x)` for x in [-1, 1] (`celt_cos_norm`).
#[must_use]
pub fn cos_norm(x: f32) -> f32 {
    let x = if x < 0.0 { -x } else { x };
    const HALF_PI: f32 = 1.570_796_4;
    if x <= 0.5 {
        cos_poly(HALF_PI * x)
    } else {
        // cos(pi/2 x) = sin(pi/2 (1 - x)); 1 - x is exact here.
        sin_poly(HALF_PI * (1.0 - x))
    }
}

fn cos_poly(y: f32) -> f32 {
    let z = y * y;
    // 1 - z/2! + z^2/4! - ... (|y| <= pi/4).
    let mut p = -1.0 / 3_628_800.0f32;
    p = p * z + 1.0 / 40320.0;
    p = p * z - 1.0 / 720.0;
    p = p * z + 1.0 / 24.0;
    p = p * z - 0.5;
    p * z + 1.0
}

fn sin_poly(y: f32) -> f32 {
    let z = y * y;
    let mut p = -1.0 / 39_916_800.0f32;
    p = p * z + 1.0 / 362_880.0;
    p = p * z - 1.0 / 5040.0;
    p = p * z + 1.0 / 120.0;
    p = p * z - 1.0 / 6.0;
    y + y * z * p
}

/// Integer square root (`isqrt32`).
#[must_use]
pub fn isqrt32(mut val: u32) -> u32 {
    if val == 0 {
        return 0;
    }
    let mut g = 0u32;
    let mut bshift = (crate::entdec::ilog(val) - 1) >> 1;
    let mut b = 1u32 << bshift;
    loop {
        let t = ((g << 1) + b) << bshift;
        if t <= val {
            g += b;
            val -= t;
        }
        b >>= 1;
        bshift -= 1;
        if bshift < 0 {
            break;
        }
    }
    g
}

/// `FRAC_MUL16`.
#[inline]
#[must_use]
pub fn frac_mul16(a: i32, b: i32) -> i32 {
    (16384 + i32::from(a as i16) * i32::from(b as i16)) >> 15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_is_exact() {
        let mut x = 1.0e-30f32;
        while x < 1.0e30 {
            let s = sqrt(x);
            let d = f64::from(x).sqrt() as f32;
            assert_eq!(s.to_bits(), d.to_bits(), "{x}");
            x *= 1.37;
        }
        for i in 0..100_000u32 {
            let x = f32::from_bits(0x3F00_0000 + i * 97);
            assert_eq!(sqrt(x).to_bits(), (f64::from(x).sqrt() as f32).to_bits());
        }
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(4.0), 2.0);
    }

    #[test]
    fn exp2_and_cos_are_close() {
        let mut worst = 0f64;
        let mut x = -60.0f32;
        while x < 60.0 {
            let r = (0.6931471805599453094 * f64::from(x)).exp() as f32;
            let e = exp2(x);
            let ulps = (i64::from(r.to_bits()) - i64::from(e.to_bits())).abs() as f64;
            worst = worst.max(ulps);
            x += 0.0137;
        }
        assert!(worst <= 2.0, "exp2 worst {worst} ulp");
        let mut worst = 0f64;
        let mut x = 0.0f32;
        while x <= 1.0 {
            let r = ((0.5 * std::f64::consts::PI) * f64::from(x)).cos() as f32;
            let c = cos_norm(x);
            worst = worst.max((f64::from(r) - f64::from(c)).abs());
            x += 0.0001;
        }
        assert!(worst < 2e-7, "cos worst {worst}");
    }
}
