//! Patches: what a part (a MIDI channel, a sequencer track) plays.
//!
//! A [`Patch`] is plain data (`Copy`, no pointers) so that presets and the
//! Sequencer's sounds can be stored as constants or in files. Every value is
//! clamped when the patch is installed ([`Patch::sanitized`]), so an
//! arbitrary patch (a corrupt file, the fuzzer) cannot make the engine
//! misbehave.
//!
//! Signal flow of a subtractive voice:
//!
//! ```text
//! osc 1 ─┐
//! osc 2 ─┼─► SVF (cutoff ← filter env, LFO, key, velocity) ─► × amp env × velocity ─► pan ─► mix + sends
//! noise ─┘     pitch ← note, glide, bend, pitch env, LFO
//! ```
//!
//! An FM voice replaces the oscillators by the operators (each with its own
//! envelope); the filter and amplitude envelope still apply. A drum voice
//! ([`crate::drum`]) is self-contained.

use crate::drum::DrumParams;
use crate::env::{Adsr, EnvCoef};
use crate::filter::FilterMode;
use crate::fm::{FmParams, OPS};
use crate::lfo::LfoParams;
use crate::math::clampf;
use crate::osc::Waveform;

/// One oscillator of a subtractive patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OscParams {
    /// Waveform.
    pub wave: Waveform,
    /// Transposition, semitones (−48 to 48).
    pub semitones: f32,
    /// Fine tuning, cents (−100 to 100).
    pub cents: f32,
    /// Level, 0–1.
    pub level: f32,
    /// Pulse width, 0.01–0.99.
    pub width: f32,
    /// Phase at note-on, turns.
    pub phase: f32,
}

impl OscParams {
    /// An oscillator of `wave` at full level.
    #[must_use]
    pub const fn new(wave: Waveform) -> Self {
        Self { wave, semitones: 0.0, cents: 0.0, level: 1.0, width: 0.5, phase: 0.0 }
    }

    /// A silent oscillator.
    pub const OFF: Self = Self { level: 0.0, ..Self::new(Waveform::Sine) };

    fn sanitized(&self) -> Self {
        Self {
            wave: self.wave,
            semitones: clampf(self.semitones, -48.0, 48.0),
            cents: clampf(self.cents, -100.0, 100.0),
            level: clampf(self.level, 0.0, 1.0),
            width: clampf(self.width, 0.01, 0.99),
            phase: clampf(self.phase, 0.0, 1.0),
        }
    }
}

/// Oscillators of a subtractive patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubParams {
    /// The two oscillators.
    pub osc: [OscParams; 2],
    /// White-noise level, 0–1.
    pub noise: f32,
}

/// The voice type of a patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Two oscillators and noise.
    Subtractive(SubParams),
    /// Two to four FM operators.
    Fm(FmParams),
    /// One drum voice on every note.
    Drum(DrumParams),
    /// A drum kit: General MIDI percussion notes ([`DrumParams::from_gm_note`]).
    Kit,
}

/// Filter settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterParams {
    /// Response.
    pub mode: FilterMode,
    /// Cutoff, Hz.
    pub cutoff: f32,
    /// Resonance, 0–1.
    pub resonance: f32,
    /// Filter envelope depth, octaves (−8 to 8).
    pub env_amount: f32,
    /// Key tracking, 0–1 (1: the cutoff follows the note one to one, around C4).
    pub key_track: f32,
    /// Velocity to cutoff: octaves below `cutoff` at velocity 0 (0–4; full
    /// velocity plays at `cutoff`).
    pub velocity: f32,
}

impl FilterParams {
    /// No filter.
    pub const OFF: Self = Self {
        mode: FilterMode::Off,
        cutoff: 20_000.0,
        resonance: 0.0,
        env_amount: 0.0,
        key_track: 0.0,
        velocity: 0.0,
    };

    /// A low pass at `cutoff` with `resonance`.
    #[must_use]
    pub const fn low_pass(cutoff: f32, resonance: f32) -> Self {
        Self { mode: FilterMode::LowPass, cutoff, resonance, ..Self::OFF }
    }

    fn sanitized(&self) -> Self {
        Self {
            mode: self.mode,
            cutoff: clampf(self.cutoff, crate::filter::MIN_CUTOFF, 24_000.0),
            resonance: clampf(self.resonance, 0.0, 1.0),
            env_amount: clampf(self.env_amount, -8.0, 8.0),
            key_track: clampf(self.key_track, 0.0, 1.0),
            velocity: clampf(self.velocity, 0.0, 4.0),
        }
    }
}

/// Everything a part plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    /// Voice type and its sources.
    pub kind: Kind,
    /// Filter.
    pub filter: FilterParams,
    /// Amplitude envelope.
    pub amp_env: Adsr,
    /// Filter envelope.
    pub filter_env: Adsr,
    /// Pitch envelope.
    pub pitch_env: Adsr,
    /// Pitch envelope depth, semitones (−48 to 48).
    pub pitch_env_amount: f32,
    /// LFO.
    pub lfo: LfoParams,
    /// Glide (portamento) time, seconds (0–5). Constant time per glide.
    pub glide: f32,
    /// One voice for the part: last-note priority, held notes remembered.
    pub mono: bool,
    /// Mono only: a note played while another is held changes the pitch
    /// (with glide) without restarting the envelopes; glide then applies
    /// only to such legato notes.
    pub legato: bool,
    /// Patch level, 0–1.
    pub level: f32,
    /// Velocity sensitivity, 0–1 (0: every note at full level).
    pub velocity: f32,
    /// Pitch-bend range, semitones (0–24).
    pub bend_range: f32,
}

impl Patch {
    /// A plain patch: one saw, no filter, organ envelope.
    pub const INIT: Self = Self {
        kind: Kind::Subtractive(SubParams {
            osc: [OscParams::new(Waveform::Saw), OscParams::OFF],
            noise: 0.0,
        }),
        filter: FilterParams::OFF,
        amp_env: Adsr::new(0.005, 0.1, 0.8, 0.2),
        filter_env: Adsr::new(0.0, 0.3, 0.0, 0.3),
        pitch_env: Adsr::new(0.0, 0.1, 0.0, 0.1),
        pitch_env_amount: 0.0,
        lfo: LfoParams {
            wave: crate::lfo::LfoWave::Sine,
            rate: crate::lfo::LfoRate::Hz(5.0),
            pitch: 0.0,
            cutoff: 0.0,
            amp: 0.0,
            width: 0.0,
            key_sync: false,
        },
        glide: 0.0,
        mono: false,
        legato: false,
        level: 0.7,
        velocity: 0.7,
        bend_range: 2.0,
    };

    /// A drum patch for `p`.
    #[must_use]
    pub const fn drum(p: DrumParams) -> Self {
        Self { kind: Kind::Drum(p), ..Self::INIT }
    }

    /// The General MIDI style kit.
    pub const KIT: Self = Self { kind: Kind::Kit, ..Self::INIT };

    /// The patch with every value in range.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let kind = match self.kind {
            Kind::Subtractive(s) => Kind::Subtractive(SubParams {
                osc: [s.osc[0].sanitized(), s.osc[1].sanitized()],
                noise: clampf(s.noise, 0.0, 1.0),
            }),
            Kind::Fm(f) => Kind::Fm(f.sanitized()),
            Kind::Drum(d) => Kind::Drum(d.sanitized()),
            Kind::Kit => Kind::Kit,
        };
        Self {
            kind,
            filter: self.filter.sanitized(),
            amp_env: self.amp_env.sanitized(),
            filter_env: self.filter_env.sanitized(),
            pitch_env: self.pitch_env.sanitized(),
            pitch_env_amount: clampf(self.pitch_env_amount, -48.0, 48.0),
            lfo: self.lfo.sanitized(),
            glide: clampf(self.glide, 0.0, 5.0),
            mono: self.mono,
            legato: self.legato,
            level: clampf(self.level, 0.0, 1.0),
            velocity: clampf(self.velocity, 0.0, 1.0),
            bend_range: clampf(self.bend_range, 0.0, 24.0),
        }
    }

    /// Whether the patch plays drum voices.
    #[must_use]
    pub fn is_drum(&self) -> bool {
        matches!(self.kind, Kind::Drum(_) | Kind::Kit)
    }
}

/// Envelope coefficients of a patch at one rate (shared by its voices).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PatchCoef {
    /// Amplitude envelope.
    pub amp: EnvCoef,
    /// Filter envelope.
    pub filter: EnvCoef,
    /// Pitch envelope.
    pub pitch: EnvCoef,
    /// FM operator envelopes.
    pub op: [EnvCoef; OPS],
    /// FM: the operator graph.
    pub fm: crate::fm::FmCoef,
    /// Subtractive: each oscillator's frequency ratio to the note
    /// (2^((semitones + cents/100)/12)).
    pub osc_ratio: [f32; 2],
}

impl PatchCoef {
    /// Coefficients of `p` at `rate` Hz.
    #[must_use]
    pub fn new(p: &Patch, rate: f32) -> Self {
        let mut c = Self {
            amp: EnvCoef::new(&p.amp_env, rate),
            filter: EnvCoef::new(&p.filter_env, rate),
            pitch: EnvCoef::new(&p.pitch_env, rate),
            op: [EnvCoef::default(); OPS],
            fm: crate::fm::FmCoef::default(),
            osc_ratio: [1.0; 2],
        };
        match &p.kind {
            Kind::Fm(f) => {
                c.fm = crate::fm::FmCoef::new(f);
                for (c, o) in c.op.iter_mut().zip(&f.op) {
                    *c = EnvCoef::new(&o.env, rate);
                }
            }
            Kind::Subtractive(s) => {
                for (r, o) in c.osc_ratio.iter_mut().zip(&s.osc) {
                    *r = crate::math::exp2((o.semitones + o.cents * 0.01) * (1.0 / 12.0));
                }
            }
            _ => {}
        }
        c
    }
}
