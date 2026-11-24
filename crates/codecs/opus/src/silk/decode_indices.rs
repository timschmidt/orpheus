//! Ported from libopus 1.5.2 silk/decode_indices.c (BSD-3-Clause, see
//! LICENSE-XIPH).

use super::tables::{
    DELTA_GAIN_ICDF, GAIN_ICDF, LTP_PER_INDEX_ICDF, LTPSCALE_ICDF, NLSF_EXT_ICDF,
    NLSF_INTERPOLATION_FACTOR_ICDF, PITCH_DELTA_ICDF, PITCH_LAG_ICDF, TYPE_OFFSET_NO_VAD_ICDF,
    TYPE_OFFSET_VAD_ICDF, UNIFORM4_ICDF, UNIFORM8_ICDF, icdf_by_sel, ltp_gain_icdf, nlsf_cb,
};
use super::{
    CODE_CONDITIONALLY, CODE_INDEPENDENTLY, ChannelState, MAX_FRAMES_PER_PACKET, MAX_LPC_ORDER, MAX_NB_SUBFR,
    TYPE_VOICED, macros::lshift, nlsf::nlsf_unpack,
};
use crate::entdec::EcDec;

const NLSF_QUANT_MAX_AMPLITUDE: i32 = 4;

/// `silk_decode_indices`: side information of one frame.
pub(crate) fn decode_indices(
    ps: &mut ChannelState,
    dec: &mut EcDec<'_>,
    frame_index: i32,
    decode_lbrr: bool,
    cond_coding: i32,
) {
    // Decode signal type and quantizer offset.
    let fi = (frame_index.max(0) as usize).min(MAX_FRAMES_PER_PACKET - 1);
    let ix = if decode_lbrr || ps.vad_flags[fi] != 0 {
        dec.icdf(&TYPE_OFFSET_VAD_ICDF, 8) + 2
    } else {
        dec.icdf(&TYPE_OFFSET_NO_VAD_ICDF, 8)
    };
    ps.indices.signal_type = ix >> 1;
    ps.indices.quant_offset_type = ix & 1;
    let st = ps.indices.signal_type.clamp(0, 2) as usize;

    // Decode gains. First subframe:
    if cond_coding == CODE_CONDITIONALLY {
        ps.indices.gains_indices[0] = dec.icdf(&DELTA_GAIN_ICDF, 8) as i8;
    } else {
        // Independent coding, in two stages: MSB bits followed by 3 LSBs.
        ps.indices.gains_indices[0] = lshift(dec.icdf(&GAIN_ICDF[st * 8..st * 8 + 8], 8), 3) as i8;
        ps.indices.gains_indices[0] =
            ps.indices.gains_indices[0].wrapping_add(dec.icdf(&UNIFORM8_ICDF, 8) as i8);
    }
    // Remaining subframes.
    let nb_subfr = (ps.nb_subfr as usize).min(MAX_NB_SUBFR);
    for i in 1..nb_subfr {
        ps.indices.gains_indices[i] = dec.icdf(&DELTA_GAIN_ICDF, 8) as i8;
    }

    // Decode LSF indices.
    let cb = nlsf_cb(ps.nlsf_cb_sel);
    let off = (st >> 1) * cb.n_vectors as usize;
    ps.indices.nlsf_indices[0] = dec.icdf(&cb.cb1_icdf[off..], 8) as i8;
    let mut ec_ix = [0i16; MAX_LPC_ORDER];
    let mut pred_q8 = [0u8; MAX_LPC_ORDER];
    nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, i32::from(ps.indices.nlsf_indices[0]));
    for i in 0..cb.order as usize {
        let mut ix = dec.icdf(cb.ec_icdf.get(ec_ix[i] as usize..).unwrap_or(&[]), 8);
        if ix == 0 {
            ix -= dec.icdf(&NLSF_EXT_ICDF, 8);
        } else if ix == 2 * NLSF_QUANT_MAX_AMPLITUDE {
            ix += dec.icdf(&NLSF_EXT_ICDF, 8);
        }
        ps.indices.nlsf_indices[i + 1] = (ix - NLSF_QUANT_MAX_AMPLITUDE) as i8;
    }

    // Decode LSF interpolation factor.
    if ps.nb_subfr == MAX_NB_SUBFR as i32 {
        ps.indices.nlsf_interp_coef_q2 = dec.icdf(&NLSF_INTERPOLATION_FACTOR_ICDF, 8);
    } else {
        ps.indices.nlsf_interp_coef_q2 = 4;
    }

    if ps.indices.signal_type == TYPE_VOICED {
        // Decode pitch lags. Get lag index:
        let mut decode_absolute_lag_index = true;
        if cond_coding == CODE_CONDITIONALLY && ps.ec_prev_signal_type == TYPE_VOICED {
            // Decode delta index.
            let mut delta_lag_index = dec.icdf(&PITCH_DELTA_ICDF, 8);
            if delta_lag_index > 0 {
                delta_lag_index -= 9;
                ps.indices.lag_index = i32::from((ps.ec_prev_lag_index + delta_lag_index) as i16);
                decode_absolute_lag_index = false;
            }
        }
        if decode_absolute_lag_index {
            // Absolute decoding.
            let hi = dec.icdf(&PITCH_LAG_ICDF, 8) * (ps.fs_khz >> 1);
            let lo = dec.icdf(icdf_by_sel(ps.pitch_lag_low_bits_sel), 8);
            ps.indices.lag_index = i32::from((hi as i16).wrapping_add(lo as i16));
        }
        ps.ec_prev_lag_index = ps.indices.lag_index;

        // Get contour index.
        ps.indices.contour_index = dec.icdf(icdf_by_sel(ps.pitch_contour_sel), 8);

        // Decode LTP gains: PERIndex value, then the codebook indices.
        ps.indices.per_index = dec.icdf(&LTP_PER_INDEX_ICDF, 8);
        for k in 0..nb_subfr {
            ps.indices.ltp_index[k] = dec.icdf(ltp_gain_icdf(ps.indices.per_index), 8) as i8;
        }

        // Decode LTP scaling.
        if cond_coding == CODE_INDEPENDENTLY {
            ps.indices.ltp_scale_index = dec.icdf(&LTPSCALE_ICDF, 8);
        } else {
            ps.indices.ltp_scale_index = 0;
        }
    }
    ps.ec_prev_signal_type = ps.indices.signal_type;

    // Decode seed.
    ps.indices.seed = dec.icdf(&UNIFORM4_ICDF, 8);
}
