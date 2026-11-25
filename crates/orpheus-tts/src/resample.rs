//! The 16 kHz → output-rate converter: a polyphase windowed-sinc
//! interpolator.
//!
//! * **Kernel**: `h(u) = fc·sinc(fc·u)·w(u/8)`, 16 taps (|u| < 8 input
//!   samples), `fc` = 0.92 (cutoff 7.36 kHz, the transition band centred
//!   just below the input's 8 kHz Nyquist), `w` a Kaiser window with β = 6.
//!   It is tabulated at [`PHASES`] + 1 fractional positions (each phase
//!   normalised to unity DC gain), built at compile time (`const fn`, f64);
//!   between table phases the coefficients are interpolated linearly.
//! * **Exact ratios**: the output time advances by 16000/R input samples
//!   with an integer accumulator, so the phase is exact and 48, 32 and
//!   24 kHz land on table phases (one 16-tap product per output); 44.1 and
//!   22.05 kHz interpolate between two phases. 16 kHz is passed through.
//! * **Below 16 kHz** (8, 11.025, 12 kHz) a fourth-order Butterworth
//!   low-pass at 0.45 × R runs first (two biquads), so the decimation does
//!   not alias.
//! * Latency: 8 input samples (0.5 ms). The ring is doubled so the 16
//!   inputs are always contiguous.

use orpheus_synth::math::{clampf, exp2, sin_turns};

/// Taps per phase.
pub const TAPS: usize = 16;
/// Table phases per input sample.
pub const PHASES: usize = 48;
const HALF: usize = TAPS / 2;

const fn sin64(x: f64) -> f64 {
    let tau = core::f64::consts::TAU;
    let pi = core::f64::consts::PI;
    let mut y = x - ((x / tau) as i64 as f64) * tau;
    if y > pi {
        y -= tau;
    } else if y < -pi {
        y += tau;
    }
    // Taylor series to y^29 on [−π, π].
    let y2 = y * y;
    let mut term = y;
    let mut sum = y;
    let mut k = 1;
    while k < 15 {
        term = -term * y2 / ((2 * k) as f64 * (2 * k + 1) as f64);
        sum += term;
        k += 1;
    }
    sum
}

/// I0(√z2) by its series.
const fn bessel_i0_sq(z2: f64) -> f64 {
    let q = z2 / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1;
    while k < 40 {
        term = term * q / ((k * k) as f64);
        sum += term;
        k += 1;
    }
    sum
}

const BETA: f64 = 6.0;
const FC: f64 = 0.92;

const fn kernel(u: f64) -> f64 {
    let r = u / HALF as f64;
    if r <= -1.0 || r >= 1.0 {
        return 0.0;
    }
    let w = bessel_i0_sq(BETA * BETA * (1.0 - r * r)) / bessel_i0_sq(BETA * BETA);
    let x = FC * u;
    let s = if x == 0.0 { 1.0 } else { sin64(core::f64::consts::PI * x) / (core::f64::consts::PI * x) };
    FC * s * w
}

const fn build() -> [[f32; TAPS]; PHASES + 1] {
    let mut t = [[0.0f32; TAPS]; PHASES + 1];
    let mut p = 0;
    while p <= PHASES {
        let phi = p as f64 / PHASES as f64;
        let mut c = [0.0f64; TAPS];
        let mut sum = 0.0;
        let mut j = 0;
        while j < TAPS {
            // Tap j weighs x[n − 7 + j]; the output is at n + φ.
            c[j] = kernel(phi + (HALF - 1) as f64 - j as f64);
            sum += c[j];
            j += 1;
        }
        let mut j = 0;
        while j < TAPS {
            t[p][j] = (c[j] / sum) as f32;
            j += 1;
        }
        p += 1;
    }
    t
}

/// The coefficient table (read-only data, 3,136 B).
pub static TABLE: [[f32; TAPS]; PHASES + 1] = build();

/// A biquad (transposed direct form II).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    s1: f32,
    s2: f32,
}

impl Biquad {
    /// A low-pass at `f` Hz (16 kHz), quality `q` (the RBJ cookbook form).
    fn low_pass(f: f32, q: f32) -> Biquad {
        let p = f / 16_000.0;
        let (cs, sn) = (sin_turns(p + 0.25), sin_turns(p));
        let alpha = sn / (2.0 * q);
        let a0 = 1.0 + alpha;
        let inv = 1.0 / a0;
        let b1 = (1.0 - cs) * inv;
        Biquad {
            b0: b1 * 0.5,
            b1,
            b2: b1 * 0.5,
            a1: -2.0 * cs * inv,
            a2: (1.0 - alpha) * inv,
            s1: 0.0,
            s2: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.s1;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The converter (see the module documentation).
#[derive(Clone, Copy, Debug)]
pub struct Resampler {
    rate: u32,
    ring: [f32; 2 * TAPS],
    /// Next write position in the ring (0..TAPS).
    w: usize,
    /// Output time's fraction, in units of 1/rate input samples.
    acc: u32,
    /// Inputs still needed before the next output (n + 9 − pushed).
    lag: u32,
    pre: [Biquad; 2],
    pre_on: bool,
}

impl Resampler {
    /// A converter from 16 kHz to `rate` (8–48 kHz; clamped).
    pub fn new(rate: u32) -> Resampler {
        let rate = rate.clamp(8000, 48_000);
        let pre_on = rate < 16_000;
        let fc = clampf(0.45 * rate as f32, 1000.0, 7000.0);
        Resampler {
            rate,
            ring: [0.0; 2 * TAPS],
            w: 0,
            acc: 0,
            lag: if rate == 16_000 { 1 } else { HALF as u32 + 1 },
            pre: [Biquad::low_pass(fc, 0.541_196_1), Biquad::low_pass(fc, 1.306_563)],
            pre_on,
        }
    }

    /// The output rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Whether [`Resampler::push`] must come before the next
    /// [`Resampler::pull`].
    #[inline]
    pub fn needs_input(&self) -> bool {
        self.lag > 0
    }

    /// Adds one 16 kHz sample.
    #[inline]
    pub fn push(&mut self, x: f32) {
        let x = if self.pre_on {
            let y = self.pre[0].tick(x);
            self.pre[1].tick(y)
        } else {
            x
        };
        self.ring[self.w] = x;
        self.ring[self.w + TAPS] = x;
        self.w = (self.w + 1) % TAPS;
        self.lag = self.lag.saturating_sub(1);
    }

    /// The next output sample (call only when [`Resampler::needs_input`]
    /// is false).
    #[inline]
    pub fn pull(&mut self) -> f32 {
        if self.rate == 16_000 {
            self.lag = 1;
            let last = (self.w + TAPS - 1) % TAPS;
            return self.ring[last];
        }
        // The window x[n−7 ..= n+8] is the last 16 pushed: ring[w .. w+16].
        let win = &self.ring[self.w..self.w + TAPS];
        // acc < rate ≤ 48000: acc·48 < 2³² (the M4's hardware divide).
        let pos = self.acc * PHASES as u32;
        let p = (pos / self.rate) as usize;
        let rem = pos % self.rate;
        let y = if rem == 0 {
            let c = &TABLE[p];
            let mut s = 0.0;
            for j in 0..TAPS {
                s += c[j] * win[j];
            }
            s
        } else {
            let f = rem as f32 / self.rate as f32;
            let (c0, c1) = (&TABLE[p], &TABLE[p + 1]);
            let (mut s0, mut s1) = (0.0, 0.0);
            for j in 0..TAPS {
                s0 += c0[j] * win[j];
                s1 += c1[j] * win[j];
            }
            s0 + (s1 - s0) * f
        };
        self.acc += 16_000;
        while self.acc >= self.rate {
            self.acc -= self.rate;
            self.lag += 1;
        }
        y
    }

    /// Clears the history (after a stop).
    pub fn reset(&mut self) {
        *self = Resampler::new(self.rate);
    }
}

/// Output frames for `n` input samples: the outputs at times
/// k·16000/rate < n.
pub fn frames_for(n: u64, rate: u32) -> u64 {
    (n * u64::from(rate)).div_ceil(16_000)
}

/// 10^(db/20) (for the tests and the voices).
pub fn db(db: f32) -> f32 {
    exp2(db * (core::f32::consts::LOG2_10 / 20.0))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn run(rate: u32, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(rate);
        let mut out = Vec::new();
        let mut i = 0;
        let want = frames_for(input.len() as u64, rate) as usize;
        while out.len() < want {
            if r.needs_input() {
                r.push(input.get(i).copied().unwrap_or(0.0));
                i += 1;
            } else {
                out.push(r.pull());
            }
        }
        out
    }

    fn tone_gain(rate: u32, hz: f64) -> f64 {
        let input: Vec<f32> = (0..16_000)
            .map(|n| (core::f64::consts::TAU * hz * f64::from(n) / 16_000.0).sin() as f32)
            .collect();
        let out = run(rate, &input);
        let mid = &out[out.len() / 4..3 * out.len() / 4];
        let rms = (mid.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / mid.len() as f64).sqrt();
        rms * core::f64::consts::SQRT_2
    }

    #[test]
    fn table_is_normalised_and_symmetric() {
        for row in &TABLE {
            let s: f32 = row.iter().sum();
            assert!((s - 1.0).abs() < 1e-5);
        }
        // Phase 0 and phase 1 (= the next sample's phase 0) are mirror images.
        for j in 0..TAPS - 1 {
            assert!((TABLE[0][j] - TABLE[PHASES][j + 1]).abs() < 1e-6);
        }
    }

    #[test]
    fn counts_and_passband() {
        for rate in [8000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000] {
            let out = run(rate, &[0.5; 1600]);
            assert_eq!(out.len() as u64, frames_for(1600, rate));
            // DC passes (after the start-up).
            let mid = out[out.len() / 2];
            assert!((mid - 0.5).abs() < 0.01, "{rate}: {mid}");
            let g = tone_gain(rate, 1000.0);
            assert!((g - 1.0).abs() < 0.02, "{rate}: 1 kHz gain {g}");
        }
        for rate in [44_100, 48_000] {
            assert!(tone_gain(rate, 5000.0) > 0.95, "{rate}: 5 kHz");
        }
    }

    #[test]
    fn images_are_attenuated() {
        // A 6 kHz tone at 48 kHz has its first image at 10 kHz: measure the
        // energy there by correlation.
        let input: Vec<f32> = (0..16_000)
            .map(|n| (core::f64::consts::TAU * 6000.0 * f64::from(n) / 16_000.0).sin() as f32)
            .collect();
        let out = run(48_000, &input);
        let corr = |hz: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (k, v) in out.iter().enumerate().skip(1000).take(40_000) {
                let w = core::f64::consts::TAU * hz * k as f64 / 48_000.0;
                re += f64::from(*v) * w.cos();
                im += f64::from(*v) * w.sin();
            }
            (re * re + im * im).sqrt() / 20_000.0
        };
        let main = corr(6000.0);
        let image = corr(10_000.0);
        assert!(main > 0.9, "{main}");
        assert!(image < main * 0.03, "image {image} vs {main}");
    }
}
