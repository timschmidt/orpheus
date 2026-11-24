//! Inverse MDCT over the mixed-radix kiss FFT: libopus 1.5.2 `celt/mdct.c`
//! (`clt_mdct_backward_c`) and `celt/kiss_fft.c` (`opus_fft_impl`), float
//! build (BSD-3-Clause, see LICENSE-XIPH).
//!
//! Complex values are interleaved `[re, im]` pairs in an `f32` slice.

use super::modes::{FFT_STATES, FFT_TWIDDLES, FftState, MDCT_N};

#[inline]
fn tw(i: usize) -> (f32, f32) {
    let t = FFT_TWIDDLES.get(i).copied().unwrap_or([1.0, 0.0]);
    (t[0], t[1])
}

#[inline]
fn get(f: &[f32], k: usize) -> (f32, f32) {
    (f[2 * k], f[2 * k + 1])
}

#[inline]
fn set(f: &mut [f32], k: usize, v: (f32, f32)) {
    f[2 * k] = v.0;
    f[2 * k + 1] = v.1;
}

#[inline]
fn cmul(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

fn bfly2(f: &mut [f32], m: usize, n: usize) {
    if m == 1 {
        for i in 0..n {
            let a = get(f, 2 * i);
            let t = get(f, 2 * i + 1);
            set(f, 2 * i + 1, (a.0 - t.0, a.1 - t.1));
            set(f, 2 * i, (a.0 + t.0, a.1 + t.1));
        }
    } else {
        let tw = 0.707_106_77f32;
        for i in 0..n {
            let b = 8 * i;
            // k = 0
            let t = get(f, b + 4);
            let a = get(f, b);
            set(f, b + 4, (a.0 - t.0, a.1 - t.1));
            set(f, b, (a.0 + t.0, a.1 + t.1));
            // k = 1
            let f21 = get(f, b + 5);
            let t = ((f21.0 + f21.1) * tw, (f21.1 - f21.0) * tw);
            let a = get(f, b + 1);
            set(f, b + 5, (a.0 - t.0, a.1 - t.1));
            set(f, b + 1, (a.0 + t.0, a.1 + t.1));
            // k = 2
            let f22 = get(f, b + 6);
            let t = (f22.1, -f22.0);
            let a = get(f, b + 2);
            set(f, b + 6, (a.0 - t.0, a.1 - t.1));
            set(f, b + 2, (a.0 + t.0, a.1 + t.1));
            // k = 3
            let f23 = get(f, b + 7);
            let t = ((f23.1 - f23.0) * tw, -(f23.1 + f23.0) * tw);
            let a = get(f, b + 3);
            set(f, b + 7, (a.0 - t.0, a.1 - t.1));
            set(f, b + 3, (a.0 + t.0, a.1 + t.1));
        }
    }
}

fn bfly4(f: &mut [f32], fstride: usize, m: usize, n: usize, mm: usize) {
    if m == 1 {
        for i in 0..n {
            let b = 4 * i;
            let f0 = get(f, b);
            let f1 = get(f, b + 1);
            let f2 = get(f, b + 2);
            let f3 = get(f, b + 3);
            let s0 = (f0.0 - f2.0, f0.1 - f2.1);
            let f0 = (f0.0 + f2.0, f0.1 + f2.1);
            let s1 = (f1.0 + f3.0, f1.1 + f3.1);
            set(f, b + 2, (f0.0 - s1.0, f0.1 - s1.1));
            set(f, b, (f0.0 + s1.0, f0.1 + s1.1));
            let s1 = (f1.0 - f3.0, f1.1 - f3.1);
            set(f, b + 1, (s0.0 + s1.1, s0.1 - s1.0));
            set(f, b + 3, (s0.0 - s1.1, s0.1 + s1.0));
        }
    } else {
        let (m2, m3) = (2 * m, 3 * m);
        for i in 0..n {
            let base = i * mm;
            for j in 0..m {
                let k = base + j;
                let s0 = cmul(get(f, k + m), tw(j * fstride));
                let s1 = cmul(get(f, k + m2), tw(j * fstride * 2));
                let s2 = cmul(get(f, k + m3), tw(j * fstride * 3));
                let fk = get(f, k);
                let s5 = (fk.0 - s1.0, fk.1 - s1.1);
                let fk = (fk.0 + s1.0, fk.1 + s1.1);
                let s3 = (s0.0 + s2.0, s0.1 + s2.1);
                let s4 = (s0.0 - s2.0, s0.1 - s2.1);
                set(f, k + m2, (fk.0 - s3.0, fk.1 - s3.1));
                set(f, k, (fk.0 + s3.0, fk.1 + s3.1));
                set(f, k + m, (s5.0 + s4.1, s5.1 - s4.0));
                set(f, k + m3, (s5.0 - s4.1, s5.1 + s4.0));
            }
        }
    }
}

fn bfly3(f: &mut [f32], fstride: usize, m: usize, n: usize, mm: usize) {
    let m2 = 2 * m;
    let epi3 = tw(fstride * m);
    for i in 0..n {
        let base = i * mm;
        for j in 0..m {
            let k = base + j;
            let s1 = cmul(get(f, k + m), tw(j * fstride));
            let s2 = cmul(get(f, k + m2), tw(j * fstride * 2));
            let s3 = (s1.0 + s2.0, s1.1 + s2.1);
            let s0 = (s1.0 - s2.0, s1.1 - s2.1);
            let fk = get(f, k);
            let fm = (fk.0 - s3.0 * 0.5, fk.1 - s3.1 * 0.5);
            let s0 = (s0.0 * epi3.1, s0.1 * epi3.1);
            set(f, k, (fk.0 + s3.0, fk.1 + s3.1));
            set(f, k + m2, (fm.0 + s0.1, fm.1 - s0.0));
            set(f, k + m, (fm.0 - s0.1, fm.1 + s0.0));
        }
    }
}

fn bfly5(f: &mut [f32], fstride: usize, m: usize, n: usize, mm: usize) {
    let ya = tw(fstride * m);
    let yb = tw(fstride * 2 * m);
    for i in 0..n {
        let base = i * mm;
        for u in 0..m {
            let k0 = base + u;
            let (k1, k2, k3, k4) = (k0 + m, k0 + 2 * m, k0 + 3 * m, k0 + 4 * m);
            let s0 = get(f, k0);
            let s1 = cmul(get(f, k1), tw(u * fstride));
            let s2 = cmul(get(f, k2), tw(2 * u * fstride));
            let s3 = cmul(get(f, k3), tw(3 * u * fstride));
            let s4 = cmul(get(f, k4), tw(4 * u * fstride));
            let s7 = (s1.0 + s4.0, s1.1 + s4.1);
            let s10 = (s1.0 - s4.0, s1.1 - s4.1);
            let s8 = (s2.0 + s3.0, s2.1 + s3.1);
            let s9 = (s2.0 - s3.0, s2.1 - s3.1);
            set(f, k0, (s0.0 + (s7.0 + s8.0), s0.1 + (s7.1 + s8.1)));
            let s5 = (s0.0 + (s7.0 * ya.0 + s8.0 * yb.0), s0.1 + (s7.1 * ya.0 + s8.1 * yb.0));
            let s6 = (s10.1 * ya.1 + s9.1 * yb.1, -(s10.0 * ya.1 + s9.0 * yb.1));
            set(f, k1, (s5.0 - s6.0, s5.1 - s6.1));
            set(f, k4, (s5.0 + s6.0, s5.1 + s6.1));
            let s11 = (s0.0 + (s7.0 * yb.0 + s8.0 * ya.0), s0.1 + (s7.1 * yb.0 + s8.1 * ya.0));
            let s12 = (s9.1 * ya.1 - s10.1 * yb.1, s10.0 * yb.1 - s9.0 * ya.1);
            set(f, k2, (s11.0 + s12.0, s11.1 + s12.1));
            set(f, k3, (s11.0 - s12.0, s11.1 - s12.1));
        }
    }
}

/// `opus_fft_impl` on `nfft` interleaved complex values.
pub fn fft_impl(st: &FftState, f: &mut [f32]) {
    let shift = if st.shift > 0 { st.shift as usize } else { 0 };
    let mut fstride = [1usize; 9];
    let mut l = 0usize;
    loop {
        let p = st.factors[2 * l] as usize;
        let m = st.factors[2 * l + 1] as usize;
        fstride[l + 1] = fstride[l] * p;
        l += 1;
        if m == 1 || l >= 8 {
            break;
        }
    }
    let mut m = st.factors[2 * l - 1] as usize;
    for i in (0..l).rev() {
        let m2 = if i != 0 { st.factors[2 * i - 1] as usize } else { 1 };
        match st.factors[2 * i] {
            2 => bfly2(f, m, fstride[i]),
            4 => bfly4(f, fstride[i] << shift, m, fstride[i], m2),
            3 => bfly3(f, fstride[i] << shift, m, fstride[i], m2),
            5 => bfly5(f, fstride[i] << shift, m, fstride[i], m2),
            _ => {}
        }
        m = m2;
    }
}

/// `clt_mdct_backward`: `input` (strided by `stride`) → `out`, which must
/// hold `overlap/2 + N/2 + overlap/2` values (`N` = 1920 >> shift); the
/// first `overlap` samples are windowed for TDAC.
pub fn mdct_backward(
    input: &[f32],
    out: &mut [f32],
    window: &[f32],
    overlap: usize,
    shift: usize,
    stride: usize,
) {
    let n = MDCT_N >> shift;
    let n2 = n >> 1;
    let n4 = n >> 2;
    let st = &FFT_STATES[shift];
    // Per size (not a `match`: its jump table would be anonymous rodata
    // outside the hot.toml cluster).
    let trig = st.trig;
    let ho = overlap >> 1;
    {
        let yp = &mut out[ho..ho + n2];
        let mut xp1 = 0usize;
        let mut xp2 = stride * (n2 - 1);
        for i in 0..n4 {
            let rev = st.bitrev[i] as usize;
            let x1 = input[xp1];
            let x2 = input[xp2];
            let yr = x2 * trig[i] + x1 * trig[n4 + i];
            let yi = x1 * trig[i] - x2 * trig[n4 + i];
            yp[2 * rev + 1] = yr;
            yp[2 * rev] = yi;
            xp1 += 2 * stride;
            xp2 = xp2.wrapping_sub(2 * stride);
        }
        fft_impl(st, yp);
    }
    {
        let yp = &mut out[ho..ho + n2];
        let mut p0 = 0usize;
        let mut p1 = n2 - 2;
        for i in 0..(n4 + 1) >> 1 {
            let re = yp[p0 + 1];
            let im = yp[p0];
            let t0 = trig[i];
            let t1 = trig[n4 + i];
            let yr = re * t0 + im * t1;
            let yi = re * t1 - im * t0;
            let re = yp[p1 + 1];
            let im = yp[p1];
            yp[p0] = yr;
            yp[p1 + 1] = yi;
            let t0 = trig[n4 - i - 1];
            let t1 = trig[n2 - i - 1];
            let yr = re * t0 + im * t1;
            let yi = re * t1 - im * t0;
            yp[p1] = yr;
            yp[p0 + 1] = yi;
            p0 += 2;
            p1 = p1.wrapping_sub(2);
        }
    }
    // Mirror on both sides for TDAC.
    for i in 0..overlap / 2 {
        let xp1 = overlap - 1 - i;
        let x1 = out[xp1];
        let x2 = out[i];
        let w1 = window[i];
        let w2 = window[overlap - 1 - i];
        out[i] = w2 * x2 - w1 * x1;
        out[xp1] = w1 * x2 + w2 * x1;
    }
}
