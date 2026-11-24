//! Dither: bit-exact for a fixed seed (against an independent model and a
//! golden checksum), TPDF statistics, and the noise-shaping spectrum.

use std::f64::consts::PI;

use yoton_dsp::dither::{DitherMode, Ditherer};
use yoton_dsp::fixed::ONE;

/// Test signal: a 24-bit-precision 997 Hz sine at −20 dBFS plus a slow ramp,
/// stereo, Q4.27.
fn signal(n: usize) -> Vec<i32> {
    (0..n)
        .map(|i| {
            let fr = (i / 2) as f64;
            let ch = (i % 2) as f64;
            let v = 0.1 * (2.0 * PI * 997.0 * fr / 44100.0 + ch).sin() + (fr / n as f64 - 0.25) * 1e-3;
            (v * f64::from(ONE)).round() as i32
        })
        .collect()
}

/// Straightforward model of the documented algorithm (shared xorshift32
/// stream, TPDF from the two halves, three-tap error feedback in Q12).
fn model(x: &[i32], seed: u32, shaped: bool) -> Vec<i32> {
    let mut s = seed;
    let mut e = [[0i64; 3]; 2];
    let c = [6648i64, -4022, 446];
    let mut out = Vec::new();
    for (i, &v) in x.iter().enumerate() {
        let ch = i % 2;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let d = (i64::from(s & 0xFFFF) + i64::from(s >> 16) - 65535) >> 4;
        let fb = if shaped { (c[0] * e[ch][0] + c[1] * e[ch][1] + c[2] * e[ch][2]) >> 12 } else { 0 };
        let w = i64::from(v) - fb;
        let q = ((w + d + 2048) >> 12).clamp(-32768, 32767);
        let err = ((q << 12) - w).clamp(-16384, 16384);
        e[ch] = [err, e[ch][0], e[ch][1]];
        out.push(q as i32);
    }
    out
}

fn fnv(v: &[i32]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in v {
        for b in x.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn run(mode: DitherMode, seed: u32, x: &[i32]) -> Vec<i32> {
    let mut d = Ditherer::new(mode);
    d.set(mode, 44100);
    d.seed(seed);
    let mut out = x.to_vec();
    // Uneven pieces: the state must carry across calls.
    let mut i = 0;
    for len in [2usize, 64, 126, 30].iter().cycle() {
        if i >= out.len() {
            break;
        }
        let end = (i + len).min(out.len());
        d.quantize(&mut out[i..end], 2);
        i = end;
    }
    out
}

#[test]
fn fixed_seed_is_bit_exact() {
    let x = signal(20_000);
    for (mode, shaped) in [(DitherMode::Tpdf, false), (DitherMode::Shaped, true)] {
        let a = run(mode, 0xDEAD_BEEF, &x);
        let b = run(mode, 0xDEAD_BEEF, &x);
        assert_eq!(a, b, "{mode:?} repeatable");
        assert_eq!(a, model(&x, 0xDEAD_BEEF, shaped), "{mode:?} matches the model");
        let c = run(mode, 0x1357_9BDF, &x);
        assert_ne!(a, c, "{mode:?} depends on the seed");
        println!("{mode:?} checksum {:#018x}", fnv(&a));
    }
    // Golden checksums: a change here changes every dithered output.
    assert_eq!(fnv(&run(DitherMode::Tpdf, 0xDEAD_BEEF, &x)), GOLDEN_TPDF);
    assert_eq!(fnv(&run(DitherMode::Shaped, 0xDEAD_BEEF, &x)), GOLDEN_SHAPED);
}

const GOLDEN_TPDF: u64 = 0xafc2_4f0b_d2c6_728f;
const GOLDEN_SHAPED: u64 = 0x26ae_cf29_983a_d884;

#[test]
fn tpdf_error_statistics() {
    // Error of flat TPDF: mean 0, variance = LSB²/12 (rounding) + LSB²/6
    // (TPDF) = LSB²/4, independent of the signal.
    let x = signal(200_000);
    let y = run(DitherMode::Tpdf, 42, &x);
    let lsb = f64::from(1 << 12);
    let errs: Vec<f64> = x.iter().zip(&y).map(|(&a, &q)| f64::from(q) - f64::from(a) / lsb).collect();
    let mean = errs.iter().sum::<f64>() / errs.len() as f64;
    let var = errs.iter().map(|e| (e - mean) * (e - mean)).sum::<f64>() / errs.len() as f64;
    println!("TPDF error mean {mean:.4} LSB, variance {var:.4} LSB²");
    assert!(mean.abs() < 0.01);
    assert!((var - 0.25).abs() < 0.01);
    // Rounding without dither is exact on values already on the grid.
    let grid: Vec<i32> = (-100..100).map(|v| v << 12).collect();
    assert_eq!(run(DitherMode::Off, 1, &grid), (-100..100).collect::<Vec<i32>>());
}

/// Power of `e` in the band f0..f1 (fraction of the sample rate) by DFT.
fn band_power(e: &[f64], f0: f64, f1: f64) -> f64 {
    let mut p = 0.0;
    let n = 2048;
    let bins = (f0 * n as f64) as usize..(f1 * n as f64) as usize;
    for seg in e.chunks_exact(n) {
        for k in bins.clone() {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in seg.iter().enumerate() {
                let w = 2.0 * PI * (k * i) as f64 / n as f64;
                re += v * w.cos();
                im -= v * w.sin();
            }
            p += re * re + im * im;
        }
    }
    p
}

#[test]
fn noise_shaping_moves_noise_up() {
    let x = signal(2 * 8192);
    let lsb = f64::from(1 << 12);
    let err = |y: &[i32]| -> Vec<f64> {
        x.iter().zip(y).step_by(2).map(|(&a, &q)| f64::from(q) - f64::from(a) / lsb).collect()
    };
    let flat = err(&run(DitherMode::Tpdf, 7, &x));
    let shaped = err(&run(DitherMode::Shaped, 7, &x));
    // 0–4 kHz at 44.1 kHz: shaped noise must be clearly lower.
    let lo_flat = band_power(&flat, 0.0, 0.09);
    let lo_shaped = band_power(&shaped, 0.0, 0.09);
    let gain = 10.0 * (lo_shaped / lo_flat).log10();
    println!("shaped vs flat noise below 4 kHz: {gain:.1} dB");
    assert!(gain < -6.0);
}
