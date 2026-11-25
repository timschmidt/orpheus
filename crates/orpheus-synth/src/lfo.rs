//! Low-frequency oscillators: sine, triangle, square, saw, sample-and-hold;
//! free rate in Hz or synchronised to the tempo as a multiple of the beat.
//!
//! LFOs run at control rate (every [`crate::CONTROL_PERIOD`] samples, and at
//! every event that touches the voice), advanced by the number of samples
//! since the last update. Output is bipolar, −1 to 1.

use crate::math::{Rng, clampf, sin_turns};

/// LFO waveform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LfoWave {
    /// Sine.
    #[default]
    Sine,
    /// Triangle.
    Triangle,
    /// Square.
    Square,
    /// Rising saw.
    Saw,
    /// A new random value every cycle, held.
    SampleHold,
}

/// LFO rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LfoRate {
    /// Cycles per second (0–50).
    Hz(f32),
    /// Cycles per beat at the engine's tempo (1/16–16: 0.25 = one cycle per
    /// bar of 4/4, 4 = sixteenth notes).
    Sync(f32),
}

impl Default for LfoRate {
    fn default() -> Self {
        Self::Hz(5.0)
    }
}

impl LfoRate {
    /// Cycles per second at `bpm`.
    #[must_use]
    pub fn hz(self, bpm: f32) -> f32 {
        match self {
            Self::Hz(f) => clampf(f, 0.0, 50.0),
            Self::Sync(m) => clampf(m, 1.0 / 16.0, 16.0) * bpm * (1.0 / 60.0),
        }
    }
}

/// LFO settings and depths of its destinations.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LfoParams {
    /// Waveform.
    pub wave: LfoWave,
    /// Rate.
    pub rate: LfoRate,
    /// Pitch depth, semitones (vibrato).
    pub pitch: f32,
    /// Filter cutoff depth, octaves.
    pub cutoff: f32,
    /// Amplitude depth, 0–1 (tremolo; 1 swings the level from 0 to 1).
    pub amp: f32,
    /// Pulse-width depth, 0–0.5.
    pub width: f32,
    /// Restart the cycle at each note-on (otherwise free-running per voice).
    pub key_sync: bool,
}

impl LfoParams {
    /// The settings with every depth in range.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let rate = match self.rate {
            LfoRate::Hz(f) => LfoRate::Hz(clampf(f, 0.0, 50.0)),
            LfoRate::Sync(m) => LfoRate::Sync(clampf(m, 1.0 / 16.0, 16.0)),
        };
        Self {
            wave: self.wave,
            rate,
            pitch: clampf(self.pitch, -24.0, 24.0),
            cutoff: clampf(self.cutoff, -8.0, 8.0),
            amp: clampf(self.amp, 0.0, 1.0),
            width: clampf(self.width, 0.0, 0.5),
            key_sync: self.key_sync,
        }
    }

    /// Whether the LFO changes anything.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.pitch != 0.0 || self.cutoff != 0.0 || self.amp != 0.0 || self.width != 0.0
    }
}

/// LFO state (12 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lfo {
    phase: f32,
    held: f32,
    value: f32,
}

impl Lfo {
    /// Phase 0, value 0.
    pub const ZERO: Self = Self { phase: 0.0, held: 0.0, value: 0.0 };

    /// Restart the cycle at phase 0.
    pub fn reset(&mut self, rng: &mut Rng) {
        self.phase = 0.0;
        self.held = rng.bipolar();
    }

    /// Advance by `n` samples at `dphase` cycles per sample; returns the
    /// value at the new position.
    pub fn advance(&mut self, wave: LfoWave, dphase: f32, n: u32, rng: &mut Rng) -> f32 {
        self.phase += dphase * n as f32;
        if self.phase >= 1.0 {
            self.phase -= self.phase as i32 as f32;
            self.held = rng.bipolar();
        }
        if !(self.phase >= 0.0 && self.phase < 1.0) {
            self.phase = 0.0;
        }
        let p = self.phase;
        self.value = match wave {
            LfoWave::Sine => sin_turns(p),
            LfoWave::Triangle => {
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            LfoWave::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWave::Saw => 2.0 * p - 1.0,
            LfoWave::SampleHold => self.held,
        };
        self.value
    }

    /// The value at the last update.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_rate_and_waves() {
        assert!((LfoRate::Sync(1.0).hz(120.0) - 2.0).abs() < 1e-6);
        assert!((LfoRate::Sync(0.25).hz(120.0) - 0.5).abs() < 1e-6);
        let mut rng = Rng::new(3);
        for w in [LfoWave::Sine, LfoWave::Triangle, LfoWave::Square, LfoWave::Saw, LfoWave::SampleHold] {
            let mut l = Lfo::default();
            l.reset(&mut rng);
            for _ in 0..1000 {
                let v = l.advance(w, 0.003, 16, &mut rng);
                assert!((-1.0..=1.0).contains(&v));
            }
        }
    }
}
