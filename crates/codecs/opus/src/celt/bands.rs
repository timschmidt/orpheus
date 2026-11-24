//! Band decoding: `celt/bands.c` of libopus 1.5.2, float build, decoder
//! side only (BSD-3-Clause, see LICENSE-XIPH).
//!
//! All vectors live in one `f32` work slice addressed by offsets (the C
//! code's pointer aliasing between `X`, `norm` and `lowband_scratch`). The
//! recursive `quant_partition` is an explicit stack here (depth ≤ 5).

use super::math::{exp2, frac_mul16, isqrt32, rsqrt, sqrt};
use super::modes::{EBANDS, EMEANS, LOGN, NB_EBANDS, cache_row};
use super::rate::{QTHETA_OFFSET, QTHETA_OFFSET_TWOPHASE, bits2pulses, get_pulses, pulses2bits};
use super::vq::{MAX_N, ROW_LEN, alg_unquant, renormalise_vector};
use crate::entdec::{BITRES, EcDec, ilog};

pub const SPREAD_AGGRESSIVE: i32 = 3;

#[inline]
#[must_use]
pub fn lcg_rand(seed: u32) -> u32 {
    seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

#[must_use]
pub fn bitexact_cos(x: i16) -> i16 {
    let tmp = (4096 + i32::from(x) * i32::from(x)) >> 13;
    let x2 = tmp;
    let x2 = (32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)));
    (1 + x2) as i16
}

#[must_use]
pub fn bitexact_log2tan(isin: i32, icos: i32) -> i32 {
    let lc = ilog(icos as u32);
    let ls = ilog(isin as u32);
    let icos = icos << (15 - lc);
    let isin = isin << (15 - ls);
    (ls - lc) * (1 << 11) + frac_mul16(isin, frac_mul16(isin, -2597) + 7932)
        - frac_mul16(icos, frac_mul16(icos, -2597) + 7932)
}

/// `denormalise_bands`: `x` normalised spectrum → `freq`.
pub fn denormalise_bands(
    x: &[f32],
    freq: &mut [f32],
    band_log_e: &[f32],
    mut start: usize,
    mut end: usize,
    m: usize,
    silence: bool,
) {
    let n = m * 120;
    let mut bound = m * EBANDS[end] as usize;
    if silence {
        bound = 0;
        start = 0;
        end = 0;
    }
    let s0 = m * EBANDS[start] as usize;
    for f in freq[..s0].iter_mut() {
        *f = 0.0;
    }
    for i in start..end {
        let j0 = m * EBANDS[i] as usize;
        let j1 = m * EBANDS[i + 1] as usize;
        let lg = band_log_e[i] + EMEANS[i];
        let g = exp2(lg.min(32.0));
        for j in j0..j1 {
            freq[j] = x[j] * g;
        }
    }
    for f in freq[bound..n].iter_mut() {
        *f = 0.0;
    }
}

/// `anti_collapse` (transient frames only: out of line).
#[inline(never)]
pub fn anti_collapse(
    xbuf: &mut [f32],
    collapse_masks: &[u8],
    lm: usize,
    c_count: usize,
    size: usize,
    start: usize,
    end: usize,
    log_e: &[f32],
    prev1: &[f32],
    prev2: &[f32],
    pulses: &[i32; NB_EBANDS],
    mut seed: u32,
) {
    for i in start..end {
        let n0 = (EBANDS[i + 1] - EBANDS[i]) as usize;
        let depth = (((1 + pulses[i]).max(0) as u32 / n0 as u32) >> lm) as i32;
        let thresh = 0.5 * exp2(-0.125 * depth as f32);
        let sqrt_1 = rsqrt((n0 << lm) as f32);
        for c in 0..c_count {
            let mut p1 = prev1[c * NB_EBANDS + i];
            let mut p2 = prev2[c * NB_EBANDS + i];
            if c_count == 1 {
                p1 = p1.max(prev1[NB_EBANDS + i]);
                p2 = p2.max(prev2[NB_EBANDS + i]);
            }
            let ediff = (log_e[c * NB_EBANDS + i] - p1.min(p2)).max(0.0);
            let mut r = 2.0 * exp2(-ediff);
            if lm == 3 {
                r *= 1.414_213_6;
            }
            r = thresh.min(r);
            r *= sqrt_1;
            let base = c * size + ((EBANDS[i] as usize) << lm);
            let mut renorm = false;
            for k in 0..1usize << lm {
                if collapse_masks[i * c_count + c] & (1 << k) == 0 {
                    for j in 0..n0 {
                        seed = lcg_rand(seed);
                        xbuf[base + (j << lm) + k] = if seed & 0x8000 != 0 { r } else { -r };
                    }
                    renorm = true;
                }
            }
            if renorm {
                renormalise_vector(&mut xbuf[base..base + (n0 << lm)], 1.0);
            }
        }
    }
}

static ORDERY_TABLE: [usize; 30] =
    [1, 0, 3, 0, 2, 1, 7, 0, 4, 3, 6, 1, 5, 2, 15, 0, 8, 7, 12, 3, 11, 4, 14, 1, 9, 6, 13, 2, 10, 5];

/// Reorder `buf[x..x + n0 * stride]` through `buf[tmp..]`.
fn deinterleave_hadamard(buf: &mut [f32], x: usize, tmp: usize, n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    if n > MAX_N || x + n > tmp || tmp + n > buf.len() {
        return;
    }
    let ordery = &ORDERY_TABLE[stride.saturating_sub(2).min(ORDERY_TABLE.len())..];
    for i in 0..stride {
        let row = if hadamard { ordery.get(i).copied().unwrap_or(i) } else { i };
        for j in 0..n0 {
            buf[tmp + row * n0 + j] = buf[x + j * stride + i];
        }
    }
    buf.copy_within(tmp..tmp + n, x);
}

fn interleave_hadamard(buf: &mut [f32], x: usize, tmp: usize, n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    if n > MAX_N || x + n > tmp || tmp + n > buf.len() {
        return;
    }
    let ordery = &ORDERY_TABLE[stride.saturating_sub(2).min(ORDERY_TABLE.len())..];
    for i in 0..stride {
        let row = if hadamard { ordery.get(i).copied().unwrap_or(i) } else { i };
        for j in 0..n0 {
            buf[tmp + j * stride + i] = buf[x + row * n0 + j];
        }
    }
    buf.copy_within(tmp..tmp + n, x);
}

/// `haar1`.
pub fn haar1(x: &mut [f32], n0: usize, stride: usize) {
    let n0 = n0 >> 1;
    for i in 0..stride {
        for j in 0..n0 {
            let a = stride * 2 * j + i;
            let b = stride * (2 * j + 1) + i;
            if b >= x.len() {
                return;
            }
            let tmp1 = 0.707_106_77 * x[a];
            let tmp2 = 0.707_106_77 * x[b];
            x[a] = tmp1 + tmp2;
            x[b] = tmp1 - tmp2;
        }
    }
}

fn compute_qn(n: i32, b: i32, offset: i32, pulse_cap: i32, stereo: bool) -> i32 {
    const EXP2_TABLE8: [i32; 8] = [16384, 17866, 19483, 21247, 23170, 25267, 27554, 30048];
    let mut n2 = 2 * n - 1;
    if stereo && n == 2 {
        n2 -= 1;
    }
    let mut qb = (b + n2 * offset) / n2.max(1);
    qb = qb.min(b - pulse_cap - (4 << BITRES));
    qb = qb.min(8 << BITRES);
    if qb < (1 << BITRES >> 1) {
        1
    } else {
        let qn = EXP2_TABLE8[(qb & 7) as usize] >> (14 - (qb >> BITRES));
        ((qn + 1) >> 1) << 1
    }
}

/// `struct band_ctx` (decoder fields).
pub struct BandCtx<'a, 'b> {
    pub ec: &'a mut EcDec<'b>,
    pub i: usize,
    pub intensity: i32,
    pub spread: i32,
    pub tf_change: i32,
    pub remaining_bits: i32,
    pub seed: u32,
    pub disable_inv: bool,
    pub avoid_split_noise: bool,
    /// Offset of the `MAX_N`-float temporary area in the work slice,
    /// followed by the `ROW_LEN` words of the PVQ row.
    pub tmp: usize,
}

struct Split {
    inv: bool,
    imid: i32,
    iside: i32,
    delta: i32,
    itheta: i32,
    qalloc: i32,
}

fn compute_theta(
    ctx: &mut BandCtx<'_, '_>,
    n: i32,
    b: &mut i32,
    bb: i32,
    b0: i32,
    lm: i32,
    stereo: bool,
    fill: &mut i32,
) -> Split {
    let i = ctx.i;
    let pulse_cap = i32::from(LOGN[i]) + lm * (1 << BITRES);
    let offset = (pulse_cap >> 1) - if stereo && n == 2 { QTHETA_OFFSET_TWOPHASE } else { QTHETA_OFFSET };
    let mut qn = compute_qn(n, *b, offset, pulse_cap, stereo);
    if stereo && i as i32 >= ctx.intensity {
        qn = 1;
    }
    let tell = ctx.ec.tell_frac() as i32;
    let mut itheta = 0i32;
    let mut inv = false;
    let ec = &mut *ctx.ec;
    if qn != 1 {
        if stereo && n > 2 {
            let p0 = 3;
            let x0 = qn / 2;
            let ft = (p0 * (x0 + 1) + x0) as u32;
            let fs = ec.decode(ft) as i32;
            let x = if fs < (x0 + 1) * p0 { fs / p0 } else { x0 + 1 + (fs - (x0 + 1) * p0) };
            let (fl, fh) = if x <= x0 {
                (p0 * x, p0 * (x + 1))
            } else {
                ((x - 1 - x0) + (x0 + 1) * p0, (x - x0) + (x0 + 1) * p0)
            };
            ec.update(fl as u32, fh as u32, ft);
            itheta = x;
        } else if b0 > 1 || stereo {
            itheta = ec.uint((qn + 1) as u32) as i32;
        } else {
            let ft = ((qn >> 1) + 1) * ((qn >> 1) + 1);
            let fm = ec.decode(ft as u32) as i32;
            let (fl, fs);
            if fm < (((qn >> 1) * ((qn >> 1) + 1)) >> 1) {
                itheta = (isqrt32(8 * fm as u32 + 1) as i32 - 1) >> 1;
                fs = itheta + 1;
                fl = (itheta * (itheta + 1)) >> 1;
            } else {
                itheta = (2 * (qn + 1) - isqrt32(8 * (ft - fm - 1).max(0) as u32 + 1) as i32) >> 1;
                fs = qn + 1 - itheta;
                fl = ft - (((qn + 1 - itheta) * (qn + 2 - itheta)) >> 1);
            }
            ec.update(fl as u32, (fl + fs) as u32, ft as u32);
        }
        itheta = ((itheta.max(0) as u32 * 16384) / qn as u32) as i32;
    } else if stereo {
        if *b > 2 << BITRES && ctx.remaining_bits > 2 << BITRES {
            inv = ec.bit_logp(2);
        }
        if ctx.disable_inv {
            inv = false;
        }
        itheta = 0;
    }
    let qalloc = ctx.ec.tell_frac() as i32 - tell;
    *b -= qalloc;
    let (imid, iside, delta);
    if itheta == 0 {
        imid = 32767;
        iside = 0;
        *fill &= (1 << bb) - 1;
        delta = -16384;
    } else if itheta == 16384 {
        imid = 0;
        iside = 32767;
        *fill &= ((1 << bb) - 1) << bb;
        delta = 16384;
    } else {
        imid = i32::from(bitexact_cos(itheta as i16));
        iside = i32::from(bitexact_cos((16384 - itheta) as i16));
        delta = frac_mul16((n - 1) << 7, bitexact_log2tan(iside, imid));
    }
    Split { inv, imid, iside, delta, itheta, qalloc }
}

fn quant_band_n1(
    ctx: &mut BandCtx<'_, '_>,
    buf: &mut [f32],
    x: usize,
    y: Option<usize>,
    lowband_out: Option<usize>,
) -> u32 {
    for ch in [Some(x), y].into_iter().flatten() {
        let mut sign = 0;
        if ctx.remaining_bits >= 1 << BITRES {
            sign = ctx.ec.bits(1);
            ctx.remaining_bits -= 1 << BITRES;
        }
        buf[ch] = if sign != 0 { -1.0 } else { 1.0 };
    }
    if let Some(lo) = lowband_out {
        buf[lo] = buf[x];
    }
    1
}

#[derive(Clone, Copy, Default)]
struct Frame {
    x: usize,
    n: usize,
    b: i32,
    bb: i32,
    lowband: Option<usize>,
    lm: i32,
    gain: f32,
    fill: i32,
    stage: u8,
    b0: i32,
    mbits: i32,
    sbits: i32,
    itheta: i32,
    mid: f32,
    side: f32,
    rebalance: i32,
    cm: u32,
    mid_first: bool,
}

/// `quant_partition` without recursion.
fn quant_partition(
    ctx: &mut BandCtx<'_, '_>,
    buf: &mut [f32],
    x: usize,
    n: usize,
    b: i32,
    bb: i32,
    lowband: Option<usize>,
    lm: i32,
    gain: f32,
    fill: i32,
) -> u32 {
    let mut stack = [Frame::default(); 5];
    let mut sp = 1usize;
    stack[0] = Frame { x, n, b, bb, lowband, lm, gain, fill, ..Frame::default() };
    let mut ret = 0u32;
    while sp > 0 {
        let mut f = stack[sp - 1];
        match f.stage {
            0 => {
                let i = ctx.i;
                let cache = cache_row((f.lm + 1).max(0) as usize, i);
                let c0 = usize::from(*cache.first().unwrap_or(&0));
                let top = i32::from(cache.get(c0).copied().unwrap_or(0));
                if f.lm != -1 && f.b > top + 12 && f.n > 2 && sp < stack.len() {
                    f.b0 = f.bb;
                    f.n >>= 1;
                    f.lm -= 1;
                    if f.bb == 1 {
                        f.fill = (f.fill & 1) | (f.fill << 1);
                    }
                    f.bb = (f.bb + 1) >> 1;
                    let mut bq = f.b;
                    let mut fillq = f.fill;
                    let s = compute_theta(ctx, f.n as i32, &mut bq, f.bb, f.b0, f.lm, false, &mut fillq);
                    f.b = bq;
                    f.fill = fillq;
                    f.mid = (1.0 / 32768.0) * s.imid as f32;
                    f.side = (1.0 / 32768.0) * s.iside as f32;
                    let mut delta = s.delta;
                    if f.b0 > 1 && (s.itheta & 0x3fff) != 0 {
                        if s.itheta > 8192 {
                            delta -= delta >> (4 - f.lm);
                        } else {
                            delta = 0.min(delta + ((f.n as i32) << BITRES >> (5 - f.lm)));
                        }
                    }
                    f.mbits = 0.max(f.b.min((f.b - delta) / 2));
                    f.sbits = f.b - f.mbits;
                    ctx.remaining_bits -= s.qalloc;
                    f.itheta = s.itheta;
                    f.rebalance = ctx.remaining_bits;
                    f.mid_first = f.mbits >= f.sbits;
                    f.stage = 1;
                    let child = if f.mid_first {
                        Frame {
                            x: f.x,
                            n: f.n,
                            b: f.mbits,
                            bb: f.bb,
                            lowband: f.lowband,
                            lm: f.lm,
                            gain: f.gain * f.mid,
                            fill: f.fill,
                            ..Frame::default()
                        }
                    } else {
                        Frame {
                            x: f.x + f.n,
                            n: f.n,
                            b: f.sbits,
                            bb: f.bb,
                            lowband: f.lowband.map(|l| l + f.n),
                            lm: f.lm,
                            gain: f.gain * f.side,
                            fill: f.fill >> f.bb,
                            ..Frame::default()
                        }
                    };
                    stack[sp - 1] = f;
                    stack[sp] = child;
                    sp += 1;
                    continue;
                }
                // Leaf.
                let mut q = bits2pulses(i, f.lm, f.b);
                let mut curr_bits = pulses2bits(i, f.lm, q);
                ctx.remaining_bits -= curr_bits;
                while ctx.remaining_bits < 0 && q > 0 {
                    ctx.remaining_bits += curr_bits;
                    q -= 1;
                    curr_bits = pulses2bits(i, f.lm, q);
                    ctx.remaining_bits -= curr_bits;
                }
                let mut cm = 0u32;
                let (xo, n) = (f.x, f.n);
                if q != 0 {
                    let k = get_pulses(q);
                    let end = (xo + n).min(buf.len());
                    let (main, rest) = buf.split_at_mut(ctx.tmp);
                    let (iy, row) = rest.split_at_mut(MAX_N);
                    let iy: &mut [i32] = bytemuck::cast_slice_mut(iy);
                    let rl = ROW_LEN.min(row.len());
                    let row: &mut [u32] = bytemuck::cast_slice_mut(&mut row[..rl]);
                    cm = alg_unquant(
                        &mut main[xo..end],
                        n,
                        k,
                        ctx.spread,
                        f.bb.max(0) as usize,
                        ctx.ec,
                        f.gain,
                        iy,
                        row,
                    );
                } else {
                    let cm_mask = ((1u64 << f.bb.clamp(0, 31)) - 1) as u32;
                    let fill = f.fill as u32 & cm_mask;
                    if fill == 0 {
                        buf[xo..xo + n].fill(0.0);
                    } else {
                        match f.lowband {
                            None => {
                                for j in 0..n {
                                    ctx.seed = lcg_rand(ctx.seed);
                                    buf[xo + j] = ((ctx.seed as i32) >> 20) as f32;
                                }
                                cm = cm_mask;
                            }
                            Some(lb) => {
                                for j in 0..n {
                                    ctx.seed = lcg_rand(ctx.seed);
                                    let tmp = if ctx.seed & 0x8000 != 0 { 1.0 / 256.0 } else { -1.0 / 256.0 };
                                    buf[xo + j] = buf[lb + j] + tmp;
                                }
                                cm = fill;
                            }
                        }
                        renormalise_vector(&mut buf[xo..xo + n], f.gain);
                    }
                }
                ret = cm;
                sp -= 1;
            }
            1 => {
                if f.mid_first {
                    f.cm = ret;
                    let rebalance = f.mbits - (f.rebalance - ctx.remaining_bits);
                    if rebalance > 3 << BITRES && f.itheta != 0 {
                        f.sbits += rebalance - (3 << BITRES);
                    }
                    let child = Frame {
                        x: f.x + f.n,
                        n: f.n,
                        b: f.sbits,
                        bb: f.bb,
                        lowband: f.lowband.map(|l| l + f.n),
                        lm: f.lm,
                        gain: f.gain * f.side,
                        fill: f.fill >> f.bb,
                        ..Frame::default()
                    };
                    f.stage = 2;
                    stack[sp - 1] = f;
                    stack[sp] = child;
                } else {
                    f.cm = ret << (f.b0 >> 1);
                    let rebalance = f.sbits - (f.rebalance - ctx.remaining_bits);
                    if rebalance > 3 << BITRES && f.itheta != 16384 {
                        f.mbits += rebalance - (3 << BITRES);
                    }
                    let child = Frame {
                        x: f.x,
                        n: f.n,
                        b: f.mbits,
                        bb: f.bb,
                        lowband: f.lowband,
                        lm: f.lm,
                        gain: f.gain * f.mid,
                        fill: f.fill,
                        ..Frame::default()
                    };
                    f.stage = 2;
                    stack[sp - 1] = f;
                    stack[sp] = child;
                }
                sp += 1;
            }
            _ => {
                ret = if f.mid_first { f.cm | (ret << (f.b0 >> 1)) } else { f.cm | ret };
                sp -= 1;
            }
        }
    }
    ret
}

/// `quant_band` (decoder). Offsets index `buf`.
fn quant_band(
    ctx: &mut BandCtx<'_, '_>,
    buf: &mut [f32],
    x: usize,
    n: usize,
    b: i32,
    bb: i32,
    mut lowband: Option<usize>,
    lm: i32,
    lowband_out: Option<usize>,
    gain: f32,
    lowband_scratch: Option<usize>,
    mut fill: i32,
) -> u32 {
    let n0 = n;
    let mut n_b = n;
    let b00 = bb;
    let mut bb = bb;
    let mut time_divide = 0;
    let mut recombine = 0;
    let long_blocks = b00 == 1;
    let mut tf_change = ctx.tf_change;
    n_b /= bb.max(1) as usize;
    if n == 1 {
        return quant_band_n1(ctx, buf, x, None, lowband_out);
    }
    if tf_change > 0 {
        recombine = tf_change;
    }
    if let (Some(ls), Some(lb)) = (lowband_scratch, lowband)
        && (recombine != 0 || ((n_b & 1) == 0 && tf_change < 0) || b00 > 1)
    {
        buf.copy_within(lb..lb + n, ls);
        lowband = Some(ls);
    }
    static BIT_INTERLEAVE: [u8; 16] = [0, 1, 1, 1, 2, 3, 3, 3, 2, 3, 3, 3, 2, 3, 3, 3];
    for k in 0..recombine {
        if let Some(lb) = lowband {
            haar1(&mut buf[lb..lb + n], n >> k, 1 << k);
        }
        fill = i32::from(BIT_INTERLEAVE[(fill & 0xF) as usize])
            | i32::from(BIT_INTERLEAVE[((fill >> 4) & 0xF) as usize]) << 2;
    }
    bb >>= recombine;
    n_b <<= recombine;
    while (n_b & 1) == 0 && tf_change < 0 {
        if let Some(lb) = lowband {
            haar1(&mut buf[lb..lb + n], n_b, bb as usize);
        }
        fill |= fill << bb;
        bb <<= 1;
        n_b >>= 1;
        time_divide += 1;
        tf_change += 1;
    }
    let b0 = bb;
    let n_b0 = n_b;
    if b0 > 1
        && let Some(lb) = lowband
    {
        deinterleave_hadamard(buf, lb, ctx.tmp, n_b >> recombine, (b0 << recombine) as usize, long_blocks);
    }
    let mut cm = quant_partition(ctx, buf, x, n, b, bb, lowband, lm, gain, fill);
    if b0 > 1 {
        interleave_hadamard(buf, x, ctx.tmp, n_b >> recombine, (b0 << recombine) as usize, long_blocks);
    }
    n_b = n_b0;
    bb = b0;
    for _ in 0..time_divide {
        bb >>= 1;
        n_b <<= 1;
        cm |= cm >> bb;
        haar1(&mut buf[x..x + n], n_b, bb as usize);
    }
    static BIT_DEINTERLEAVE: [u8; 16] =
        [0x00, 0x03, 0x0C, 0x0F, 0x30, 0x33, 0x3C, 0x3F, 0xC0, 0xC3, 0xCC, 0xCF, 0xF0, 0xF3, 0xFC, 0xFF];
    for k in 0..recombine {
        cm = u32::from(BIT_DEINTERLEAVE[(cm & 0xF) as usize]);
        haar1(&mut buf[x..x + n], n0 >> k, 1 << k);
    }
    bb <<= recombine;
    if let Some(lo) = lowband_out {
        let nn = sqrt(n0 as f32);
        for j in 0..n0 {
            buf[lo + j] = nn * buf[x + j];
        }
    }
    cm & ((1u32 << bb.clamp(0, 31)) - 1)
}

fn stereo_merge(buf: &mut [f32], x: usize, y: usize, mid: f32, n: usize) {
    let mut xp = 0f32;
    let mut side = 0f32;
    for j in 0..n {
        xp += buf[y + j] * buf[x + j];
        side += buf[y + j] * buf[y + j];
    }
    xp *= mid;
    let mid2 = mid;
    let el = mid2 * mid2 + side - 2.0 * xp;
    let er = mid2 * mid2 + side + 2.0 * xp;
    if er < 6e-4 || el < 6e-4 {
        buf.copy_within(x..x + n, y);
        return;
    }
    let lgain = rsqrt(el);
    let rgain = rsqrt(er);
    for j in 0..n {
        let l = mid * buf[x + j];
        let r = buf[y + j];
        buf[x + j] = lgain * (l - r);
        buf[y + j] = rgain * (l + r);
    }
}

fn quant_band_stereo(
    ctx: &mut BandCtx<'_, '_>,
    buf: &mut [f32],
    x: usize,
    y: usize,
    n: usize,
    b: i32,
    bb: i32,
    lowband: Option<usize>,
    lm: i32,
    lowband_out: Option<usize>,
    lowband_scratch: Option<usize>,
    fill: i32,
) -> u32 {
    if n == 1 {
        return quant_band_n1(ctx, buf, x, Some(y), lowband_out);
    }
    let orig_fill = fill;
    let mut b = b;
    let mut fill = fill;
    let s = compute_theta(ctx, n as i32, &mut b, bb, bb, lm, true, &mut fill);
    let mid = (1.0 / 32768.0) * s.imid as f32;
    let side = (1.0 / 32768.0) * s.iside as f32;
    let mut cm;
    if n == 2 {
        let mut mbits = b;
        let mut sbits = 0;
        if s.itheta != 0 && s.itheta != 16384 {
            sbits = 1 << BITRES;
        }
        mbits -= sbits;
        let c = s.itheta > 8192;
        ctx.remaining_bits -= s.qalloc + sbits;
        let (x2, y2) = if c { (y, x) } else { (x, y) };
        let mut sign = 0;
        if sbits != 0 {
            sign = ctx.ec.bits(1) as i32;
        }
        let sign = 1 - 2 * sign;
        cm =
            quant_band(ctx, buf, x2, n, mbits, bb, lowband, lm, lowband_out, 1.0, lowband_scratch, orig_fill);
        buf[y2] = -(sign as f32) * buf[x2 + 1];
        buf[y2 + 1] = sign as f32 * buf[x2];
        buf[x] *= mid;
        buf[x + 1] *= mid;
        buf[y] *= side;
        buf[y + 1] *= side;
        let tmp = buf[x];
        buf[x] = tmp - buf[y];
        buf[y] += tmp;
        let tmp = buf[x + 1];
        buf[x + 1] = tmp - buf[y + 1];
        buf[y + 1] += tmp;
    } else {
        let mut mbits = 0.max(b.min((b - s.delta) / 2));
        let mut sbits = b - mbits;
        ctx.remaining_bits -= s.qalloc;
        let rebalance = ctx.remaining_bits;
        if mbits >= sbits {
            cm = quant_band(ctx, buf, x, n, mbits, bb, lowband, lm, lowband_out, 1.0, lowband_scratch, fill);
            let rebalance = mbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && s.itheta != 0 {
                sbits += rebalance - (3 << BITRES);
            }
            cm |= quant_band(ctx, buf, y, n, sbits, bb, None, lm, None, side, None, fill >> bb);
        } else {
            cm = quant_band(ctx, buf, y, n, sbits, bb, None, lm, None, side, None, fill >> bb);
            let rebalance = sbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && s.itheta != 16384 {
                mbits += rebalance - (3 << BITRES);
            }
            cm |= quant_band(ctx, buf, x, n, mbits, bb, lowband, lm, lowband_out, 1.0, lowband_scratch, fill);
        }
    }
    if n != 2 {
        stereo_merge(buf, x, y, mid, n);
    }
    if s.inv {
        for v in &mut buf[y..y + n] {
            *v = -*v;
        }
    }
    cm
}

/// Floats [`quant_all_bands`] needs in its work slice after `X`/`Y`
/// (`C * (M * eBands[nbEBands - 1] - norm_offset)`, worst case).
pub const NORM_LEN: usize = 2 * 8 * 78;

/// `quant_all_bands` for the decoder. `buf` holds `X` at 0 (and `Y` at
/// `n` when `c == 2`) followed by the norm area at `norm_base`.
pub fn quant_all_bands(
    start: usize,
    end: usize,
    buf: &mut [f32],
    n: usize,
    c: usize,
    norm_base: usize,
    collapse_masks: &mut [u8],
    pulses: &[i32; NB_EBANDS],
    short_blocks: bool,
    spread: i32,
    mut dual_stereo: bool,
    intensity: i32,
    tf_res: &[i32; NB_EBANDS],
    total_bits: i32,
    mut balance: i32,
    ec: &mut EcDec<'_>,
    lm: usize,
    coded_bands: i32,
    seed: &mut u32,
    disable_inv: bool,
) {
    let m = 1usize << lm;
    let bb: i32 = if short_blocks { m as i32 } else { 1 };
    let norm_offset = m * EBANDS[start] as usize;
    let norm = norm_base;
    let norm2 = norm_base + m * EBANDS[NB_EBANDS - 1] as usize - norm_offset;
    let scratch_default = m * EBANDS[NB_EBANDS - 1] as usize;
    let mut lowband_offset = 0usize;
    let mut update_lowband = true;
    let mut ctx = BandCtx {
        ec,
        i: 0,
        intensity,
        spread,
        tf_change: 0,
        remaining_bits: 0,
        seed: *seed,
        disable_inv,
        avoid_split_noise: bb > 1,
        tmp: norm_base + NORM_LEN,
    };
    for i in start..end {
        ctx.i = i;
        let last = i == end - 1;
        let x = m * EBANDS[i] as usize;
        let y = if c == 2 { Some(n + m * EBANDS[i] as usize) } else { None };
        let nn = m * (EBANDS[i + 1] - EBANDS[i]) as usize;
        let tell = ctx.ec.tell_frac() as i32;
        if i != start {
            balance -= tell;
        }
        let remaining_bits = total_bits - tell - 1;
        ctx.remaining_bits = remaining_bits;
        let b = if (i as i32) < coded_bands {
            let d = 3.min(coded_bands - i as i32);
            let curr_balance = if balance >= 0 { balance / d } else { -((-balance) / d) };
            0.max(16383.min((remaining_bits + 1).min(pulses[i] + curr_balance)))
        } else {
            0
        };
        if (m * EBANDS[i] as usize >= nn + m * EBANDS[start] as usize || i == start + 1)
            && (update_lowband || lowband_offset == 0)
        {
            lowband_offset = i;
        }
        if i == start + 1 {
            // special_hybrid_folding
            let n1 = m * (EBANDS[start + 1] - EBANDS[start]) as usize;
            let n2 = m * (EBANDS[start + 2] - EBANDS[start + 1]) as usize;
            if n2 > n1 {
                buf.copy_within(norm + 2 * n1 - n2..norm + n1, norm + n1);
                if dual_stereo {
                    buf.copy_within(norm2 + 2 * n1 - n2..norm2 + n1, norm2 + n1);
                }
            }
        }
        let tf_change = tf_res[i];
        ctx.tf_change = tf_change;
        let lowband_scratch = if last { None } else { Some(scratch_default) };
        let mut effective_lowband: Option<usize> = None;
        let (mut x_cm, mut y_cm);
        if lowband_offset != 0 && (spread != SPREAD_AGGRESSIVE || bb > 1 || tf_change < 0) {
            let el = (m * EBANDS[lowband_offset] as usize).saturating_sub(norm_offset + nn);
            effective_lowband = Some(el);
            let mut fold_start = lowband_offset;
            loop {
                fold_start -= 1;
                if m * EBANDS[fold_start] as usize <= el + norm_offset || fold_start == 0 {
                    break;
                }
            }
            let mut fold_end = lowband_offset - 1;
            loop {
                fold_end += 1;
                if !(fold_end < i && (m * EBANDS[fold_end] as usize) < el + norm_offset + nn) {
                    break;
                }
            }
            x_cm = 0u32;
            y_cm = 0u32;
            let mut fi = fold_start;
            loop {
                x_cm |= u32::from(collapse_masks[fi * c]);
                y_cm |= u32::from(collapse_masks[fi * c + c - 1]);
                fi += 1;
                if fi >= fold_end {
                    break;
                }
            }
        } else {
            x_cm = (1u32 << bb) - 1;
            y_cm = x_cm;
        }
        if dual_stereo && i as i32 == intensity {
            dual_stereo = false;
            for j in 0..(m * EBANDS[i] as usize - norm_offset) {
                buf[norm + j] = 0.5 * (buf[norm + j] + buf[norm2 + j]);
            }
        }
        let out_off = m * EBANDS[i] as usize - norm_offset;
        if dual_stereo {
            let yy = y.unwrap_or(x);
            x_cm = quant_band(
                &mut ctx,
                buf,
                x,
                nn,
                b / 2,
                bb,
                effective_lowband.map(|e| norm + e),
                lm as i32,
                if last { None } else { Some(norm + out_off) },
                1.0,
                lowband_scratch,
                x_cm as i32,
            );
            y_cm = quant_band(
                &mut ctx,
                buf,
                yy,
                nn,
                b / 2,
                bb,
                effective_lowband.map(|e| norm2 + e),
                lm as i32,
                if last { None } else { Some(norm2 + out_off) },
                1.0,
                lowband_scratch,
                y_cm as i32,
            );
        } else {
            x_cm = if let Some(yy) = y {
                quant_band_stereo(
                    &mut ctx,
                    buf,
                    x,
                    yy,
                    nn,
                    b,
                    bb,
                    effective_lowband.map(|e| norm + e),
                    lm as i32,
                    if last { None } else { Some(norm + out_off) },
                    lowband_scratch,
                    (x_cm | y_cm) as i32,
                )
            } else {
                quant_band(
                    &mut ctx,
                    buf,
                    x,
                    nn,
                    b,
                    bb,
                    effective_lowband.map(|e| norm + e),
                    lm as i32,
                    if last { None } else { Some(norm + out_off) },
                    1.0,
                    lowband_scratch,
                    (x_cm | y_cm) as i32,
                )
            };
            y_cm = x_cm;
        }
        collapse_masks[i * c] = x_cm as u8;
        collapse_masks[i * c + c - 1] = y_cm as u8;
        balance += pulses[i] + tell;
        update_lowband = b > (nn << BITRES) as i32;
        ctx.avoid_split_noise = false;
    }
    *seed = ctx.seed;
}
