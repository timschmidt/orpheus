//! Ported from libopus 1.5.2 silk/resampler.c,
//! silk/resampler_private_IIR_FIR.c and silk/resampler_private_up2_HQ.c
//! (BSD-3-Clause, see LICENSE-XIPH).
//!
//! The decoder's upsampling paths only (copy, 2x, and 2x + fractional
//! FIR); `resampler_init` refuses downsampling.

use super::ResamplerState;
use super::macros::{div32, lshift, rshift_round, sat16, smlabb, smlawb, smulbb, smulwb, smulww};
use super::tables::{RESAMPLER_FRAC_FIR_12, RESAMPLER_UP2_HQ_0, RESAMPLER_UP2_HQ_1};
use bytemuck::Zeroable;

const USE_COPY: i32 = 0;
const USE_UP2_HQ: i32 = 1;
const USE_IIR_FIR: i32 = 2;
const RESAMPLER_MAX_BATCH_SIZE_MS: i32 = 10;
const RESAMPLER_ORDER_FIR_12: usize = 8;

/// `delay_matrix_dec`: input delay by internal rate (8/12/16 kHz) and
/// output rate (8/12/16/24/48 kHz).
const DELAY_MATRIX_DEC: [[i8; 5]; 3] = [[4, 0, 2, 0, 0], [0, 9, 4, 7, 4], [0, 3, 12, 7, 7]];

/// `rateID`.
fn rate_id(r: i32) -> usize {
    ((((r >> 12) - i32::from(r > 16000)) >> i32::from(r > 24000)) - 1) as usize
}

/// `silk_resampler_init` for the decoder (`forEnc` = 0). Returns -1 for
/// unsupported rates, including any downsampling.
pub(crate) fn resampler_init(s: &mut ResamplerState, fs_hz_in: i32, fs_hz_out: i32) -> i32 {
    *s = ResamplerState::zeroed();
    if !matches!(fs_hz_in, 8000 | 12000 | 16000)
        || !matches!(fs_hz_out, 8000 | 12000 | 16000 | 24000 | 48000)
        || fs_hz_out < fs_hz_in
    {
        return -1;
    }
    s.input_delay = i32::from(DELAY_MATRIX_DEC[rate_id(fs_hz_in)][rate_id(fs_hz_out)]);
    s.fs_in_khz = fs_hz_in / 1000;
    s.fs_out_khz = fs_hz_out / 1000;
    s.batch_size = s.fs_in_khz * RESAMPLER_MAX_BATCH_SIZE_MS;

    let mut up2x = 0;
    if fs_hz_out > fs_hz_in {
        if fs_hz_out == 2 * fs_hz_in {
            // Fs_out : Fs_in = 2 : 1.
            s.resampler_function = USE_UP2_HQ;
        } else {
            s.resampler_function = USE_IIR_FIR;
            up2x = 1;
        }
    } else {
        s.resampler_function = USE_COPY;
    }

    s.inv_ratio_q16 = lshift(div32(lshift(fs_hz_in, 14 + up2x), fs_hz_out), 2);
    while smulww(s.inv_ratio_q16, fs_hz_out) < lshift(fs_hz_in, up2x) {
        s.inv_ratio_q16 += 1;
    }
    0
}

/// `silk_resampler_private_up2_HQ`: 2x upsampling with allpass filters.
fn up2_hq(s: &mut [i32; 6], out: &mut [i16], input: &[i16]) {
    for (k, &x) in input.iter().enumerate() {
        // Convert to Q10.
        let in32 = i32::from(x) << 10;

        // First all-pass section for even output sample.
        let y = in32.wrapping_sub(s[0]);
        let xx = smulwb(y, i32::from(RESAMPLER_UP2_HQ_0[0]));
        let out32_1 = s[0].wrapping_add(xx);
        s[0] = in32.wrapping_add(xx);

        // Second all-pass section for even output sample.
        let y = out32_1.wrapping_sub(s[1]);
        let xx = smulwb(y, i32::from(RESAMPLER_UP2_HQ_0[1]));
        let out32_2 = s[1].wrapping_add(xx);
        s[1] = out32_1.wrapping_add(xx);

        // Third all-pass section for even output sample.
        let y = out32_2.wrapping_sub(s[2]);
        let xx = smlawb(y, y, i32::from(RESAMPLER_UP2_HQ_0[2]));
        let out32_1 = s[2].wrapping_add(xx);
        s[2] = out32_2.wrapping_add(xx);

        // Apply gain in Q15, convert back to int16 and store to output.
        out[2 * k] = sat16(rshift_round(out32_1, 10)) as i16;

        // First all-pass section for odd output sample.
        let y = in32.wrapping_sub(s[3]);
        let xx = smulwb(y, i32::from(RESAMPLER_UP2_HQ_1[0]));
        let out32_1 = s[3].wrapping_add(xx);
        s[3] = in32.wrapping_add(xx);

        // Second all-pass section for odd output sample.
        let y = out32_1.wrapping_sub(s[4]);
        let xx = smulwb(y, i32::from(RESAMPLER_UP2_HQ_1[1]));
        let out32_2 = s[4].wrapping_add(xx);
        s[4] = out32_1.wrapping_add(xx);

        // Third all-pass section for odd output sample.
        let y = out32_2.wrapping_sub(s[5]);
        let xx = smlawb(y, y, i32::from(RESAMPLER_UP2_HQ_1[2]));
        let out32_1 = s[5].wrapping_add(xx);
        s[5] = out32_2.wrapping_add(xx);

        // Apply gain in Q15, convert back to int16 and store to output.
        out[2 * k + 1] = sat16(rshift_round(out32_1, 10)) as i16;
    }
}

/// `silk_resampler_private_IIR_FIR_INTERPOL`; returns the samples written.
fn iir_fir_interpol(out: &mut [i16], buf: &[i16], max_index_q16: i32, index_increment_q16: i32) -> usize {
    let mut n = 0;
    let mut index_q16 = 0i32;
    while index_q16 < max_index_q16 && n < out.len() {
        let table_index = smulwb(index_q16 & 0xFFFF, 12) as usize;
        let b = &buf[(index_q16 >> 16) as usize..];
        let f = |row: usize, col: usize| i32::from(RESAMPLER_FRAC_FIR_12[row * 4 + col]);
        let mut res_q15 = smulbb(i32::from(b[0]), f(table_index, 0));
        res_q15 = smlabb(res_q15, i32::from(b[1]), f(table_index, 1));
        res_q15 = smlabb(res_q15, i32::from(b[2]), f(table_index, 2));
        res_q15 = smlabb(res_q15, i32::from(b[3]), f(table_index, 3));
        res_q15 = smlabb(res_q15, i32::from(b[4]), f(11 - table_index, 3));
        res_q15 = smlabb(res_q15, i32::from(b[5]), f(11 - table_index, 2));
        res_q15 = smlabb(res_q15, i32::from(b[6]), f(11 - table_index, 1));
        res_q15 = smlabb(res_q15, i32::from(b[7]), f(11 - table_index, 0));
        out[n] = sat16(rshift_round(res_q15, 15)) as i16;
        n += 1;
        index_q16 = index_q16.wrapping_add(index_increment_q16);
    }
    n
}

/// `silk_resampler_private_IIR_FIR`: 2x upsampling then fractional FIR
/// interpolation. Returns the samples written to `out`.
fn iir_fir(s: &mut ResamplerState, out: &mut [i16], input: &[i16], buf: &mut [i16]) -> usize {
    let batch = (s.batch_size.max(1) as usize).min((buf.len() - RESAMPLER_ORDER_FIR_12) / 2);
    let index_increment_q16 = s.inv_ratio_q16.max(1);
    buf[..RESAMPLER_ORDER_FIR_12].copy_from_slice(&s.s_fir);
    let mut written = 0;
    let mut pos = 0;
    let mut n_samples_in = 0;
    while pos < input.len() || n_samples_in == 0 {
        n_samples_in = (input.len() - pos).min(batch);
        up2_hq(
            &mut s.s_iir,
            &mut buf[RESAMPLER_ORDER_FIR_12..RESAMPLER_ORDER_FIR_12 + 2 * n_samples_in],
            &input[pos..pos + n_samples_in],
        );
        let max_index_q16 = lshift(n_samples_in as i32, 16 + 1); // + 1 because 2x upsampling
        written += iir_fir_interpol(&mut out[written..], buf, max_index_q16, index_increment_q16);
        pos += n_samples_in;
        if pos < input.len() {
            buf.copy_within(n_samples_in << 1..(n_samples_in << 1) + RESAMPLER_ORDER_FIR_12, 0);
        } else {
            break;
        }
    }
    s.s_fir.copy_from_slice(&buf[n_samples_in << 1..(n_samples_in << 1) + RESAMPLER_ORDER_FIR_12]);
    written
}

/// `silk_resampler`: resample `input` into `out`. Returns 0 (libopus's
/// return value).
pub(crate) fn resampler(s: &mut ResamplerState, out: &mut [i16], input: &[i16], buf: &mut [i16]) -> i32 {
    let in_len = input.len();
    let fs_in = s.fs_in_khz.max(0) as usize;
    let fs_out = s.fs_out_khz.max(0) as usize;
    let delay = s.input_delay.max(0) as usize;
    // libopus asserts inLen >= Fs_in_kHz and inputDelay <= Fs_in_kHz; the
    // output length check guards against a state set up for other rates
    // (call sequences libopus never makes).
    if in_len < fs_in || delay > fs_in || fs_in == 0 || fs_in > 48 || out.len() * fs_in != in_len * fs_out {
        out.fill(0);
        return -1;
    }
    let n_samples = fs_in - delay;

    // Copy to delay buffer.
    s.delay_buf[delay..fs_in].copy_from_slice(&input[..n_samples]);
    let mut delay_buf = [0i16; 48];
    delay_buf[..fs_in].copy_from_slice(&s.delay_buf[..fs_in]);

    match s.resampler_function {
        USE_UP2_HQ => {
            up2_hq(&mut s.s_iir, &mut out[..2 * fs_in], &delay_buf[..fs_in]);
            up2_hq(
                &mut s.s_iir,
                &mut out[fs_out..fs_out + 2 * (in_len - fs_in)],
                &input[n_samples..n_samples + in_len - fs_in],
            );
        }
        USE_IIR_FIR => {
            let first = fs_out.min(out.len());
            iir_fir(s, &mut out[..first], &delay_buf[..fs_in], buf);
            iir_fir(s, &mut out[fs_out..], &input[n_samples..n_samples + in_len - fs_in], buf);
        }
        _ => {
            out[..fs_in].copy_from_slice(&delay_buf[..fs_in]);
            out[fs_out..fs_out + in_len - fs_in]
                .copy_from_slice(&input[n_samples..n_samples + in_len - fs_in]);
        }
    }

    // Copy to delay buffer.
    s.delay_buf[..delay].copy_from_slice(&input[in_len - delay..]);
    0
}
