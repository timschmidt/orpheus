//! Ported from libopus 1.5.2 silk/decode_pulses.c, silk/shell_coder.c
//! (decoder part) and silk/code_signs.c (`silk_decode_signs`)
//! (BSD-3-Clause, see LICENSE-XIPH).

use super::tables::{
    LSB_ICDF, PULSES_PER_BLOCK_ICDF, RATE_LEVELS_ICDF, SHELL_CODE_TABLE_OFFSETS, SHELL_CODE_TABLE0,
    SHELL_CODE_TABLE1, SHELL_CODE_TABLE2, SHELL_CODE_TABLE3, SIGN_ICDF,
};
use super::{MAX_FRAME_LENGTH, MAX_NB_SHELL_BLOCKS, SHELL_CODEC_FRAME_LENGTH, macros::smulbb};
use crate::entdec::EcDec;

const SILK_MAX_PULSES: i32 = 16;
const N_RATE_LEVELS: usize = 10;
const PPB_ROW: usize = 18;

/// `decode_split`: split `p` pulses between two children.
#[inline]
fn decode_split(dec: &mut EcDec<'_>, p: i32, shell_table: &[u8]) -> (i16, i16) {
    if p > 0 {
        let off = SHELL_CODE_TABLE_OFFSETS.get(p as usize).copied().unwrap_or(0) as usize;
        let c1 = dec.icdf(&shell_table[off..], 8);
        (c1 as i16, (p - c1) as i16)
    } else {
        (0, 0)
    }
}

/// `silk_shell_decoder`: one shell block of 16 pulse amplitudes.
fn shell_decoder(pulses0: &mut [i16], dec: &mut EcDec<'_>, pulses4: i32) {
    let mut pulses3 = [0i16; 2];
    let mut pulses2 = [0i16; 4];
    let mut pulses1 = [0i16; 8];
    let t0: &[u8] = &SHELL_CODE_TABLE0;
    let t1: &[u8] = &SHELL_CODE_TABLE1;
    let t2: &[u8] = &SHELL_CODE_TABLE2;
    let t3: &[u8] = &SHELL_CODE_TABLE3;

    (pulses3[0], pulses3[1]) = decode_split(dec, pulses4, t3);
    (pulses2[0], pulses2[1]) = decode_split(dec, i32::from(pulses3[0]), t2);
    (pulses1[0], pulses1[1]) = decode_split(dec, i32::from(pulses2[0]), t1);
    (pulses0[0], pulses0[1]) = decode_split(dec, i32::from(pulses1[0]), t0);
    (pulses0[2], pulses0[3]) = decode_split(dec, i32::from(pulses1[1]), t0);
    (pulses1[2], pulses1[3]) = decode_split(dec, i32::from(pulses2[1]), t1);
    (pulses0[4], pulses0[5]) = decode_split(dec, i32::from(pulses1[2]), t0);
    (pulses0[6], pulses0[7]) = decode_split(dec, i32::from(pulses1[3]), t0);
    (pulses2[2], pulses2[3]) = decode_split(dec, i32::from(pulses3[1]), t2);
    (pulses1[4], pulses1[5]) = decode_split(dec, i32::from(pulses2[2]), t1);
    (pulses0[8], pulses0[9]) = decode_split(dec, i32::from(pulses1[4]), t0);
    (pulses0[10], pulses0[11]) = decode_split(dec, i32::from(pulses1[5]), t0);
    (pulses1[6], pulses1[7]) = decode_split(dec, i32::from(pulses2[3]), t1);
    (pulses0[12], pulses0[13]) = decode_split(dec, i32::from(pulses1[6]), t0);
    (pulses0[14], pulses0[15]) = decode_split(dec, i32::from(pulses1[7]), t0);
}

/// `silk_decode_signs`.
fn decode_signs(
    dec: &mut EcDec<'_>,
    pulses: &mut [i16],
    length: i32,
    signal_type: i32,
    quant_offset_type: i32,
    sum_pulses: &[i32; MAX_NB_SHELL_BLOCKS],
) {
    let i0 = smulbb(7, quant_offset_type + (signal_type << 1)).clamp(0, 35) as usize;
    let icdf_ptr = &SIGN_ICDF[i0..];
    let length =
        ((length + SHELL_CODEC_FRAME_LENGTH as i32 / 2) >> 4).clamp(0, MAX_NB_SHELL_BLOCKS as i32) as usize;
    for i in 0..length {
        let p = sum_pulses[i];
        if p > 0 {
            let icdf = [icdf_ptr[(p & 0x1F).min(6) as usize], 0];
            let q = &mut pulses[i * SHELL_CODEC_FRAME_LENGTH..(i + 1) * SHELL_CODEC_FRAME_LENGTH];
            for qj in q.iter_mut() {
                if *qj > 0 {
                    // silk_dec_map: 0 -> -1, 1 -> +1.
                    let s = (dec.icdf(&icdf, 8) << 1) - 1;
                    *qj = (*qj).wrapping_mul(s as i16);
                }
            }
        }
    }
}

/// `silk_decode_pulses`: excitation pulses of one frame.
pub(crate) fn decode_pulses(
    dec: &mut EcDec<'_>,
    pulses: &mut [i16; MAX_FRAME_LENGTH],
    signal_type: i32,
    quant_offset_type: i32,
    frame_length: i32,
) {
    let mut sum_pulses = [0i32; MAX_NB_SHELL_BLOCKS];
    let mut n_lshifts = [0i32; MAX_NB_SHELL_BLOCKS];

    // Decode rate level.
    let row = ((signal_type >> 1).clamp(0, 1) as usize) * 9;
    let rate_level_index = dec.icdf(&RATE_LEVELS_ICDF[row..row + 9], 8) as usize;

    // Calculate number of shell blocks.
    let mut iter = frame_length >> 4;
    if iter * (SHELL_CODEC_FRAME_LENGTH as i32) < frame_length {
        iter += 1; // only for 10 ms at 12 kHz
    }
    let iter = (iter.max(0) as usize).min(MAX_NB_SHELL_BLOCKS);

    // Sum-weighted-pulses decoding.
    let cdf_row = rate_level_index.min(N_RATE_LEVELS - 1) * PPB_ROW;
    let cdf = &PULSES_PER_BLOCK_ICDF[cdf_row..cdf_row + PPB_ROW];
    let last_row = (N_RATE_LEVELS - 1) * PPB_ROW;
    for i in 0..iter {
        n_lshifts[i] = 0;
        sum_pulses[i] = dec.icdf(cdf, 8);
        // LSB indication.
        while sum_pulses[i] == SILK_MAX_PULSES + 1 {
            n_lshifts[i] += 1;
            // When we've already got 10 LSBs, we shift the table to not allow (SILK_MAX_PULSES + 1).
            let start = last_row + usize::from(n_lshifts[i] == 10);
            sum_pulses[i] = dec.icdf(&PULSES_PER_BLOCK_ICDF[start..last_row + PPB_ROW], 8);
        }
    }

    // Shell decoding.
    for i in 0..iter {
        let blk = &mut pulses[i * SHELL_CODEC_FRAME_LENGTH..(i + 1) * SHELL_CODEC_FRAME_LENGTH];
        if sum_pulses[i] > 0 {
            shell_decoder(blk, dec, sum_pulses[i]);
        } else {
            blk.fill(0);
        }
    }

    // LSB decoding.
    for i in 0..iter {
        if n_lshifts[i] > 0 {
            let n_ls = n_lshifts[i];
            let blk = &mut pulses[i * SHELL_CODEC_FRAME_LENGTH..(i + 1) * SHELL_CODEC_FRAME_LENGTH];
            for p in blk.iter_mut() {
                let mut abs_q = i32::from(*p);
                for _ in 0..n_ls {
                    abs_q = abs_q.wrapping_shl(1);
                    abs_q = abs_q.wrapping_add(dec.icdf(&LSB_ICDF, 8));
                }
                *p = abs_q as i16;
            }
            // Mark the number of pulses non-zero for sign decoding.
            sum_pulses[i] |= n_ls << 5;
        }
    }

    // Decode and add signs to pulse signal.
    decode_signs(dec, pulses, frame_length, signal_type, quant_offset_type, &sum_pulses);
}
