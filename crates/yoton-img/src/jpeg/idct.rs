//! Scaled inverse DCT: an 8 × 8 block of coefficients to an `nw` × `nh`
//! block of pixels, `nw`, `nh` ∈ {1, 2, 4, 8}.
//!
//! An N-point output uses the N lowest coefficients and the N-point inverse
//! DCT normalised so that its pixels are the averages of the full-size
//! ones' low-pass part: `f(x) = Σ_{u<N} C(u)/2 · cos((2x+1)uπ / 2N) · F(u)`
//! with `C(0) = 1/√2`, `C(u) = 1` otherwise (for N = 1 this is the DC
//! coefficient / 8, the block average). Separable, even/odd symmetric,
//! fixed point with 13-bit constants and 2 extra bits between the passes.

/// `round(C(u)/2 · cos((2x+1)uπ / 16) · 2^13)` for x < 4.
const M8: [[i32; 8]; 4] = [
    [2896, 4017, 3784, 3406, 2896, 2276, 1567, 799],
    [2896, 3406, 1567, -799, -2896, -4017, -3784, -2276],
    [2896, 2276, -1567, -4017, -2896, 799, 3784, 3406],
    [2896, 799, -3784, -2276, 2896, 3406, -1567, -4017],
];
const M4: [[i32; 4]; 2] = [[2896, 3784, 2896, 1567], [2896, 1567, -2896, -3784]];
const M2: i32 = 2896;

const PASS1_SHIFT: u32 = 11; // keep 2 fractional bits
const PASS2_SHIFT: u32 = 15;
/// Intermediate values are clamped so the second pass cannot overflow on
/// corrupt input (real pictures stay far below).
const TMP_LIMIT: i32 = 1 << 15;

/// 1-D inverse DCT of `n` outputs from `f[0..n]` (stride `fs`), into
/// `out[0..n]` (stride `os`); results are scaled by 2^13.
#[inline]
fn idct1(f: &[i32], fs: usize, n: usize, out: &mut [i32], os: usize) {
    match n {
        8 => {
            let (f0, f1, f2, f3) = (f[0], f[fs], f[2 * fs], f[3 * fs]);
            let (f4, f5, f6, f7) = (f[4 * fs], f[5 * fs], f[6 * fs], f[7 * fs]);
            for (x, m) in M8.iter().enumerate() {
                let e = m[0] * f0 + m[2] * f2 + m[4] * f4 + m[6] * f6;
                let o = m[1] * f1 + m[3] * f3 + m[5] * f5 + m[7] * f7;
                out[x * os] = e + o;
                out[(7 - x) * os] = e - o;
            }
        }
        4 => {
            let (f0, f1, f2, f3) = (f[0], f[fs], f[2 * fs], f[3 * fs]);
            for (x, m) in M4.iter().enumerate() {
                let e = m[0] * f0 + m[2] * f2;
                let o = m[1] * f1 + m[3] * f3;
                out[x * os] = e + o;
                out[(3 - x) * os] = e - o;
            }
        }
        2 => {
            let (e, o) = (M2 * f[0], M2 * f[fs]);
            out[0] = e + o;
            out[os] = e - o;
        }
        _ => out[0] = M2 * f[0],
    }
}

/// `coef` (natural order, dequantised, each within ±32767) to `nw` × `nh`
/// pixels written at `out[y * stride + x]`.
pub(crate) fn idct(coef: &[i32; 64], nw: usize, nh: usize, out: &mut [u8], stride: usize) {
    if nw == 1 && nh == 1 {
        out[0] = clamp((coef[0] + 4 + 1024) >> 3);
        return;
    }
    // Columns: for each of the first nw columns, nh outputs.
    let mut tmp = [0i32; 64];
    for u in 0..nw {
        let col = &coef[u..];
        let ac = (1..nh).any(|v| col[v * 8] != 0);
        if ac {
            idct1(col, 8, nh, &mut tmp[u..], 8);
            for y in 0..nh {
                let t = &mut tmp[y * 8 + u];
                *t = ((*t + (1 << (PASS1_SHIFT - 1))) >> PASS1_SHIFT).clamp(-TMP_LIMIT, TMP_LIMIT);
            }
        } else {
            // Only the DC of this column: constant (scaled like idct1).
            let dc = ((M2 * col[0] + (1 << (PASS1_SHIFT - 1))) >> PASS1_SHIFT).clamp(-TMP_LIMIT, TMP_LIMIT);
            for y in 0..nh {
                tmp[y * 8 + u] = dc;
            }
        }
    }
    // Rows: nw outputs from the nw intermediate values of each row.
    let mut row = [0i32; 8];
    for y in 0..nh {
        idct1(&tmp[y * 8..], 1, nw, &mut row, 1);
        let o = &mut out[y * stride..];
        for x in 0..nw {
            o[x] = clamp((row[x] + (1 << (PASS2_SHIFT - 1)) + (128 << PASS2_SHIFT)) >> PASS2_SHIFT);
        }
    }
}

#[inline]
fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Floating-point reference of the same transform.
    fn reference(coef: &[i32; 64], nw: usize, nh: usize) -> std::vec::Vec<f64> {
        let c = |u: usize| if u == 0 { core::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
        let mut out = std::vec::Vec::new();
        for y in 0..nh {
            for x in 0..nw {
                let mut s = 0.0;
                for v in 0..nh {
                    for u in 0..nw {
                        let cy = c(v) / 2.0
                            * ((2 * y + 1) as f64 * v as f64 * core::f64::consts::PI / (2 * nh) as f64).cos();
                        let cx = c(u) / 2.0
                            * ((2 * x + 1) as f64 * u as f64 * core::f64::consts::PI / (2 * nw) as f64).cos();
                        s += cy * cx * f64::from(coef[v * 8 + u]);
                    }
                }
                out.push((s + 128.0).clamp(0.0, 255.0));
            }
        }
        out
    }

    #[test]
    fn matches_float_reference() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as i32
        };
        for _ in 0..200 {
            let mut coef = [0i32; 64];
            for (i, c) in coef.iter_mut().enumerate() {
                let r = 512 / (1 + i as i32);
                *c = rnd() % (2 * r + 1) - r;
            }
            coef[0] = rnd() % 1024 - 512;
            for &(nw, nh) in &[(8, 8), (4, 4), (2, 2), (1, 1), (8, 4), (4, 8), (2, 1)] {
                let mut out = [0u8; 64];
                idct(&coef, nw, nh, &mut out, 8);
                let r = reference(&coef, nw, nh);
                for y in 0..nh {
                    for x in 0..nw {
                        let d = (f64::from(out[y * 8 + x]) - r[y * nw + x]).abs();
                        assert!(d <= 1.0, "{nw}x{nh} at {x},{y}: {} vs {}", out[y * 8 + x], r[y * nw + x]);
                    }
                }
            }
        }
    }
}
