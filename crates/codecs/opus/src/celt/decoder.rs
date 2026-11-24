//! The CELT decoder: libopus 1.5.2 `celt/celt_decoder.c`, `celt/celt.c`
//! (`comb_filter`, tables) and `celt/quant_bands.c` (energy decoding),
//! float build (BSD-3-Clause, see LICENSE-XIPH).
//!
//! State lives in the arena: [`CeltState`] (a `Pod` struct) plus the
//! per-channel history `decode_mem` (`channels * (2048 + 120)` floats).
//! Temporaries come from a caller scratch slice of [`SCRATCH_LEN`] floats.

use bytemuck::{Pod, Zeroable};

use super::bands::{NORM_LEN, anti_collapse, denormalise_bands, lcg_rand, quant_all_bands};
use super::math::sqrt;
use super::mdct::mdct_backward;
use super::modes::{
    DECODE_BUFFER_SIZE, E_PROB_MODEL, EBANDS, LPC_ORDER, MAX_PERIOD, NB_EBANDS, OVERLAP, PREEMPH, SHORT_MDCT,
    WINDOW,
};
use super::plc;
use super::rate::{MAX_FINE_BITS, compute_allocation, init_caps};
use super::vq::renormalise_vector;
use crate::entdec::{BITRES, EcDec};

/// History per channel.
pub const MEM_PER_CH: usize = DECODE_BUFFER_SIZE + OVERLAP;
/// Scratch floats [`decode`] needs (`X` for two channels and the band
/// normalisation area; the concealment fits in the same space).
pub const SCRATCH_LEN: usize = 2 * 960 + NORM_LEN + super::vq::MAX_N;

const PLC_PITCH_LAG_MAX: usize = 720;
const PLC_PITCH_LAG_MIN: usize = 100;
const COMBFILTER_MINPERIOD: i32 = 15;
const SPREAD_NORMAL: i32 = 2;

static TRIM_ICDF: [u8; 11] = [126, 124, 119, 109, 87, 41, 19, 9, 4, 2, 0];
static SPREAD_ICDF: [u8; 4] = [25, 23, 2, 0];
static TAPSET_ICDF: [u8; 3] = [2, 1, 0];
static SMALL_ENERGY_ICDF: [u8; 3] = [2, 1, 0];
static TF_SELECT_TABLE: [[i8; 8]; 4] = [
    [0, -1, 0, -1, 0, -1, 0, -1],
    [0, -1, 0, -2, 1, 0, 1, -1],
    [0, -2, 0, -3, 2, 0, 1, -1],
    [0, -2, 0, -3, 3, 0, 1, -1],
];
static PRED_COEF: [f32; 4] = [29440.0 / 32768.0, 26112.0 / 32768.0, 21248.0 / 32768.0, 16384.0 / 32768.0];
static BETA_COEF: [f32; 4] = [30147.0 / 32768.0, 22282.0 / 32768.0, 12124.0 / 32768.0, 6554.0 / 32768.0];
const BETA_INTRA: f32 = 4915.0 / 32768.0;

/// CELT decoder state (everything but `decode_mem`).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct CeltState {
    pub channels: i32,
    pub stream_channels: i32,
    pub start: i32,
    pub end: i32,
    pub disable_inv: i32,
    // Cleared on reset from here.
    pub rng: u32,
    pub error: i32,
    last_pitch_index: i32,
    loss_duration: i32,
    skip_plc: i32,
    postfilter_period: i32,
    postfilter_period_old: i32,
    postfilter_gain: f32,
    postfilter_gain_old: f32,
    postfilter_tapset: i32,
    postfilter_tapset_old: i32,
    prefilter_and_fold: i32,
    preemph_mem_d: [f32; 2],
    lpc: [f32; 2 * LPC_ORDER],
    old_band_e: [f32; 2 * NB_EBANDS],
    old_log_e: [f32; 2 * NB_EBANDS],
    old_log_e2: [f32; 2 * NB_EBANDS],
    background_log_e: [f32; 2 * NB_EBANDS],
}

impl CeltState {
    /// `celt_decoder_init` for `channels` (1 or 2) at 48 kHz.
    pub fn init(&mut self, mem: &mut [f32], channels: usize) {
        *self = Self::zeroed();
        self.channels = channels as i32;
        self.stream_channels = channels as i32;
        self.start = 0;
        self.end = NB_EBANDS as i32;
        self.disable_inv = i32::from(channels == 1);
        self.reset(mem);
    }

    /// `OPUS_RESET_STATE`.
    pub fn reset(&mut self, mem: &mut [f32]) {
        let keep = (self.channels, self.stream_channels, self.start, self.end, self.disable_inv);
        *self = Self::zeroed();
        (self.channels, self.stream_channels, self.start, self.end, self.disable_inv) = keep;
        for v in mem.iter_mut() {
            *v = 0.0;
        }
        self.old_log_e = [-28.0; 2 * NB_EBANDS];
        self.old_log_e2 = [-28.0; 2 * NB_EBANDS];
        self.skip_plc = 1;
    }
}

fn unquant_coarse_energy(
    old: &mut [f32],
    start: usize,
    end: usize,
    intra: bool,
    dec: &mut EcDec<'_>,
    c: usize,
    lm: usize,
) {
    let prob = &E_PROB_MODEL[(lm * 2 + usize::from(intra)) * 42..];
    let mut prev = [0f32; 2];
    let (coef, beta) = if intra { (0.0, BETA_INTRA) } else { (PRED_COEF[lm], BETA_COEF[lm]) };
    let budget = dec.storage as i32 * 8;
    for i in start..end {
        for ch in 0..c {
            let tell = dec.tell();
            let qi = if budget - tell >= 15 {
                let pi = 2 * i.min(20);
                dec.laplace(u32::from(prob[pi]) << 7, i32::from(prob[pi + 1]) << 6)
            } else if budget - tell >= 2 {
                let q = dec.icdf(&SMALL_ENERGY_ICDF, 2);
                (q >> 1) ^ -(q & 1)
            } else if budget - tell >= 1 {
                -i32::from(dec.bit_logp(1))
            } else {
                -1
            };
            let q = qi as f32;
            let k = i + ch * NB_EBANDS;
            old[k] = old[k].max(-9.0);
            let tmp = coef * old[k] + prev[ch] + q;
            old[k] = tmp;
            prev[ch] = prev[ch] + q - beta * q;
        }
    }
}

fn unquant_fine_energy(
    old: &mut [f32],
    start: usize,
    end: usize,
    fine_quant: &[i32],
    dec: &mut EcDec<'_>,
    c: usize,
) {
    for i in start..end {
        let fq = fine_quant[i];
        if fq <= 0 {
            continue;
        }
        for ch in 0..c {
            let q2 = dec.bits(fq as u32);
            let offset = (q2 as f32 + 0.5) * (1i32 << (14 - fq).max(0)) as f32 * (1.0 / 16384.0) - 0.5;
            old[i + ch * NB_EBANDS] += offset;
        }
    }
}

fn unquant_energy_finalise(
    old: &mut [f32],
    start: usize,
    end: usize,
    fine_quant: &[i32],
    fine_priority: &[i32],
    mut bits_left: i32,
    dec: &mut EcDec<'_>,
    c: usize,
) {
    for prio in 0..2 {
        let mut i = start;
        while i < end && bits_left >= c as i32 {
            if fine_quant[i] >= MAX_FINE_BITS || fine_priority[i] != prio {
                i += 1;
                continue;
            }
            for ch in 0..c {
                let q2 = dec.bits(1);
                let offset =
                    (q2 as f32 - 0.5) * (1i32 << (14 - fine_quant[i] - 1).max(0)) as f32 * (1.0 / 16384.0);
                old[i + ch * NB_EBANDS] += offset;
                bits_left -= 1;
            }
            i += 1;
        }
    }
}

fn tf_decode(
    start: usize,
    end: usize,
    transient: bool,
    tf_res: &mut [i32; NB_EBANDS],
    lm: usize,
    dec: &mut EcDec<'_>,
) {
    let mut budget = dec.storage * 8;
    let mut tell = dec.tell() as u32;
    let mut logp: u32 = if transient { 2 } else { 4 };
    let tf_select_rsv = lm > 0 && tell + logp < budget;
    budget -= u32::from(tf_select_rsv);
    let mut tf_changed = 0;
    let mut curr = 0;
    for r in tf_res.iter_mut().take(end).skip(start) {
        if tell + logp <= budget {
            curr ^= i32::from(dec.bit_logp(logp));
            tell = dec.tell() as u32;
            tf_changed |= curr;
        }
        *r = curr;
        logp = if transient { 4 } else { 5 };
    }
    let t = usize::from(transient);
    let mut tf_select = 0;
    if tf_select_rsv
        && TF_SELECT_TABLE[lm][4 * t + tf_changed as usize]
            != TF_SELECT_TABLE[lm][4 * t + 2 + tf_changed as usize]
    {
        tf_select = usize::from(dec.bit_logp(1));
    }
    for r in tf_res.iter_mut().take(end).skip(start) {
        *r = i32::from(TF_SELECT_TABLE[lm][4 * t + 2 * tf_select + (*r & 1) as usize]);
    }
}

/// `comb_filter` in place on `buf[x..x + n]` (history before `x`), or from
/// `src` (same layout) into `dst[..n]` when `out` is given.
fn comb_filter(
    buf: &mut [f32],
    x: usize,
    out: Option<&mut [f32]>,
    t0: i32,
    t1: i32,
    n: usize,
    g0: f32,
    g1: f32,
    tapset0: i32,
    tapset1: i32,
    window: Option<&[f32]>,
    mut overlap: usize,
) {
    const GAINS: [[f32; 3]; 3] = [
        [0.306_640_63, 0.217_041_02, 0.129_638_67],
        [0.463_867_2, 0.268_066_4, 0.0],
        [0.799_804_7, 0.100_097_656, 0.0],
    ];
    let mut out = out;
    if g0 == 0.0 && g1 == 0.0 {
        if let Some(o) = out.as_deref_mut() {
            o[..n].copy_from_slice(&buf[x..x + n]);
        }
        return;
    }
    let t0 = t0.max(COMBFILTER_MINPERIOD) as usize;
    let t1 = t1.max(COMBFILTER_MINPERIOD) as usize;
    let ts0 = tapset0.clamp(0, 2) as usize;
    let ts1 = tapset1.clamp(0, 2) as usize;
    let g00 = g0 * GAINS[ts0][0];
    let g01 = g0 * GAINS[ts0][1];
    let g02 = g0 * GAINS[ts0][2];
    let g10 = g1 * GAINS[ts1][0];
    let g11 = g1 * GAINS[ts1][1];
    let g12 = g1 * GAINS[ts1][2];
    if x < t0.max(t1) + 2 {
        return;
    }
    let mut x1 = buf[x - t1 + 1];
    let mut x2 = buf[x - t1];
    let mut x3 = buf[x - t1 - 1];
    let mut x4 = buf[x - t1 - 2];
    if g0 == g1 && t0 == t1 && tapset0 == tapset1 {
        overlap = 0;
    }
    let window = window.unwrap_or(&[]);
    let overlap = overlap.min(window.len()).min(n);
    let mut i = 0;
    while i < overlap {
        let x0 = buf[x + i - t1 + 2];
        let f = window[i] * window[i];
        let v = buf[x + i]
            + ((1.0 - f) * g00) * buf[x + i - t0]
            + ((1.0 - f) * g01) * (buf[x + i - t0 + 1] + buf[x + i - t0 - 1])
            + ((1.0 - f) * g02) * (buf[x + i - t0 + 2] + buf[x + i - t0 - 2])
            + (f * g10) * x2
            + (f * g11) * (x1 + x3)
            + (f * g12) * (x0 + x4);
        match out.as_deref_mut() {
            Some(o) => o[i] = v,
            None => buf[x + i] = v,
        }
        x4 = x3;
        x3 = x2;
        x2 = x1;
        x1 = x0;
        i += 1;
    }
    if g1 == 0.0 {
        if let Some(o) = out.as_deref_mut() {
            o[overlap..n].copy_from_slice(&buf[x + overlap..x + n]);
        }
        return;
    }
    // comb_filter_const
    let mut x4 = buf[x + i - t1 - 2];
    let mut x3 = buf[x + i - t1 - 1];
    let mut x2 = buf[x + i - t1];
    let mut x1 = buf[x + i - t1 + 1];
    for k in i..n {
        let x0 = buf[x + k - t1 + 2];
        let v = buf[x + k] + g10 * x2 + g11 * (x1 + x3) + g12 * (x0 + x4);
        match out.as_deref_mut() {
            Some(o) => o[k] = v,
            None => buf[x + k] = v,
        }
        x4 = x3;
        x3 = x2;
        x2 = x1;
        x1 = x0;
    }
}

/// `deemphasis` of `cc` channels of `n` samples ending at
/// `DECODE_BUFFER_SIZE` in each channel's history into interleaved `pcm`.
fn deemphasis(mem: &[f32], pcm: &mut [f32], n: usize, cc: usize, mem_d: &mut [f32; 2]) {
    for c in 0..cc {
        let x = &mem[c * MEM_PER_CH + DECODE_BUFFER_SIZE - n..c * MEM_PER_CH + DECODE_BUFFER_SIZE];
        let mut m = mem_d[c];
        for (j, &xv) in x.iter().enumerate() {
            let tmp = xv + 1e-30 + m;
            m = PREEMPH * tmp;
            pcm[j * cc + c] = tmp * (1.0 / 32768.0);
        }
        mem_d[c] = m;
    }
}

/// `celt_synthesis`: `X` (normalised, `c` channels of `n` at `scratch[0..]`)
/// to time domain in the history of `cc` channels. Uses the norm area of
/// `scratch` for the spectrum.
fn synthesis(
    scratch: &mut [f32],
    mem: &mut [f32],
    old_band_e: &[f32],
    start: usize,
    eff_end: usize,
    c: usize,
    cc: usize,
    transient: bool,
    lm: usize,
    silence: bool,
) {
    let m = 1usize << lm;
    let n = m * SHORT_MDCT;
    let (b, nb, shift) = if transient { (m, SHORT_MDCT, 3) } else { (1, SHORT_MDCT << lm, 3 - lm) };
    let (xs, freq_area) = scratch.split_at_mut(2 * 960);
    let freq = &mut freq_area[..n];
    let out_off = |ch: usize| ch * MEM_PER_CH + DECODE_BUFFER_SIZE - n;
    if cc == 2 && c == 1 {
        denormalise_bands(xs, freq, old_band_e, start, eff_end, m, silence);
        // Copy to the second channel's output as scratch (the IMDCT destroys its input).
        let f2 = out_off(1) + OVERLAP / 2;
        mem[f2..f2 + n].copy_from_slice(freq);
        for bi in 0..b {
            let o = out_off(0) + nb * bi;
            // Input is in mem: copy it out first to avoid aliasing.
            let (lo, hi) = mem.split_at_mut(f2);
            mdct_backward(&hi[bi..], &mut lo[o..], &WINDOW, OVERLAP, shift, b);
        }
        for bi in 0..b {
            let o = out_off(1) + nb * bi;
            mdct_backward(&freq[bi..], &mut mem[o..], &WINDOW, OVERLAP, shift, b);
        }
    } else if cc == 1 && c == 2 {
        denormalise_bands(xs, freq, old_band_e, start, eff_end, m, silence);
        let f2 = out_off(0) + OVERLAP / 2;
        denormalise_bands(
            &xs[n..],
            &mut mem[f2..f2 + n],
            &old_band_e[NB_EBANDS..],
            start,
            eff_end,
            m,
            silence,
        );
        for i in 0..n {
            freq[i] = 0.5 * freq[i] + 0.5 * mem[f2 + i];
        }
        for bi in 0..b {
            let o = out_off(0) + nb * bi;
            mdct_backward(&freq[bi..], &mut mem[o..], &WINDOW, OVERLAP, shift, b);
        }
    } else {
        for ch in 0..cc {
            denormalise_bands(&xs[ch * n..], freq, &old_band_e[ch * NB_EBANDS..], start, eff_end, m, silence);
            for bi in 0..b {
                let o = out_off(ch) + nb * bi;
                mdct_backward(&freq[bi..], &mut mem[o..], &WINDOW, OVERLAP, shift, b);
            }
        }
    }
}

fn prefilter_and_fold(st: &CeltState, mem: &mut [f32], n: usize) {
    let cc = st.channels as usize;
    let mut etmp = [0f32; OVERLAP];
    for c in 0..cc {
        let base = c * MEM_PER_CH;
        comb_filter(
            &mut mem[base..base + MEM_PER_CH],
            DECODE_BUFFER_SIZE - n,
            Some(&mut etmp),
            st.postfilter_period_old,
            st.postfilter_period,
            OVERLAP,
            -st.postfilter_gain_old,
            -st.postfilter_gain,
            st.postfilter_tapset_old,
            st.postfilter_tapset,
            None,
            0,
        );
        for i in 0..OVERLAP / 2 {
            mem[base + DECODE_BUFFER_SIZE - n + i] =
                WINDOW[i] * etmp[OVERLAP - 1 - i] + WINDOW[OVERLAP - i - 1] * etmp[i];
        }
    }
}

fn decode_lost(st: &mut CeltState, mem: &mut [f32], n: usize, lm: usize, scratch: &mut [f32]) {
    let c_count = st.channels as usize;
    let loss_duration = st.loss_duration;
    let start = st.start as usize;
    let noise_based = loss_duration >= 40 || start != 0 || st.skip_plc != 0;
    if noise_based {
        let end = st.end as usize;
        let eff_end = start.max(end.min(NB_EBANDS));
        for c in 0..c_count {
            let b = c * MEM_PER_CH;
            mem.copy_within(b + n..b + MEM_PER_CH, b);
        }
        if st.prefilter_and_fold != 0 {
            prefilter_and_fold(st, mem, n);
        }
        let decay = if loss_duration == 0 { 1.5 } else { 0.5 };
        for c in 0..c_count {
            for i in start..end {
                let k = c * NB_EBANDS + i;
                st.old_band_e[k] = st.background_log_e[k].max(st.old_band_e[k] - decay);
            }
        }
        let mut seed = st.rng;
        for c in 0..c_count {
            for i in start..eff_end {
                let boffs = n * c + ((EBANDS[i] as usize) << lm);
                let blen = ((EBANDS[i + 1] - EBANDS[i]) as usize) << lm;
                for j in 0..blen {
                    seed = lcg_rand(seed);
                    scratch[boffs + j] = ((seed as i32) >> 20) as f32;
                }
                renormalise_vector(&mut scratch[boffs..boffs + blen], 1.0);
            }
        }
        st.rng = seed;
        synthesis(scratch, mem, &st.old_band_e, start, eff_end, c_count, c_count, false, lm, false);
        st.prefilter_and_fold = 0;
        st.skip_plc = 1;
    } else {
        let mut fade = 1.0f32;
        let pitch_index;
        if loss_duration == 0 {
            // celt_plc_pitch_search
            let (lp, rest) = scratch.split_at_mut(DECODE_BUFFER_SIZE >> 1);
            let x1 =
                if c_count == 2 { Some(&mem[MEM_PER_CH..MEM_PER_CH + DECODE_BUFFER_SIZE]) } else { None };
            plc::pitch_downsample(&mem[..DECODE_BUFFER_SIZE], x1, lp, DECODE_BUFFER_SIZE, rest);
            let p = plc::pitch_search(
                &lp[PLC_PITCH_LAG_MAX >> 1..],
                lp,
                DECODE_BUFFER_SIZE - PLC_PITCH_LAG_MAX,
                PLC_PITCH_LAG_MAX - PLC_PITCH_LAG_MIN,
                rest,
            );
            pitch_index = (PLC_PITCH_LAG_MAX as i32 - p).clamp(1, MAX_PERIOD as i32) as usize;
            st.last_pitch_index = pitch_index as i32;
        } else {
            pitch_index = (st.last_pitch_index as usize).clamp(1, MAX_PERIOD);
            fade = 0.8;
        }
        let exc_length = (2 * pitch_index).min(MAX_PERIOD);
        let (exc_all, rest) = scratch.split_at_mut(MAX_PERIOD + LPC_ORDER);
        let (fir_tmp, rest) = rest.split_at_mut(MAX_PERIOD);
        let xx = &mut rest[..MAX_PERIOD];
        for c in 0..c_count {
            let base = c * MEM_PER_CH;
            for i in 0..MAX_PERIOD + LPC_ORDER {
                exc_all[i] = mem[base + DECODE_BUFFER_SIZE - MAX_PERIOD - LPC_ORDER + i];
            }
            let lpc = &mut st.lpc[c * LPC_ORDER..(c + 1) * LPC_ORDER];
            if loss_duration == 0 {
                let mut ac = [0f32; LPC_ORDER + 1];
                plc::autocorr(
                    &exc_all[LPC_ORDER..],
                    &mut ac,
                    Some(&WINDOW),
                    OVERLAP,
                    LPC_ORDER,
                    MAX_PERIOD,
                    xx,
                );
                ac[0] *= 1.0001;
                for (i, a) in ac.iter_mut().enumerate().skip(1) {
                    *a -= *a * (0.008 * 0.008) * i as f32 * i as f32;
                }
                plc::lpc(lpc, &ac, LPC_ORDER);
            }
            let x0 = LPC_ORDER + MAX_PERIOD - exc_length;
            plc::fir(exc_all, x0, lpc, fir_tmp, exc_length, LPC_ORDER);
            exc_all[x0..x0 + exc_length].copy_from_slice(&fir_tmp[..exc_length]);
            let exc = &exc_all[LPC_ORDER..];
            let decay;
            {
                let mut e1 = 1f32;
                let mut e2 = 1f32;
                let dl = exc_length >> 1;
                for i in 0..dl {
                    let e = exc[MAX_PERIOD - dl + i];
                    e1 += e * e;
                    let e = exc[MAX_PERIOD - 2 * dl + i];
                    e2 += e * e;
                }
                e1 = e1.min(e2);
                decay = sqrt(e1 / e2);
            }
            mem.copy_within(base + n..base + DECODE_BUFFER_SIZE, base);
            let extrapolation_offset = MAX_PERIOD - pitch_index;
            let extrapolation_len = n + OVERLAP;
            let mut attenuation = fade * decay;
            let mut s1 = 0f32;
            let mut j = 0usize;
            for i in 0..extrapolation_len {
                if j >= pitch_index {
                    j -= pitch_index;
                    attenuation *= decay;
                }
                mem[base + DECODE_BUFFER_SIZE - n + i] = attenuation * exc[extrapolation_offset + j];
                let tmp = mem[base + DECODE_BUFFER_SIZE - MAX_PERIOD - n + extrapolation_offset + j];
                s1 += tmp * tmp;
                j += 1;
            }
            let mut lpc_mem = [0f32; LPC_ORDER];
            for (i, m) in lpc_mem.iter_mut().enumerate() {
                *m = mem[base + DECODE_BUFFER_SIZE - n - 1 - i];
            }
            let lpc = &st.lpc[c * LPC_ORDER..(c + 1) * LPC_ORDER];
            plc::iir(
                &mut mem[base + DECODE_BUFFER_SIZE - n..base + DECODE_BUFFER_SIZE - n + extrapolation_len],
                lpc,
                extrapolation_len,
                LPC_ORDER,
                &mut lpc_mem,
            );
            let mut s2 = 0f32;
            for i in 0..extrapolation_len {
                let tmp = mem[base + DECODE_BUFFER_SIZE - n + i];
                s2 += tmp * tmp;
            }
            if !(s1 > 0.2 * s2) {
                for i in 0..extrapolation_len {
                    mem[base + DECODE_BUFFER_SIZE - n + i] = 0.0;
                }
            } else if s1 < s2 {
                let ratio = sqrt((s1 + 1.0) / (s2 + 1.0));
                for i in 0..OVERLAP {
                    let g = 1.0 - WINDOW[i] * (1.0 - ratio);
                    mem[base + DECODE_BUFFER_SIZE - n + i] *= g;
                }
                for i in OVERLAP..extrapolation_len {
                    mem[base + DECODE_BUFFER_SIZE - n + i] *= ratio;
                }
            }
        }
        st.prefilter_and_fold = 1;
    }
    st.loss_duration = (loss_duration + (1 << lm)).min(10000);
}

/// Errors from [`decode`] (the C code's negative returns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeltError {
    BadArg,
    Internal,
}

/// `celt_decode_with_ec`: decode `frame_size` samples (120 << LM) into
/// interleaved `pcm` (`channels` of them). `data == None` (or one byte or
/// less) runs the concealment. With `ec`, the range decoder of a hybrid
/// frame continues; otherwise one is made over `data`.
pub fn decode<'d>(
    st: &mut CeltState,
    mem: &mut [f32],
    data: Option<&'d [u8]>,
    pcm: &mut [f32],
    frame_size: usize,
    ec: Option<&mut EcDec<'d>>,
    scratch: &mut [f32],
) -> Result<usize, CeltError> {
    let cc = st.channels as usize;
    let c = st.stream_channels as usize;
    let start = st.start as usize;
    let end = st.end as usize;
    let Some(lm) = (0..=3).find(|&l| SHORT_MDCT << l == frame_size) else {
        return Err(CeltError::BadArg);
    };
    let m = 1usize << lm;
    let n = m * SHORT_MDCT;
    let len = data.map_or(0, <[u8]>::len);
    if len > 1275 {
        return Err(CeltError::BadArg);
    }
    let eff_end = end.min(NB_EBANDS);
    let data = match data {
        Some(d) if d.len() > 1 => d,
        _ => {
            decode_lost(st, mem, n, lm, scratch);
            deemphasis(mem, pcm, n, cc, &mut st.preemph_mem_d);
            return Ok(frame_size);
        }
    };
    if st.loss_duration == 0 {
        st.skip_plc = 0;
    }
    let mut own;
    let dec: &mut EcDec<'d> = match ec {
        Some(d) => d,
        None => {
            own = EcDec::new(data);
            &mut own
        }
    };
    if c == 1 {
        for i in 0..NB_EBANDS {
            st.old_band_e[i] = st.old_band_e[i].max(st.old_band_e[NB_EBANDS + i]);
        }
    }
    let mut total_bits = (len * 8) as i32;
    let mut tell = dec.tell();
    let silence = if tell >= total_bits {
        true
    } else if tell == 1 {
        dec.bit_logp(15)
    } else {
        false
    };
    if silence {
        // Pretend we've read all the remaining bits.
        tell = len as i32 * 8;
        dec.skip_to(tell);
    }
    let mut postfilter_gain = 0f32;
    let mut postfilter_pitch = 0i32;
    let mut postfilter_tapset = 0i32;
    if start == 0 && tell + 16 <= total_bits {
        if dec.bit_logp(1) {
            let octave = dec.uint(6) as i32;
            postfilter_pitch = (16 << octave) + dec.bits((4 + octave) as u32) as i32 - 1;
            let qg = dec.bits(3) as i32;
            if dec.tell() + 2 <= total_bits {
                postfilter_tapset = dec.icdf(&TAPSET_ICDF, 2);
            }
            postfilter_gain = 0.09375 * (qg + 1) as f32;
        }
        tell = dec.tell();
    }
    let transient = if lm > 0 && tell + 3 <= total_bits {
        let t = dec.bit_logp(3);
        tell = dec.tell();
        t
    } else {
        false
    };
    let short_blocks = transient;
    let intra_ener = if tell + 3 <= total_bits { dec.bit_logp(3) } else { false };
    if !intra_ener && st.loss_duration != 0 {
        for ch in 0..2 {
            let missing = 10.min(st.loss_duration >> lm);
            let safety = match lm {
                0 => 1.5,
                1 => 0.5,
                _ => 0.0,
            };
            for i in start..end {
                let k = ch * NB_EBANDS + i;
                if st.old_band_e[k] < st.old_log_e[k].max(st.old_log_e2[k]) {
                    let e0 = st.old_band_e[k];
                    let e1 = st.old_log_e[k];
                    let e2 = st.old_log_e2[k];
                    let slope = (e1 - e0).max(0.5 * (e2 - e0));
                    let e0 = e0 - (0.0f32).max((1 + missing) as f32 * slope);
                    st.old_band_e[k] = (-20.0f32).max(e0);
                } else {
                    st.old_band_e[k] = st.old_band_e[k].min(st.old_log_e[k]).min(st.old_log_e2[k]);
                }
                st.old_band_e[k] -= safety;
            }
        }
    }
    unquant_coarse_energy(&mut st.old_band_e, start, end, intra_ener, dec, c, lm);
    let mut tf_res = [0i32; NB_EBANDS];
    tf_decode(start, end, transient, &mut tf_res, lm, dec);
    tell = dec.tell();
    let mut spread = SPREAD_NORMAL;
    if tell + 4 <= total_bits {
        spread = dec.icdf(&SPREAD_ICDF, 5);
    }
    let mut cap = [0i32; NB_EBANDS];
    init_caps(&mut cap, lm, c);
    let mut offsets = [0i32; NB_EBANDS];
    let mut dynalloc_logp = 6;
    total_bits <<= BITRES;
    let mut tellf = dec.tell_frac() as i32;
    for i in start..end {
        let width = (c as i32 * (EBANDS[i + 1] - EBANDS[i]) as i32) << lm;
        let quanta = (width << BITRES).min((6 << BITRES).max(width));
        let mut loop_logp = dynalloc_logp;
        let mut boost = 0;
        while tellf + (loop_logp << BITRES) < total_bits && boost < cap[i] {
            let flag = dec.bit_logp(loop_logp as u32);
            tellf = dec.tell_frac() as i32;
            if !flag {
                break;
            }
            boost += quanta;
            total_bits -= quanta;
            loop_logp = 1;
        }
        offsets[i] = boost;
        if boost > 0 {
            dynalloc_logp = 2.max(dynalloc_logp - 1);
        }
    }
    let alloc_trim = if tellf + (6 << BITRES) <= total_bits { dec.icdf(&TRIM_ICDF, 7) } else { 5 };
    let mut bits = (((len * 8) as i32) << BITRES) - dec.tell_frac() as i32 - 1;
    let anti_collapse_rsv =
        if transient && lm >= 2 && bits >= ((lm as i32 + 2) << BITRES) { 1 << BITRES } else { 0 };
    bits -= anti_collapse_rsv;
    let alloc = compute_allocation(start, end, &offsets, &cap, alloc_trim, bits, c as i32, lm as i32, dec);
    unquant_fine_energy(&mut st.old_band_e, start, end, &alloc.ebits, dec, c);
    for ch in 0..cc {
        let b = ch * MEM_PER_CH;
        mem.copy_within(b + n..b + MEM_PER_CH, b);
    }
    let mut collapse_masks = [0u8; 2 * NB_EBANDS];
    let mut seed = st.rng;
    quant_all_bands(
        start,
        end,
        scratch,
        n,
        c,
        2 * 960,
        &mut collapse_masks,
        &alloc.pulses,
        short_blocks,
        spread,
        alloc.dual_stereo,
        alloc.intensity,
        &tf_res,
        (len as i32 * (8 << BITRES)) - anti_collapse_rsv,
        alloc.balance,
        dec,
        lm,
        alloc.coded_bands,
        &mut seed,
        st.disable_inv != 0,
    );
    st.rng = seed;
    let anti_collapse_on = anti_collapse_rsv > 0 && dec.bits(1) != 0;
    let left = len as i32 * 8 - dec.tell();
    unquant_energy_finalise(&mut st.old_band_e, start, end, &alloc.ebits, &alloc.fine_priority, left, dec, c);
    if anti_collapse_on {
        anti_collapse(
            scratch,
            &collapse_masks,
            lm,
            c,
            n,
            start,
            end,
            &st.old_band_e,
            &st.old_log_e,
            &st.old_log_e2,
            &alloc.pulses,
            st.rng,
        );
    }
    if silence {
        for v in st.old_band_e[..c * NB_EBANDS].iter_mut() {
            *v = -28.0;
        }
    }
    if st.prefilter_and_fold != 0 {
        prefilter_and_fold(st, mem, n);
    }
    synthesis(scratch, mem, &st.old_band_e, start, eff_end, c, cc, transient, lm, silence);
    for ch in 0..cc {
        let base = ch * MEM_PER_CH;
        let x = DECODE_BUFFER_SIZE - n;
        st.postfilter_period = st.postfilter_period.max(COMBFILTER_MINPERIOD);
        st.postfilter_period_old = st.postfilter_period_old.max(COMBFILTER_MINPERIOD);
        comb_filter(
            &mut mem[base..base + MEM_PER_CH],
            x,
            None,
            st.postfilter_period_old,
            st.postfilter_period,
            SHORT_MDCT,
            st.postfilter_gain_old,
            st.postfilter_gain,
            st.postfilter_tapset_old,
            st.postfilter_tapset,
            Some(&WINDOW),
            OVERLAP,
        );
        if lm != 0 {
            comb_filter(
                &mut mem[base..base + MEM_PER_CH],
                x + SHORT_MDCT,
                None,
                st.postfilter_period,
                postfilter_pitch,
                n - SHORT_MDCT,
                st.postfilter_gain,
                postfilter_gain,
                st.postfilter_tapset,
                postfilter_tapset,
                Some(&WINDOW),
                OVERLAP,
            );
        }
    }
    st.postfilter_period_old = st.postfilter_period;
    st.postfilter_gain_old = st.postfilter_gain;
    st.postfilter_tapset_old = st.postfilter_tapset;
    st.postfilter_period = postfilter_pitch;
    st.postfilter_gain = postfilter_gain;
    st.postfilter_tapset = postfilter_tapset;
    if lm != 0 {
        st.postfilter_period_old = st.postfilter_period;
        st.postfilter_gain_old = st.postfilter_gain;
        st.postfilter_tapset_old = st.postfilter_tapset;
    }
    if c == 1 {
        let (a, b) = st.old_band_e.split_at_mut(NB_EBANDS);
        b.copy_from_slice(a);
    }
    if transient {
        for i in 0..2 * NB_EBANDS {
            st.old_log_e[i] = st.old_log_e[i].min(st.old_band_e[i]);
        }
    } else {
        st.old_log_e2 = st.old_log_e;
        st.old_log_e = st.old_band_e;
    }
    let max_bg = (160.min(st.loss_duration + m as i32)) as f32 * 0.001;
    for i in 0..2 * NB_EBANDS {
        st.background_log_e[i] = (st.background_log_e[i] + max_bg).min(st.old_band_e[i]);
    }
    for ch in 0..2 {
        for i in (0..start).chain(end..NB_EBANDS) {
            let k = ch * NB_EBANDS + i;
            st.old_band_e[k] = 0.0;
            st.old_log_e[k] = -28.0;
            st.old_log_e2[k] = -28.0;
        }
    }
    st.rng = dec.rng;
    deemphasis(mem, pcm, n, cc, &mut st.preemph_mem_d);
    st.loss_duration = 0;
    st.prefilter_and_fold = 0;
    if dec.tell() > 8 * len as i32 {
        return Err(CeltError::Internal);
    }
    if dec.error {
        st.error = 1;
    }
    Ok(frame_size)
}
