//! Ported from libopus 1.5.2 silk/CNG.c (BSD-3-Clause, see LICENSE-XIPH).

use super::macros::{
    add_sat16, add_sat32, div32, lshift, lshift_sat32, rand, rshift_round, sat16, smlawb, smultt, smulwb,
    smulww, sqrt_approx, sub_lshift32,
};
use super::nlsf::nlsf2a;
use super::{ChannelState, DecCtrl, MAX_FRAME_LENGTH, MAX_LPC_ORDER, TYPE_NO_VOICE_ACTIVITY};

const CNG_BUF_MASK_MAX: i32 = 255; // 2^floor(log2(MAX_FRAME_LENGTH))-1
const CNG_GAIN_SMTH_Q16: i32 = 4634; // 0.25^(1/4)
const CNG_GAIN_SMTH_THRESHOLD_Q16: i32 = 46396; // -3 dB
const CNG_NLSF_SMTH_Q16: i32 = 16348; // 0.25

/// `silk_CNG_exc`: excitation for CNG LPC synthesis.
fn cng_exc(exc_q14: &mut [i32], exc_buf_q14: &[i32; MAX_FRAME_LENGTH], rand_seed: &mut i32) {
    let length = exc_q14.len() as i32;
    let mut exc_mask = CNG_BUF_MASK_MAX;
    while exc_mask > length {
        exc_mask >>= 1;
    }
    let mut seed = *rand_seed;
    for e in exc_q14.iter_mut() {
        seed = rand(seed);
        let idx = ((seed >> 24) & exc_mask) as usize;
        *e = exc_buf_q14[idx];
    }
    *rand_seed = seed;
}

/// `silk_CNG_Reset`.
pub(crate) fn cng_reset(ps: &mut ChannelState) {
    let order = (ps.lpc_order.max(0) as usize).min(MAX_LPC_ORDER);
    let nlsf_step_q15 = div32(i32::from(i16::MAX), ps.lpc_order + 1);
    let mut nlsf_acc_q15 = 0i32;
    for i in 0..order {
        nlsf_acc_q15 += nlsf_step_q15;
        ps.cng.smth_nlsf_q15[i] = nlsf_acc_q15 as i16;
    }
    ps.cng.smth_gain_q16 = 0;
    ps.cng.rand_seed = 3_176_576;
}

/// `silk_CNG`: update the CNG estimate, and add comfort noise to `frame`
/// when the packet was lost.
pub(crate) fn cng(
    ps: &mut ChannelState,
    ctrl: &DecCtrl,
    frame: &mut [i16],
    length: usize,
    cng_sig_q14: &mut [i32; MAX_FRAME_LENGTH + MAX_LPC_ORDER],
) {
    let order = ps.lpc_order as usize;
    if ps.fs_khz != ps.cng.fs_khz {
        // Reset state.
        cng_reset(ps);
        ps.cng.fs_khz = ps.fs_khz;
    }
    let nb_subfr = ps.nb_subfr as usize;
    let subfr_length = ps.subfr_length as usize;
    let cng = &mut ps.cng;

    if ps.loss_cnt == 0 && ps.prev_signal_type == TYPE_NO_VOICE_ACTIVITY {
        // Update CNG parameters. Smoothing of LSF's.
        for i in 0..order {
            let d = i32::from(ps.prev_nlsf_q15[i]) - i32::from(cng.smth_nlsf_q15[i]);
            cng.smth_nlsf_q15[i] = cng.smth_nlsf_q15[i].wrapping_add(smulwb(d, CNG_NLSF_SMTH_Q16) as i16);
        }
        // Find the subframe with the highest gain.
        let mut max_gain_q16 = 0;
        let mut subfr = 0;
        for i in 0..nb_subfr {
            if ctrl.gains_q16[i] > max_gain_q16 {
                max_gain_q16 = ctrl.gains_q16[i];
                subfr = i;
            }
        }
        // Update CNG excitation buffer with excitation from this subframe.
        cng.exc_buf_q14.copy_within(0..(nb_subfr - 1) * subfr_length, subfr_length);
        cng.exc_buf_q14[..subfr_length]
            .copy_from_slice(&ps.exc_q14[subfr * subfr_length..(subfr + 1) * subfr_length]);

        // Smooth gains.
        for i in 0..nb_subfr {
            cng.smth_gain_q16 = cng
                .smth_gain_q16
                .wrapping_add(smulwb(ctrl.gains_q16[i].wrapping_sub(cng.smth_gain_q16), CNG_GAIN_SMTH_Q16));
            // If the smoothed gain is 3 dB greater than this subframe's
            // gain, use this subframe's gain to adapt faster.
            if smulww(cng.smth_gain_q16, CNG_GAIN_SMTH_THRESHOLD_Q16) > ctrl.gains_q16[i] {
                cng.smth_gain_q16 = ctrl.gains_q16[i];
            }
        }
    }

    // Add CNG when packet is lost or during DTX.
    if ps.loss_cnt != 0 {
        // Generate CNG excitation.
        let mut gain_q16 = smulww(i32::from(ps.plc.rand_scale_q14), ps.plc.prev_gain_q16[1]);
        if gain_q16 >= (1 << 21) || cng.smth_gain_q16 > (1 << 23) {
            gain_q16 = smultt(gain_q16, gain_q16);
            gain_q16 = sub_lshift32(smultt(cng.smth_gain_q16, cng.smth_gain_q16), gain_q16, 5);
            gain_q16 = lshift(sqrt_approx(gain_q16), 16);
        } else {
            gain_q16 = smulww(gain_q16, gain_q16);
            gain_q16 = sub_lshift32(smulww(cng.smth_gain_q16, cng.smth_gain_q16), gain_q16, 5);
            gain_q16 = lshift(sqrt_approx(gain_q16), 8);
        }
        let gain_q10 = gain_q16 >> 6;

        cng_exc(
            &mut cng_sig_q14[MAX_LPC_ORDER..MAX_LPC_ORDER + length],
            &cng.exc_buf_q14,
            &mut cng.rand_seed,
        );

        // Convert CNG NLSF to filter representation.
        let mut a_q12 = [0i16; MAX_LPC_ORDER];
        nlsf2a(&mut a_q12, &cng.smth_nlsf_q15, order);

        // Generate CNG signal, by synthesis filtering.
        cng_sig_q14[..MAX_LPC_ORDER].copy_from_slice(&cng.synth_state);
        for i in 0..length {
            // Avoids introducing a bias because silk_SMLAWB() always rounds to -inf.
            let mut lpc_pred_q10 = (order >> 1) as i32;
            for j in 0..order {
                lpc_pred_q10 =
                    smlawb(lpc_pred_q10, cng_sig_q14[MAX_LPC_ORDER + i - j - 1], i32::from(a_q12[j]));
            }
            // Update states.
            cng_sig_q14[MAX_LPC_ORDER + i] =
                add_sat32(cng_sig_q14[MAX_LPC_ORDER + i], lshift_sat32(lpc_pred_q10, 4));
            // Scale with gain and add to input signal.
            frame[i] =
                add_sat16(frame[i], sat16(rshift_round(smulww(cng_sig_q14[MAX_LPC_ORDER + i], gain_q10), 8)));
        }
        cng.synth_state.copy_from_slice(&cng_sig_q14[length..length + MAX_LPC_ORDER]);
    } else {
        cng.synth_state[..order].fill(0);
    }
}
