//! Ported from libopus 1.5.2 silk/PLC.c and silk/PLC.h (BSD-3-Clause, see
//! LICENSE-XIPH), without the deep PLC.

use super::lpc::{bwexpander, lpc_analysis_filter, lpc_inverse_pred_gain};
use super::macros::{
    add_sat32, clz32, div32, inverse32_varq, lshift, lshift_sat32, rand, rshift, rshift_round, sat16, smlawb,
    smulbb, smulwb, smulww, sqrt_approx, sum_sqr_shift,
};
use super::{ChannelState, DecCtrl, FrameScratch, LTP_ORDER, MAX_LPC_ORDER, MAX_NB_SUBFR, TYPE_VOICED};

const NB_ATT: usize = 2;
const HARM_ATT_Q15: [i32; NB_ATT] = [32440, 31130]; // 0.99, 0.95
const PLC_RAND_ATTENUATE_V_Q15: [i32; NB_ATT] = [31130, 26214]; // 0.95, 0.8
const PLC_RAND_ATTENUATE_UV_Q15: [i32; NB_ATT] = [32440, 29491]; // 0.99, 0.9

/// `SILK_FIX_CONST( BWE_COEF, 16 )` with BWE_COEF = 0.99.
const BWE_COEF_Q16: i32 = 64881;
const V_PITCH_GAIN_START_MIN_Q14: i32 = 11469; // 0.7 in Q14
const V_PITCH_GAIN_START_MAX_Q14: i32 = 15565; // 0.95 in Q14
const MAX_PITCH_LAG_MS: i32 = 18;
const RAND_BUF_SIZE: i32 = 128;
const RAND_BUF_MASK: i32 = RAND_BUF_SIZE - 1;
const LOG2_INV_LPC_GAIN_HIGH_THRES: i32 = 3; // 2^3 = 8 dB LPC gain
const LOG2_INV_LPC_GAIN_LOW_THRES: i32 = 8; // 2^8 = 24 dB LPC gain
const PITCH_DRIFT_FAC_Q16: i32 = 655; // 0.01 in Q16

/// `silk_PLC_Reset`.
pub(crate) fn plc_reset(ps: &mut ChannelState) {
    ps.plc.pitch_l_q8 = lshift(ps.frame_length, 8 - 1);
    ps.plc.prev_gain_q16 = [1 << 16, 1 << 16];
    ps.plc.subfr_length = 20;
    ps.plc.nb_subfr = 2;
}

/// `silk_PLC`: update the concealment state after a good frame, or
/// conceal a lost one into `frame`.
pub(crate) fn plc(
    ps: &mut ChannelState,
    ctrl: &mut DecCtrl,
    frame: &mut [i16],
    lost: bool,
    fs: &mut FrameScratch,
) {
    if ps.fs_khz != ps.plc.fs_khz {
        plc_reset(ps);
        ps.plc.fs_khz = ps.fs_khz;
    }
    if lost {
        // Generate signal.
        plc_conceal(ps, ctrl, frame, fs);
        ps.loss_cnt += 1;
    } else {
        // Update state.
        plc_update(ps, ctrl);
    }
}

/// `silk_PLC_update`.
fn plc_update(ps: &mut ChannelState, ctrl: &DecCtrl) {
    let nb_subfr = ps.nb_subfr as usize;
    let plc = &mut ps.plc;

    // Update parameters used in case of packet loss.
    ps.prev_signal_type = ps.indices.signal_type;
    let mut ltp_gain_q14: i32 = 0;
    if ps.indices.signal_type == TYPE_VOICED {
        // Find the parameters for the last subframe which contains a pitch pulse.
        let mut j = 0usize;
        while (j as i32) * ps.subfr_length < ctrl.pitch_l[nb_subfr - 1] {
            if j == nb_subfr {
                break;
            }
            let base = (nb_subfr - 1 - j) * LTP_ORDER;
            let mut temp_ltp_gain_q14 = 0i32;
            for i in 0..LTP_ORDER {
                temp_ltp_gain_q14 += i32::from(ctrl.ltp_coef_q14[base + i]);
            }
            if temp_ltp_gain_q14 > ltp_gain_q14 {
                ltp_gain_q14 = temp_ltp_gain_q14;
                plc.ltp_coef_q14.copy_from_slice(&ctrl.ltp_coef_q14[base..base + LTP_ORDER]);
                plc.pitch_l_q8 = lshift(ctrl.pitch_l[nb_subfr - 1 - j], 8);
            }
            j += 1;
        }

        plc.ltp_coef_q14 = [0; LTP_ORDER];
        plc.ltp_coef_q14[LTP_ORDER / 2] = ltp_gain_q14 as i16;

        // Limit LT coefs.
        if ltp_gain_q14 < V_PITCH_GAIN_START_MIN_Q14 {
            let tmp = lshift(V_PITCH_GAIN_START_MIN_Q14, 10);
            let scale_q10 = div32(tmp, ltp_gain_q14.max(1));
            for c in &mut plc.ltp_coef_q14 {
                *c = rshift(smulbb(i32::from(*c), scale_q10), 10) as i16;
            }
        } else if ltp_gain_q14 > V_PITCH_GAIN_START_MAX_Q14 {
            let tmp = lshift(V_PITCH_GAIN_START_MAX_Q14, 14);
            let scale_q14 = div32(tmp, ltp_gain_q14.max(1));
            for c in &mut plc.ltp_coef_q14 {
                *c = rshift(smulbb(i32::from(*c), scale_q14), 14) as i16;
            }
        }
    } else {
        plc.pitch_l_q8 = lshift(smulbb(ps.fs_khz, 18), 8);
        plc.ltp_coef_q14 = [0; LTP_ORDER];
    }

    // Save LPC coeficients.
    let order = ps.lpc_order as usize;
    plc.prev_lpc_q12[..order].copy_from_slice(&ctrl.pred_coef_q12[1][..order]);
    plc.prev_ltp_scale_q14 = ctrl.ltp_scale_q14 as i16;

    // Save last two gains.
    plc.prev_gain_q16.copy_from_slice(&ctrl.gains_q16[nb_subfr - 2..nb_subfr]);

    plc.subfr_length = ps.subfr_length;
    plc.nb_subfr = ps.nb_subfr;
}

/// `silk_PLC_energy`: energies of the last two subframes' excitation.
fn plc_energy(
    exc_buf: &mut [i16],
    exc_q14: &[i32],
    prev_gain_q10: &[i32; 2],
    subfr_length: usize,
    nb_subfr: usize,
) -> (i32, i32, i32, i32) {
    // Find random noise component. Scale previous excitation signal.
    for k in 0..2 {
        for i in 0..subfr_length {
            exc_buf[k * subfr_length + i] =
                sat16(rshift(smulww(exc_q14[i + (k + nb_subfr - 2) * subfr_length], prev_gain_q10[k]), 8))
                    as i16;
        }
    }
    // Find the subframe with lowest energy of the last two and use that as
    // random noise generator.
    let (energy1, shift1) = sum_sqr_shift(&exc_buf[..subfr_length]);
    let (energy2, shift2) = sum_sqr_shift(&exc_buf[subfr_length..2 * subfr_length]);
    (energy1, shift1, energy2, shift2)
}

/// `silk_PLC_conceal`.
fn plc_conceal(ps: &mut ChannelState, ctrl: &mut DecCtrl, frame: &mut [i16], fs: &mut FrameScratch) {
    let FrameScratch { s_ltp_q15: s_ltp_q14, s_ltp, exc_buf, .. } = fs;
    let ltp_mem = ps.ltp_mem_length as usize;
    let frame_length = ps.frame_length as usize;
    let subfr_length = ps.subfr_length as usize;
    let nb_subfr = ps.nb_subfr as usize;
    let order = ps.lpc_order as usize;
    let max_lag = (ltp_mem - order - LTP_ORDER / 2 - 1) as i32;

    let prev_gain_q10 = [ps.plc.prev_gain_q16[0] >> 6, ps.plc.prev_gain_q16[1] >> 6];

    if ps.first_frame_after_reset != 0 {
        ps.plc.prev_lpc_q12 = [0; MAX_LPC_ORDER];
    }

    let (energy1, shift1, energy2, shift2) =
        plc_energy(exc_buf, &ps.exc_q14, &prev_gain_q10, subfr_length, nb_subfr);

    let rand_off = if rshift(energy1, shift2) < rshift(energy2, shift1) {
        // First sub-frame has lowest energy.
        0.max((ps.plc.nb_subfr - 1) * ps.plc.subfr_length - RAND_BUF_SIZE)
    } else {
        // Second sub-frame has lowest energy.
        0.max(ps.plc.nb_subfr * ps.plc.subfr_length - RAND_BUF_SIZE)
    };
    let rand_off = (rand_off as usize).min(ps.exc_q14.len() - RAND_BUF_SIZE as usize);

    // Set up gain to random noise component.
    let mut rand_scale_q14 = ps.plc.rand_scale_q14;

    // Set up attenuation gains.
    let att = (ps.loss_cnt.max(0) as usize).min(NB_ATT - 1);
    let harm_gain_q15 = HARM_ATT_Q15[att];
    let mut rand_gain_q15 = if ps.prev_signal_type == TYPE_VOICED {
        PLC_RAND_ATTENUATE_V_Q15[att]
    } else {
        PLC_RAND_ATTENUATE_UV_Q15[att]
    };

    // LPC concealment. Apply BWE to previous LPC.
    bwexpander(&mut ps.plc.prev_lpc_q12[..order], BWE_COEF_Q16);

    // Preload LPC coeficients to array on stack.
    let a_q12 = ps.plc.prev_lpc_q12;

    // First lost frame.
    if ps.loss_cnt == 0 {
        rand_scale_q14 = 1 << 14;

        // Reduce random noise gain for voiced frames.
        if ps.prev_signal_type == TYPE_VOICED {
            for &b in &ps.plc.ltp_coef_q14 {
                rand_scale_q14 = rand_scale_q14.wrapping_sub(b);
            }
            rand_scale_q14 = rand_scale_q14.max(3277); // 0.2
            rand_scale_q14 =
                rshift(smulbb(i32::from(rand_scale_q14), i32::from(ps.plc.prev_ltp_scale_q14)), 14) as i16;
        } else {
            // Reduce random noise for unvoiced frames with high LPC gain.
            let inv_gain_q30 = lpc_inverse_pred_gain(&ps.plc.prev_lpc_q12[..order]);
            let mut down_scale_q30 = ((1i32 << 30) >> LOG2_INV_LPC_GAIN_HIGH_THRES).min(inv_gain_q30);
            down_scale_q30 = ((1i32 << 30) >> LOG2_INV_LPC_GAIN_LOW_THRES).max(down_scale_q30);
            down_scale_q30 = lshift(down_scale_q30, LOG2_INV_LPC_GAIN_HIGH_THRES);
            rand_gain_q15 = rshift(smulwb(down_scale_q30, rand_gain_q15), 14);
        }
    }

    let mut rand_seed = ps.plc.rand_seed;
    let mut lag = rshift_round(ps.plc.pitch_l_q8, 8).clamp(1, max_lag);
    let mut s_ltp_buf_idx = ltp_mem;

    // Rewhiten LTP state.
    let idx = ltp_mem - lag as usize - order - LTP_ORDER / 2;
    lpc_analysis_filter(&mut s_ltp[idx..ltp_mem], &ps.out_buf[idx..ltp_mem], &a_q12[..order]);

    // Scale LTP state.
    let mut inv_gain_q30 = inverse32_varq(ps.plc.prev_gain_q16[1], 46);
    inv_gain_q30 = inv_gain_q30.min(i32::MAX >> 1);
    for i in idx + order..ltp_mem {
        s_ltp_q14[i] = smulwb(inv_gain_q30, i32::from(s_ltp[i]));
    }

    // LTP synthesis filtering.
    for _ in 0..nb_subfr {
        // Set up pointer.
        let mut p = s_ltp_buf_idx - lag as usize + LTP_ORDER / 2;
        let b_q14 = ps.plc.ltp_coef_q14;
        for _ in 0..subfr_length {
            // Avoids introducing a bias because silk_SMLAWB() always rounds to -inf.
            let mut ltp_pred_q12 = 2;
            ltp_pred_q12 = smlawb(ltp_pred_q12, s_ltp_q14[p], i32::from(b_q14[0]));
            ltp_pred_q12 = smlawb(ltp_pred_q12, s_ltp_q14[p - 1], i32::from(b_q14[1]));
            ltp_pred_q12 = smlawb(ltp_pred_q12, s_ltp_q14[p - 2], i32::from(b_q14[2]));
            ltp_pred_q12 = smlawb(ltp_pred_q12, s_ltp_q14[p - 3], i32::from(b_q14[3]));
            ltp_pred_q12 = smlawb(ltp_pred_q12, s_ltp_q14[p - 4], i32::from(b_q14[4]));
            p += 1;

            // Generate LPC excitation.
            rand_seed = rand(rand_seed);
            let ridx = ((rand_seed >> 25) & RAND_BUF_MASK) as usize;
            s_ltp_q14[s_ltp_buf_idx] =
                lshift(smlawb(ltp_pred_q12, ps.exc_q14[rand_off + ridx], i32::from(rand_scale_q14)), 2);
            s_ltp_buf_idx += 1;
        }

        // Gradually reduce LTP gain.
        for b in &mut ps.plc.ltp_coef_q14 {
            *b = rshift(smulbb(harm_gain_q15, i32::from(*b)), 15) as i16;
        }
        // Gradually reduce excitation gain.
        rand_scale_q14 = rshift(smulbb(i32::from(rand_scale_q14), rand_gain_q15), 15) as i16;

        // Slowly increase pitch lag.
        ps.plc.pitch_l_q8 = smlawb(ps.plc.pitch_l_q8, ps.plc.pitch_l_q8, PITCH_DRIFT_FAC_Q16);
        ps.plc.pitch_l_q8 = ps.plc.pitch_l_q8.min(lshift(smulbb(MAX_PITCH_LAG_MS, ps.fs_khz), 8));
        lag = rshift_round(ps.plc.pitch_l_q8, 8).clamp(1, max_lag);
    }

    // LPC synthesis filtering.
    let base = ltp_mem - MAX_LPC_ORDER;
    let s_lpc_q14 = &mut s_ltp_q14[base..base + MAX_LPC_ORDER + frame_length];

    // Copy LPC state.
    s_lpc_q14[..MAX_LPC_ORDER].copy_from_slice(&ps.s_lpc_q14_buf);

    for i in 0..frame_length {
        // Avoids introducing a bias because silk_SMLAWB() always rounds to -inf.
        let mut lpc_pred_q10 = (order >> 1) as i32;
        for j in 0..order {
            lpc_pred_q10 = smlawb(lpc_pred_q10, s_lpc_q14[MAX_LPC_ORDER + i - j - 1], i32::from(a_q12[j]));
        }
        // Add prediction to LPC excitation.
        s_lpc_q14[MAX_LPC_ORDER + i] = add_sat32(s_lpc_q14[MAX_LPC_ORDER + i], lshift_sat32(lpc_pred_q10, 4));
        // Scale with gain.
        frame[i] = sat16(rshift_round(smulww(s_lpc_q14[MAX_LPC_ORDER + i], prev_gain_q10[1]), 8)) as i16;
    }

    // Save LPC state.
    ps.s_lpc_q14_buf.copy_from_slice(&s_lpc_q14[frame_length..frame_length + MAX_LPC_ORDER]);

    // Update states.
    ps.plc.rand_seed = rand_seed;
    ps.plc.rand_scale_q14 = rand_scale_q14;
    ctrl.pitch_l = [lag; MAX_NB_SUBFR];
}

/// `silk_PLC_glue_frames`: smooth the transition from concealed to good
/// frames.
pub(crate) fn plc_glue_frames(ps: &mut ChannelState, frame: &mut [i16], length: usize) {
    let plc = &mut ps.plc;
    if ps.loss_cnt != 0 {
        // Calculate energy in concealed residual.
        (plc.conc_energy, plc.conc_energy_shift) = sum_sqr_shift(&frame[..length]);
        plc.last_frame_lost = 1;
    } else {
        if plc.last_frame_lost != 0 {
            // Calculate residual in decoded signal if last frame was lost.
            let (mut energy, energy_shift) = sum_sqr_shift(&frame[..length]);

            // Normalize energies.
            if energy_shift > plc.conc_energy_shift {
                plc.conc_energy = rshift(plc.conc_energy, energy_shift - plc.conc_energy_shift);
            } else if energy_shift < plc.conc_energy_shift {
                energy = rshift(energy, plc.conc_energy_shift - energy_shift);
            }

            // Fade in the energy difference.
            if energy > plc.conc_energy {
                let lz = clz32(plc.conc_energy) - 1;
                plc.conc_energy = lshift(plc.conc_energy, lz);
                energy = rshift(energy, (24 - lz).max(0));

                let frac_q24 = div32(plc.conc_energy, energy.max(1));

                let mut gain_q16 = lshift(sqrt_approx(frac_q24), 4);
                let mut slope_q16 = div32((1i32 << 16) - gain_q16, length as i32);
                // Make slope 4x steeper to avoid missing onsets after DTX.
                slope_q16 = lshift(slope_q16, 2);

                for s in &mut frame[..length] {
                    *s = smulwb(gain_q16, i32::from(*s)) as i16;
                    gain_q16 = gain_q16.wrapping_add(slope_q16);
                    if gain_q16 > 1 << 16 {
                        break;
                    }
                }
            }
        }
        plc.last_frame_lost = 0;
    }
}
