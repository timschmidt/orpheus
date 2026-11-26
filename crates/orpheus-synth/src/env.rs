//! ADSR envelopes with linear or exponential segments.
//!
//! [`Adsr`] holds the times (seconds) and the sustain level; [`EnvCoef`] the
//! per-sample coefficients for one sample rate (computed once per patch,
//! shared by all voices of a part); [`Env`] is the per-voice state (stage and
//! level: 12 bytes).
//!
//! Exponential segments are one-pole approaches to a target placed beyond
//! the segment's end, so a segment ends after exactly its time (the analogue
//! envelope technique): attack aims at 1.3 (a slightly convex rise), decay at
//! `sustain − 10⁻⁴`, release at `−10⁻⁴` (so the release time is the time
//! from full level; from a lower sustain it is shorter). Linear release keeps
//! its time from whatever level it starts at. A retrigger starts the attack from the
//! current level, so stolen and legato voices do not click.
//!
//! A segment time of 0 takes one sample: an attack of 0 reaches full level on
//! the note-on sample itself and a release of 0 is silent on the note-off
//! sample (the scheduling test relies on it). Times are clamped to 20 s
//! (beyond, the exponential coefficient would round to 1).

use crate::math::{clampf, exp2, log2};

/// Segment shape.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Curve {
    /// Straight lines.
    Linear,
    /// One-pole curves (fast start, slow end), as analogue envelopes.
    Exponential,
}

/// Envelope settings.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct Adsr {
    /// Attack time, seconds.
    pub attack: f32,
    /// Decay time to the sustain level, seconds.
    pub decay: f32,
    /// Sustain level, 0–1. With 0 the envelope ends after the decay (a
    /// one-shot: percussive sounds).
    pub sustain: f32,
    /// Release time, seconds.
    pub release: f32,
    /// Segment shape.
    pub curve: Curve,
}

impl Adsr {
    /// An envelope with exponential segments.
    #[must_use]
    pub const fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Self { attack, decay, sustain, release, curve: Curve::Exponential }
    }

    /// The same with linear segments.
    #[must_use]
    pub const fn linear(self) -> Self {
        Self { curve: Curve::Linear, ..self }
    }

    /// Full level while held, no attack or release.
    pub const GATE: Self = Self { attack: 0.0, decay: 0.0, sustain: 1.0, release: 0.0, curve: Curve::Linear };

    /// The settings with every field in range (NaN replaced).
    #[must_use]
    pub fn sanitized(&self) -> Self {
        Self {
            attack: clampf(self.attack, 0.0, MAX_TIME),
            decay: clampf(self.decay, 0.0, MAX_TIME),
            sustain: clampf(self.sustain, 0.0, 1.0),
            release: clampf(self.release, 0.0, MAX_TIME),
            curve: self.curve,
        }
    }
}

/// Longest segment time, seconds.
pub const MAX_TIME: f32 = 20.0;

const RA: f32 = 0.3;
const RD: f32 = 1.0e-4;

/// The coefficients of one step (one sample, or one control period).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq)]
struct Step {
    a_mul: f32,
    a_add: f32,
    d_mul: f32,
    d_add: f32,
    r_mul: f32,
    r_add: f32,
    /// Linear: attack and decay steps; the release step is per voice.
    a_step: f32,
    d_step: f32,
    /// Samples per step (1 or [`crate::CONTROL_PERIOD`]).
    k: f32,
}

impl Step {
    /// The step of `k` samples made of `k` one-sample steps (the same
    /// recurrence iterated, so the result is what `k` single steps give
    /// within a segment).
    fn times(&self, k: u32) -> Self {
        let it = |m: f32, a: f32| {
            let (mut mm, mut aa) = (1.0f32, 0.0f32);
            for _ in 0..k {
                mm *= m;
                aa = aa * m + a;
            }
            (mm, aa)
        };
        let (a_mul, a_add) = it(self.a_mul, self.a_add);
        let (d_mul, d_add) = it(self.d_mul, self.d_add);
        let (r_mul, r_add) = it(self.r_mul, self.r_add);
        let kf = k as f32;
        Self {
            a_mul,
            a_add,
            d_mul,
            d_add,
            r_mul,
            r_add,
            a_step: self.a_step * kf,
            d_step: self.d_step * kf,
            k: kf,
        }
    }
}

/// Coefficients of an [`Adsr`] at one sample rate, per sample and per
/// control period.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq)]
pub struct EnvCoef {
    linear: bool,
    sustain: f32,
    r_samples: f32,
    one: Step,
    period: Step,
}

impl EnvCoef {
    /// Coefficients of `e` at `rate` Hz.
    #[must_use]
    pub fn new(e: &Adsr, rate: f32) -> Self {
        let e = e.sanitized();
        let n = |t: f32| clampf(t * rate, 1.0, MAX_TIME * 200_000.0);
        let (na, nd, nr) = (n(e.attack), n(e.decay), n(e.release));
        let s = e.sustain;
        let a_mul = exp2(log2(RA / (1.0 + RA)) / na);
        let d_mul = exp2(log2(RD / (1.0 - s + RD)) / nd);
        let r_mul = exp2(log2(RD / (1.0 + RD)) / nr);
        let one = Step {
            a_mul,
            a_add: (1.0 + RA) * (1.0 - a_mul),
            d_mul,
            d_add: (s - RD) * (1.0 - d_mul),
            r_mul,
            r_add: -RD * (1.0 - r_mul),
            a_step: 1.0 / na,
            d_step: (1.0 - s) / nd,
            k: 1.0,
        };
        Self {
            linear: e.curve == Curve::Linear,
            sustain: s,
            r_samples: nr,
            one,
            period: one.times(crate::CONTROL_PERIOD as u32),
        }
    }
}

/// Envelope stage.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// Finished (or never started): level 0.
    #[default]
    Idle,
    /// Rising to 1.
    Attack,
    /// Falling to the sustain level.
    Decay,
    /// Holding the sustain level.
    Sustain,
    /// Falling to 0 after the gate closed.
    Release,
}

/// Per-voice envelope state.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq)]
pub struct Env {
    stage: Stage,
    level: f32,
    r_step: f32,
}

impl Env {
    /// Finished, level 0.
    pub const IDLE: Self = Self { stage: Stage::Idle, level: 0.0, r_step: 0.0 };

    /// Current level, 0–1.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Current stage.
    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Whether the envelope has finished.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Open the gate: attack from the current level.
    pub fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    /// Start at level 0 with the gate open.
    pub fn trigger(&mut self) {
        self.level = 0.0;
        self.stage = Stage::Attack;
    }

    /// Close the gate: release from the current level.
    pub fn gate_off(&mut self, c: &EnvCoef) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
            self.r_step = self.level / c.r_samples;
        }
    }

    /// Stop at once.
    pub fn kill(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    /// Advance one sample; returns the new level.
    #[inline]
    pub fn step(&mut self, c: &EnvCoef) -> f32 {
        self.step_by(c, &c.one)
    }

    #[inline]
    fn step_by(&mut self, c: &EnvCoef, s: &Step) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level = if c.linear { self.level + s.a_step } else { self.level * s.a_mul + s.a_add };
                if self.level >= 1.0 - 1.0e-6 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                    if c.sustain >= 1.0 {
                        self.stage = Stage::Sustain;
                    }
                }
            }
            Stage::Decay => {
                self.level = if c.linear { self.level - s.d_step } else { self.level * s.d_mul + s.d_add };
                if self.level <= c.sustain {
                    self.level = c.sustain;
                    self.stage = if c.sustain <= 0.0 { Stage::Idle } else { Stage::Sustain };
                }
            }
            Stage::Sustain => self.level = c.sustain,
            Stage::Release => {
                self.level =
                    if c.linear { self.level - self.r_step * s.k } else { self.level * s.r_mul + s.r_add };
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }

    /// Advance `n` samples (the control-rate envelopes); returns the level.
    /// A whole control period is one step with the period's coefficients
    /// (a segment that ends inside it ends at the period's end); other
    /// lengths take `n` single steps.
    pub fn advance(&mut self, c: &EnvCoef, n: u32) -> f32 {
        if matches!(self.stage, Stage::Idle | Stage::Sustain) {
            return self.level;
        }
        if n == crate::CONTROL_PERIOD as u32 {
            return self.step_by(c, &c.period);
        }
        for _ in 0..n {
            if matches!(self.stage, Stage::Idle | Stage::Sustain) {
                break;
            }
            self.step(c);
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn run(e: Adsr, hold: usize) -> std::vec::Vec<f32> {
        let c = EnvCoef::new(&e, 1000.0);
        let mut s = Env::default();
        s.trigger();
        let mut v = std::vec::Vec::new();
        for i in 0..5000 {
            if i == hold {
                s.gate_off(&c);
            }
            v.push(s.step(&c));
            if s.is_idle() {
                break;
            }
        }
        v
    }

    #[test]
    fn segment_times() {
        for curve in [Curve::Linear, Curve::Exponential] {
            let e = Adsr { attack: 0.1, decay: 0.2, sustain: 0.5, release: 0.3, curve };
            let v = run(e, 1000);
            // Attack reaches 1 at sample 100 (index 99), decay reaches the sustain level
            // 200 samples later, release ends 300 samples after the gate closes.
            let top = v.iter().position(|&x| x >= 1.0).unwrap();
            assert!((98..=101).contains(&top), "{curve:?} attack {top}");
            let sus = v.iter().position(|&x| x == 0.5).unwrap();
            assert!((top + 198..=top + 202).contains(&sus), "{curve:?} decay {sus}");
            // Linear release keeps its time from any level; the exponential one is
            // timed from full level, so from 0.5 it takes ln(5001)/ln(10001) of it.
            let rel = if curve == Curve::Linear { 300 } else { 277 };
            assert!((1000 + rel - 2..=1000 + rel + 2).contains(&v.len()), "{curve:?} release {}", v.len());
        }
    }

    #[test]
    fn zero_times_take_one_sample() {
        let v = run(Adsr::GATE, 3);
        assert_eq!(v, [1.0, 1.0, 1.0, 0.0]);
        let v = run(Adsr::new(0.0, 0.0, 1.0, 0.0), 2);
        assert_eq!(v, [1.0, 1.0, 0.0]);
    }

    #[test]
    fn one_shot_ends_after_decay() {
        let v = run(Adsr::new(0.0, 0.05, 0.0, 1.0), 4000);
        assert!(v.len() <= 52, "{}", v.len());
    }
}
