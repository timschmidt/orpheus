//! Oscillators: band-limited saw, pulse and triangle, sine, wavetables, white
//! and pink noise.
//!
//! The phase is a 32-bit accumulator (exact, drift-free wrap); the
//! waveform code sees it as `p` in [0, 1) with 24 bits of precision.
//!
//! **Band-limiting.** Saw and pulse use the two-sample polynomial BLEP: the
//! naive waveform plus, at each jump of height `h`, `h·R(x)` on the sample
//! before and after it, where `x` is the distance from the jump in samples
//! and `R` is the residual of the integrated triangle kernel
//! (`R(x) = (x+1)²/2` for x in [−1, 0), `−(1−x)²/2` for x in [0, 1)). The
//! triangle uses the integral of that residual (polyBLAMP, `(1−|x|)³/6`)
//! at each change of slope, scaled by the slope change per sample. Both are
//! written from the published descriptions (Välimäki & Huovilainen 2007 for
//! polyBLEP; Esqueda, Välimäki & Bilbao 2016 for polyBLAMP). The spectrum
//! test (`tests/spectrum.rs`) measures the aliasing.
//!
//! An oscillator whose fundamental is at or above 0.45 of the sample rate
//! outputs silence rather than an aliased wave.

use crate::math::{Rng, sin_phase};
use crate::wavetable;

/// Waveform of an oscillator.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Waveform {
    /// Sine (polynomial, error < 2.5·10⁻⁷).
    Sine,
    /// Rising sawtooth, band-limited (polyBLEP).
    Saw,
    /// Pulse with the oscillator's width (0.5 = square), band-limited
    /// (polyBLEP), zero-mean: the DC of an asymmetric pulse (2w − 1) is
    /// removed, so width modulation does not move the baseline (a narrow
    /// pulse then peaks near ±2(1 − w)).
    Pulse,
    /// Triangle, band-limited (polyBLAMP).
    Triangle,
    /// Built-in wavetable `n` ([`crate::wavetable`]), mip-mapped.
    Table(u8),
    /// White noise (uniform, xorshift32).
    White,
    /// Pink noise (Voss–McCartney, 12 rows: −3 dB/octave over ~12 octaves).
    Pink,
}

/// Highest fundamental as a fraction of the sample rate that still sounds.
pub const MAX_DT: f32 = 0.45;

const PHASE_TO_F: f32 = 1.0 / 16_777_216.0;

/// Phase as a fraction in [0, 1).
#[inline]
fn frac(phase: u32) -> f32 {
    (phase >> 8) as f32 * PHASE_TO_F
}

/// Phase increment for `dt` cycles per sample (`dt` in [0, 1)).
#[inline]
#[must_use]
pub fn inc_for(dt: f32) -> u32 {
    (dt * 4_294_967_296.0) as u32
}

/// polyBLEP residual of a unit upward step at phase 0, for phase `p` and
/// increment `dt` (both in cycles; `inv` = 1/`dt`, so the edge samples cost
/// a multiply rather than a 14-cycle division on the M4).
#[inline]
#[must_use]
pub fn blep(p: f32, dt: f32, inv: f32) -> f32 {
    if p < dt {
        let x = p * inv;
        let y = 1.0 - x;
        -0.5 * y * y
    } else if p > 1.0 - dt {
        let x = (p - 1.0) * inv;
        let y = x + 1.0;
        0.5 * y * y
    } else {
        0.0
    }
}

/// polyBLAMP residual of a unit change of slope (per sample) at phase 0.
#[inline]
#[must_use]
pub fn blamp(p: f32, dt: f32, inv: f32) -> f32 {
    let y = if p < dt {
        1.0 - p * inv
    } else if p > 1.0 - dt {
        1.0 + (p - 1.0) * inv
    } else {
        return 0.0;
    };
    y * y * y * (1.0 / 6.0)
}

/// Pink noise by the Voss–McCartney method: row `k` of [`PINK_ROWS`] takes a
/// new random value every 2^(k+1) samples (the trailing zeros of a counter
/// pick the row), and the output is the sum of the rows plus a white sample.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pink {
    rows: [i16; PINK_ROWS],
    sum: i32,
    n: u32,
}

/// Rows of the pink-noise generator.
pub const PINK_ROWS: usize = 12;

impl Pink {
    /// The next sample, in [−1, 1].
    #[inline]
    pub fn next(&mut self, rng: &mut Rng) -> f32 {
        self.n = self.n.wrapping_add(1);
        let k = self.n.trailing_zeros() as usize;
        let r = rng.next_u32();
        if k < PINK_ROWS {
            let v = (r >> 16) as u16 as i16;
            self.sum += i32::from(v) - i32::from(self.rows[k]);
            self.rows[k] = v;
        }
        let white = i32::from(r as u16 as i16);
        (self.sum + white) as f32 * (1.0 / (32768.0 * (PINK_ROWS as f32 + 1.0)))
    }
}

/// One oscillator. Used inside the voices and on its own (the signal
/// generator, 05 §7.17).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct Osc {
    /// Waveform.
    pub wave: Waveform,
    phase: u32,
    inc: u32,
    dt: f32,
    inv_dt: f32,
    width: f32,
    level: u8,
    mute: bool,
    rng: Rng,
    pink: Pink,
}

impl Osc {
    /// An oscillator at phase 0, silent until [`Osc::set_dt`] or
    /// [`Osc::set_freq`]; `seed` drives the noise waveforms.
    #[must_use]
    pub const fn new(wave: Waveform, seed: u32) -> Self {
        Self {
            wave,
            phase: 0,
            inc: 0,
            dt: 0.0,
            inv_dt: 0.0,
            width: 0.5,
            level: 0,
            mute: !matches!(wave, Waveform::White | Waveform::Pink),
            rng: Rng::new(seed),
            pink: Pink { rows: [0; PINK_ROWS], sum: 0, n: 0 },
        }
    }

    /// Set the frequency in Hz at sample rate `rate`.
    pub fn set_freq(&mut self, hz: f32, rate: f32) {
        self.set_dt(hz / rate);
    }

    /// Set the frequency in cycles per sample. At or above [`MAX_DT`] (and
    /// for NaN or negative values) the oscillator is muted.
    pub fn set_dt(&mut self, dt: f32) {
        if (0.0..MAX_DT).contains(&dt) {
            self.dt = dt;
            self.inv_dt = if dt > 0.0 { 1.0 / dt } else { 0.0 };
            self.inc = inc_for(dt);
            self.mute = false;
            if let Waveform::Table(_) = self.wave {
                match wavetable::level_for(dt) {
                    Some(k) => self.level = k,
                    None => self.mute = true,
                }
            }
        } else {
            self.dt = 0.0;
            self.inc = 0;
            self.mute = !matches!(self.wave, Waveform::White | Waveform::Pink);
        }
    }

    /// Pulse width in (0, 1), clamped to [0.01, 0.99]. (When the width is
    /// narrower than one sample period the two residuals overlap and add,
    /// which is still the band-limited sum of the two steps.)
    pub fn set_width(&mut self, w: f32) {
        self.width = crate::math::clampf(w, 0.01, 0.99);
    }

    /// Set the phase to `p` turns (wrapped).
    pub fn set_phase(&mut self, p: f32) {
        let f = p - (p as i32) as f32;
        let f = if f < 0.0 { f + 1.0 } else { f };
        self.phase = inc_for(crate::math::clampf(f, 0.0, 0.999_999_9));
    }

    /// Current phase in turns.
    #[must_use]
    pub fn phase(&self) -> f32 {
        frac(self.phase)
    }

    /// The next sample.
    #[inline]
    pub fn sample(&mut self) -> f32 {
        let mut one = [0.0];
        self.add_to(&mut one, 1.0);
        one[0]
    }

    /// Add `gain` × the next `out.len()` samples to `out`.
    pub fn add_to(&mut self, out: &mut [f32], gain: f32) {
        if self.mute {
            return;
        }
        let (dt, inv) = (self.dt, self.inv_dt);
        let inc = self.inc;
        let mut ph = self.phase;
        match self.wave {
            Waveform::Sine => {
                for o in out.iter_mut() {
                    *o += gain * sin_phase(ph);
                    ph = ph.wrapping_add(inc);
                }
            }
            Waveform::Saw => {
                for o in out.iter_mut() {
                    let p = frac(ph);
                    *o += gain * (2.0 * p - 1.0 - 2.0 * blep(p, dt, inv));
                    ph = ph.wrapping_add(inc);
                }
            }
            Waveform::Pulse => {
                let w = self.width;
                let dc = 2.0 * w - 1.0;
                for o in out.iter_mut() {
                    let p = frac(ph);
                    let q = if p >= w { p - w } else { p - w + 1.0 };
                    let naive = if p < w { 1.0 - dc } else { -1.0 - dc };
                    *o += gain * (naive + 2.0 * blep(p, dt, inv) - 2.0 * blep(q, dt, inv));
                    ph = ph.wrapping_add(inc);
                }
            }
            Waveform::Triangle => {
                let k = 8.0 * dt;
                for o in out.iter_mut() {
                    let p = frac(ph);
                    let q = if p >= 0.5 { p - 0.5 } else { p + 0.5 };
                    let naive = 1.0 - 4.0 * (p - 0.5).abs();
                    *o += gain * (naive + k * (blamp(p, dt, inv) - blamp(q, dt, inv)));
                    ph = ph.wrapping_add(inc);
                }
            }
            Waveform::Table(t) => {
                let lv = self.level;
                for o in out.iter_mut() {
                    *o += gain * wavetable::sample(t, lv, frac(ph));
                    ph = ph.wrapping_add(inc);
                }
            }
            Waveform::White => {
                for o in out.iter_mut() {
                    *o += gain * self.rng.bipolar();
                }
            }
            Waveform::Pink => {
                for o in out.iter_mut() {
                    *o += gain * self.pink.next(&mut self.rng);
                }
            }
        }
        self.phase = ph;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blep_residuals_are_continuous() {
        let (dt, inv) = (0.1, 10.0);
        assert!((blep(0.0, dt, inv) + 0.5).abs() < 1e-6);
        assert!((blep(0.999_999, dt, inv) - 0.5).abs() < 1e-4);
        assert!(blep(dt, dt, inv).abs() < 1e-6 && blep(1.0 - dt, dt, inv).abs() < 1e-6);
        assert!((blamp(0.0, dt, inv) - 1.0 / 6.0).abs() < 1e-6);
        assert!(blamp(0.5, dt, inv) == 0.0);
    }

    #[test]
    fn waves_are_bounded_and_zero_mean() {
        for w in [Waveform::Saw, Waveform::Pulse, Waveform::Triangle, Waveform::Sine, Waveform::Table(1)] {
            let mut o = Osc::new(w, 1);
            o.set_dt(0.0123);
            let mut buf = [0.0f32; 8130];
            o.add_to(&mut buf, 1.0);
            let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let mean = buf.iter().sum::<f32>() / buf.len() as f32;
            assert!(peak <= 1.1, "{w:?} peak {peak}");
            assert!(mean.abs() < 0.01, "{w:?} mean {mean}");
        }
    }

    #[test]
    fn noise_is_seeded_and_bounded() {
        let mut a = Osc::new(Waveform::Pink, 7);
        let mut b = Osc::new(Waveform::Pink, 7);
        let (mut x, mut y) = ([0.0f32; 4096], [0.0f32; 4096]);
        a.add_to(&mut x, 1.0);
        b.add_to(&mut y, 1.0);
        assert_eq!(x, y);
        assert!(x.iter().all(|v| v.abs() <= 1.0));
        let mut c = Osc::new(Waveform::Pink, 8);
        c.add_to(&mut y, 1.0);
        assert_ne!(x, y);
    }

    #[test]
    fn too_high_is_silent() {
        let mut o = Osc::new(Waveform::Saw, 1);
        o.set_dt(0.46);
        let mut buf = [0.0f32; 64];
        o.add_to(&mut buf, 1.0);
        assert!(buf.iter().all(|&v| v == 0.0));
    }
}
