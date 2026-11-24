//! Ported from libopus 1.5.2 silk/decode_core.c (BSD-3-Clause, see
//! LICENSE-XIPH).

use super::lpc::lpc_analysis_filter;
use super::macros::{
    add_lshift32, add_sat32, div32_varq, inverse32_varq, lshift, lshift_sat32, rand, rshift_round, sat16,
    smlawb, smulwb, smulww,
};
use super::tables::QUANTIZATION_OFFSETS_Q10;
use super::{ChannelState, DecCtrl, FrameScratch, LTP_ORDER, MAX_LPC_ORDER, MAX_NB_SUBFR, TYPE_VOICED};

const QUANT_LEVEL_ADJUST_Q10: i32 = 80;

/// Short-term (LPC) synthesis of one subframe: `s_lpc[MAX_LPC_ORDER..]`
/// gets `pres + prediction`, `xq` the gain-scaled output.
#[inline]
fn lpc_synthesis(
    s_lpc: &mut [i32],
    pres: &[i32],
    a_q12: &[i16; MAX_LPC_ORDER],
    order: usize,
    gain_q10: i32,
    xq: &mut [i16],
) {
    for i in 0..xq.len() {
        // Avoids introducing a bias because silk_SMLAWB() always rounds to -inf.
        let mut lpc_pred_q10 = (order >> 1) as i32;
        for j in 0..order {
            lpc_pred_q10 = smlawb(lpc_pred_q10, s_lpc[MAX_LPC_ORDER + i - j - 1], i32::from(a_q12[j]));
        }
        // Add prediction to LPC excitation.
        s_lpc[MAX_LPC_ORDER + i] = add_sat32(pres[i], lshift_sat32(lpc_pred_q10, 4));
        // Scale with gain.
        xq[i] = sat16(rshift_round(smulww(s_lpc[MAX_LPC_ORDER + i], gain_q10), 8)) as i16;
    }
}

/// `silk_decode_core`: inverse NSQ (LTP + LPC synthesis) into `xq`.
pub(crate) fn decode_core(ps: &mut ChannelState, ctrl: &mut DecCtrl, xq: &mut [i16], fs: &mut FrameScratch) {
    let FrameScratch { s_ltp_q15, res_q14, s_lpc_q14, pulses, s_ltp, .. } = fs;
    let ltp_mem = ps.ltp_mem_length as usize;
    let frame_length = ps.frame_length as usize;
    let subfr_length = ps.subfr_length as usize;
    let order = ps.lpc_order as usize;
    let nb_subfr = (ps.nb_subfr as usize).min(MAX_NB_SUBFR);
    // Largest lag that keeps the rewhitening start index positive (every
    // valid lag is below it; corrupt state is clamped instead of indexing
    // out of bounds).
    let max_lag = (ltp_mem - order - LTP_ORDER / 2 - 1) as i32;

    let st = ps.indices.signal_type.clamp(0, 2) as usize;
    let qot = ps.indices.quant_offset_type.clamp(0, 1) as usize;
    let offset_q10 = i32::from(QUANTIZATION_OFFSETS_Q10[st >> 1][qot]);

    let nlsf_interpolation_flag = ps.indices.nlsf_interp_coef_q2 < 1 << 2;

    // Decode excitation.
    let mut rand_seed = ps.indices.seed;
    for i in 0..frame_length {
        rand_seed = rand(rand_seed);
        let mut e = lshift(i32::from(pulses[i]), 14);
        if e > 0 {
            e -= QUANT_LEVEL_ADJUST_Q10 << 4;
        } else if e < 0 {
            e += QUANT_LEVEL_ADJUST_Q10 << 4;
        }
        e += offset_q10 << 4;
        if rand_seed < 0 {
            e = -e;
        }
        ps.exc_q14[i] = e;
        rand_seed = rand_seed.wrapping_add(i32::from(pulses[i]));
    }

    // Copy LPC state.
    s_lpc_q14[..MAX_LPC_ORDER].copy_from_slice(&ps.s_lpc_q14_buf);

    let mut pexc = 0usize;
    let mut s_ltp_buf_idx = ltp_mem;
    let mut lag = 0i32;

    // Loop over subframes.
    for k in 0..nb_subfr {
        let a_q12 = ctrl.pred_coef_q12[k >> 1];
        let b_off = k * LTP_ORDER;
        let mut signal_type = ps.indices.signal_type;

        let gain_q10 = ctrl.gains_q16[k] >> 6;
        let mut inv_gain_q31 = inverse32_varq(ctrl.gains_q16[k], 47);

        // Calculate gain adjustment factor.
        let gain_adj_q16 = if ctrl.gains_q16[k] != ps.prev_gain_q16 {
            let g = div32_varq(ps.prev_gain_q16, ctrl.gains_q16[k], 16);
            // Scale short term state.
            for v in &mut s_lpc_q14[..MAX_LPC_ORDER] {
                *v = smulww(g, *v);
            }
            g
        } else {
            1 << 16
        };

        // Save inv_gain.
        ps.prev_gain_q16 = ctrl.gains_q16[k];

        // Avoid abrupt transition from voiced PLC to unvoiced normal decoding.
        if ps.loss_cnt != 0
            && ps.prev_signal_type == TYPE_VOICED
            && ps.indices.signal_type != TYPE_VOICED
            && k < MAX_NB_SUBFR / 2
        {
            ctrl.ltp_coef_q14[b_off..b_off + LTP_ORDER].fill(0);
            ctrl.ltp_coef_q14[b_off + LTP_ORDER / 2] = 4096; // SILK_FIX_CONST( 0.25, 14 )
            signal_type = TYPE_VOICED;
            ctrl.pitch_l[k] = ps.lag_prev;
        }

        if signal_type == TYPE_VOICED {
            // Voiced.
            lag = ctrl.pitch_l[k].clamp(1, max_lag);

            // Re-whitening.
            if k == 0 || (k == 2 && nlsf_interpolation_flag) {
                // Rewhiten with new A coefs.
                let start_idx = ltp_mem - lag as usize - order - LTP_ORDER / 2;
                if k == 2 {
                    ps.out_buf[ltp_mem..ltp_mem + 2 * subfr_length].copy_from_slice(&xq[..2 * subfr_length]);
                }
                let in_start = start_idx + k * subfr_length;
                lpc_analysis_filter(
                    &mut s_ltp[start_idx..ltp_mem],
                    &ps.out_buf[in_start..in_start + ltp_mem - start_idx],
                    &a_q12[..order],
                );

                // After rewhitening the LTP state is unscaled.
                if k == 0 {
                    // Do LTP downscaling to reduce inter-packet dependency.
                    inv_gain_q31 = lshift(smulwb(inv_gain_q31, ctrl.ltp_scale_q14), 2);
                }
                for i in 0..(lag as usize + LTP_ORDER / 2) {
                    s_ltp_q15[s_ltp_buf_idx - i - 1] =
                        smulwb(inv_gain_q31, i32::from(s_ltp[ltp_mem - i - 1]));
                }
            } else if gain_adj_q16 != 1 << 16 {
                // Update LTP state when gain changes.
                for i in 0..(lag as usize + LTP_ORDER / 2) {
                    let idx = s_ltp_buf_idx - i - 1;
                    s_ltp_q15[idx] = smulww(gain_adj_q16, s_ltp_q15[idx]);
                }
            }
        }

        let pexc_sub = &ps.exc_q14[pexc..pexc + subfr_length];
        if signal_type == TYPE_VOICED {
            // Long-term prediction.
            let b_q14 = &ctrl.ltp_coef_q14[b_off..b_off + LTP_ORDER];
            let mut p = s_ltp_buf_idx - lag as usize + LTP_ORDER / 2;
            for i in 0..subfr_length {
                // Avoids introducing a bias because silk_SMLAWB() always rounds to -inf.
                let mut ltp_pred_q13 = 2;
                ltp_pred_q13 = smlawb(ltp_pred_q13, s_ltp_q15[p], i32::from(b_q14[0]));
                ltp_pred_q13 = smlawb(ltp_pred_q13, s_ltp_q15[p - 1], i32::from(b_q14[1]));
                ltp_pred_q13 = smlawb(ltp_pred_q13, s_ltp_q15[p - 2], i32::from(b_q14[2]));
                ltp_pred_q13 = smlawb(ltp_pred_q13, s_ltp_q15[p - 3], i32::from(b_q14[3]));
                ltp_pred_q13 = smlawb(ltp_pred_q13, s_ltp_q15[p - 4], i32::from(b_q14[4]));
                p += 1;

                // Generate LPC excitation.
                res_q14[i] = add_lshift32(pexc_sub[i], ltp_pred_q13, 1);

                // Update states.
                s_ltp_q15[s_ltp_buf_idx] = lshift(res_q14[i], 1);
                s_ltp_buf_idx += 1;
            }
        } else {
            res_q14[..subfr_length].copy_from_slice(pexc_sub);
        }

        let xq_sub = &mut xq[k * subfr_length..(k + 1) * subfr_length];
        lpc_synthesis(s_lpc_q14, &res_q14[..subfr_length], &a_q12, order, gain_q10, xq_sub);

        // Update LPC filter state.
        s_lpc_q14.copy_within(subfr_length..subfr_length + MAX_LPC_ORDER, 0);
        pexc += subfr_length;
    }

    // Save LPC state.
    ps.s_lpc_q14_buf.copy_from_slice(&s_lpc_q14[..MAX_LPC_ORDER]);
}
