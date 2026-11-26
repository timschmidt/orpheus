//! State-variable filter: low, band and high pass with resonance.
//!
//! The linear trapezoidal-integrator SVF (the topology-preserving transform
//! of the analogue state-variable filter, as described by Zavalishin, "The
//! Art of VA Filter Design", and Simper, "Linear Trapezoidal Integrated
//! SVF", 2013; written from those descriptions). With `g = tan(π fc/fs)` and
//! damping `k = 1/Q` it is unconditionally stable for every `g > 0, k > 0`,
//! keeps its tuning up to the clamp at 0.49 fs, and its coefficients can
//! change every control period without blowing up (the stability test sweeps
//! the cutoff to the top at full resonance over noise).
//!
//! Per sample: 2 multiplies by state, 3 by coefficients, 6 adds (+2 for the
//! high-pass output). State: 8 bytes; coefficients: 16 bytes.

use core::f32::consts::PI;

use crate::math::{clampf, flush, tan_lo};

/// Filter response.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum FilterMode {
    /// No filter (bypassed, no cost).
    #[default]
    Off,
    /// Low pass, 12 dB/octave.
    LowPass,
    /// Band pass, constant skirt gain (peak gain = Q).
    BandPass,
    /// High pass, 12 dB/octave.
    HighPass,
}

/// Lowest cutoff, Hz.
pub const MIN_CUTOFF: f32 = 16.0;
/// Highest cutoff as a fraction of the sample rate.
pub const MAX_CUTOFF_RATIO: f32 = 0.49;

/// State-variable filter state and coefficients.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

impl Svf {
    /// Cleared state, no coefficients (outputs 0 until [`Svf::set`]).
    pub const ZERO: Self = Self { ic1: 0.0, ic2: 0.0, a1: 0.0, a2: 0.0, a3: 0.0, k: 0.0 };

    /// Set the cutoff (Hz, clamped to [[`MIN_CUTOFF`], 0.49 fs]) and
    /// resonance (0 = Q 0.5, no peak; 1 = Q 50, close to self-oscillation).
    pub fn set(&mut self, cutoff_hz: f32, resonance: f32, rate: f32) {
        let fc = clampf(cutoff_hz, MIN_CUTOFF, MAX_CUTOFF_RATIO * rate);
        let g = tan_lo(PI * fc / rate);
        let k = 2.0 - 1.98 * clampf(resonance, 0.0, 1.0);
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.k = k;
    }

    /// One sample; returns (low pass, band pass).
    #[inline]
    pub fn tick(&mut self, v0: f32) -> (f32, f32) {
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1)
    }

    /// One sample of the high-pass output.
    #[inline]
    pub fn tick_hp(&mut self, v0: f32) -> f32 {
        let k = self.k;
        let (lp, bp) = self.tick(v0);
        v0 - k * bp - lp
    }

    /// Filter `buf` in place.
    pub fn process(&mut self, mode: FilterMode, buf: &mut [f32]) {
        match mode {
            FilterMode::Off => {}
            FilterMode::LowPass => {
                for x in buf.iter_mut() {
                    *x = self.tick(*x).0;
                }
            }
            FilterMode::BandPass => {
                for x in buf.iter_mut() {
                    *x = self.tick(*x).1;
                }
            }
            FilterMode::HighPass => {
                let k = self.k;
                for x in buf.iter_mut() {
                    let v0 = *x;
                    let (lp, bp) = self.tick(v0);
                    *x = v0 - k * bp - lp;
                }
            }
        }
    }

    /// Clear tiny state (end of each control period).
    pub fn flush(&mut self) {
        self.ic1 = flush(self.ic1);
        self.ic2 = flush(self.ic2);
    }

    /// Clear the state.
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Whether the state is finite (a test hook).
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.ic1.is_finite() && self.ic2.is_finite()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn gain(mode: FilterMode, fc: f32, f: f32) -> f32 {
        let rate = 48_000.0;
        let mut s = Svf::default();
        s.set(fc, 0.0, rate);
        let mut o = crate::osc::Osc::new(crate::osc::Waveform::Sine, 1);
        o.set_freq(f, rate);
        let mut peak = 0.0f32;
        for i in 0..40 {
            let mut b = [0.0f32; 1200];
            o.add_to(&mut b, 1.0);
            s.process(mode, &mut b);
            if i > 20 {
                peak = b.iter().fold(peak, |m, v| m.max(v.abs()));
            }
        }
        peak
    }

    #[test]
    fn responses() {
        // Butterworth-like (k = 2 is Q 0.5: −6 dB at the cutoff).
        assert!((gain(FilterMode::LowPass, 1000.0, 100.0) - 1.0).abs() < 0.02);
        assert!(gain(FilterMode::LowPass, 1000.0, 10_000.0) < 0.02);
        assert!((gain(FilterMode::LowPass, 1000.0, 1000.0) - 0.5).abs() < 0.02);
        assert!(gain(FilterMode::HighPass, 1000.0, 100.0) < 0.02);
        assert!((gain(FilterMode::HighPass, 1000.0, 10_000.0) - 1.0).abs() < 0.03);
        assert!((gain(FilterMode::BandPass, 1000.0, 1000.0) - 0.5).abs() < 0.02);
    }
}
