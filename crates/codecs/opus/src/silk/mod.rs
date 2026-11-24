//! SILK decoder, ported from libopus 1.5.2 silk/dec_API.c (BSD-3-Clause, see
//! LICENSE-XIPH), with the state layouts of silk/structs.h and
//! silk/resampler_structs.h and the constants of silk/define.h.
//!
//! Integer arithmetic, bit-exact with libopus (every libopus build decodes
//! SILK in fixed point). Not ported: the encoder, deep PLC, OSCE, DRED, the
//! SIMD variants, the downsampling resampler paths (the API rate must be at
//! least the internal rate; Opus always asks for 48 kHz here) and decoding
//! LBRR/FEC data into audio (`lost_flag == 2` returns an error; LBRR data in
//! normal packets is parsed and skipped exactly as libopus does).
//!
//! All persistent state is in [`SilkDecoder`] and every large temporary in
//! [`SilkScratch`]; both are plain-old-data so they can come zeroed from the
//! codec arena.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::explicit_counter_loop
)]

mod cng;
mod decode_core;
mod decode_frame;
mod decode_indices;
mod decode_parameters;
mod decode_pulses;
mod lpc;
mod macros;
mod nlsf;
mod plc;
mod resampler;
mod stereo;
mod tables;

#[cfg(test)]
mod tests;

use crate::entdec::EcDec;
use bytemuck::{Pod, Zeroable};

// silk/define.h
pub(crate) const MAX_FRAMES_PER_PACKET: usize = 3;
pub(crate) const MAX_FS_KHZ: usize = 16;
pub(crate) const MAX_NB_SUBFR: usize = 4;
pub(crate) const SUB_FRAME_LENGTH_MS: i32 = 5;
pub(crate) const LTP_MEM_LENGTH_MS: i32 = 20;
pub(crate) const MAX_SUB_FRAME_LENGTH: usize = 5 * MAX_FS_KHZ;
pub(crate) const MAX_FRAME_LENGTH: usize = 20 * MAX_FS_KHZ;
pub(crate) const MAX_LTP_MEM_LENGTH: usize = LTP_MEM_LENGTH_MS as usize * MAX_FS_KHZ;
pub(crate) const MAX_LPC_ORDER: usize = 16;
pub(crate) const MIN_LPC_ORDER: usize = 10;
pub(crate) const LTP_ORDER: usize = 5;
pub(crate) const TYPE_NO_VOICE_ACTIVITY: i32 = 0;
pub(crate) const TYPE_VOICED: i32 = 2;
pub(crate) const CODE_INDEPENDENTLY: i32 = 0;
pub(crate) const CODE_INDEPENDENTLY_NO_LTP_SCALING: i32 = 1;
pub(crate) const CODE_CONDITIONALLY: i32 = 2;
pub(crate) const FLAG_DECODE_NORMAL: i32 = 0;
pub(crate) const FLAG_PACKET_LOST: i32 = 1;
pub(crate) const FLAG_DECODE_LBRR: i32 = 2;
pub(crate) const SHELL_CODEC_FRAME_LENGTH: usize = 16;
pub(crate) const MAX_NB_SHELL_BLOCKS: usize = MAX_FRAME_LENGTH / SHELL_CODEC_FRAME_LENGTH;
const MAX_API_FS_KHZ: i32 = 48;

// silk/errors.h
pub const SILK_DEC_INVALID_SAMPLING_FREQUENCY: i32 = -200;
pub const SILK_DEC_PAYLOAD_ERROR: i32 = -202;
pub const SILK_DEC_INVALID_FRAME_SIZE: i32 = -203;
/// Not a libopus code: an argument outside what this port supports
/// (channel counts, output buffer too small, FEC decoding).
pub const SILK_DEC_UNSUPPORTED: i32 = -210;

/// `silk_DecControlStruct` (the fields the decoder uses).
#[derive(Debug, Clone, Copy, Default)]
pub struct DecControl {
    /// Output channels (1 or 2).
    pub n_channels_api: i32,
    /// Coded channels (1 or 2).
    pub n_channels_internal: i32,
    /// Output sample rate in Hz (8000, 12000, 16000, 24000 or 48000, and at
    /// least the internal rate).
    pub api_sample_rate: i32,
    /// Internal sample rate in Hz: 8000, 12000 or 16000.
    pub internal_sample_rate: i32,
    /// Packet duration: 10, 20, 40 or 60 ms (0 counts as 10 for loss).
    pub payload_size_ms: i32,
    /// Out: pitch lag of the last frame at 48 kHz (0 if unvoiced).
    pub prev_pitch_lag: i32,
}

/// `silk_resampler_state_struct` (decoder paths only). The C union `sFIR`
/// is only used through its `i16` view by the upsamplers.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct ResamplerState {
    pub s_iir: [i32; 6],
    /// 0: copy, 1: 2x upsampler, 2: 2x upsampler + FIR interpolation.
    pub resampler_function: i32,
    pub batch_size: i32,
    pub inv_ratio_q16: i32,
    pub fs_in_khz: i32,
    pub fs_out_khz: i32,
    pub input_delay: i32,
    pub s_fir: [i16; 8],
    pub delay_buf: [i16; 48],
}

/// `SideInfoIndices`.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct SideInfoIndices {
    /// `opus_int16` in C.
    pub lag_index: i32,
    pub contour_index: i32,
    pub signal_type: i32,
    pub quant_offset_type: i32,
    pub nlsf_interp_coef_q2: i32,
    pub per_index: i32,
    pub ltp_scale_index: i32,
    pub seed: i32,
    pub gains_indices: [i8; MAX_NB_SUBFR],
    pub ltp_index: [i8; MAX_NB_SUBFR],
    /// `MAX_LPC_ORDER + 1` used; padded to a multiple of 4.
    pub nlsf_indices: [i8; 20],
}

/// `silk_PLC_struct`.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct PlcState {
    pub pitch_l_q8: i32,
    pub last_frame_lost: i32,
    pub rand_seed: i32,
    pub conc_energy: i32,
    pub conc_energy_shift: i32,
    pub prev_gain_q16: [i32; 2],
    pub fs_khz: i32,
    pub nb_subfr: i32,
    pub subfr_length: i32,
    pub ltp_coef_q14: [i16; LTP_ORDER],
    pub rand_scale_q14: i16,
    pub prev_lpc_q12: [i16; MAX_LPC_ORDER],
    pub prev_ltp_scale_q14: i16,
    pub _pad: i16,
}

/// `silk_CNG_struct`.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct CngState {
    pub exc_buf_q14: [i32; MAX_FRAME_LENGTH],
    pub synth_state: [i32; MAX_LPC_ORDER],
    pub smth_gain_q16: i32,
    pub rand_seed: i32,
    pub fs_khz: i32,
    pub smth_nlsf_q15: [i16; MAX_LPC_ORDER],
}

/// `silk_decoder_state` (one channel). C pointers to tables are selectors:
/// `nlsf_cb_sel` 0 = NB/MB, 1 = WB; the iCDF selectors are the
/// `tables::ICDF_*` codes.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct ChannelState {
    pub prev_gain_q16: i32,
    pub exc_q14: [i32; MAX_FRAME_LENGTH],
    pub s_lpc_q14_buf: [i32; MAX_LPC_ORDER],
    pub lag_prev: i32,
    /// `opus_int8` in C.
    pub last_gain_index: i32,
    pub fs_khz: i32,
    pub fs_api_hz: i32,
    pub nb_subfr: i32,
    pub frame_length: i32,
    pub subfr_length: i32,
    pub ltp_mem_length: i32,
    pub lpc_order: i32,
    pub first_frame_after_reset: i32,
    pub pitch_lag_low_bits_sel: i32,
    pub pitch_contour_sel: i32,
    pub nlsf_cb_sel: i32,
    pub n_frames_decoded: i32,
    pub n_frames_per_packet: i32,
    pub ec_prev_signal_type: i32,
    /// `opus_int16` in C.
    pub ec_prev_lag_index: i32,
    pub vad_flags: [i32; MAX_FRAMES_PER_PACKET],
    pub lbrr_flag: i32,
    pub lbrr_flags: [i32; MAX_FRAMES_PER_PACKET],
    pub loss_cnt: i32,
    pub prev_signal_type: i32,
    pub resampler: ResamplerState,
    pub indices: SideInfoIndices,
    pub cng: CngState,
    pub plc: PlcState,
    pub out_buf: [i16; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH],
    pub prev_nlsf_q15: [i16; MAX_LPC_ORDER],
}

/// `silk_decoder_control` (per frame, on the stack: 140 bytes).
#[derive(Clone, Copy, Default)]
pub(crate) struct DecCtrl {
    pub pitch_l: [i32; MAX_NB_SUBFR],
    pub gains_q16: [i32; MAX_NB_SUBFR],
    pub pred_coef_q12: [[i16; MAX_LPC_ORDER]; 2],
    pub ltp_coef_q14: [i16; LTP_ORDER * MAX_NB_SUBFR],
    pub ltp_scale_q14: i32,
}

/// The persistent SILK decoder (`silk_decoder` in dec_API.c), stereo-capable.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct SilkDecoder {
    channel_state: [ChannelState; 2],
    n_channels_api: i32,
    n_channels_internal: i32,
    prev_decode_only_middle: i32,
    /// `stereo_dec_state`.
    pred_prev_q13: [i16; 2],
    s_mid: [i16; 2],
    s_side: [i16; 2],
}

/// Temporaries of one frame's decode (shared by decode_core, PLC and CNG,
/// which never run at the same time).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct FrameScratch {
    /// `sLTP_Q15` (decode_core) / `sLTP_Q14` (PLC): ltp_mem + frame
    /// length, plus slack so corrupt lags cannot index out of bounds.
    pub s_ltp_q15: [i32; MAX_LTP_MEM_LENGTH + MAX_FRAME_LENGTH + 8],
    pub res_q14: [i32; MAX_SUB_FRAME_LENGTH],
    pub s_lpc_q14: [i32; MAX_SUB_FRAME_LENGTH + MAX_LPC_ORDER],
    pub cng_sig_q14: [i32; MAX_FRAME_LENGTH + MAX_LPC_ORDER],
    pub pulses: [i16; MAX_FRAME_LENGTH],
    pub s_ltp: [i16; MAX_LTP_MEM_LENGTH],
    pub exc_buf: [i16; 2 * MAX_SUB_FRAME_LENGTH],
}

/// Every temporary `silk_Decode` would put on the stack, sized for the
/// worst case (20 ms at 16 kHz internal, 48 kHz stereo output).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct SilkScratch {
    frame: FrameScratch,
    /// `samplesOut1_tmp`: two channels of `frame_length + 2`.
    out1: [i16; 2 * (MAX_FRAME_LENGTH + 2)],
    /// `samplesOut2_tmp`: one channel at the API rate.
    out2: [i16; 20 * MAX_API_FS_KHZ as usize],
    /// The IIR/FIR resampler's `buf`: 2 * batch + FIR order.
    resamp_buf: [i16; 2 * 10 * MAX_FS_KHZ + 8],
}

/// `silk_reset_decoder`: clears the whole channel state (libopus clears
/// from `prev_gain_Q16`, the first field without OSCE).
fn reset_channel(ps: &mut ChannelState) {
    *ps = ChannelState::zeroed();
    ps.first_frame_after_reset = 1;
    ps.prev_gain_q16 = 65536;
    cng::cng_reset(ps);
    plc::plc_reset(ps);
}

/// `silk_decoder_set_fs`.
fn decoder_set_fs(ps: &mut ChannelState, fs_khz: i32, fs_api_hz: i32) -> i32 {
    use tables::{
        ICDF_PITCH_CONTOUR, ICDF_PITCH_CONTOUR_10_MS, ICDF_PITCH_CONTOUR_10_MS_NB, ICDF_PITCH_CONTOUR_NB,
        ICDF_UNIFORM4, ICDF_UNIFORM6, ICDF_UNIFORM8,
    };
    let mut ret = 0;
    ps.subfr_length = macros::smulbb(SUB_FRAME_LENGTH_MS, fs_khz);
    let frame_length = macros::smulbb(ps.nb_subfr, ps.subfr_length);

    if ps.fs_khz != fs_khz || ps.fs_api_hz != fs_api_hz {
        ret += resampler::resampler_init(&mut ps.resampler, macros::smulbb(fs_khz, 1000), fs_api_hz);
        ps.fs_api_hz = fs_api_hz;
    }

    if ps.fs_khz != fs_khz || frame_length != ps.frame_length {
        ps.pitch_contour_sel = match (fs_khz == 8, ps.nb_subfr == MAX_NB_SUBFR as i32) {
            (true, true) => ICDF_PITCH_CONTOUR_NB,
            (true, false) => ICDF_PITCH_CONTOUR_10_MS_NB,
            (false, true) => ICDF_PITCH_CONTOUR,
            (false, false) => ICDF_PITCH_CONTOUR_10_MS,
        };
        if ps.fs_khz != fs_khz {
            ps.ltp_mem_length = macros::smulbb(LTP_MEM_LENGTH_MS, fs_khz);
            if fs_khz == 8 || fs_khz == 12 {
                ps.lpc_order = MIN_LPC_ORDER as i32;
                ps.nlsf_cb_sel = 0;
            } else {
                ps.lpc_order = MAX_LPC_ORDER as i32;
                ps.nlsf_cb_sel = 1;
            }
            ps.pitch_lag_low_bits_sel = match fs_khz {
                16 => ICDF_UNIFORM8,
                12 => ICDF_UNIFORM6,
                _ => ICDF_UNIFORM4,
            };
            ps.first_frame_after_reset = 1;
            ps.lag_prev = 100;
            ps.last_gain_index = 10;
            ps.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
            ps.out_buf = [0; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH];
            ps.s_lpc_q14_buf = [0; MAX_LPC_ORDER];
        }
        ps.fs_khz = fs_khz;
        ps.frame_length = frame_length;
    }
    ret
}

/// True when the channel's geometry is one the decoder can run with (it
/// always is after `decoder_set_fs`; this guards call sequences libopus
/// would never make).
fn channel_ok(ps: &ChannelState) -> bool {
    matches!(ps.fs_khz, 8 | 12 | 16)
        && (ps.nb_subfr == 2 || ps.nb_subfr == 4)
        && ps.subfr_length == 5 * ps.fs_khz
        && ps.frame_length == ps.nb_subfr * ps.subfr_length
        && ps.ltp_mem_length == 20 * ps.fs_khz
        && (ps.lpc_order == if ps.fs_khz == 16 { 16 } else { 10 })
}

impl SilkDecoder {
    /// `silk_InitDecoder`.
    pub fn init(&mut self) {
        for cs in &mut self.channel_state {
            reset_channel(cs);
        }
        self.pred_prev_q13 = [0; 2];
        self.s_mid = [0; 2];
        self.s_side = [0; 2];
        self.prev_decode_only_middle = 0;
    }

    /// `silk_ResetDecoder` (identical to [`Self::init`] without OSCE).
    pub fn reset(&mut self) {
        self.init();
    }

    /// `silk_Decode`: one 10 or 20 ms SILK frame per call. `new_packet` is
    /// true on the first call for an Opus frame. `out` receives
    /// `n_samples_out` samples per channel, interleaved, at the API rate
    /// with `n_channels_api` channels.
    pub fn decode(
        &mut self,
        scratch: &mut SilkScratch,
        ctl: &mut DecControl,
        lost_flag: i32,
        new_packet: bool,
        dec: &mut EcDec<'_>,
        out: &mut [i16],
        n_samples_out: &mut i32,
    ) -> Result<(), i32> {
        let nci = ctl.n_channels_internal;
        let napi = ctl.n_channels_api;
        if !(nci == 1 || nci == 2) || !(napi == 1 || napi == 2) {
            return Err(SILK_DEC_UNSUPPORTED);
        }
        if lost_flag == FLAG_DECODE_LBRR
            || !(lost_flag == FLAG_DECODE_NORMAL || lost_flag == FLAG_PACKET_LOST)
        {
            return Err(SILK_DEC_UNSUPPORTED);
        }
        // Not in libopus: the downsampling resampler is not ported.
        if !matches!(ctl.api_sample_rate, 8000 | 12000 | 16000 | 24000 | 48000)
            || ctl.api_sample_rate < ctl.internal_sample_rate
        {
            return Err(SILK_DEC_INVALID_SAMPLING_FREQUENCY);
        }
        let nci_u = nci as usize;
        let mut ret = 0i32;
        let mut decode_only_middle = 0i32;
        let mut ms_pred_q13 = [0i32; 2];

        // Test if first frame in payload.
        if new_packet {
            for cs in &mut self.channel_state[..nci_u] {
                cs.n_frames_decoded = 0;
            }
        }

        // Mono -> stereo transition in bitstream: init state of second channel.
        if nci > self.n_channels_internal {
            reset_channel(&mut self.channel_state[1]);
        }

        let stereo_to_mono = nci == 1
            && self.n_channels_internal == 2
            && ctl.internal_sample_rate == 1000 * self.channel_state[0].fs_khz;

        if self.channel_state[0].n_frames_decoded == 0 {
            for cs in &mut self.channel_state[..nci_u] {
                let (fpp, nb) = match ctl.payload_size_ms {
                    0 | 10 => (1, 2),
                    20 => (1, 4),
                    40 => (2, 4),
                    60 => (3, 4),
                    _ => return Err(SILK_DEC_INVALID_FRAME_SIZE),
                };
                cs.n_frames_per_packet = fpp;
                cs.nb_subfr = nb;
                let fs_khz_dec = (ctl.internal_sample_rate >> 10) + 1;
                if fs_khz_dec != 8 && fs_khz_dec != 12 && fs_khz_dec != 16 {
                    return Err(SILK_DEC_INVALID_SAMPLING_FREQUENCY);
                }
                ret += decoder_set_fs(cs, fs_khz_dec, ctl.api_sample_rate);
            }
        }

        if napi == 2 && nci == 2 && (self.n_channels_api == 1 || self.n_channels_internal == 1) {
            self.pred_prev_q13 = [0; 2];
            self.s_side = [0; 2];
            self.channel_state[1].resampler = self.channel_state[0].resampler;
        }
        self.n_channels_api = napi;
        self.n_channels_internal = nci;

        if ctl.api_sample_rate > MAX_API_FS_KHZ * 1000 || ctl.api_sample_rate < 8000 {
            return Err(SILK_DEC_INVALID_SAMPLING_FREQUENCY);
        }

        for cs in &self.channel_state[..nci_u] {
            if !channel_ok(cs) || cs.frame_length != self.channel_state[0].frame_length {
                return Err(SILK_DEC_PAYLOAD_ERROR);
            }
        }

        if lost_flag != FLAG_PACKET_LOST && self.channel_state[0].n_frames_decoded == 0 {
            // First decoder call for this payload: VAD and LBRR flags.
            for cs in &mut self.channel_state[..nci_u] {
                let fpp = (cs.n_frames_per_packet as usize).min(MAX_FRAMES_PER_PACKET);
                for i in 0..fpp {
                    cs.vad_flags[i] = i32::from(dec.bit_logp(1));
                }
                cs.lbrr_flag = i32::from(dec.bit_logp(1));
            }
            for cs in &mut self.channel_state[..nci_u] {
                cs.lbrr_flags = [0; MAX_FRAMES_PER_PACKET];
                if cs.lbrr_flag != 0 {
                    if cs.n_frames_per_packet == 1 {
                        cs.lbrr_flags[0] = 1;
                    } else {
                        let sym = dec.icdf(tables::lbrr_flags_icdf(cs.n_frames_per_packet), 8) + 1;
                        let fpp = (cs.n_frames_per_packet as usize).min(MAX_FRAMES_PER_PACKET);
                        for i in 0..fpp {
                            cs.lbrr_flags[i] = (sym >> i) & 1;
                        }
                    }
                }
            }
            if lost_flag == FLAG_DECODE_NORMAL {
                // Regular decoding: skip all LBRR data.
                let fpp = (self.channel_state[0].n_frames_per_packet as usize).min(MAX_FRAMES_PER_PACKET);
                for i in 0..fpp {
                    for n in 0..nci_u {
                        if self.channel_state[n].lbrr_flags[i] != 0 {
                            if nci == 2 && n == 0 {
                                stereo::stereo_decode_pred(dec, &mut ms_pred_q13);
                                if self.channel_state[1].lbrr_flags[i] == 0 {
                                    decode_only_middle = stereo::stereo_decode_mid_only(dec);
                                }
                            }
                            let cond_coding = if i > 0 && self.channel_state[n].lbrr_flags[i - 1] != 0 {
                                CODE_CONDITIONALLY
                            } else {
                                CODE_INDEPENDENTLY
                            };
                            let cs = &mut self.channel_state[n];
                            decode_indices::decode_indices(cs, dec, i as i32, true, cond_coding);
                            decode_pulses::decode_pulses(
                                dec,
                                &mut scratch.frame.pulses,
                                cs.indices.signal_type,
                                cs.indices.quant_offset_type,
                                cs.frame_length,
                            );
                        }
                    }
                }
            }
        }

        // Get MS predictor index.
        if nci == 2 {
            if lost_flag == FLAG_DECODE_NORMAL {
                stereo::stereo_decode_pred(dec, &mut ms_pred_q13);
                let fd = (self.channel_state[0].n_frames_decoded as usize).min(MAX_FRAMES_PER_PACKET - 1);
                if self.channel_state[1].vad_flags[fd] == 0 {
                    decode_only_middle = stereo::stereo_decode_mid_only(dec);
                } else {
                    decode_only_middle = 0;
                }
            } else {
                ms_pred_q13 = [i32::from(self.pred_prev_q13[0]), i32::from(self.pred_prev_q13[1])];
            }
        }

        // Reset side channel decoder prediction memory for first frame with side coding.
        if nci == 2 && decode_only_middle == 0 && self.prev_decode_only_middle == 1 {
            let cs1 = &mut self.channel_state[1];
            cs1.out_buf = [0; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH];
            cs1.s_lpc_q14_buf = [0; MAX_LPC_ORDER];
            cs1.lag_prev = 100;
            cs1.last_gain_index = 10;
            cs1.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
            cs1.first_frame_after_reset = 1;
        }

        let fl0 = self.channel_state[0].frame_length as usize;
        let ch_stride = fl0 + 2;

        let has_side = if lost_flag == FLAG_DECODE_NORMAL {
            decode_only_middle == 0
        } else {
            self.prev_decode_only_middle == 0
        };

        // Call decoder for one frame.
        let mut n_samples_out_dec: i32 = 0;
        for n in 0..nci_u {
            if n == 0 || has_side {
                let frame_index = self.channel_state[0].n_frames_decoded - n as i32;
                let cond_coding = if frame_index <= 0 {
                    CODE_INDEPENDENTLY
                } else if n > 0 && self.prev_decode_only_middle != 0 {
                    // If we skipped a side frame in this packet, we don't
                    // need LTP scaling; the LTP state is well-defined.
                    CODE_INDEPENDENTLY_NO_LTP_SCALING
                } else {
                    CODE_CONDITIONALLY
                };
                let start = n * ch_stride + 2;
                let end = start + self.channel_state[n].frame_length as usize;
                ret += decode_frame::decode_frame(
                    &mut self.channel_state[n],
                    dec,
                    &mut scratch.out1[start..end],
                    &mut n_samples_out_dec,
                    lost_flag,
                    cond_coding,
                    &mut scratch.frame,
                );
            } else {
                let start = n * ch_stride + 2;
                scratch.out1[start..start + n_samples_out_dec as usize].fill(0);
            }
            self.channel_state[n].n_frames_decoded += 1;
        }
        let nsod = n_samples_out_dec as usize;

        if napi == 2 && nci == 2 {
            // Convert Mid/Side to Left/Right.
            let (x1, x2) = scratch.out1.split_at_mut(ch_stride);
            stereo::stereo_ms_to_lr(
                &mut self.pred_prev_q13,
                &mut self.s_mid,
                &mut self.s_side,
                x1,
                x2,
                &ms_pred_q13,
                self.channel_state[0].fs_khz,
                nsod,
            );
        } else {
            // Buffering.
            scratch.out1[..2].copy_from_slice(&self.s_mid);
            self.s_mid.copy_from_slice(&scratch.out1[nsod..nsod + 2]);
        }

        // Number of output samples.
        let nso = macros::div32(
            n_samples_out_dec * ctl.api_sample_rate,
            macros::smulbb(self.channel_state[0].fs_khz, 1000),
        );
        let nso_u = nso as usize;
        if nso_u > scratch.out2.len() || out.len() < nso_u * napi as usize {
            return Err(SILK_DEC_UNSUPPORTED);
        }
        *n_samples_out = nso;

        for n in 0..(napi.min(nci) as usize) {
            // Resample decoded signal to API_sampleRate.
            let input = &scratch.out1[n * ch_stride + 1..n * ch_stride + 1 + nsod];
            if napi == 2 {
                ret += resampler::resampler(
                    &mut self.channel_state[n].resampler,
                    &mut scratch.out2[..nso_u],
                    input,
                    &mut scratch.resamp_buf,
                );
                // Interleave if stereo output and stereo stream.
                for i in 0..nso_u {
                    out[n + 2 * i] = scratch.out2[i];
                }
            } else {
                ret += resampler::resampler(
                    &mut self.channel_state[n].resampler,
                    &mut out[..nso_u],
                    input,
                    &mut scratch.resamp_buf,
                );
            }
        }

        // Create two channel output from mono stream.
        if napi == 2 && nci == 1 {
            if stereo_to_mono {
                // Resample right channel for newly collapsed stereo just in
                // case we weren't doing collapsing when switching to mono.
                ret += resampler::resampler(
                    &mut self.channel_state[1].resampler,
                    &mut scratch.out2[..nso_u],
                    &scratch.out1[1..1 + nsod],
                    &mut scratch.resamp_buf,
                );
                for i in 0..nso_u {
                    out[1 + 2 * i] = scratch.out2[i];
                }
            } else {
                for i in 0..nso_u {
                    out[1 + 2 * i] = out[2 * i];
                }
            }
        }

        // Export pitch lag, measured at 48 kHz sampling rate.
        let cs0 = &self.channel_state[0];
        ctl.prev_pitch_lag = if cs0.prev_signal_type == TYPE_VOICED {
            const MULT_TAB: [i32; 3] = [6, 4, 3];
            cs0.lag_prev * MULT_TAB[(((cs0.fs_khz - 8) >> 2) as usize).min(2)]
        } else {
            0
        };

        if lost_flag == FLAG_PACKET_LOST {
            // On packet loss, remove the gain clamping to prevent having the
            // energy "bounce back" if we lose packets when the energy is
            // going down.
            for cs in &mut self.channel_state[..self.n_channels_internal as usize] {
                cs.last_gain_index = 10;
            }
        } else {
            self.prev_decode_only_middle = decode_only_middle;
        }

        if ret == 0 { Ok(()) } else { Err(ret) }
    }
}
