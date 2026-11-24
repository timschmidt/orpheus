//! Single-precision maths without `libm`: `ldexp`, sine for table set-up,
//! a quarter-wave cosine table, radix-2 FFT, the DCT-IV (via a half-size
//! complex FFT with pre- and post-twiddle) and the Vorbis window slope.

use core::f32::consts::FRAC_PI_2;

/// `x · 2^e`.
pub(crate) fn ldexp(mut x: f32, mut e: i32) -> f32 {
    const UP: f32 = 1.267_650_6e30; // 2^100
    const DOWN: f32 = 7.888_609e-31; // 2^-100
    while e > 100 {
        x *= UP;
        e -= 100;
    }
    while e < -100 {
        x *= DOWN;
        e += 100;
    }
    x * f32::from_bits(((e + 127) as u32) << 23)
}

/// sin(x) for x in [0, π/2] (Taylor series to x^15: error < 1e-9 there).
pub(crate) fn sin_q(x: f32) -> f32 {
    let x2 = x * x;
    let mut t = x;
    let mut s = x;
    let mut k = 1.0f32;
    for _ in 0..7 {
        t *= -x2 / ((k + 1.0) * (k + 2.0));
        s += t;
        k += 2.0;
    }
    s
}

/// The window slope of length `l` (Vorbis I §1.3.2):
/// `sin(π/2 · sin²((i + ½)/l · π/2))`.
pub(crate) fn window(out: &mut [f32]) {
    let l = out.len() as f32;
    for (i, w) in out.iter_mut().enumerate() {
        let s = sin_q((i as f32 + 0.5) / l * FRAC_PI_2);
        *w = sin_q(FRAC_PI_2 * s * s);
    }
}

/// Quarter-wave cosine table: `q[t] = cos(π t / (4 m))` for t in 0..=2m,
/// where `m` is half the long block size. Angles elsewhere are in units of
/// π/(4m) and reduced by symmetry.
pub(crate) fn fill_quarter(q: &mut [f32]) {
    let two_m = (q.len() - 1) as f32;
    let n = q.len() - 1;
    for (t, c) in q.iter_mut().enumerate() {
        // cos(θ) = sin(π/2 − θ), θ = π/2 · t / 2m.
        *c = sin_q(FRAC_PI_2 * ((n - t) as f32) / two_m);
    }
}

/// (cos, sin) of angle `a` units of π/(4m), from the quarter table.
#[inline]
pub(crate) fn cs(q: &[f32], a: usize) -> (f32, f32) {
    let h = q.len() - 1; // 2m: a quarter turn
    let a = a % (4 * h);
    let (quad, r) = (a / h, a % h);
    match quad {
        0 => (q[r], q[h - r]),
        1 => (-q[h - r], q[r]),
        2 => (-q[r], -q[h - r]),
        _ => (q[h - r], -q[r]),
    }
}

/// In-place forward FFT (e^{-2πi jk/L}) of `z` (length L, a power of two);
/// `unit` = angle units of 2π/L in the table's units.
fn fft(z: &mut [[f32; 2]], q: &[f32], unit: usize) {
    let l = z.len();
    if l <= 1 {
        return;
    }
    let bits = l.trailing_zeros();
    for i in 0..l {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            z.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= l {
        let half = len / 2;
        let step = l / len;
        for base in (0..l).step_by(len) {
            for j in 0..half {
                let (c, s) = cs(q, unit * j * step);
                let [br, bi] = z[base + j + half];
                // (c − i s)·b
                let tr = br * c + bi * s;
                let ti = bi * c - br * s;
                let [ar, ai] = z[base + j];
                z[base + j] = [ar + tr, ai + ti];
                z[base + j + half] = [ar - tr, ai - ti];
            }
        }
        len *= 2;
    }
}

/// DCT-IV of `x` (length M = block/2, M ≤ m_long) in place:
/// `u[k] = Σ x[j] cos(π/M (k + ½)(j + ½))`. `scratch` holds M/2 complex.
pub(crate) fn dct4(x: &mut [f32], scratch: &mut [[f32; 2]], q: &[f32]) {
    let m = x.len();
    let half = m / 2;
    let m_long = (q.len() - 1) / 2;
    let s = m_long / m;
    let z = &mut scratch[..half];
    for (k, zk) in z.iter_mut().enumerate() {
        let (re, im) = (x[2 * k], x[m - 1 - 2 * k]);
        let (c, sn) = cs(q, (4 * k + 1) * s);
        *zk = [re * c + im * sn, im * c - re * sn];
    }
    fft(z, q, 16 * s);
    for (j, zj) in z.iter().enumerate() {
        let (c, sn) = cs(q, 4 * j * s);
        let [zr, zi] = *zj;
        x[2 * j] = zr * c + zi * sn;
        x[m - 1 - 2 * j] = -(zi * c - zr * sn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dct4_matches_definition() {
        for m in [8usize, 64, 128, 1024] {
            let m_long = 1024;
            let mut q = std::vec![0f32; 2 * m_long + 1];
            fill_quarter(&mut q);
            let x: std::vec::Vec<f32> = (0..m).map(|i| ((i * 7919) % 97) as f32 / 97.0 - 0.5).collect();
            let mut y = x.clone();
            let mut sc = std::vec![[0f32; 2]; m / 2];
            dct4(&mut y, &mut sc, &q);
            for k in 0..m {
                let want: f64 = (0..m)
                    .map(|j| {
                        f64::from(x[j])
                            * (core::f64::consts::PI / m as f64 * (k as f64 + 0.5) * (j as f64 + 0.5)).cos()
                    })
                    .sum();
                assert!((f64::from(y[k]) - want).abs() < 2e-4 * (m as f64).sqrt(), "m {m} k {k}");
            }
        }
    }

    #[test]
    fn trig() {
        assert!((sin_q(1.0) - 0.841_470_96).abs() < 1e-7);
        assert!((sin_q(FRAC_PI_2) - 1.0).abs() < 1e-7);
        assert_eq!(ldexp(3.0, 4), 48.0);
        assert_eq!(ldexp(1.0, -130), f32::from_bits(1 << 19));
    }
}
