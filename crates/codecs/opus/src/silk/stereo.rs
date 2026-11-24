//! Ported from libopus 1.5.2 silk/stereo_decode_pred.c and
//! silk/stereo_MS_to_LR.c (BSD-3-Clause, see LICENSE-XIPH).

use super::macros::{add_lshift32, div32, rshift_round, sat16, smlabb, smlawb, smulbb, smulwb};
use super::tables::{
    STEREO_ONLY_CODE_MID_ICDF, STEREO_PRED_JOINT_ICDF, STEREO_PRED_QUANT_Q13, UNIFORM3_ICDF, UNIFORM5_ICDF,
};
use crate::entdec::EcDec;

const STEREO_INTERP_LEN_MS: i32 = 8;
/// `SILK_FIX_CONST( 0.5 / STEREO_QUANT_SUB_STEPS, 16 )`.
const HALF_SUB_STEP_Q16: i32 = 6554;

/// `silk_stereo_decode_pred`: mid/side predictors (Q13).
pub(crate) fn stereo_decode_pred(dec: &mut EcDec<'_>, pred_q13: &mut [i32; 2]) {
    let mut ix = [[0i32; 3]; 2];

    // Entropy decoding.
    let n = dec.icdf(&STEREO_PRED_JOINT_ICDF, 8);
    ix[0][2] = div32(n, 5);
    ix[1][2] = n - 5 * ix[0][2];
    for row in &mut ix {
        row[0] = dec.icdf(&UNIFORM3_ICDF, 8);
        row[1] = dec.icdf(&UNIFORM5_ICDF, 8);
    }

    // Dequantize.
    for n in 0..2 {
        ix[n][0] += 3 * ix[n][2];
        let i0 = (ix[n][0].max(0) as usize).min(STEREO_PRED_QUANT_Q13.len() - 2);
        let low_q13 = i32::from(STEREO_PRED_QUANT_Q13[i0]);
        let step_q13 = smulwb(i32::from(STEREO_PRED_QUANT_Q13[i0 + 1]) - low_q13, HALF_SUB_STEP_Q16);
        pred_q13[n] = smlabb(low_q13, step_q13, 2 * ix[n][1] + 1);
    }

    // Subtract second from first predictor (helps when actually applying these).
    pred_q13[0] -= pred_q13[1];
}

/// `silk_stereo_decode_mid_only`: flag that only the mid channel is coded.
pub(crate) fn stereo_decode_mid_only(dec: &mut EcDec<'_>) -> i32 {
    dec.icdf(&STEREO_ONLY_CODE_MID_ICDF, 8)
}

/// `silk_stereo_MS_to_LR`: convert the adaptive mid/side representation
/// in `x1`/`x2` (each `frame_length + 2` samples) to left/right.
pub(crate) fn stereo_ms_to_lr(
    pred_prev_q13: &mut [i16; 2],
    s_mid: &mut [i16; 2],
    s_side: &mut [i16; 2],
    x1: &mut [i16],
    x2: &mut [i16],
    pred_q13: &[i32; 2],
    fs_khz: i32,
    frame_length: usize,
) {
    // Buffering.
    x1[..2].copy_from_slice(s_mid);
    x2[..2].copy_from_slice(s_side);
    s_mid.copy_from_slice(&x1[frame_length..frame_length + 2]);
    s_side.copy_from_slice(&x2[frame_length..frame_length + 2]);

    // Interpolate predictors and add prediction to side channel.
    let mut pred0_q13 = i32::from(pred_prev_q13[0]);
    let mut pred1_q13 = i32::from(pred_prev_q13[1]);
    let interp_len = STEREO_INTERP_LEN_MS * fs_khz;
    let denom_q16 = div32(1 << 16, interp_len);
    let delta0_q13 = rshift_round(smulbb(pred_q13[0] - i32::from(pred_prev_q13[0]), denom_q16), 16);
    let delta1_q13 = rshift_round(smulbb(pred_q13[1] - i32::from(pred_prev_q13[1]), denom_q16), 16);
    let interp_len = (interp_len.max(0) as usize).min(frame_length);
    let side = |x1: &[i16], x2: &[i16], n: usize, p0: i32, p1: i32| -> i16 {
        let mut sum = add_lshift32(i32::from(x1[n]) + i32::from(x1[n + 2]), i32::from(x1[n + 1]), 1) << 9; // Q11
        sum = smlawb(i32::from(x2[n + 1]) << 8, sum, p0); // Q8
        sum = smlawb(sum, i32::from(x1[n + 1]) << 11, p1); // Q8
        sat16(rshift_round(sum, 8)) as i16
    };
    for n in 0..interp_len {
        pred0_q13 += delta0_q13;
        pred1_q13 += delta1_q13;
        x2[n + 1] = side(x1, x2, n, pred0_q13, pred1_q13);
    }
    pred0_q13 = pred_q13[0];
    pred1_q13 = pred_q13[1];
    for n in interp_len..frame_length {
        x2[n + 1] = side(x1, x2, n, pred0_q13, pred1_q13);
    }
    pred_prev_q13[0] = pred_q13[0] as i16;
    pred_prev_q13[1] = pred_q13[1] as i16;

    // Convert to left/right signals.
    for n in 0..frame_length {
        let sum = i32::from(x1[n + 1]) + i32::from(x2[n + 1]);
        let diff = i32::from(x1[n + 1]) - i32::from(x2[n + 1]);
        x1[n + 1] = sat16(sum) as i16;
        x2[n + 1] = sat16(diff) as i16;
    }
}
