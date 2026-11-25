//! The speed setting and the duration model (04 §8): what TTS-2's
//! synthesiser needs per phone and per pause.
//!
//! # Model
//!
//! Each phoneme has an inherent and a minimum duration at 1× (after the
//! MITalk table of Klatt's 1979 duration rules; the unstressed `AH` and
//! `IH` use the reduced vowels' values). A percentage `P` built from a
//! subset of Klatt's published rules (phrase-final lengthening, non-final
//! and non-word-final shortening, polysyllabic shortening, non-initial
//! consonant shortening, unstressed shortening, the following consonant's
//! effect on a vowel, clusters) gives the 1× duration
//!
//! ```text
//! d(1×) = MIN + (INH − MIN) · P / 100
//! ```
//!
//! Speed `s` (0.25×–10× in 0.25× steps) then acts per class:
//!
//! * **faster (s > 1)**: the compressible part `(INH − MIN) · P / 100`
//!   scales by `1/s`; the minimum scales by `1/s` for vowels and sonorants
//!   but only by `2/(1+s)` for stops, fricatives, affricates and `HH`, so
//!   they keep enough length to be heard; nothing goes below its class
//!   floor ([`floor_at`]: [`floor_ms`] up to 4×, lowered by up to 37 % at
//!   10× so the top speeds still gain).
//! * **slower (s < 1)**: vowels and diphthongs stretch by `1/s`; nasals,
//!   liquids and glides by `1 + (1/s − 1)/2`; obstruents only by
//!   `1 + (1/s − 1)/4` ("vowels and pauses lengthen, not stops").
//! * **pauses**: `base/s` up to 2×, `base · 2/s²` above (pauses compress
//!   faster than speech); sentence ends keep at least 30 ms.
//! * **above 4×**: unstressed full vowels reduce to the schwa ([`reduce`]);
//!   the pitch movements flatten ([`pitch_depth_pct`]).
//!
//! All arithmetic is integer on the speed in quarters, so the tables are
//! the same on every target.

use crate::phoneme::{Class, Ph, Stress};
use crate::prosody::BreakKind;

/// Speaking rate: 0.25× to 10× in 0.25× steps (stored in quarters, 1–40).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Speed(u8);

impl Speed {
    /// 1×.
    pub const NORMAL: Speed = Speed(4);
    /// The slowest, 0.25×.
    pub const MIN: Speed = Speed(1);
    /// The fastest, 10×.
    pub const MAX: Speed = Speed(40);
    /// Number of settings (0.25, 0.5, …, 10: 40 values).
    pub const STEPS: u8 = 40;

    /// From quarters (clamped to 1–40).
    pub const fn from_quarters(q: u8) -> Speed {
        Speed(if q < 1 {
            1
        } else if q > 40 {
            40
        } else {
            q
        })
    }

    /// From a factor, rounded to the nearest quarter and clamped; NaN is 1×.
    pub fn from_f32(x: f32) -> Speed {
        if x.is_nan() {
            return Speed::NORMAL;
        }
        let q = (x * 4.0 + 0.5).clamp(1.0, 40.0);
        Speed(q as u8)
    }

    /// Quarters (1–40).
    pub const fn quarters(self) -> u8 {
        self.0
    }

    /// The factor.
    pub fn as_f32(self) -> f32 {
        f32::from(self.0) / 4.0
    }

    /// The next faster setting (saturating).
    pub const fn faster(self) -> Speed {
        Speed::from_quarters(self.0 + 1)
    }

    /// The next slower setting (saturating).
    pub const fn slower(self) -> Speed {
        Speed::from_quarters(self.0.saturating_sub(1))
    }
}

impl Default for Speed {
    fn default() -> Speed {
        Speed::NORMAL
    }
}

/// Inherent and minimum durations at 1× (ms), by [`Ph`] discriminant.
const TABLE: [(u16, u16); 39] = [
    (240, 100), // AA
    (230, 80),  // AE
    (140, 60),  // AH
    (240, 100), // AO
    (260, 100), // AW
    (250, 150), // AY
    (150, 70),  // EH
    (180, 80),  // ER
    (180, 100), // EY
    (135, 40),  // IH
    (155, 55),  // IY
    (220, 80),  // OW
    (280, 150), // OY
    (160, 60),  // UH
    (210, 70),  // UW
    (85, 60),   // B
    (70, 50),   // CH
    (75, 50),   // D
    (50, 30),   // DH
    (100, 80),  // F
    (80, 60),   // G
    (80, 20),   // HH
    (70, 50),   // JH
    (80, 60),   // K
    (80, 40),   // L
    (70, 60),   // M
    (60, 50),   // N
    (95, 60),   // NG
    (90, 50),   // P
    (80, 30),   // R
    (105, 60),  // S
    (105, 80),  // SH
    (75, 50),   // T
    (90, 60),   // TH
    (60, 40),   // V
    (80, 60),   // W
    (80, 40),   // Y
    (75, 40),   // Z
    (70, 40),   // ZH
];

/// Inherent and minimum duration at 1× (ms). Unstressed `AH` is the schwa
/// (120/60), unstressed `IH` the reduced /ɨ/ (110/40).
pub const fn inherent(ph: Ph, stress: Stress) -> (u16, u16) {
    match (ph, stress) {
        (Ph::AH, Stress::Unstressed) => (120, 60),
        (Ph::IH, Stress::Unstressed) => (110, 40),
        _ => TABLE[ph as usize],
    }
}

/// The shortest a phone of this class may become up to 4× (ms).
pub const fn floor_ms(class: Class) -> u16 {
    match class {
        Class::Stop => 22,
        Class::Affricate => 30,
        Class::Fricative => 25,
        Class::Aspirate => 12,
        Class::Nasal => 18,
        Class::Liquid | Class::Glide => 15,
        Class::Vowel => 15,
        Class::Diphthong => 25,
    }
}

/// Where a phone sits: the inputs of the duration rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// The phone is the first of its word.
    pub word_initial: bool,
    /// In the word's last syllable (from its last vowel on).
    pub word_final_syllable: bool,
    /// In the last syllable before a phrase break.
    pub phrase_final_syllable: bool,
    /// The word has two or more vowels.
    pub polysyllabic: bool,
    /// The next phone in the word, if any.
    pub next: Option<Ph>,
    /// A consonant next to another consonant.
    pub cluster: bool,
}

/// Klatt's percentage `P` for `ph` in `ctx` (100 = inherent).
pub fn percent(ph: Ph, stress: Stress, ctx: &Context) -> u32 {
    let mut p: u32 = 100;
    let apply = |p: &mut u32, pct: u32| *p = *p * pct / 100;
    if ph.is_vowel() {
        if ctx.phrase_final_syllable {
            apply(&mut p, 140);
        } else {
            apply(&mut p, 60);
        }
        if !ctx.word_final_syllable {
            apply(&mut p, 85);
        }
        if ctx.polysyllabic {
            apply(&mut p, 80);
        }
        match stress {
            Stress::Unstressed => apply(&mut p, 50),
            Stress::Secondary => apply(&mut p, 85),
            _ => {}
        }
        // The following consonant (fully in a phrase-final syllable, half
        // elsewhere).
        let effect = match ctx.next.map(Ph::class) {
            Some(Class::Fricative) if ctx.next.is_some_and(Ph::is_voiced) => 160,
            Some(Class::Stop) if ctx.next.is_some_and(Ph::is_voiced) => 120,
            Some(Class::Nasal) => 85,
            Some(Class::Stop | Class::Affricate) => 70,
            _ => 100,
        };
        let effect = if ctx.phrase_final_syllable { effect } else { (100 + effect) / 2 };
        apply(&mut p, effect);
    } else {
        if !ctx.word_initial {
            apply(&mut p, 85);
        }
        if ctx.cluster {
            apply(&mut p, 70);
        }
        if ctx.phrase_final_syllable {
            apply(&mut p, 120);
        }
    }
    p
}

/// The floor at `speed`: [`floor_ms`] up to 4×, then lowered by
/// `40/(q + 24)` (q in quarters; 0.625 at 10×) so that the top speeds still
/// gain on the floors (measured, not gated, above 4×: 04 §8).
pub fn floor_at(class: Class, speed: Speed) -> u32 {
    let f = u32::from(floor_ms(class));
    let q = u32::from(speed.quarters());
    if q <= 16 { f } else { (f * 40).div_ceil(q + 24) }
}

/// The phone's duration in milliseconds at `speed`.
pub fn duration_ms(ph: Ph, stress: Stress, ctx: &Context, speed: Speed) -> u16 {
    let (inh, min) = inherent(ph, stress);
    let min = if ph.is_vowel() && stress == Stress::Unstressed { min / 2 } else { min };
    let p = percent(ph, stress, ctx);
    // Work in hundredths of a millisecond × 4 to keep the quarters exact.
    let var = u32::from(inh.saturating_sub(min)) * p; // ms × 100
    let min = u32::from(min) * 100;
    let q = u32::from(speed.quarters());
    let class = ph.class();
    let obstruent = matches!(class, Class::Stop | Class::Fricative | Class::Affricate | Class::Aspirate);
    let d = if q >= 4 {
        let var = var * 4 / q;
        let min = if obstruent { min * 8 / (4 + q) } else { min * 4 / q };
        var + min
    } else {
        let d1 = var + min;
        // Stretch 1/s = 4/q for vowels, 1 + (1/s − 1)/2 = (q + 4)/(2q) for
        // sonorants, 1 + (1/s − 1)/4 = (3q + 4)/(4q) for obstruents.
        match class {
            Class::Vowel | Class::Diphthong => d1 * 4 / q,
            Class::Nasal | Class::Liquid | Class::Glide => d1 * (q + 4) / (2 * q),
            _ => d1 * (3 * q + 4) / (4 * q),
        }
    };
    let ms = d.div_ceil(100);
    let at1 = (var + min).div_ceil(100);
    let floor = floor_at(class, speed).min(at1);
    ms.max(floor).min(u32::from(u16::MAX)) as u16
}

/// A pause's duration in milliseconds at `speed`.
pub fn pause_ms(kind: BreakKind, speed: Speed) -> u16 {
    let base = u32::from(kind.base_ms());
    let q = u32::from(speed.quarters());
    let d = if q <= 8 { base * 4 / q } else { base * 32 / (q * q) };
    let d = if kind.ends_sentence() { d.max(30.min(base)) } else { d };
    d.min(u32::from(u16::MAX)) as u16
}

/// Vowel reduction above 4×: unstressed full vowels become the schwa
/// (`IY`/`IH` become `IH`, `ER` stays); others are unchanged.
pub fn reduce(ph: Ph, stress: Stress, speed: Speed) -> Ph {
    if speed.quarters() <= 16 || stress != Stress::Unstressed || !ph.is_vowel() {
        return ph;
    }
    match ph {
        Ph::IY | Ph::IH => Ph::IH,
        Ph::ER => Ph::ER,
        _ => Ph::AH,
    }
}

/// How much of the normal pitch movement to keep (percent): 100 up to 4×,
/// then down linearly to 40 at 10× ("above 4× the prosody flattens").
pub fn pitch_depth_pct(speed: Speed) -> u8 {
    let q = u32::from(speed.quarters());
    if q <= 16 { 100 } else { (100 - (q - 16) * 60 / 24) as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds() {
        assert_eq!(Speed::from_f32(1.0), Speed::NORMAL);
        assert_eq!(Speed::from_f32(0.1), Speed::MIN);
        assert_eq!(Speed::from_f32(99.0), Speed::MAX);
        assert_eq!(Speed::from_f32(f32::NAN), Speed::NORMAL);
        assert_eq!(Speed::from_f32(2.6).quarters(), 10);
        assert_eq!(Speed::MAX.faster(), Speed::MAX);
        assert_eq!(Speed::MIN.slower(), Speed::MIN);
        assert_eq!(Speed::from_quarters(0), Speed::MIN);
        assert_eq!((1..=40).map(Speed::from_quarters).count(), usize::from(Speed::STEPS));
        assert_eq!(Speed::from_quarters(10).as_f32(), 2.5);
    }

    #[test]
    fn pitch_depth() {
        assert_eq!(pitch_depth_pct(Speed::NORMAL), 100);
        assert_eq!(pitch_depth_pct(Speed::from_quarters(16)), 100);
        assert_eq!(pitch_depth_pct(Speed::MAX), 40);
    }
}
