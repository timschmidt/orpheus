//! Ported from libopus 1.5.2 silk/decode_parameters.c, silk/gain_quant.c
//! (`silk_gains_dequant`) and silk/decode_pitch.c (BSD-3-Clause, see
//! LICENSE-XIPH).

use super::lpc::bwexpander;
use super::macros::{limit, log2lin, lshift, mul, rshift, smulbb, smulwb};
use super::nlsf::{nlsf_decode, nlsf2a};
use super::tables::{
    CB_LAGS_STAGE2, CB_LAGS_STAGE2_10_MS, CB_LAGS_STAGE3, CB_LAGS_STAGE3_10_MS, LTP_SCALES_TABLE_Q14,
    ltp_vq_q7, nlsf_cb,
};
use super::{CODE_CONDITIONALLY, ChannelState, DecCtrl, LTP_ORDER, MAX_LPC_ORDER, MAX_NB_SUBFR, TYPE_VOICED};

const N_LEVELS_QGAIN: i32 = 64;
const MAX_DELTA_GAIN_QUANT: i32 = 36;
const MIN_DELTA_GAIN_QUANT: i32 = -4;
const MIN_QGAIN_DB: i32 = 2;
const MAX_QGAIN_DB: i32 = 88;
const OFFSET: i32 = (MIN_QGAIN_DB * 128) / 6 + 16 * 128;
const INV_SCALE_Q16: i32 = (65536 * (((MAX_QGAIN_DB - MIN_QGAIN_DB) * 128) / 6)) / (N_LEVELS_QGAIN - 1);
const BWE_AFTER_LOSS_Q16: i32 = 63570;
const PE_MIN_LAG_MS: i32 = 2;
const PE_MAX_LAG_MS: i32 = 18;

/// `silk_gains_dequant`: gains (Q16) from indices; `prev_ind` holds an
/// `opus_int8`.
pub(crate) fn gains_dequant(
    gain_q16: &mut [i32; MAX_NB_SUBFR],
    ind: &[i8; MAX_NB_SUBFR],
    prev_ind: &mut i32,
    conditional: bool,
    nb_subfr: usize,
) {
    for k in 0..nb_subfr {
        if k == 0 && !conditional {
            // Gain index is not allowed to go down more than 16 steps (~21.8 dB).
            *prev_ind = i32::from(ind[k]).max(*prev_ind - 16) as i8 as i32;
        } else {
            // Delta index.
            let ind_tmp = i32::from(ind[k]) + MIN_DELTA_GAIN_QUANT;
            // Accumulate deltas.
            let double_step_size_threshold = 2 * MAX_DELTA_GAIN_QUANT - N_LEVELS_QGAIN + *prev_ind;
            if ind_tmp > double_step_size_threshold {
                *prev_ind = (*prev_ind + lshift(ind_tmp, 1) - double_step_size_threshold) as i8 as i32;
            } else {
                *prev_ind = (*prev_ind + ind_tmp) as i8 as i32;
            }
        }
        *prev_ind = limit(*prev_ind, 0, N_LEVELS_QGAIN - 1);
        // Scale and convert to linear scale.
        gain_q16[k] = log2lin((smulwb(INV_SCALE_Q16, *prev_ind) + OFFSET).min(3967)); // 3967 = 31 in Q7
    }
}

/// `silk_decode_pitch`: per-subframe pitch lags.
fn decode_pitch(
    lag_index: i32,
    contour_index: i32,
    pitch_lags: &mut [i32; MAX_NB_SUBFR],
    fs_khz: i32,
    nb_subfr: usize,
) {
    let (lag_cb, cbk_size): (&[i8], usize) = if fs_khz == 8 {
        if nb_subfr == MAX_NB_SUBFR { (&CB_LAGS_STAGE2, 11) } else { (&CB_LAGS_STAGE2_10_MS, 3) }
    } else if nb_subfr == MAX_NB_SUBFR {
        (&CB_LAGS_STAGE3, 34)
    } else {
        (&CB_LAGS_STAGE3_10_MS, 12)
    };
    let min_lag = smulbb(PE_MIN_LAG_MS, fs_khz);
    let max_lag = smulbb(PE_MAX_LAG_MS, fs_khz);
    let lag = min_lag + lag_index;
    let ci = (contour_index.max(0) as usize).min(cbk_size - 1);
    for k in 0..nb_subfr {
        let d = i32::from(lag_cb.get(k * cbk_size + ci).copied().unwrap_or(0));
        pitch_lags[k] = limit(lag + d, min_lag, max_lag);
    }
}

/// `silk_decode_parameters`: dequantize the side information.
pub(crate) fn decode_parameters(ps: &mut ChannelState, ctrl: &mut DecCtrl, cond_coding: i32) {
    let nb_subfr = (ps.nb_subfr as usize).min(MAX_NB_SUBFR);
    let order = ps.lpc_order as usize;
    let mut p_nlsf_q15 = [0i16; MAX_LPC_ORDER];
    let mut p_nlsf0_q15 = [0i16; MAX_LPC_ORDER];

    // Dequant gains.
    gains_dequant(
        &mut ctrl.gains_q16,
        &ps.indices.gains_indices,
        &mut ps.last_gain_index,
        cond_coding == CODE_CONDITIONALLY,
        nb_subfr,
    );

    // Decode NLSFs.
    nlsf_decode(&mut p_nlsf_q15, &ps.indices.nlsf_indices, nlsf_cb(ps.nlsf_cb_sel));

    // Convert NLSF parameters to AR prediction filter coefficients.
    nlsf2a(&mut ctrl.pred_coef_q12[1], &p_nlsf_q15, order);

    // If just reset, e.g., because internal Fs changed, do not allow
    // interpolation; improves the case of packet loss in the first frame
    // after a switch.
    if ps.first_frame_after_reset == 1 {
        ps.indices.nlsf_interp_coef_q2 = 4;
    }

    if ps.indices.nlsf_interp_coef_q2 < 4 {
        // Calculation of the interpolated NLSF0 vector from the
        // interpolation factor, the previous NLSF1, and the current NLSF1.
        for i in 0..order {
            let prev = i32::from(ps.prev_nlsf_q15[i]);
            p_nlsf0_q15[i] = (prev
                + rshift(mul(ps.indices.nlsf_interp_coef_q2, i32::from(p_nlsf_q15[i]) - prev), 2))
                as i16;
        }
        // Convert NLSF parameters to AR prediction filter coefficients.
        nlsf2a(&mut ctrl.pred_coef_q12[0], &p_nlsf0_q15, order);
    } else {
        // Copy LPC coefficients for first half from second half.
        ctrl.pred_coef_q12[0] = ctrl.pred_coef_q12[1];
    }

    ps.prev_nlsf_q15[..order].copy_from_slice(&p_nlsf_q15[..order]);

    // After a packet loss do BWE of LPC coefs.
    if ps.loss_cnt != 0 {
        bwexpander(&mut ctrl.pred_coef_q12[0][..order], BWE_AFTER_LOSS_Q16);
        bwexpander(&mut ctrl.pred_coef_q12[1][..order], BWE_AFTER_LOSS_Q16);
    }

    if ps.indices.signal_type == TYPE_VOICED {
        // Decode pitch values.
        decode_pitch(ps.indices.lag_index, ps.indices.contour_index, &mut ctrl.pitch_l, ps.fs_khz, nb_subfr);

        // Decode codebook index.
        let cbk_q7 = ltp_vq_q7(ps.indices.per_index);
        for k in 0..nb_subfr {
            let ix = ps.indices.ltp_index[k].max(0) as usize;
            for i in 0..LTP_ORDER {
                let c = i32::from(cbk_q7.get(ix * LTP_ORDER + i).copied().unwrap_or(0));
                ctrl.ltp_coef_q14[k * LTP_ORDER + i] = lshift(c, 7) as i16;
            }
        }

        // Decode LTP scaling.
        let ix = ps.indices.ltp_scale_index.clamp(0, 2) as usize;
        ctrl.ltp_scale_q14 = i32::from(LTP_SCALES_TABLE_Q14[ix]);
    } else {
        ctrl.pitch_l[..nb_subfr].fill(0);
        ctrl.ltp_coef_q14[..LTP_ORDER * nb_subfr].fill(0);
        ps.indices.per_index = 0;
        ctrl.ltp_scale_q14 = 0;
    }
}
