//! Ported from libopus 1.5.2 silk/macros.h, silk/SigProc_FIX.h and
//! silk/Inlines.h (BSD-3-Clause, see LICENSE-XIPH).
//!
//! The fixed-point primitives of SILK, one `#[inline]` function per C macro.
//! Where C relies on two's-complement wrap-around (the `_ovflw` macros, and
//! in practice `silk_ADD32`, `silk_LSHIFT` of negative numbers and the
//! multiply-accumulates), the functions wrap explicitly so that they never
//! panic with overflow checks on. Shift counts are masked (`wrapping_sh*`):
//! for the in-range counts of valid streams that is exactly C; for corrupt
//! input it only avoids a panic.

#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]

pub const I16_MAX: i32 = i16::MAX as i32;
pub const I16_MIN: i32 = i16::MIN as i32;

/// `silk_LSHIFT` / `silk_LSHIFT32` / `silk_LSHIFT_ovflw`: shift through
/// `u32`, wrapping.
#[inline]
pub fn lshift(a: i32, shift: i32) -> i32 {
    (a as u32).wrapping_shl(shift as u32) as i32
}

/// `silk_RSHIFT`: arithmetic right shift.
#[inline]
pub fn rshift(a: i32, shift: i32) -> i32 {
    a.wrapping_shr(shift as u32)
}

/// `silk_RSHIFT_ROUND`.
#[inline]
pub fn rshift_round(a: i32, shift: i32) -> i32 {
    if shift == 1 {
        (a >> 1).wrapping_add(a & 1)
    } else {
        (a.wrapping_shr((shift - 1) as u32).wrapping_add(1)) >> 1
    }
}

/// `silk_RSHIFT_ROUND64`.
#[inline]
pub fn rshift_round64(a: i64, shift: i32) -> i64 {
    if shift == 1 {
        (a >> 1).wrapping_add(a & 1)
    } else {
        (a.wrapping_shr((shift - 1) as u32).wrapping_add(1)) >> 1
    }
}

/// `silk_SMULWB`: (a32 * (i16)b32) >> 16.
#[inline]
pub fn smulwb(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b as i16)) >> 16) as i32
}

/// `silk_SMLAWB`: a32 + ((b32 * (i16)c32) >> 16).
#[inline]
pub fn smlawb(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulwb(b, c))
}

/// `silk_SMULWW`: (a32 * b32) >> 16.
#[inline]
pub fn smulww(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> 16) as i32
}

/// `silk_SMLAWW`: a32 + ((b32 * c32) >> 16).
#[inline]
pub fn smlaww(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulww(b, c))
}

/// `silk_SMULBB`: (i16)a32 * (i16)b32.
#[inline]
pub fn smulbb(a: i32, b: i32) -> i32 {
    i32::from(a as i16) * i32::from(b as i16)
}

/// `silk_SMLABB` / `silk_SMLABB_ovflw`: a32 + (i16)b32 * (i16)c32.
#[inline]
pub fn smlabb(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(smulbb(b, c))
}

/// `silk_SMULTT`: (a32 >> 16) * (b32 >> 16).
#[inline]
pub fn smultt(a: i32, b: i32) -> i32 {
    (a >> 16).wrapping_mul(b >> 16)
}

/// `silk_SMMUL`: (a32 * b32) >> 32.
#[inline]
pub fn smmul(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> 32) as i32
}

/// `silk_SMULL`: 64-bit product.
#[inline]
pub fn smull(a: i32, b: i32) -> i64 {
    i64::from(a) * i64::from(b)
}

/// `silk_MUL` (wrapping).
#[inline]
pub fn mul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// `silk_MLA` (wrapping).
#[inline]
pub fn mla(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_add(b.wrapping_mul(c))
}

/// `silk_ADD_SAT32`.
#[inline]
pub fn add_sat32(a: i32, b: i32) -> i32 {
    a.saturating_add(b)
}

/// `silk_SUB_SAT32`.
#[inline]
pub fn sub_sat32(a: i32, b: i32) -> i32 {
    a.saturating_sub(b)
}

/// `silk_SAT16`.
#[inline]
pub fn sat16(a: i32) -> i32 {
    a.clamp(I16_MIN, I16_MAX)
}

/// `silk_ADD_SAT16`.
#[inline]
pub fn add_sat16(a: i16, b: i32) -> i16 {
    sat16(i32::from(a).wrapping_add(b)) as i16
}

/// `silk_LIMIT` (works with the limits in either order).
#[inline]
pub fn limit(a: i32, limit1: i32, limit2: i32) -> i32 {
    if limit1 > limit2 {
        if a > limit1 {
            limit1
        } else if a < limit2 {
            limit2
        } else {
            a
        }
    } else if a > limit2 {
        limit2
    } else if a < limit1 {
        limit1
    } else {
        a
    }
}

/// `silk_LSHIFT_SAT32`: saturate, then shift.
#[inline]
pub fn lshift_sat32(a: i32, shift: i32) -> i32 {
    lshift(limit(a, rshift(i32::MIN, shift), rshift(i32::MAX, shift)), shift)
}

/// `silk_ADD_LSHIFT32`.
#[inline]
pub fn add_lshift32(a: i32, b: i32, shift: i32) -> i32 {
    a.wrapping_add(lshift(b, shift))
}

/// `silk_SUB_LSHIFT32`.
#[inline]
pub fn sub_lshift32(a: i32, b: i32, shift: i32) -> i32 {
    a.wrapping_sub(lshift(b, shift))
}

/// `silk_ADD_RSHIFT32`.
#[inline]
pub fn add_rshift32(a: i32, b: i32, shift: i32) -> i32 {
    a.wrapping_add(rshift(b, shift))
}

/// `silk_DIV32` / `silk_DIV32_16`. C would trap (or be undefined) on a
/// zero divisor; here that gives 0, which only corrupt input can reach.
#[inline]
pub fn div32(a: i32, b: i32) -> i32 {
    if b == 0 { 0 } else { a.wrapping_div(b) }
}

/// `silk_abs` (wraps on `i32::MIN` like the C in practice).
#[inline]
pub fn abs(a: i32) -> i32 {
    a.wrapping_abs()
}

/// `silk_CLZ32`.
#[inline]
pub fn clz32(a: i32) -> i32 {
    (a as u32).leading_zeros() as i32
}

/// `silk_ROR32`.
#[inline]
pub fn ror32(a: i32, rot: i32) -> i32 {
    (a as u32).rotate_right(rot.rem_euclid(32) as u32) as i32
}

/// `silk_CLZ_FRAC`: leading zeros and the 7 bits after the leading one.
#[inline]
pub fn clz_frac(a: i32) -> (i32, i32) {
    let lz = clz32(a);
    (lz, ror32(a, 24 - lz) & 0x7f)
}

/// `silk_SQRT_APPROX`.
#[inline]
pub fn sqrt_approx(x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let (lz, frac_q7) = clz_frac(x);
    let mut y: i32 = if lz & 1 != 0 { 32768 } else { 46214 };
    y >>= lz >> 1;
    smlawb(y, y, smulbb(213, frac_q7))
}

/// `silk_DIV32_varQ`: approximation of `(a32 << qres) / b32`.
#[inline]
pub fn div32_varq(a32: i32, b32: i32, qres: i32) -> i32 {
    let a_headrm = clz32(abs(a32)) - 1;
    let mut a32_nrm = lshift(a32, a_headrm);
    let b_headrm = clz32(abs(b32)) - 1;
    let b32_nrm = lshift(b32, b_headrm);
    let b32_inv = div32(i32::MAX >> 2, b32_nrm >> 16);
    let mut result = smulwb(a32_nrm, b32_inv);
    a32_nrm = a32_nrm.wrapping_sub(lshift(smmul(b32_nrm, result), 3));
    result = smlawb(result, a32_nrm, b32_inv);
    let lshift_ = 29 + a_headrm - b_headrm - qres;
    if lshift_ < 0 {
        lshift_sat32(result, -lshift_)
    } else if lshift_ < 32 {
        rshift(result, lshift_)
    } else {
        0
    }
}

/// `silk_INVERSE32_varQ`: approximation of `(1 << qres) / b32`.
#[inline]
pub fn inverse32_varq(b32: i32, qres: i32) -> i32 {
    let b_headrm = clz32(abs(b32)) - 1;
    let b32_nrm = lshift(b32, b_headrm);
    let b32_inv = div32(i32::MAX >> 2, b32_nrm >> 16);
    let mut result = lshift(b32_inv, 16);
    let err_q32 = lshift((1i32 << 29).wrapping_sub(smulwb(b32_nrm, b32_inv)), 3);
    result = smlaww(result, err_q32, b32_inv);
    let lshift_ = 61 - b_headrm - qres;
    if lshift_ <= 0 {
        lshift_sat32(result, -lshift_)
    } else if lshift_ < 32 {
        rshift(result, lshift_)
    } else {
        0
    }
}

/// `silk_RAND`: linear congruential generator.
#[inline]
pub fn rand(seed: i32) -> i32 {
    907_633_515u32.wrapping_add((seed as u32).wrapping_mul(196_314_165)) as i32
}

/// `silk_log2lin`: approximation of `2^(in/128)`.
pub fn log2lin(in_log_q7: i32) -> i32 {
    if in_log_q7 < 0 {
        return 0;
    } else if in_log_q7 >= 3967 {
        return i32::MAX;
    }
    let mut out = lshift(1, in_log_q7 >> 7);
    let frac_q7 = in_log_q7 & 0x7F;
    let t = smlawb(frac_q7, smulbb(frac_q7, 128 - frac_q7), -174);
    if in_log_q7 < 2048 {
        out = add_rshift32(out, mul(out, t), 7);
    } else {
        out = mla(out, out >> 7, t);
    }
    out
}

/// `silk_sum_sqr_shift`: energy of `x` and the right shift that makes it
/// fit in 32 bits with two bits of headroom.
pub fn sum_sqr_shift(x: &[i16]) -> (i32, i32) {
    let len = x.len() as i32;
    let mut shft = 31 - clz32(len);
    let mut nrg: i32 = len;
    let pass = |nrg0: i32, shft: i32| -> i32 {
        let mut nrg = nrg0;
        let mut i = 0usize;
        while i + 1 < x.len() {
            let mut nrg_tmp = smulbb(i32::from(x[i]), i32::from(x[i])) as u32;
            nrg_tmp = nrg_tmp.wrapping_add(smulbb(i32::from(x[i + 1]), i32::from(x[i + 1])) as u32);
            nrg = (nrg as u32).wrapping_add(nrg_tmp.wrapping_shr(shft as u32)) as i32;
            i += 2;
        }
        if i < x.len() {
            let nrg_tmp = smulbb(i32::from(x[i]), i32::from(x[i])) as u32;
            nrg = (nrg as u32).wrapping_add(nrg_tmp.wrapping_shr(shft as u32)) as i32;
        }
        nrg
    };
    nrg = pass(nrg, shft);
    shft = 0.max(shft + 3 - clz32(nrg));
    nrg = pass(0, shft);
    (nrg, shft)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macros_basic() {
        assert_eq!(smulwb(65536, 3), 3);
        assert_eq!(smulwb(-1, 1), -1);
        assert_eq!(rshift_round(5, 1), 3);
        assert_eq!(rshift_round(-5, 1), -2);
        assert_eq!(rshift_round(6, 2), 2);
        assert_eq!(clz32(0), 32);
        assert_eq!(clz32(1), 31);
        assert_eq!(sqrt_approx(1 << 16), 256);
        assert_eq!(lshift_sat32(1 << 30, 2), i32::MAX & !3);
        assert_eq!(log2lin(128 * 16), 65536);
        assert_eq!(ror32(0x0000_0180, 1), 0xC0);
        assert_eq!(ror32(1, -1), 2);
    }
}
