//! Signal processing of the libspeex decoder, float build: `filters.c`
//! (`bw_lpc`, `sanitize_values32`, `highpass`, `iir_mem16`, `compute_rms`,
//! `qmf_synth`, `interp_pitch`, `multicomb`), `lsp.c` (`lsp_interpolate`,
//! `lsp_to_lpc`), `quant_lsp.c` (the three LSP de-quantisers), `ltp.c`
//! (`pitch_unquant_3tap`, `forced_pitch_unquant`, `inner_prod`) and
//! `cb_search.c` (`split_cb_shape_sign_unquant`, `noise_codebook_unquant`).
//!
//! The C code addresses the excitation with negative offsets from a pointer
//! into a history buffer; here a buffer and a base index stand for that
//! pointer. The index bounds are those libspeex relies on (pitch ≤ 144,
//! the history sizes of `nb_celp.h`); every pitch value is clamped to them
//! before use.

use crate::bits::Bits;
use crate::math;
use crate::modes::{LspKind, SplitCb};
use crate::tables;

/// `inner_prod` in the summation order of libspeex's SSE build (four
/// partial sums over blocks of eight), which the reference decoder uses on
/// x86-64. `len` is a multiple of 8.
#[must_use]
pub fn inner_prod(a: &[f32], b: &[f32], len: usize) -> f32 {
    let mut s = [0.0f32; 4];
    for (ca, cb) in a[..len].chunks_exact(8).zip(b[..len].chunks_exact(8)) {
        for k in 0..4 {
            s[k] += ca[k] * cb[k];
        }
        for k in 0..4 {
            s[k] += ca[4 + k] * cb[4 + k];
        }
    }
    (s[0] + s[2]) + (s[1] + s[3])
}

/// `bw_lpc`: bandwidth expansion `lpc[i] *= gamma^(i+1)`.
pub fn bw_lpc(gamma: f32, lpc_in: &[f32], lpc_out: &mut [f32]) {
    let mut tmp = gamma;
    for (o, &i) in lpc_out.iter_mut().zip(lpc_in) {
        *o = tmp * i;
        tmp *= gamma;
    }
}

/// `sanitize_values32`: clamp, NaN to 0.
pub fn sanitize(v: &mut [f32], min: f32, max: f32) {
    for x in v {
        if !(*x >= min && *x <= max) {
            *x = if *x < min {
                min
            } else if *x > max {
                max
            } else {
                0.0
            };
        }
    }
}

const HP_POLE: [[f32; 3]; 5] = [
    [1.00000, -1.91120, 0.91498],
    [1.00000, -1.92683, 0.93071],
    [1.00000, -1.93338, 0.93553],
    [1.00000, -1.97226, 0.97332],
    [1.00000, -1.37000, 0.39900],
];
const HP_ZERO: [[f32; 3]; 5] = [
    [0.95654, -1.91309, 0.95654],
    [0.96446, -1.92879, 0.96446],
    [0.96723, -1.93445, 0.96723],
    [0.98645, -1.97277, 0.98645],
    [0.88000, -1.76000, 0.88000],
];

/// `highpass` in place (`filt` = libspeex's filter id).
pub fn highpass(y: &mut [f32], filt: usize, mem: &mut [f32; 2]) {
    let den = &HP_POLE[filt.min(4)];
    let num = &HP_ZERO[filt.min(4)];
    for v in y {
        let x = *v;
        let vout = num[0] * x + mem[0];
        mem[0] = (mem[1] + num[1] * x) + -den[1] * vout;
        mem[1] = num[2] * x + -den[2] * vout;
        *v = vout;
    }
}

/// `iir_mem16`: all-pole synthesis filter `1/A(z)`, in place.
pub fn iir_mem16(y: &mut [f32], den: &[f32], mem: &mut [f32]) {
    let ord = den.len().min(mem.len());
    if ord == 0 {
        return;
    }
    for v in y {
        let yi = *v + mem[0];
        let nyi = -yi;
        for j in 0..ord - 1 {
            mem[j] = mem[j + 1] + den[j] * nyi;
        }
        mem[ord - 1] = den[ord - 1] * nyi;
        *v = yi;
    }
}

/// `iir_mem16` from `x` into `y` (distinct buffers).
pub fn iir_mem16_to(x: &[f32], den: &[f32], y: &mut [f32], mem: &mut [f32]) {
    y.copy_from_slice(&x[..y.len()]);
    iir_mem16(y, den, mem);
}

/// `compute_rms16` (float): `sqrt(0.1 + Σx²/len)`.
#[must_use]
pub fn compute_rms(x: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for &v in x {
        sum += v * v;
    }
    math::sqrt(0.1 + sum / x.len().max(1) as f32)
}

/// The QMF prototype filter of `sb_celp.c` (64 taps).
pub static H0: [f32; 64] = [
    3.596189e-05,
    -0.0001123515,
    -0.0001104587,
    0.0002790277,
    0.0002298438,
    -0.0005953563,
    -0.0003823631,
    0.00113826,
    0.0005308539,
    -0.001986177,
    -0.0006243724,
    0.003235877,
    0.0005743159,
    -0.004989147,
    -0.0002584767,
    0.007367171,
    -0.0004857935,
    -0.01050689,
    0.001894714,
    0.01459396,
    -0.004313674,
    -0.01994365,
    0.00828756,
    0.02716055,
    -0.01485397,
    -0.03764973,
    0.026447,
    0.05543245,
    -0.05095487,
    -0.09779096,
    0.1382363,
    0.4600981,
    0.4600981,
    0.1382363,
    -0.09779096,
    -0.05095487,
    0.05543245,
    0.026447,
    -0.03764973,
    -0.01485397,
    0.02716055,
    0.00828756,
    -0.01994365,
    -0.004313674,
    0.01459396,
    0.001894714,
    -0.01050689,
    -0.0004857935,
    0.007367171,
    -0.0002584767,
    -0.004989147,
    0.0005743159,
    0.003235877,
    -0.0006243724,
    -0.001986177,
    0.0005308539,
    0.00113826,
    -0.0003823631,
    -0.0005953563,
    0.0002298438,
    0.0002790277,
    -0.0001104587,
    -0.0001123515,
    3.596189e-05,
];

/// Half the QMF order: the history samples kept per band.
pub const QMF_M2: usize = 32;

/// `qmf_synth` with `M = 64`, in place: `y[0..2·n2]` from the low band
/// `y[0..n2]` and the high band `y[n2..2·n2]`. `mem1`/`mem2` hold the odd
/// entries of libspeex's 64-entry memories; `scratch` takes
/// `2·(n2 + 32)` values.
pub fn qmf_synth(
    y: &mut [f32],
    n2: usize,
    mem1: &mut [f32; QMF_M2],
    mem2: &mut [f32; QMF_M2],
    scratch: &mut [f32],
) {
    let a = &H0;
    let m2 = QMF_M2;
    let (xx1, rest) = scratch.split_at_mut(n2 + m2);
    let xx2 = &mut rest[..n2 + m2];
    for i in 0..n2 {
        xx1[i] = y[n2 - 1 - i];
        xx2[i] = y[2 * n2 - 1 - i];
    }
    xx1[n2..].copy_from_slice(mem1);
    xx2[n2..].copy_from_slice(mem2);
    let mut i = 0;
    while i < n2 {
        let (mut y0, mut y1, mut y2, mut y3) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut x10 = xx1[n2 - 2 - i];
        let mut x20 = xx2[n2 - 2 - i];
        let mut j = 0;
        while j < m2 {
            let mut a0 = a[2 * j];
            let mut a1 = a[2 * j + 1];
            let x11 = xx1[n2 - 1 + j - i];
            let x21 = xx2[n2 - 1 + j - i];
            y0 += a0 * (x11 - x21);
            y1 += a1 * (x11 + x21);
            y2 += a0 * (x10 - x20);
            y3 += a1 * (x10 + x20);
            a0 = a[2 * j + 2];
            a1 = a[2 * j + 3];
            x10 = xx1[n2 + j - i];
            x20 = xx2[n2 + j - i];
            y0 += a0 * (x10 - x20);
            y1 += a1 * (x10 + x20);
            y2 += a0 * (x11 - x21);
            y3 += a1 * (x11 + x21);
            j += 2;
        }
        y[2 * i] = 2.0 * y0;
        y[2 * i + 1] = 2.0 * y1;
        y[2 * i + 2] = 2.0 * y2;
        y[2 * i + 3] = 2.0 * y3;
        i += 2;
    }
    mem1.copy_from_slice(&xx1[..m2]);
    mem2.copy_from_slice(&xx2[..m2]);
}

static SHIFT_FILT: [[f32; 7]; 3] = [
    [-0.011915, 0.046995, -0.152373, 0.614108, 0.614108, -0.152373, 0.046995],
    [-0.0324855, 0.0859768, -0.2042986, 0.9640297, 0.2086420, -0.0302054, -0.0063646],
    [-0.0063646, -0.0302054, 0.2086420, 0.9640297, -0.2042986, 0.0859768, -0.0324855],
];

/// Index `base + off` (the C code's `ptr[off]`).
#[inline]
fn at(base: usize, off: i32) -> usize {
    (base as isize + off as isize) as usize
}

/// `interp_pitch`: the excitation `pitch` samples back (or ahead for a
/// negative `pitch`), interpolated at the best fractional delay.
fn interp_pitch(buf: &[f32], exc: usize, interp: &mut [f32], pitch: i32, len: usize) {
    let mut corr = [[0.0f32; 7]; 4];
    for i in 0..7 {
        let o = at(exc, -pitch - 3 + i as i32);
        corr[0][i] = inner_prod(&buf[exc..], &buf[o..], len);
    }
    for i in 0..3 {
        for j in 0..7usize {
            let i1 = 3usize.saturating_sub(j);
            let i2 = (10 - j).min(7);
            let mut tmp = 0.0f32;
            for k in i1..i2 {
                tmp += SHIFT_FILT[i][k] * corr[0][j + k - 3];
            }
            corr[i + 1][j] = tmp;
        }
    }
    let (mut maxi, mut maxj) = (0usize, 0usize);
    let mut maxcorr = corr[0][0];
    for (i, row) in corr.iter().enumerate() {
        for (j, &c) in row.iter().enumerate() {
            if c > maxcorr {
                maxcorr = c;
                maxi = i;
                maxj = j;
            }
        }
    }
    let shift = pitch - maxj as i32 + 3;
    for (i, out) in interp[..len].iter_mut().enumerate() {
        let mut tmp = 0.0f32;
        if maxi > 0 {
            for k in 0..7 {
                tmp += buf[at(exc, i as i32 - shift + k - 3)] * SHIFT_FILT[maxi - 1][k as usize];
            }
        } else {
            tmp = buf[at(exc, i as i32 - shift)];
        }
        *out = tmp;
    }
}

/// `multicomb`: the perceptual enhancer's comb filter over one 80-sample
/// half frame of excitation at `buf[exc..]`, into `new_exc`. `c1`/`c2` are
/// the comb constants libspeex derives from the sub-mode's comb gain.
#[allow(clippy::too_many_arguments)]
pub fn multicomb(
    buf: &[f32],
    exc: usize,
    new_exc: &mut [f32],
    nsf: usize,
    pitch: i32,
    max_pitch: i32,
    c1: f32,
    c2: f32,
) {
    let mut iexc = [0.0f32; 160];
    let corr_pitch = pitch;
    interp_pitch(buf, exc, &mut iexc[..nsf], corr_pitch, 80);
    if corr_pitch > max_pitch {
        interp_pitch(buf, exc, &mut iexc[nsf..2 * nsf], 2 * corr_pitch, 80);
    } else {
        interp_pitch(buf, exc, &mut iexc[nsf..2 * nsf], -corr_pitch, 80);
    }
    let e = &buf[exc..exc + nsf];
    let iexc0_mag = math::sqrt(1000.0 + inner_prod(&iexc, &iexc, nsf));
    let iexc1_mag = math::sqrt(1000.0 + inner_prod(&iexc[nsf..], &iexc[nsf..], nsf));
    let exc_mag = math::sqrt(1.0 + inner_prod(e, e, nsf));
    let mut corr0 = inner_prod(&iexc, e, nsf);
    if corr0 < 0.0 {
        corr0 = 0.0;
    }
    let mut corr1 = inner_prod(&iexc[nsf..], e, nsf);
    if corr1 < 0.0 {
        corr1 = 0.0;
    }
    let pgain1 = if corr0 > iexc0_mag * exc_mag { 1.0 } else { corr0 / exc_mag / iexc0_mag };
    let pgain2 = if corr1 > iexc1_mag * exc_mag { 1.0 } else { corr1 / exc_mag / iexc1_mag };
    let gg1 = exc_mag / iexc0_mag;
    let gg2 = exc_mag / iexc1_mag;
    let mut g1 = 1.0 - c2 * pgain1 * pgain1;
    let mut g2 = 1.0 - c2 * pgain2 * pgain2;
    if g1 < c1 {
        g1 = c1;
    }
    if g2 < c1 {
        g2 = c1;
    }
    g1 = c1 / g1;
    g2 = c1 / g2;
    let (gain0, gain1) = if corr_pitch > max_pitch {
        (0.7 * (g1 * gg1), 0.3 * (g2 * gg2))
    } else {
        (0.6 * (g1 * gg1), 0.6 * (g2 * gg2))
    };
    for i in 0..nsf {
        new_exc[i] = e[i] + (gain0 * iexc[i] + gain1 * iexc[i + nsf]);
    }
    let mut new_ener = compute_rms(&new_exc[..nsf]);
    let mut old_ener = compute_rms(e);
    if old_ener < 1.0 {
        old_ener = 1.0;
    }
    if new_ener < 1.0 {
        new_ener = 1.0;
    }
    if old_ener > new_ener {
        old_ener = new_ener;
    }
    let ngain = old_ener / new_ener;
    for v in &mut new_exc[..nsf] {
        *v *= ngain;
    }
}

/// `lsp_interpolate` (float build). `top` is `π − margin` rounded as the C
/// code rounds it (in double precision).
#[allow(clippy::too_many_arguments)]
pub fn lsp_interpolate(
    old: &[f32],
    new: &[f32],
    lsp: &mut [f32],
    sub: usize,
    nb_sub: usize,
    margin: f32,
    top: f32,
) {
    let len = lsp.len();
    let tmp = (1.0 + sub as f32) / nb_sub as f32;
    for i in 0..len {
        lsp[i] = (1.0 - tmp) * old[i] + tmp * new[i];
    }
    if lsp[0] < margin {
        lsp[0] = margin;
    }
    if lsp[len - 1] > top {
        lsp[len - 1] = top;
    }
    for i in 1..len - 1 {
        if lsp[i] < lsp[i - 1] + margin {
            lsp[i] = lsp[i - 1] + margin;
        }
        if lsp[i] > lsp[i + 1] - margin {
            lsp[i] = 0.5 * (lsp[i] + lsp[i + 1] - margin);
        }
    }
}

/// `lsp_to_lpc` (float build), order 8 or 10.
pub fn lsp_to_lpc(freq: &[f32], ak: &mut [f32]) {
    let order = freq.len();
    let m = order >> 1;
    let mut wp = [0.0f32; 4 * 5 + 2];
    let mut x_freq = [0.0f32; 10];
    for (x, &f) in x_freq.iter_mut().zip(freq) {
        *x = math::spx_cos(f);
    }
    let mut xin1 = 1.0f32;
    let mut xin2 = 1.0f32;
    for j in 0..=order {
        let mut i2 = 0;
        let mut n4 = 0;
        for i in 0..m {
            let n1 = i * 4;
            let (n2, n3) = (n1 + 1, n1 + 2);
            n4 = n1 + 3;
            let xout1 = xin1 - 2.0 * x_freq[i2] * wp[n1] + wp[n2];
            let xout2 = xin2 - 2.0 * x_freq[i2 + 1] * wp[n3] + wp[n4];
            wp[n2] = wp[n1];
            wp[n4] = wp[n3];
            wp[n1] = xin1;
            wp[n3] = xin2;
            xin1 = xout1;
            xin2 = xout2;
            i2 += 2;
        }
        let xout1 = xin1 + wp[n4 + 1];
        let xout2 = xin2 - wp[n4 + 2];
        if j > 0 {
            ak[j - 1] = (xout1 + xout2) * 0.5;
        }
        wp[n4 + 1] = xin1;
        wp[n4 + 2] = xin2;
        xin1 = 0.0;
        xin2 = 0.0;
    }
}

/// The LSP de-quantisers of `quant_lsp.c`.
pub fn lsp_unquant(kind: LspKind, lsp: &mut [f32], buf: &[u8], bits: &mut Bits) {
    const DIV_256: f32 = 0.003_906_2;
    const DIV_512: f32 = 0.001_953_1;
    const DIV_1024: f32 = 0.000_976_56;
    let c = |v: i8, k: f32| k * f32::from(v);
    match kind {
        LspKind::Nb => {
            for (i, l) in lsp.iter_mut().enumerate() {
                *l = 0.25 * i as f32 + 0.25;
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..10 {
                lsp[i] += c(tables::CDBK_NB[id * 10 + i], DIV_256);
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..5 {
                lsp[i] += c(tables::CDBK_NB_LOW1[id * 5 + i], DIV_512);
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..5 {
                lsp[i] += c(tables::CDBK_NB_LOW2[id * 5 + i], DIV_1024);
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..5 {
                lsp[i + 5] += c(tables::CDBK_NB_HIGH1[id * 5 + i], DIV_512);
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..5 {
                lsp[i + 5] += c(tables::CDBK_NB_HIGH2[id * 5 + i], DIV_1024);
            }
        }
        LspKind::Lbr => {
            for (i, l) in lsp.iter_mut().enumerate() {
                *l = 0.25 * i as f32 + 0.25;
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..10 {
                lsp[i] += c(tables::CDBK_NB[id * 10 + i], DIV_256);
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..5 {
                lsp[i] += c(tables::CDBK_NB_LOW1[id * 5 + i], DIV_512);
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..5 {
                lsp[i + 5] += c(tables::CDBK_NB_HIGH1[id * 5 + i], DIV_512);
            }
        }
        LspKind::High => {
            let order = lsp.len();
            for (i, l) in lsp.iter_mut().enumerate() {
                *l = 0.3125 * i as f32 + 0.75;
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..order {
                lsp[i] += c(tables::HIGH_LSP_CDBK[id * order + i], DIV_256);
            }
            let id = bits.unpack(buf, 6) as usize;
            for i in 0..order {
                lsp[i] += c(tables::HIGH_LSP_CDBK2[id * order + i], DIV_512);
            }
        }
    }
}

/// `pitch_unquant_3tap` (no packet loss): reads the pitch and the gain
/// index, writes the adaptive-codebook contribution into `exc_out`.
/// Returns the pitch and the three gains.
#[allow(clippy::too_many_arguments)]
pub fn pitch_unquant_3tap(
    buf: &[f32],
    exc: usize,
    exc_out: &mut [f32],
    start: i32,
    gain_cdbk: &[i8],
    gain_bits: u32,
    pitch_bits: u32,
    pkt: &[u8],
    bits: &mut Bits,
) -> (i32, [f32; 3]) {
    let pitch = (bits.unpack(pkt, pitch_bits) as i32 + start).clamp(1, crate::nb::PITCH_END);
    let gi = bits.unpack(pkt, gain_bits) as usize * 4;
    let g = |k: usize| 0.015625 * f32::from(gain_cdbk.get(gi + k).copied().unwrap_or(0)) + 0.5;
    let gain = [g(0), g(1), g(2)];
    let nsf = exc_out.len() as i32;
    exc_out.fill(0.0);
    for i in 0..3 {
        let pp = pitch + 1 - i as i32;
        let tmp1 = nsf.min(pp);
        for j in 0..tmp1 {
            exc_out[j as usize] += gain[2 - i] * buf[at(exc, j - pp)];
        }
        let tmp3 = nsf.min(pp + pitch);
        for j in tmp1..tmp3 {
            exc_out[j as usize] += gain[2 - i] * buf[at(exc, j - pp - pitch)];
        }
    }
    (pitch, gain)
}

/// `forced_pitch_unquant`: a pitch of `start` with gain `coef`, written to
/// both `exc_out` and the excitation itself.
pub fn forced_pitch_unquant(
    buf: &mut [f32],
    exc: usize,
    exc_out: &mut [f32],
    start: i32,
    coef: f32,
) -> (i32, [f32; 3]) {
    let coef = if coef > 0.99 { 0.99 } else { coef };
    for (i, o) in exc_out.iter_mut().enumerate() {
        *o = buf[at(exc, i as i32 - start)] * coef;
        buf[exc + i] = *o;
    }
    (start, [0.0, coef, 0.0])
}

/// `split_cb_shape_sign_unquant`: adds the codebook vectors to `exc`.
pub fn split_cb_unquant(exc: &mut [f32], p: &SplitCb, pkt: &[u8], bits: &mut Bits) {
    let mut ind = [0usize; 10];
    let mut signs = [false; 10];
    let n = p.nb_subvect.min(10);
    for i in 0..n {
        signs[i] = p.have_sign && bits.unpack(pkt, 1) != 0;
        ind[i] = bits.unpack(pkt, p.shape_bits) as usize;
    }
    let sz = p.subvect_size;
    for i in 0..n {
        let s = if signs[i] { -1.0f32 } else { 1.0 };
        for j in 0..sz {
            let v = f32::from(p.shape_cb.get(ind[i] * sz + j).copied().unwrap_or(0));
            if let Some(e) = exc.get_mut(sz * i + j) {
                *e += s * 0.03125 * v;
            }
        }
    }
}

/// `noise_codebook_unquant`.
pub fn noise_unquant(exc: &mut [f32], seed: &mut u32) {
    for e in exc {
        *e = math::rand(1.0, seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_to_lpc_flat() {
        // Evenly spaced LSPs give a flat (all-zero) predictor.
        let n = 10;
        let freq: [f32; 10] =
            core::array::from_fn(|i| core::f32::consts::PI * (i as f32 + 1.0) / (n as f32 + 1.0));
        let mut ak = [1.0f32; 10];
        lsp_to_lpc(&freq, &mut ak);
        for a in ak {
            assert!(a.abs() < 1e-4, "{ak:?}");
        }
    }

    #[test]
    fn inner_prod_matches_plain_sum() {
        let a: [f32; 40] = core::array::from_fn(|i| i as f32 * 0.5);
        let b: [f32; 40] = core::array::from_fn(|i| 3.0 - i as f32);
        let plain: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        assert!((inner_prod(&a, &b, 40) - plain).abs() < 1e-3);
    }
}
