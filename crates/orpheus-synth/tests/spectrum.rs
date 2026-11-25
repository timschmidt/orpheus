//! Band-limiting checked by spectrum.
//!
//! Each oscillator runs at a high fundamental (not a divisor of the sample
//! rate, so aliases fall between the harmonics); 65 536 samples are
//! windowed (4-term Blackman–Harris, side lobes −92 dB) and transformed
//! (radix-2 FFT, `f64`, below). Every bin within ±8 bins of a harmonic
//! below Nyquist (or of DC) counts as signal; everything else is aliasing.
//! The test prints two alias-to-total ratios for each case: all aliases, and
//! the aliases below 12 kHz (where they are audible; the two-sample polyBLEP
//! leaves most of its residue in the octave below Nyquist, where the first
//! folded harmonics land). It checks both against budgets (the measured
//! values of 2026-11-25 plus about 2 dB) and the improvement over the naive
//! (non-band-limited) waveform. The floor of the method is the window's
//! side lobes, about −93 dB (what the sine measures).

use orpheus_synth::osc::{Osc, Waveform};

const N: usize = 1 << 16;
const RATE: f64 = 48_000.0;

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

/// Alias energy / total energy in dB of `x` with fundamental `f0`: all
/// aliases, and those below 12 kHz.
fn alias_db(x: &[f32], f0: f64) -> (f64, f64) {
    let mut re: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = 2.0 * std::f64::consts::PI * i as f64 / N as f64;
            let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
            f64::from(v) * w
        })
        .collect();
    let mut im = vec![0.0; N];
    fft(&mut re, &mut im);
    let bin = RATE / N as f64;
    let mut signal = vec![false; N / 2 + 1];
    let mut h = 0.0;
    while h * f0 < RATE / 2.0 {
        let c = (h * f0 / bin).round() as i64;
        for k in c - 8..=c + 8 {
            if (0..=(N / 2) as i64).contains(&k) {
                signal[k as usize] = true;
            }
        }
        h += 1.0;
    }
    let (mut total, mut alias, mut low) = (0.0, 0.0, 0.0);
    for k in 0..=N / 2 {
        let p = re[k] * re[k] + im[k] * im[k];
        total += p;
        if !signal[k] {
            alias += p;
            if k as f64 * bin < 12_000.0 {
                low += p;
            }
        }
    }
    let db = |a: f64| 10.0 * (a.max(1e-300) / total).log10();
    (db(alias), db(low))
}

fn render(wave: Waveform, width: f32, f0: f64) -> Vec<f32> {
    let mut o = Osc::new(wave, 1);
    o.set_dt((f0 / RATE) as f32);
    o.set_width(width);
    let mut buf = vec![0.0f32; N];
    o.add_to(&mut buf, 1.0);
    buf
}

/// The naive waveforms, for comparison.
fn naive(wave: Waveform, width: f32, f0: f64) -> Vec<f32> {
    let dt = f0 / RATE;
    (0..N)
        .map(|i| {
            let p = (i as f64 * dt).fract() as f32;
            match wave {
                Waveform::Saw => 2.0 * p - 1.0,
                Waveform::Pulse => {
                    if p < width {
                        1.0
                    } else {
                        -1.0
                    }
                }
                _ => 1.0 - 4.0 * (p - 0.5).abs(),
            }
        })
        .collect()
}

#[test]
fn aliasing_below_budget() {
    // (wave, width, f0, budget all aliases dB, budget below 12 kHz dB,
    // minimum improvement over naive dB, all aliases).
    let cases = [
        (Waveform::Saw, 0.5, 1046.5, -29.0, -48.0, 13.0),
        (Waveform::Saw, 0.5, 2793.8, -24.5, -41.5, 13.0),
        (Waveform::Saw, 0.5, 4186.0, -21.5, -41.0, 12.0),
        (Waveform::Pulse, 0.5, 1046.5, -30.0, -49.0, 13.0),
        (Waveform::Pulse, 0.5, 4186.0, -27.0, -40.5, 15.0),
        (Waveform::Pulse, 0.25, 2793.8, -26.0, -43.5, 12.5),
        (Waveform::Triangle, 0.5, 1046.5, -57.5, -79.5, 9.5),
        (Waveform::Triangle, 0.5, 4186.0, -43.5, -59.5, 12.5),
    ];
    let mut failures = Vec::new();
    for (wave, width, f0, budget, budget_low, gain) in cases {
        let (bl, bl_low) = alias_db(&render(wave, width, f0), f0);
        let (nv, nv_low) = alias_db(&naive(wave, width, f0), f0);
        println!(
            "{wave:?} w{width} {f0:7.1} Hz: alias {bl:6.1} dB, below 12 kHz {bl_low:6.1} dB \
             (naive {nv:6.1} / {nv_low:6.1} dB; {:4.1} dB better)",
            nv - bl
        );
        if bl > budget || bl_low > budget_low || nv - bl < gain {
            failures.push(format!(
                "{wave:?} {width} {f0}: {bl:.1} / {bl_low:.1} dB (budget {budget} / {budget_low})"
            ));
        }
    }
    // Wavetables are band-limited by construction (mip levels); what is
    // left is the linear interpolation's images.
    for t in 0..4u8 {
        for f0 in [261.6, 1046.5, 4186.0] {
            let (a, low) = alias_db(&render(Waveform::Table(t), 0.5, f0), f0);
            println!("Table({t}) {f0:7.1} Hz: alias {a:6.1} dB, below 12 kHz {low:6.1} dB");
            if a > -45.0 {
                failures.push(format!("Table({t}) {f0}: {a:.1} dB"));
            }
        }
    }
    // The sine has no harmonics at all: only the measurement floor.
    let (f, _) = alias_db(&render(Waveform::Sine, 0.5, 4186.0), 4186.0);
    println!("Sine 4186 Hz: everything off the harmonic grid {f:.1} dB");
    if f > -90.0 {
        failures.push(format!("Sine: {f:.1} dB"));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
