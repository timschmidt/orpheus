//! Ported from libopus 1.5.2 silk/NLSF_unpack.c, silk/NLSF_decode.c,
//! silk/NLSF_stabilize.c, silk/sort.c
//! (`silk_insertion_sort_increasing_all_values_int16`) and silk/NLSF2A.c
//! (BSD-3-Clause, see LICENSE-XIPH).

use super::MAX_LPC_ORDER;
use super::lpc::{bwexpander_32, lpc_fit, lpc_inverse_pred_gain};
use super::macros::{
    add_lshift32, div32, limit, lshift, rshift, rshift_round, rshift_round64, smlawb, smulbb, smull,
};
use super::tables::{LSF_COS_TAB_FIX_Q12, NlsfCb};

const NLSF_QUANT_MAX_AMPLITUDE: i32 = 4;
/// `SILK_FIX_CONST( NLSF_QUANT_LEVEL_ADJ, 10 )`.
const NLSF_QUANT_LEVEL_ADJ_Q10: i32 = 102;
const MAX_LOOPS: usize = 20;
const MAX_LPC_STABILIZE_ITERATIONS: i32 = 16;
const QA: i32 = 16;

/// `silk_NLSF_unpack`: entropy-table indices and predictor for a first
/// stage vector.
pub(crate) fn nlsf_unpack(
    ec_ix: &mut [i16; MAX_LPC_ORDER],
    pred_q8: &mut [u8; MAX_LPC_ORDER],
    cb: &NlsfCb,
    cb1_index: i32,
) {
    let order = cb.order as usize;
    let base = (cb1_index.clamp(0, cb.n_vectors - 1) as usize) * order / 2;
    for i in (0..order).step_by(2) {
        let entry = i32::from(cb.ec_sel.get(base + i / 2).copied().unwrap_or(0));
        ec_ix[i] = smulbb((entry >> 1) & 7, 2 * NLSF_QUANT_MAX_AMPLITUDE + 1) as i16;
        pred_q8[i] = cb.pred_q8.get(i + (entry & 1) as usize * (order - 1)).copied().unwrap_or(0);
        ec_ix[i + 1] = smulbb((entry >> 5) & 7, 2 * NLSF_QUANT_MAX_AMPLITUDE + 1) as i16;
        pred_q8[i + 1] =
            cb.pred_q8.get(i + ((entry >> 4) & 1) as usize * (order - 1) + 1).copied().unwrap_or(0);
    }
}

/// `silk_NLSF_residual_dequant`.
fn nlsf_residual_dequant(
    x_q10: &mut [i16; MAX_LPC_ORDER],
    indices: &[i8],
    pred_coef_q8: &[u8; MAX_LPC_ORDER],
    quant_step_size_q16: i32,
    order: usize,
) {
    let mut out_q10: i32 = 0;
    for i in (0..order).rev() {
        let pred_q10 = rshift(smulbb(out_q10, i32::from(pred_coef_q8[i])), 8);
        out_q10 = lshift(i32::from(indices[i]), 10);
        if out_q10 > 0 {
            out_q10 -= NLSF_QUANT_LEVEL_ADJ_Q10;
        } else if out_q10 < 0 {
            out_q10 += NLSF_QUANT_LEVEL_ADJ_Q10;
        }
        out_q10 = smlawb(pred_q10, out_q10, quant_step_size_q16);
        x_q10[i] = out_q10 as i16;
    }
}

/// `silk_NLSF_decode`: NLSF vector from the codebook path.
pub(crate) fn nlsf_decode(p_nlsf_q15: &mut [i16; MAX_LPC_ORDER], nlsf_indices: &[i8; 20], cb: &NlsfCb) {
    let order = cb.order as usize;
    let mut pred_q8 = [0u8; MAX_LPC_ORDER];
    let mut ec_ix = [0i16; MAX_LPC_ORDER];
    let mut res_q10 = [0i16; MAX_LPC_ORDER];
    let idx0 = i32::from(nlsf_indices[0]).clamp(0, cb.n_vectors - 1) as usize;

    // Unpack entropy table indices and predictor for current CB1 index.
    nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, idx0 as i32);

    // Predictive residual dequantizer.
    nlsf_residual_dequant(&mut res_q10, &nlsf_indices[1..], &pred_q8, cb.quant_step_size_q16, order);

    // Apply inverse square-rooted weights to first stage and add to output.
    let cb_element = &cb.cb1_nlsf_q8[idx0 * order..idx0 * order + order];
    let cb_wght_q9 = &cb.cb1_wght_q9[idx0 * order..idx0 * order + order];
    for i in 0..order {
        let nlsf_q15_tmp = add_lshift32(
            div32(lshift(i32::from(res_q10[i]), 14), i32::from(cb_wght_q9[i])),
            i32::from(cb_element[i]),
            7,
        );
        p_nlsf_q15[i] = limit(nlsf_q15_tmp, 0, 32767) as i16;
    }

    // NLSF stabilization.
    nlsf_stabilize(&mut p_nlsf_q15[..order], cb.delta_min_q15);
}

/// `silk_insertion_sort_increasing_all_values_int16`.
fn insertion_sort_increasing_all_values_int16(a: &mut [i16]) {
    for i in 1..a.len() {
        let value = a[i];
        let mut j = i;
        while j > 0 && value < a[j - 1] {
            a[j] = a[j - 1];
            j -= 1;
        }
        a[j] = value;
    }
}

/// `silk_NLSF_stabilize`: move NLSFs apart and away from the borders.
pub(crate) fn nlsf_stabilize(nlsf_q15: &mut [i16], ndelta_min_q15: &[i16]) {
    let l = nlsf_q15.len();
    let dm = |i: usize| i32::from(ndelta_min_q15[i]);
    let mut loops = 0;
    while loops < MAX_LOOPS {
        // Find smallest distance. First element:
        let mut min_diff_q15 = i32::from(nlsf_q15[0]) - dm(0);
        let mut big_i = 0usize;
        // Middle elements.
        for i in 1..l {
            let diff_q15 = i32::from(nlsf_q15[i]) - (i32::from(nlsf_q15[i - 1]) + dm(i));
            if diff_q15 < min_diff_q15 {
                min_diff_q15 = diff_q15;
                big_i = i;
            }
        }
        // Last element.
        let diff_q15 = (1 << 15) - (i32::from(nlsf_q15[l - 1]) + dm(l));
        if diff_q15 < min_diff_q15 {
            min_diff_q15 = diff_q15;
            big_i = l;
        }

        // Now check if the smallest distance is non-negative.
        if min_diff_q15 >= 0 {
            return;
        }

        if big_i == 0 {
            // Move away from lower limit.
            nlsf_q15[0] = ndelta_min_q15[0];
        } else if big_i == l {
            // Move away from higher limit.
            nlsf_q15[l - 1] = ((1 << 15) - dm(l)) as i16;
        } else {
            // Find the lower extreme for the location of the current center frequency.
            let mut min_center_q15 = 0i32;
            for k in 0..big_i {
                min_center_q15 += dm(k);
            }
            min_center_q15 += dm(big_i) >> 1;

            // Find the upper extreme for the location of the current center frequency.
            let mut max_center_q15 = 1i32 << 15;
            for k in (big_i + 1..=l).rev() {
                max_center_q15 -= dm(k);
            }
            max_center_q15 -= dm(big_i) >> 1;

            // Move apart, sorted by value, keeping the same center frequency.
            let center_freq_q15 = limit(
                rshift_round(i32::from(nlsf_q15[big_i - 1]) + i32::from(nlsf_q15[big_i]), 1),
                min_center_q15,
                max_center_q15,
            ) as i16;
            nlsf_q15[big_i - 1] = (i32::from(center_freq_q15) - (dm(big_i) >> 1)) as i16;
            nlsf_q15[big_i] = (i32::from(nlsf_q15[big_i - 1]) + dm(big_i)) as i16;
        }
        loops += 1;
    }

    // Safe and simple fall back method, which is less ideal than the above.
    if loops == MAX_LOOPS {
        insertion_sort_increasing_all_values_int16(nlsf_q15);
        // First NLSF should be no less than NDeltaMin[0].
        nlsf_q15[0] = i32::from(nlsf_q15[0]).max(dm(0)) as i16;
        // Keep delta_min distance between the NLSFs.
        for i in 1..l {
            let s = (i32::from(nlsf_q15[i - 1]) + dm(i)).clamp(-32768, 32767);
            nlsf_q15[i] = i32::from(nlsf_q15[i]).max(s) as i16;
        }
        // Last NLSF should be no higher than 1 - NDeltaMin[L].
        nlsf_q15[l - 1] = i32::from(nlsf_q15[l - 1]).min((1 << 15) - dm(l)) as i16;
        // Keep NDeltaMin distance between the NLSFs.
        for i in (0..l - 1).rev() {
            nlsf_q15[i] = i32::from(nlsf_q15[i]).min(i32::from(nlsf_q15[i + 1]) - dm(i + 1)) as i16;
        }
    }
}

/// `silk_NLSF2A_find_poly`.
fn nlsf2a_find_poly(out: &mut [i32; MAX_LPC_ORDER / 2 + 1], c_lsf: &[i32], dd: usize) {
    out[0] = 1 << QA;
    out[1] = c_lsf[0].wrapping_neg();
    for k in 1..dd {
        let ftmp = c_lsf[2 * k]; // QA
        out[k + 1] = lshift(out[k - 1], 1).wrapping_sub(rshift_round64(smull(ftmp, out[k]), QA) as i32);
        for n in (2..=k).rev() {
            out[n] = out[n]
                .wrapping_add(out[n - 2].wrapping_sub(rshift_round64(smull(ftmp, out[n - 1]), QA) as i32));
        }
        out[1] = out[1].wrapping_sub(ftmp);
    }
}

/// `silk_NLSF2A`: whitening filter coefficients (Q12) from NLSFs (Q15).
pub(crate) fn nlsf2a(a_q12: &mut [i16; MAX_LPC_ORDER], nlsf: &[i16; MAX_LPC_ORDER], d: usize) {
    // This ordering was found to maximize quality. It improves numerical
    // accuracy of silk_NLSF2A_find_poly() compared to "standard" ordering.
    const ORDERING16: [u8; 16] = [0, 15, 8, 7, 4, 11, 12, 3, 2, 13, 10, 5, 6, 9, 14, 1];
    const ORDERING10: [u8; 10] = [0, 9, 6, 3, 4, 5, 8, 1, 2, 7];
    let ordering: &[u8] = if d == 16 { &ORDERING16 } else { &ORDERING10 };
    let mut cos_lsf_qa = [0i32; MAX_LPC_ORDER];
    let mut p = [0i32; MAX_LPC_ORDER / 2 + 1];
    let mut q = [0i32; MAX_LPC_ORDER / 2 + 1];
    let mut a32_qa1 = [0i32; MAX_LPC_ORDER];

    // Convert LSFs to 2*cos(LSF), using piecewise linear curve from table.
    for k in 0..d {
        // Valid NLSFs are in 0..=32767; clamp so corrupt state cannot index
        // outside the table.
        let n = i32::from(nlsf[k]).max(0);
        // f_int on a scale 0-127 (rounded down).
        let f_int = n >> (15 - 7);
        // f_frac, range: 0..255.
        let f_frac = n - (f_int << (15 - 7));
        let cos_val = i32::from(LSF_COS_TAB_FIX_Q12[f_int as usize]); // Q12
        let delta = i32::from(LSF_COS_TAB_FIX_Q12[f_int as usize + 1]) - cos_val; // Q12, range 0..200
        // Linear interpolation.
        cos_lsf_qa[ordering[k] as usize] = rshift_round((cos_val << 8) + delta * f_frac, 20 - QA); // QA
    }

    let dd = d >> 1;

    // Generate even and odd polynomials using convolution.
    nlsf2a_find_poly(&mut p, &cos_lsf_qa[0..], dd);
    nlsf2a_find_poly(&mut q, &cos_lsf_qa[1..], dd);

    // Convert even and odd polynomials to opus_int32 Q12 filter coefs.
    for k in 0..dd {
        let ptmp = p[k + 1].wrapping_add(p[k]);
        let qtmp = q[k + 1].wrapping_sub(q[k]);
        // The Ptmp and Qtmp values at this stage need to fit in int32.
        a32_qa1[k] = qtmp.wrapping_neg().wrapping_sub(ptmp); // QA+1
        a32_qa1[d - k - 1] = qtmp.wrapping_sub(ptmp); // QA+1
    }

    // Convert int32 coefficients to Q12 int16 coefs.
    lpc_fit(&mut a_q12[..d], &mut a32_qa1[..d], 12, QA + 1);

    let mut i = 0;
    while lpc_inverse_pred_gain(&a_q12[..d]) == 0 && i < MAX_LPC_STABILIZE_ITERATIONS {
        // Prediction coefficients are (too close to) unstable; apply
        // bandwidth expansion on the unscaled coefficients, convert to Q12
        // and measure again.
        bwexpander_32(&mut a32_qa1[..d], 65536 - lshift(2, i));
        for k in 0..d {
            a_q12[k] = rshift_round(a32_qa1[k], QA + 1 - 12) as i16; // QA+1 -> Q12
        }
        i += 1;
    }
}
