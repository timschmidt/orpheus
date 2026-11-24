//! Biquad magnitude responses against the analytic cookbook curves.
//!
//! The reference is the RBJ design evaluated in `f64` (host only). Two
//! checks: (1) the quantised Q3.28 coefficients' response within 0.01 dB of
//! the analytic one; (2) the response measured by running sines through the
//! fixed-point cascade within 0.1 dB.

use std::f64::consts::PI;

use yoton_dsp::biquad::{BiquadCascade, Coeffs, Shape, SoftCascade, design};
use yoton_dsp::fixed::ONE;

/// Analytic RBJ coefficients `[b0, b1, b2, a1, a2]` in f64.
fn rbj(shape: Shape, f: f64, q: f64, db: f64, fs: f64) -> [f64; 5] {
    let a = 10f64.powf(db / 40.0);
    let w0 = 2.0 * PI * f / fs;
    let (c, s) = (w0.cos(), w0.sin());
    let al = s / (2.0 * q);
    let sa = a.sqrt();
    let (b0, b1, b2, a0, a1, a2) = match shape {
        Shape::Peaking => (1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a),
        Shape::LowShelf => (
            a * ((a + 1.0) - (a - 1.0) * c + 2.0 * sa * al),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - 2.0 * sa * al),
            (a + 1.0) + (a - 1.0) * c + 2.0 * sa * al,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - 2.0 * sa * al,
        ),
        Shape::HighShelf => (
            a * ((a + 1.0) + (a - 1.0) * c + 2.0 * sa * al),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - 2.0 * sa * al),
            (a + 1.0) - (a - 1.0) * c + 2.0 * sa * al,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - 2.0 * sa * al,
        ),
    };
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

fn mag_db(c: [f64; 5], f: f64, fs: f64) -> f64 {
    let w = 2.0 * PI * f / fs;
    let (re1, im1) = (w.cos(), -w.sin());
    let (re2, im2) = ((2.0 * w).cos(), -(2.0 * w).sin());
    let nr = c[0] + c[1] * re1 + c[2] * re2;
    let ni = c[1] * im1 + c[2] * im2;
    let dr = 1.0 + c[3] * re1 + c[4] * re2;
    let di = c[3] * im1 + c[4] * im2;
    10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
}

fn quantised(c: Coeffs) -> [f64; 5] {
    let f = |v: i32| f64::from(v) / f64::from(1 << 28);
    [f(c.b0), f(c.b1), f(c.b2), f(c.a1), f(c.a2)]
}

const CASES: &[(Shape, f64, f64, f64)] = &[
    (Shape::Peaking, 60.0, 0.7, 12.0),
    (Shape::Peaking, 60.0, 0.7, -12.0),
    (Shape::Peaking, 230.0, 0.7, 6.0),
    (Shape::Peaking, 910.0, 2.0, -9.0),
    (Shape::Peaking, 3600.0, 0.7, 12.0),
    (Shape::Peaking, 14000.0, 0.7, 12.0),
    (Shape::Peaking, 14000.0, 0.7, -12.0),
    (Shape::Peaking, 1000.0, 4.0, 3.0),
    (Shape::LowShelf, 120.0, 0.71, 12.0),
    (Shape::LowShelf, 120.0, 0.71, -12.0),
    (Shape::HighShelf, 6000.0, 0.71, 12.0),
    (Shape::HighShelf, 6000.0, 0.71, -6.0),
    (Shape::LowShelf, 40.0, 0.71, 6.0),
];

const RATES: &[u32] = &[8000, 22050, 32000, 44100, 48000];

#[test]
fn quantised_coefficients_match_analytic_response() {
    let mut worst: f64 = 0.0;
    for &(shape, f, q, db) in CASES {
        for &rate in RATES {
            let fs = f64::from(rate);
            if f >= 0.49 * fs {
                assert!(design(shape, f as f32, q as f32, db as f32, rate).is_identity());
                continue;
            }
            let c = quantised(design(shape, f as f32, q as f32, db as f32, rate));
            let r = rbj(shape, f, q, db, fs);
            // 200 log-spaced points from 10 Hz to 0.499 fs.
            for k in 0..200 {
                let fr = 10.0 * (0.499 * fs / 10.0).powf(f64::from(k) / 199.0);
                let e = (mag_db(c, fr, fs) - mag_db(r, fr, fs)).abs();
                worst = worst.max(e);
                assert!(e < 0.01, "{shape:?} {f} Hz Q{q} {db} dB at {rate}: {fr:.1} Hz off by {e:.4} dB");
            }
        }
    }
    println!("worst coefficient-response error {worst:.5} dB");
}

/// Amplitude and phase of a sine at `f` in `y` (least squares on sin/cos).
fn fit(y: &[f64], f: f64, fs: f64, n0: usize) -> f64 {
    let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &v) in y.iter().enumerate() {
        let t = 2.0 * PI * f * (i + n0) as f64 / fs;
        let (s, c) = (t.sin(), t.cos());
        ss += s * s;
        sc += s * c;
        cc += c * c;
        ys += v * s;
        yc += v * c;
    }
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    (a * a + b * b).sqrt()
}

/// Gain of the fixed-point cascade at `f`, measured with a sine.
fn measured_db(c: Coeffs, f: f64, rate: u32) -> f64 {
    let fs = f64::from(rate);
    let mut cas = SoftCascade::<1>::new();
    cas.set(0, c, 0);
    let amp = f64::from(ONE) * 0.2;
    // Settle for 1 s (60 Hz poles decay within ~0.1 s), measure over whole
    // periods (at least 4096 samples).
    let settle = rate as usize;
    let periods = ((4096.0 * f / fs).ceil()).max(1.0);
    let len = (periods * fs / f).round() as usize;
    let mut out = Vec::with_capacity(len);
    let mut n = 0usize;
    let mut buf = [0i32; 64];
    while n < settle + len {
        for (i, x) in buf.iter_mut().enumerate() {
            *x = (amp * (2.0 * PI * f * (n + i) as f64 / fs).sin()).round() as i32;
        }
        cas.process(&mut buf, 1);
        for (i, &y) in buf.iter().enumerate() {
            if n + i >= settle && out.len() < len {
                out.push(f64::from(y));
            }
        }
        n += 64;
    }
    20.0 * (fit(&out, f, fs, settle) / amp).log10()
}

#[test]
fn fixed_point_sections_match_analytic_response() {
    let mut worst: f64 = 0.0;
    for &(shape, f, q, db) in CASES {
        for &rate in &[8000u32, 44100, 48000] {
            let fs = f64::from(rate);
            if f >= 0.49 * fs {
                continue;
            }
            let c = design(shape, f as f32, q as f32, db as f32, rate);
            let r = rbj(shape, f, q, db, fs);
            for fr in [20.0, f / 2.0, f, f * 1.5, 1000.0, 0.45 * fs] {
                if fr >= 0.49 * fs {
                    continue;
                }
                let e = (measured_db(c, fr, rate) - mag_db(r, fr, fs)).abs();
                worst = worst.max(e);
                assert!(
                    e < 0.1,
                    "{shape:?} {f} Hz Q{q} {db} dB at {rate}: measured at {fr} Hz off by {e:.4} dB"
                );
            }
        }
    }
    println!("worst measured error {worst:.5} dB");
}
