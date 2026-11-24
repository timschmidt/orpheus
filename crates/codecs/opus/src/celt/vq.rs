//! PVQ decoding: `celt/cwrs.c` (`decode_pulses`) and `celt/vq.c`
//! (`alg_unquant`, `renormalise_vector`), libopus 1.5.2 float build
//! (BSD-3-Clause, see LICENSE-XIPH).

use super::math::{cos_norm, rsqrt};
use super::modes::pvq_u_at;
use crate::entdec::EcDec;

/// Largest band (in coefficients) a PVQ vector can cover.
pub const MAX_N: usize = 176;

#[inline]
fn pvq_v(n: usize, k: usize) -> u32 {
    pvq_u_at(n, k).wrapping_add(pvq_u_at(n, k + 1))
}

/// `cwrsi`: index → pulse vector; returns the squared norm.
fn cwrsi(mut n: usize, mut k: i32, mut i: u32, y: &mut [i32]) -> f32 {
    let mut yy = 0f32;
    let mut pos = 0usize;
    let u = |a: usize, b: i32| pvq_u_at(a, b.max(0) as usize);
    while n > 2 {
        let val;
        if k as usize >= n {
            let p0 = u(n, k + 1);
            let s = -i32::from(i >= p0);
            i = i.wrapping_sub(p0 & s as u32);
            let k0 = k;
            let q = u(n, n as i32);
            let mut p;
            if q > i {
                k = n as i32;
                loop {
                    k -= 1;
                    p = u(k.max(0) as usize, n as i32);
                    if p <= i || k <= 0 {
                        break;
                    }
                }
            } else {
                p = u(n, k);
                while p > i && k > 0 {
                    k -= 1;
                    p = u(n, k);
                }
            }
            i = i.wrapping_sub(p);
            val = (k0 - k + s) ^ s;
        } else {
            let p = u(k.max(0) as usize, n as i32);
            let q = u((k + 1) as usize, n as i32);
            if p <= i && i < q {
                i -= p;
                val = 0;
            } else {
                let s = -i32::from(i >= q);
                i = i.wrapping_sub(q & s as u32);
                let k0 = k;
                let mut p;
                loop {
                    k -= 1;
                    p = u(k.max(0) as usize, n as i32);
                    if p <= i || k <= 0 {
                        break;
                    }
                }
                i = i.wrapping_sub(p);
                val = (k0 - k + s) ^ s;
            }
        }
        if let Some(d) = y.get_mut(pos) {
            *d = val;
        }
        pos += 1;
        yy += (val * val) as f32;
        n -= 1;
    }
    // n == 2
    let p = (2 * k + 1) as u32;
    let s = -i32::from(i >= p);
    i = i.wrapping_sub(p & s as u32);
    let k0 = k;
    k = ((i.wrapping_add(1)) >> 1) as i32;
    if k != 0 {
        i = i.wrapping_sub((2 * k - 1) as u32);
    }
    let val = (k0 - k + s) ^ s;
    if let Some(d) = y.get_mut(pos) {
        *d = val;
    }
    yy += (val.wrapping_mul(val)) as f32;
    // n == 1
    let s = -(i as i32);
    let val = (k + s) ^ s;
    if let Some(d) = y.get_mut(pos + 1) {
        *d = val;
    }
    yy += (val.wrapping_mul(val)) as f32;
    yy
}

/// `decode_pulses`.
pub fn decode_pulses(y: &mut [i32], n: usize, k: i32, dec: &mut EcDec<'_>) -> f32 {
    let v = pvq_v(n, k.max(0) as usize);
    let idx = dec.uint(v.max(2));
    cwrsi(n, k, idx, y)
}

fn exp_rotation1(x: &mut [f32], len: usize, stride: usize, c: f32, s: f32) {
    let ms = -s;
    if len <= stride {
        return;
    }
    for i in 0..len - stride {
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 + ms * x2;
    }
    if len < 2 * stride + 1 {
        return;
    }
    let mut i = len - 2 * stride - 1;
    loop {
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 + ms * x2;
        if i == 0 {
            break;
        }
        i -= 1;
    }
}

/// `exp_rotation` (spreading); `dir` < 0 for the decoder.
pub fn exp_rotation(x: &mut [f32], len: usize, dir: i32, stride: usize, k: i32, spread: i32) {
    const SPREAD_FACTOR: [i32; 3] = [15, 10, 5];
    if 2 * k >= len as i32 || spread == 0 || stride == 0 {
        return;
    }
    let factor = SPREAD_FACTOR[(spread - 1).clamp(0, 2) as usize];
    let gain = len as f32 / (len as i32 + factor * k) as f32;
    let theta = 0.5 * (gain * gain);
    let c = cos_norm(theta);
    let s = cos_norm(1.0 - theta);
    let mut stride2 = 0usize;
    if len >= 8 * stride {
        stride2 = 1;
        while (stride2 * stride2 + stride2) * stride + (stride >> 2) < len {
            stride2 += 1;
        }
    }
    let l = len / stride;
    for i in 0..stride {
        let Some(xs) = x.get_mut(i * l..(i + 1) * l) else { return };
        if dir < 0 {
            if stride2 != 0 {
                exp_rotation1(xs, l, stride2, s, c);
            }
            exp_rotation1(xs, l, 1, c, s);
        } else {
            exp_rotation1(xs, l, 1, c, -s);
            if stride2 != 0 {
                exp_rotation1(xs, l, stride2, s, -c);
            }
        }
    }
}

fn extract_collapse_mask(iy: &[i32], n: usize, b: usize) -> u32 {
    if b <= 1 {
        return 1;
    }
    let n0 = n / b;
    let mut mask = 0u32;
    for i in 0..b {
        let mut tmp = 0i32;
        for j in 0..n0 {
            tmp |= iy.get(i * n0 + j).copied().unwrap_or(0);
        }
        mask |= u32::from(tmp != 0) << i;
    }
    mask
}

/// `alg_unquant`.
pub fn alg_unquant(
    x: &mut [f32],
    n: usize,
    k: i32,
    spread: i32,
    b: usize,
    dec: &mut EcDec<'_>,
    gain: f32,
    iy: &mut [i32],
) -> u32 {
    let n = n.min(MAX_N).min(x.len()).min(iy.len());
    let ryy = decode_pulses(&mut iy[..n], n, k, dec);
    let g = rsqrt(ryy) * gain;
    for (xv, &y) in x[..n].iter_mut().zip(&iy[..n]) {
        *xv = g * y as f32;
    }
    exp_rotation(x, n, -1, b, k, spread);
    extract_collapse_mask(iy, n, b)
}

/// `celt_inner_prod`.
#[inline]
#[must_use]
pub fn inner_prod(a: &[f32], b: &[f32]) -> f32 {
    let mut s = 0f32;
    for (x, y) in a.iter().zip(b) {
        s += x * y;
    }
    s
}

/// `renormalise_vector`.
pub fn renormalise_vector(x: &mut [f32], gain: f32) {
    let e = 1e-15 + inner_prod(x, x);
    let g = rsqrt(e) * gain;
    for v in x.iter_mut() {
        *v *= g;
    }
}
