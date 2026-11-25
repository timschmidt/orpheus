//! Voices: parameter sets of the formant synthesiser.
//!
//! A voice is plain data: the speaker's mean pitch and pitch range, a
//! scale on all formant frequencies (vocal-tract length: ≈ 1.15–1.2 for an
//! adult female tract relative to the male reference of the target table,
//! Peterson & Barney 1952), the glottal open quotient, the spectral tilt
//! and the breathiness (aspiration mixed into the voicing). Settings
//! (A11Y-1) may shift the pitch with [`Voice::with_pitch`].

use orpheus_synth::math::{clampf, exp2};

/// A voice (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    /// Name for menus.
    pub name: &'static str,
    /// Mean F0 at the top of a sentence (Hz).
    pub f0: f32,
    /// Pitch range: scales every accent and contour (1 = reference).
    pub range: f32,
    /// Formant frequency scale.
    pub formants: f32,
    /// Glottal open quotient (0.4 pressed … 0.7 breathy).
    pub oq: f32,
    /// Spectral tilt: attenuation of the source at 3 kHz (dB).
    pub tilt_db: f32,
    /// Aspiration noise mixed into voicing, relative to the voicing (dB).
    pub breath_db: f32,
    /// Output gain (linear).
    pub gain: f32,
}

impl Voice {
    /// A mid-pitched male-reference voice, plain.
    pub const NEUTRAL: Voice = Voice {
        name: "Neutral",
        f0: 118.0,
        range: 1.0,
        formants: 1.0,
        oq: 0.55,
        tilt_db: 3.0,
        breath_db: -34.0,
        gain: 1.0,
    };
    /// Lower and darker: lower pitch, a longer tract, more tilt.
    pub const LOW: Voice = Voice {
        name: "Low",
        f0: 92.0,
        range: 0.85,
        formants: 0.93,
        oq: 0.5,
        tilt_db: 6.0,
        breath_db: -36.0,
        gain: 1.1,
    };
    /// Higher and brighter: a female-range pitch, a shorter tract, a
    /// breathier, less tilted source.
    pub const BRIGHT: Voice = Voice {
        name: "Bright",
        f0: 195.0,
        range: 1.2,
        formants: 1.16,
        oq: 0.62,
        tilt_db: 2.0,
        breath_db: -26.0,
        gain: 0.9,
    };

    /// The built-in voices.
    pub const ALL: [Voice; 3] = [Voice::NEUTRAL, Voice::LOW, Voice::BRIGHT];

    /// The voice with its pitch moved by `semitones` (−12 … +12).
    #[must_use]
    pub fn with_pitch(self, semitones: f32) -> Voice {
        let s = clampf(semitones, -12.0, 12.0);
        Voice { f0: self.f0 * exp2(s * (1.0 / 12.0)), ..self }
    }

    /// The voice with every field clamped to its sane range (NaN included).
    #[must_use]
    pub fn sanitised(self) -> Voice {
        Voice {
            name: self.name,
            f0: clampf(self.f0, 50.0, 400.0),
            range: clampf(self.range, 0.0, 2.0),
            formants: clampf(self.formants, 0.8, 1.3),
            oq: clampf(self.oq, 0.35, 0.75),
            tilt_db: clampf(self.tilt_db, 0.0, 24.0),
            breath_db: clampf(self.breath_db, -60.0, -6.0),
            gain: clampf(self.gain, 0.0, 2.0),
        }
    }
}

impl Default for Voice {
    fn default() -> Voice {
        Voice::NEUTRAL
    }
}

/// The one-pole low-pass coefficient `a` (y = (1−a)x + a·y₁) whose
/// attenuation at 3 kHz and 16 kHz is `db` (solved by bisection; runs when
/// a voice is set, not per sample).
pub fn tilt_coef(db: f32) -> f32 {
    if db <= 0.0 {
        return 0.0;
    }
    // |H(w)|² = (1−a)² / (1 − 2a·cos w + a²); target 10^(−db/10).
    let cw = orpheus_synth::math::sin_turns(3000.0 / 16000.0 + 0.25);
    let want = exp2(-db * (core::f32::consts::LOG2_10 / 10.0));
    let (mut lo, mut hi) = (0.0f32, 0.99f32);
    for _ in 0..30 {
        let a = 0.5 * (lo + hi);
        let g = (1.0 - a) * (1.0 - a) / (1.0 - 2.0 * a * cw + a * a);
        if g > want {
            lo = a;
        } else {
            hi = a;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn tilt_solves() {
        assert_eq!(tilt_coef(0.0), 0.0);
        let a = tilt_coef(6.0);
        let cw = (core::f32::consts::TAU * 3000.0 / 16000.0).cos();
        let g = (1.0 - a) * (1.0 - a) / (1.0 - 2.0 * a * cw + a * a);
        assert!((10.0 * g.log10() + 6.0).abs() < 0.05, "{g}");
        assert!(tilt_coef(12.0) > a);
    }

    #[test]
    fn pitch_shift_and_clamps() {
        let v = Voice::NEUTRAL.with_pitch(12.0);
        assert!((v.f0 - 236.0).abs() < 0.1);
        let w = Voice { f0: f32::NAN, formants: 9.0, ..Voice::NEUTRAL }.sanitised();
        assert_eq!(w.f0, 50.0);
        assert_eq!(w.formants, 1.3);
    }
}
