//! Ported from libopus 1.5.2 silk/LPC_fit.c, silk/LPC_inv_pred_gain.c,
//! silk/bwexpander.c, silk/bwexpander_32.c and silk/LPC_analysis_filter.c
//! (BSD-3-Clause, see LICENSE-XIPH).

use super::MAX_LPC_ORDER;
use super::macros::{
    abs, clz32, div32, inverse32_varq, lshift, mul, rshift, rshift_round, rshift_round64, sat16, smlabb,
    smmul, smulbb, smull, smulww, sub_sat32,
};

const QA24: i32 = 24;
/// `SILK_FIX_CONST( 0.99975, QA )`.
const A_LIMIT: i32 = 16_773_022;
/// `SILK_FIX_CONST( 1.0f / MAX_PREDICTION_POWER_GAIN, 30 )`.
const MIN_INV_GAIN_Q30: i32 = 107_374;

/// `silk_bwexpander_32`: chirp an AR filter (Q16 chirp factor).
pub(crate) fn bwexpander_32(ar: &mut [i32], mut chirp_q16: i32) {
    let d = ar.len();
    let chirp_minus_one_q16 = chirp_q16.wrapping_sub(65536);
    for a in &mut ar[..d - 1] {
        *a = smulww(chirp_q16, *a);
        chirp_q16 = chirp_q16.wrapping_add(rshift_round(mul(chirp_q16, chirp_minus_one_q16), 16));
    }
    ar[d - 1] = smulww(chirp_q16, ar[d - 1]);
}

/// `silk_bwexpander`: chirp an AR filter of Q12 coefficients.
pub(crate) fn bwexpander(ar: &mut [i16], mut chirp_q16: i32) {
    let d = ar.len();
    let chirp_minus_one_q16 = chirp_q16.wrapping_sub(65536);
    // NB: Dont use silk_SMULWB, instead of silk_RSHIFT_ROUND( silk_MUL(), 16 ), below.
    // Bias in silk_SMULWB can lead to unstable filters.
    for a in &mut ar[..d - 1] {
        *a = rshift_round(mul(chirp_q16, i32::from(*a)), 16) as i16;
        chirp_q16 = chirp_q16.wrapping_add(rshift_round(mul(chirp_q16, chirp_minus_one_q16), 16));
    }
    ar[d - 1] = rshift_round(mul(chirp_q16, i32::from(ar[d - 1])), 16) as i16;
}

/// `silk_LPC_fit`: convert int32 coefficients to int16 without
/// wrap-around.
pub(crate) fn lpc_fit(a_qout: &mut [i16], a_qin: &mut [i32], qout: i32, qin: i32) {
    let d = a_qin.len();
    let mut idx = 0usize;
    let mut i = 0;
    // Limit the maximum absolute value of the prediction coefficients, so
    // that they'll fit in int16.
    while i < 10 {
        // Find maximum absolute value and its index.
        let mut maxabs = 0i32;
        for (k, &a) in a_qin.iter().enumerate() {
            let absval = abs(a);
            if absval > maxabs {
                maxabs = absval;
                idx = k;
            }
        }
        maxabs = rshift_round(maxabs, qin - qout);

        if maxabs > i32::from(i16::MAX) {
            // Reduce magnitude of prediction coefficients.
            maxabs = maxabs.min(163_838); // ( silk_int32_MAX >> 14 ) + silk_int16_MAX = 163838
            let chirp_q16 = 65470 // SILK_FIX_CONST( 0.999, 16 )
                - div32(lshift(maxabs - i32::from(i16::MAX), 14), rshift(mul(maxabs, idx as i32 + 1), 2));
            bwexpander_32(a_qin, chirp_q16);
        } else {
            break;
        }
        i += 1;
    }

    if i == 10 {
        // Reached the last iteration, clip the coefficients.
        for k in 0..d {
            a_qout[k] = sat16(rshift_round(a_qin[k], qin - qout)) as i16;
            a_qin[k] = lshift(i32::from(a_qout[k]), qin - qout);
        }
    } else {
        for k in 0..d {
            a_qout[k] = rshift_round(a_qin[k], qin - qout) as i16;
        }
    }
}

#[inline]
fn mul32_frac_q(a32: i32, b32: i32, q: i32) -> i32 {
    rshift_round64(smull(a32, b32), q) as i32
}

/// `LPC_inverse_pred_gain_QA_c`.
fn lpc_inverse_pred_gain_qa(a_qa: &mut [i32; MAX_LPC_ORDER], order: usize) -> i32 {
    let mut inv_gain_q30: i32 = 1 << 30;
    let mut k = order - 1;
    while k > 0 {
        // Check for stability.
        if a_qa[k] > A_LIMIT || a_qa[k] < -A_LIMIT {
            return 0;
        }
        // Set RC equal to negated AR coef.
        let rc_q31 = lshift(a_qa[k], 31 - QA24).wrapping_neg();
        // rc_mult1_Q30 range: [ 1 : 2^30 ].
        let rc_mult1_q30 = (1i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));
        // Update inverse gain; invGain_Q30 range: [ 0 : 2^30 ].
        inv_gain_q30 = lshift(smmul(inv_gain_q30, rc_mult1_q30), 2);
        if inv_gain_q30 < MIN_INV_GAIN_Q30 {
            return 0;
        }
        // rc_mult2 range: [ 2^30 : silk_int32_MAX ].
        let mult2q = 32 - clz32(abs(rc_mult1_q30));
        let rc_mult2 = inverse32_varq(rc_mult1_q30, mult2q + 30);

        // Update AR coefficient.
        for n in 0..(k + 1) >> 1 {
            let tmp1 = a_qa[n];
            let tmp2 = a_qa[k - n - 1];
            let tmp64 =
                rshift_round64(smull(sub_sat32(tmp1, mul32_frac_q(tmp2, rc_q31, 31)), rc_mult2), mult2q);
            if tmp64 > i64::from(i32::MAX) || tmp64 < i64::from(i32::MIN) {
                return 0;
            }
            a_qa[n] = tmp64 as i32;
            let tmp64 =
                rshift_round64(smull(sub_sat32(tmp2, mul32_frac_q(tmp1, rc_q31, 31)), rc_mult2), mult2q);
            if tmp64 > i64::from(i32::MAX) || tmp64 < i64::from(i32::MIN) {
                return 0;
            }
            a_qa[k - n - 1] = tmp64 as i32;
        }
        k -= 1;
    }

    // Check for stability.
    if a_qa[0] > A_LIMIT || a_qa[0] < -A_LIMIT {
        return 0;
    }
    // Set RC equal to negated AR coef.
    let rc_q31 = lshift(a_qa[0], 31 - QA24).wrapping_neg();
    // Range: [ 1 : 2^30 ].
    let rc_mult1_q30 = (1i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));
    // Update inverse gain. Range: [ 0 : 2^30 ].
    inv_gain_q30 = lshift(smmul(inv_gain_q30, rc_mult1_q30), 2);
    if inv_gain_q30 < MIN_INV_GAIN_Q30 {
        return 0;
    }
    inv_gain_q30
}

/// `silk_LPC_inverse_pred_gain`: inverse prediction gain (Q30) of Q12
/// coefficients, 0 if unstable.
pub(crate) fn lpc_inverse_pred_gain(a_q12: &[i16]) -> i32 {
    let order = a_q12.len().min(MAX_LPC_ORDER);
    let mut atmp_qa = [0i32; MAX_LPC_ORDER];
    let mut dc_resp: i32 = 0;
    // Increase Q domain of the AR coefficients.
    for k in 0..order {
        dc_resp += i32::from(a_q12[k]);
        atmp_qa[k] = lshift(i32::from(a_q12[k]), QA24 - 12);
    }
    // If the DC is unstable, we don't even need to do the full calculations.
    if dc_resp >= 4096 {
        return 0;
    }
    lpc_inverse_pred_gain_qa(&mut atmp_qa, order)
}

/// `silk_LPC_analysis_filter`: `out[d..len]` = residual of `input`; the
/// first `d` outputs are zero. `out` and `input` hold `len` samples.
pub(crate) fn lpc_analysis_filter(out: &mut [i16], input: &[i16], b: &[i16]) {
    let d = b.len();
    let len = input.len();
    for ix in d..len {
        let mut out32_q12 = smulbb(i32::from(input[ix - 1]), i32::from(b[0]));
        // Allowing wrap around so that two wraps can cancel each other.
        // The rare cases where the result wraps around can only be
        // triggered by invalid streams.
        for j in 1..d {
            out32_q12 = smlabb(out32_q12, i32::from(input[ix - 1 - j]), i32::from(b[j]));
        }
        // Subtract prediction.
        out32_q12 = lshift(i32::from(input[ix]), 12).wrapping_sub(out32_q12);
        // Scale to Q0 and saturate.
        out[ix] = sat16(rshift_round(out32_q12, 12)) as i16;
    }
    // Set first d output samples to zero.
    out[..d].fill(0);
}
