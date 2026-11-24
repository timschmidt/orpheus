//! Ported from libopus 1.5.2 silk/decode_frame.c (BSD-3-Clause, see
//! LICENSE-XIPH).

use super::{
    ChannelState, DecCtrl, FLAG_DECODE_LBRR, FLAG_DECODE_NORMAL, FrameScratch, MAX_FRAMES_PER_PACKET, cng,
    decode_core, decode_indices, decode_parameters, decode_pulses, plc,
};
use crate::entdec::EcDec;

/// Move the output history and append the new frame (`outBuf` update).
fn update_out_buf(ps: &mut ChannelState, p_out: &[i16]) {
    let fl = ps.frame_length as usize;
    let mv_len = ps.ltp_mem_length as usize - fl;
    ps.out_buf.copy_within(fl..fl + mv_len, 0);
    ps.out_buf[mv_len..mv_len + fl].copy_from_slice(&p_out[..fl]);
}

/// `silk_decode_frame`: decode (or conceal) one frame into `p_out`
/// (`frame_length` samples).
pub(crate) fn decode_frame(
    ps: &mut ChannelState,
    dec: &mut EcDec<'_>,
    p_out: &mut [i16],
    p_n: &mut i32,
    lost_flag: i32,
    cond_coding: i32,
    fs: &mut FrameScratch,
) -> i32 {
    let l = ps.frame_length;
    let mut ctrl = DecCtrl { ltp_scale_q14: 0, ..DecCtrl::default() };
    let fd = (ps.n_frames_decoded as usize).min(MAX_FRAMES_PER_PACKET - 1);

    if lost_flag == FLAG_DECODE_NORMAL || (lost_flag == FLAG_DECODE_LBRR && ps.lbrr_flags[fd] == 1) {
        // Decode quantization indices of side info.
        decode_indices::decode_indices(ps, dec, ps.n_frames_decoded, lost_flag != 0, cond_coding);
        // Decode quantization indices of excitation.
        decode_pulses::decode_pulses(
            dec,
            &mut fs.pulses,
            ps.indices.signal_type,
            ps.indices.quant_offset_type,
            ps.frame_length,
        );
        // Decode parameters and pulse signal.
        decode_parameters::decode_parameters(ps, &mut ctrl, cond_coding);
        // Run inverse NSQ.
        decode_core::decode_core(ps, &mut ctrl, p_out, fs);
        // Update output buffer.
        update_out_buf(ps, p_out);
        // Update PLC state.
        plc::plc(ps, &mut ctrl, p_out, false, fs);
        ps.loss_cnt = 0;
        ps.prev_signal_type = ps.indices.signal_type;
        // A frame has been decoded without errors.
        ps.first_frame_after_reset = 0;
    } else {
        // Handle packet loss by extrapolation.
        plc::plc(ps, &mut ctrl, p_out, true, fs);
        update_out_buf(ps, p_out);
    }

    // Comfort noise generation / estimation.
    cng::cng(ps, &ctrl, p_out, l as usize, &mut fs.cng_sig_q14);

    // Ensure smooth connection of extrapolated and good frames.
    plc::plc_glue_frames(ps, p_out, l as usize);

    // Update some decoder state variables.
    ps.lag_prev = ctrl.pitch_l[ps.nb_subfr as usize - 1];

    *p_n = l;
    0
}
