//! The half-band tables against their documented design, their stop-band
//! attenuation and pass-band ripple, and the decimator's SNR on real
//! signals.

use std::f64::consts::PI;

use yoton_dsp::decimate::{Decimator, HB27, HB27_DESIGN, HB81, HB81_DESIGN};
use yoton_dsp::fixed::ONE;

/// Modified Bessel function of the first kind, order 0 (power series).
fn i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..60 {
        term *= (x / 2.0) * (x / 2.0) / f64::from(k * k);
        sum += term;
    }
    sum
}

/// Odd taps h[2k+1] of a Kaiser-windowed half-band, Q2.30.
fn kaiser_halfband(n: usize, beta: f64) -> Vec<i32> {
    let c = (n - 1) / 2;
    let mut v = Vec::new();
    let mut k = 1;
    while k <= c {
        let x = k as f64;
        let sinc = (PI * x / 2.0).sin() / (PI * x / 2.0);
        let r = 2.0 * (c + k) as f64 / (n - 1) as f64 - 1.0;
        let w = i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0(beta);
        v.push((0.5 * sinc * w * f64::from(1u32 << 30)).round() as i32);
        k += 2;
    }
    // Trailing zero taps (an N = 4P+1 design's even outermost tap) are not
    // in the table.
    while v.last() == Some(&0) {
        v.pop();
    }
    v
}

#[test]
fn tables_follow_the_documented_design() {
    for (table, (n, beta)) in [(&HB81[..], HB81_DESIGN), (&HB27[..], HB27_DESIGN)] {
        let want = kaiser_halfband(n, f64::from(beta));
        assert_eq!(want.len(), table.len(), "N = {n}");
        for (k, (&w, &t)) in want.iter().zip(table).enumerate() {
            assert!((w - t).abs() <= 1, "N = {n}, tap {k}: {t} vs {w}");
        }
    }
}

/// |H(f)| in dB of the quantised half-band (f as a fraction of its input
/// rate).
fn response_db(table: &[i32], f: f64) -> f64 {
    let w = 2.0 * PI * f;
    let mut re = 0.5;
    for (k, &h) in table.iter().enumerate() {
        re += 2.0 * f64::from(h) / f64::from(1u32 << 30) * (w * (2 * k + 1) as f64).cos();
    }
    20.0 * re.abs().log10()
}

fn band(table: &[i32], f0: f64, f1: f64) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for i in 0..=4000 {
        let f = f0 + (f1 - f0) * f64::from(i) / 4000.0;
        let d = response_db(table, f);
        lo = lo.min(d);
        hi = hi.max(d);
    }
    (lo, hi)
}

#[test]
fn stop_band_and_pass_band() {
    // HB81: pass 0 … 0.2125, stop 0.2875 … 0.5.
    let (lo, hi) = band(&HB81, 0.0, 0.2125);
    println!("HB81 pass band ripple {lo:+.6} … {hi:+.6} dB");
    assert!(lo > -0.0002 && hi < 0.0002);
    let (_, stop) = band(&HB81, 0.2875, 0.5);
    println!("HB81 stop band {stop:.1} dB");
    assert!(stop < -93.9);
    // HB27: pass 0 … 0.1125, stop 0.3875 … 0.5.
    let (lo, hi) = band(&HB27, 0.0, 0.1125);
    println!("HB27 pass band ripple {lo:+.6} … {hi:+.6} dB");
    assert!(lo > -0.0001 && hi < 0.0001);
    let (_, stop) = band(&HB27, 0.3875, 0.5);
    println!("HB27 stop band {stop:.1} dB");
    assert!(stop < -100.0);
}

/// Run a stereo sine at `f` Hz (amplitude `amp` of full scale, Q4.27)
/// through a decimator in 64-frame chunks; returns the left output channel.
fn run(factor: u8, rate: f64, f: f64, amp: f64, frames: usize) -> Vec<f64> {
    let mut d = Decimator::new();
    d.set_factor(factor);
    let mut out = Vec::new();
    let mut n = 0;
    while n < frames {
        let mut buf = [0i32; 128];
        for i in 0..64 {
            let v = (amp * f64::from(ONE) * (2.0 * PI * f * (n + i) as f64 / rate).sin()).round() as i32;
            buf[2 * i] = v;
            buf[2 * i + 1] = -v;
        }
        let m = d.process(&mut buf, 2);
        for i in 0..m {
            out.push(f64::from(buf[2 * i]) / f64::from(ONE));
            // The right channel (inverted input) is filtered independently.
            assert!((buf[2 * i + 1] + buf[2 * i]).abs() <= 1);
        }
        n += 64;
    }
    out
}

/// Fit a sine at `f` (output rate `fs`) and return (amplitude, SNR in dB).
fn fit(y: &[f64], f: f64, fs: f64) -> (f64, f64) {
    let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &v) in y.iter().enumerate() {
        let t = 2.0 * PI * f * i as f64 / fs;
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
    let mut err = 0.0;
    let mut sig = 0.0;
    for (i, &v) in y.iter().enumerate() {
        let t = 2.0 * PI * f * i as f64 / fs;
        let m = a * t.sin() + b * t.cos();
        err += (v - m) * (v - m);
        sig += m * m;
    }
    ((a * a + b * b).sqrt(), 10.0 * (sig / err).log10())
}

#[test]
fn decimated_sines_keep_level_and_snr() {
    for (factor, rate) in [(2u8, 96000.0), (2, 88200.0), (4, 192_000.0), (4, 176_400.0)] {
        let out_rate = rate / f64::from(factor);
        for f in [100.0, 1000.0, 6000.0, 15000.0, 18000.0] {
            let y = run(factor, rate, f, 0.9, 32768);
            // Skip the start (filter delay and history fill).
            let (a, snr) = fit(&y[512..], f, out_rate);
            let db = 20.0 * (a / 0.9).log10();
            println!("{factor}× {rate} Hz: {f} Hz level {db:+.5} dB, SNR {snr:.1} dB");
            assert!(db.abs() < 0.001, "level {db}");
            assert!(snr > 110.0, "SNR {snr}");
        }
    }
}

#[test]
fn stop_band_signals_are_removed() {
    // Frequencies whose aliases would land in the audio band.
    for (factor, rate, f) in
        [(2u8, 96000.0, 30000.0), (2, 96000.0, 40000.0), (4, 192_000.0, 30000.0), (4, 192_000.0, 80000.0)]
    {
        let y = run(factor, rate, f, 0.9, 32768);
        let rms = (y[512..].iter().map(|v| v * v).sum::<f64>() / (y.len() - 512) as f64).sqrt();
        let db = 20.0 * (rms * 2f64.sqrt() / 0.9).log10();
        println!("{factor}× {rate} Hz: {f} Hz → {db:.1} dB");
        assert!(db < -93.0, "{f} Hz leaks at {db} dB");
    }
}
