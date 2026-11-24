//! Helpers of the pitch-based packet-loss concealment: libopus 1.5.2
//! `celt/celt_lpc.c` (`_celt_lpc`, `celt_fir`, `celt_iir`,
//! `_celt_autocorr`) and `celt/pitch.c` (`pitch_downsample`,
//! `pitch_search`), float build (BSD-3-Clause, see LICENSE-XIPH).
//!
//! The concealment is not normative; sums are evaluated in natural order
//! (libopus unrolls some of them), so concealed audio may differ from
//! libopus by rounding.

/// `_celt_lpc`: Levinson-Durbin, `ac` has `p + 1` values.
pub fn lpc(out: &mut [f32], ac: &[f32], p: usize) {
    let mut error = ac[0];
    for v in out[..p].iter_mut() {
        *v = 0.0;
    }
    if ac[0] > 1e-10 {
        for i in 0..p {
            let mut rr = 0f32;
            for j in 0..i {
                rr += out[j] * ac[i - j];
            }
            rr += ac[i + 1];
            let r = -(rr / error);
            out[i] = r;
            for j in 0..(i + 1) >> 1 {
                let tmp1 = out[j];
                let tmp2 = out[i - 1 - j];
                out[j] = tmp1 + r * tmp2;
                out[i - 1 - j] = tmp2 + r * tmp1;
            }
            error -= r * r * error;
            if error <= 0.001 * ac[0] {
                break;
            }
        }
    }
}

/// `celt_fir`: `y[i] = x[i] + sum(num[j] * x[i - j - 1])`, where `x` holds
/// `ord` history samples before `x0`.
pub fn fir(x: &[f32], x0: usize, num: &[f32], y: &mut [f32], n: usize, ord: usize) {
    for i in 0..n {
        let mut sum = x[x0 + i];
        for j in 0..ord {
            sum += num[ord - j - 1] * x[x0 + i + j - ord];
        }
        y[i] = sum;
    }
}

/// `celt_iir` in place on `buf[..n]`, with `mem` the last `ord` outputs
/// (most recent first); `mem` is updated.
pub fn iir(buf: &mut [f32], den: &[f32], n: usize, ord: usize, mem: &mut [f32]) {
    for i in 0..n {
        let mut sum = buf[i];
        for j in 0..ord {
            sum -= den[j] * mem[j];
        }
        for j in (1..ord).rev() {
            mem[j] = mem[j - 1];
        }
        mem[0] = sum;
        buf[i] = sum;
    }
}

/// `_celt_autocorr` with an optional window over `overlap` samples at both
/// ends; `xx` is scratch of `n` values.
pub fn autocorr(
    x: &[f32],
    ac: &mut [f32],
    window: Option<&[f32]>,
    overlap: usize,
    lag: usize,
    n: usize,
    xx: &mut [f32],
) {
    xx[..n].copy_from_slice(&x[..n]);
    if let Some(w) = window {
        for i in 0..overlap {
            xx[i] = x[i] * w[i];
            xx[n - i - 1] = x[n - i - 1] * w[i];
        }
    }
    for k in 0..=lag {
        let mut d = 0f32;
        for i in k..n {
            d += xx[i] * xx[i - k];
        }
        ac[k] = d;
    }
}

/// `pitch_downsample` of `C` channels of `len` samples into `x_lp`
/// (`len / 2` values); `xx` is scratch of `len / 2` values.
pub fn pitch_downsample(x0: &[f32], x1: Option<&[f32]>, x_lp: &mut [f32], len: usize, xx: &mut [f32]) {
    let h = len >> 1;
    for i in 1..h {
        x_lp[i] = 0.25 * x0[2 * i - 1] + 0.25 * x0[2 * i + 1] + 0.5 * x0[2 * i];
    }
    x_lp[0] = 0.25 * x0[1] + 0.5 * x0[0];
    if let Some(x1) = x1 {
        for i in 1..h {
            x_lp[i] += 0.25 * x1[2 * i - 1] + 0.25 * x1[2 * i + 1] + 0.5 * x1[2 * i];
        }
        x_lp[0] += 0.25 * x1[1] + 0.5 * x1[0];
    }
    let mut ac = [0f32; 5];
    autocorr(x_lp, &mut ac, None, 0, 4, h, xx);
    ac[0] *= 1.0001;
    for i in 1..=4 {
        let f = 0.008 * i as f32;
        ac[i] -= ac[i] * f * f;
    }
    let mut l = [0f32; 4];
    lpc(&mut l, &ac, 4);
    let mut tmp = 1.0f32;
    for v in &mut l {
        tmp *= 0.9;
        *v *= tmp;
    }
    let c1 = 0.8f32;
    let lpc2 = [l[0] + 0.8, l[1] + c1 * l[0], l[2] + c1 * l[1], l[3] + c1 * l[2], c1 * l[3]];
    let mut mem = [0f32; 5];
    for v in x_lp[..h].iter_mut() {
        let x = *v;
        let sum =
            x + lpc2[0] * mem[0] + lpc2[1] * mem[1] + lpc2[2] * mem[2] + lpc2[3] * mem[3] + lpc2[4] * mem[4];
        mem = [x, mem[0], mem[1], mem[2], mem[3]];
        *v = sum;
    }
}

fn find_best_pitch(xcorr: &[f32], y: &[f32], len: usize, max_pitch: usize, best: &mut [usize; 2]) {
    let mut syy = 1f32;
    let mut best_num = [-1f32; 2];
    let mut best_den = [0f32; 2];
    *best = [0, 1];
    for v in &y[..len] {
        syy += v * v;
    }
    for i in 0..max_pitch {
        if xcorr[i] > 0.0 {
            let x16 = xcorr[i] * 1e-12;
            let num = x16 * x16;
            if num * best_den[1] > best_num[1] * syy {
                if num * best_den[0] > best_num[0] * syy {
                    best_num[1] = best_num[0];
                    best_den[1] = best_den[0];
                    best[1] = best[0];
                    best_num[0] = num;
                    best_den[0] = syy;
                    best[0] = i;
                } else {
                    best_num[1] = num;
                    best_den[1] = syy;
                    best[1] = i;
                }
            }
        }
        syy += y[i + len] * y[i + len] - y[i] * y[i];
        syy = syy.max(1.0);
    }
}

/// `pitch_search`; `scratch` needs `len/4 + (len+max_pitch)/4 + max_pitch/2`
/// values.
pub fn pitch_search(x_lp: &[f32], y: &[f32], len: usize, max_pitch: usize, scratch: &mut [f32]) -> i32 {
    let lag = len + max_pitch;
    let (x4, rest) = scratch.split_at_mut(len >> 2);
    let (y4, rest) = rest.split_at_mut(lag >> 2);
    let xcorr = &mut rest[..max_pitch >> 1];
    for j in 0..len >> 2 {
        x4[j] = x_lp[2 * j];
    }
    for j in 0..lag >> 2 {
        y4[j] = y[2 * j];
    }
    let n4 = len >> 2;
    for i in 0..max_pitch >> 2 {
        let mut s = 0f32;
        for j in 0..n4 {
            s += x4[j] * y4[i + j];
        }
        xcorr[i] = s;
    }
    let mut best = [0usize; 2];
    find_best_pitch(xcorr, y4, n4, max_pitch >> 2, &mut best);
    for i in 0..max_pitch >> 1 {
        xcorr[i] = 0.0;
        if (i as i32 - 2 * best[0] as i32).abs() > 2 && (i as i32 - 2 * best[1] as i32).abs() > 2 {
            continue;
        }
        let mut s = 0f32;
        for j in 0..len >> 1 {
            s += x_lp[j] * y[i + j];
        }
        xcorr[i] = s.max(-1.0);
    }
    find_best_pitch(xcorr, y, len >> 1, max_pitch >> 1, &mut best);
    let offset = if best[0] > 0 && best[0] < (max_pitch >> 1) - 1 {
        let a = xcorr[best[0] - 1];
        let b = xcorr[best[0]];
        let c = xcorr[best[0] + 1];
        if (c - a) > 0.7 * (b - a) {
            1
        } else if (a - c) > 0.7 * (b - c) {
            -1
        } else {
            0
        }
    } else {
        0
    };
    2 * best[0] as i32 - offset
}
