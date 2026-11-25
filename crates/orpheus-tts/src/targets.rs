//! Phoneme targets for the formant synthesiser: formant frequencies,
//! bandwidths, source amplitudes and frication spectra for the 39 ARPAbet
//! phonemes, the loci of the consonants' formant transitions, and the
//! timings of stop bursts and aspiration.
//!
//! # Sources
//!
//! The numbers were written for this table from the published phonetics
//! literature (no synthesiser's data files were copied):
//!
//! * **Vowels** F1–F3: Peterson & Barney (1952), "Control methods used in a
//!   study of the vowels", JASA 24(2), Table II, the men's averages.
//!   Unstressed `AH` is the schwa (500/1500/2500, a uniform tube's
//!   resonances for a 17.5 cm tract); unstressed `IH` is a centralised
//!   /ɪ/.
//! * **Diphthongs**: start and end targets after Holbrook & Fairbanks
//!   (1962), "Diphthong formants and their movements", JSHR 5, and the
//!   diphthong descriptions in Klatt (1980).
//! * **Sonorants and the consonants' own targets** (L R W Y, nasal murmurs,
//!   fricatives): after the consonant parameter discussion in Klatt (1980),
//!   "Software for a cascade/parallel formant synthesizer", JASA 67(3),
//!   and Allen, Hunnicutt & Klatt (1987), *From Text to Speech: the MITalk
//!   System*, ch. 9; the nasal pole/zero pair (pole 270 Hz, zero moving
//!   to ≈ 450 Hz in a nasal) is Klatt (1980)'s scheme; murmur formants as
//!   in Fujimura (1962), "Analysis of nasal consonants", JASA 34(12).
//! * **Loci** (formant onsets at a consonant's release, as a function of
//!   the vowel's target): linear "locus equations" after Lindblom (1963)
//!   and Sussman, McCaffrey & Matthews (1991), "An investigation of locus
//!   equations as a source of relational invariance for stop place
//!   categorization", JASA 90(3): labials have steep slopes (onset near
//!   the vowel, slightly lower), alveolars shallow slopes around 1.7–1.8 kHz,
//!   velars split into front (high, "velar pinch" of F2 and F3) and back
//!   contexts.
//! * **Frication spectra**: Hughes & Halle (1956), "Spectral properties of
//!   fricative consonants", JASA 28(2); Jongman, Wayland & Wong (2000),
//!   "Acoustic characteristics of English fricatives", JASA 108(3): /s z/
//!   peak above 4–5 kHz, /ʃ ʒ/ around 2.5–3 kHz with a broad high band,
//!   /f v θ ð/ flat and weak (≈ 20–25 dB below /s/).
//! * **Bursts and voice-onset time**: Lisker & Abramson (1964), "A
//!   cross-language study of voicing in initial stops", Word 20 (English
//!   word-initial VOT ≈ 58 ms /p/, 70 ms /t/, 80 ms /k/; ≈ 1–20 ms for
//!   /b d g/); Stevens & Blumstein (1978), "Invariant cues for place of
//!   articulation in stop consonants", JASA 64(5) (diffuse-falling labial,
//!   diffuse-rising alveolar, compact mid-frequency velar burst spectra).
//!
//! Values were then adjusted by measuring the synthesiser's spectra
//! (`tests/speech.rs`), not by copying another synthesiser's tables.

use crate::phoneme::{Class, Ph, Stress};

/// "Off" for an amplitude in dB.
pub const OFF: i8 = i8::MIN;

/// Place of articulation (selects the loci and the burst spectrum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Vowels, liquids, glides: no locus (targets meet halfway).
    Vocalic,
    /// P B M W-like closure at the lips.
    Labial,
    /// F V.
    Labiodental,
    /// TH DH.
    Dental,
    /// T D N S Z L-like.
    Alveolar,
    /// SH ZH CH JH.
    Postalveolar,
    /// K G NG.
    Velar,
    /// HH: takes the neighbouring vowel's formants.
    Glottal,
}

/// A frication (or burst) spectrum for the parallel branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fric {
    /// No frication.
    None,
    /// /s z/: high (P5, P6).
    S,
    /// /ʃ ʒ tʃ dʒ/: mid (P3, P4) with a broad high band.
    Sh,
    /// /f v/: flat, weak.
    F,
    /// /θ ð/: flat and high, weak.
    Th,
    /// Labial burst: diffuse, falling.
    BurstLabial,
    /// Alveolar burst: diffuse, rising.
    BurstAlveolar,
    /// Velar burst: compact, at the onset's F2/F3.
    BurstVelar,
}

/// The parallel amplitudes A2, A3, A4, A5, A6, AB of a spectrum (dB, `OFF`).
pub const fn spectrum(f: Fric) -> [i8; 6] {
    match f {
        Fric::None => [OFF; 6],
        Fric::S => [OFF, OFF, -24, -9, 0, -26],
        Fric::Sh => [OFF, 0, -6, -14, -16, -30],
        Fric::F => [OFF, OFF, -22, -16, -10, -4],
        Fric::Th => [OFF, OFF, -22, -12, -6, -8],
        Fric::BurstLabial => [-4, -10, -18, -26, OFF, -8],
        Fric::BurstAlveolar => [OFF, -20, -12, -4, 0, -16],
        Fric::BurstVelar => [-2, 0, -10, -22, OFF, -24],
    }
}

/// A phoneme's targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// F1–F3 at the start (Hz, adult male reference).
    pub f: [u16; 3],
    /// F1–F3 at the end (diphthongs; equal to `f` otherwise).
    pub g: [u16; 3],
    /// B1–B3 (Hz).
    pub b: [u16; 3],
    /// Voicing amplitude (dB re a vowel; `OFF`).
    pub av: i8,
    /// Aspiration into the cascade (dB re the aspiration reference; `OFF`).
    pub ah: i8,
    /// Frication into the parallel branch (dB re the frication reference;
    /// `OFF`).
    pub af: i8,
    /// The frication spectrum.
    pub fric: Fric,
    /// Place (loci, bursts).
    pub place: Place,
    /// Nasal: the nasal zero moves up to [`FNZ_NASAL`] (Hz).
    pub nasal: bool,
}

/// The nasal pole (Hz).
pub const FNP: u16 = 270;
/// The nasal zero in a nasal murmur (Hz).
pub const FNZ_NASAL: u16 = 450;

const fn v(f: [u16; 3], b: [u16; 3]) -> Target {
    Target { f, g: f, b, av: 0, ah: OFF, af: OFF, fric: Fric::None, place: Place::Vocalic, nasal: false }
}

const fn d(f: [u16; 3], g: [u16; 3], b: [u16; 3]) -> Target {
    Target { f, g, b, av: 0, ah: OFF, af: OFF, fric: Fric::None, place: Place::Vocalic, nasal: false }
}

const fn son(f: [u16; 3], b: [u16; 3], av: i8) -> Target {
    Target { f, g: f, b, av, ah: OFF, af: OFF, fric: Fric::None, place: Place::Vocalic, nasal: false }
}

const fn nasal(f: [u16; 3], b: [u16; 3], place: Place) -> Target {
    Target { f, g: f, b, av: -7, ah: OFF, af: OFF, fric: Fric::None, place, nasal: true }
}

const fn fric(f: [u16; 3], b: [u16; 3], av: i8, af: i8, fric: Fric, place: Place) -> Target {
    Target { f, g: f, b, av, ah: OFF, af, fric, place, nasal: false }
}

/// A stop: `av` is the voice bar's level; `af` the burst's.
const fn stop(f: [u16; 3], av: i8, af: i8, fric: Fric, place: Place) -> Target {
    Target { f, g: f, b: [90, 110, 150], av, ah: OFF, af, fric, place, nasal: false }
}

/// By [`Ph`] discriminant.
const TABLE: [Target; 39] = [
    v([730, 1090, 2440], [90, 90, 150]),                     // AA  father
    v([660, 1720, 2410], [90, 100, 150]),                    // AE  cat
    v([640, 1190, 2390], [80, 90, 150]),                     // AH  but (stressed)
    v([570, 840, 2410], [80, 80, 150]),                      // AO  caught
    d([700, 1300, 2550], [430, 900, 2350], [80, 90, 150]),   // AW  cow
    d([660, 1200, 2550], [400, 1880, 2500], [80, 90, 150]),  // AY  hide
    v([530, 1840, 2480], [70, 100, 150]),                    // EH  red
    v([490, 1350, 1690], [70, 90, 110]),                     // ER  bird
    d([480, 1720, 2520], [330, 2200, 2600], [60, 100, 150]), // EY  say
    v([390, 1990, 2550], [60, 100, 150]),                    // IH  bit
    v([270, 2290, 3010], [55, 100, 200]),                    // IY  see
    d([540, 1100, 2300], [450, 900, 2300], [70, 80, 150]),   // OW  go
    d([550, 960, 2400], [360, 1820, 2450], [70, 80, 150]),   // OY  toy
    v([440, 1020, 2240], [70, 90, 150]),                     // UH  book
    v([300, 870, 2240], [60, 80, 150]),                      // UW  two
    stop([200, 1100, 2150], -22, -14, Fric::BurstLabial, Place::Labial), // B
    fric([300, 1840, 2750], [200, 110, 150], OFF, 3, Fric::Sh, Place::Postalveolar), // CH
    stop([200, 1600, 2600], -22, -4, Fric::BurstAlveolar, Place::Alveolar), // D
    fric([270, 1290, 2540], [60, 90, 150], -8, -22, Fric::Th, Place::Dental), // DH
    fric([340, 1100, 2080], [200, 120, 150], OFF, -18, Fric::F, Place::Labiodental), // F
    stop([200, 1990, 2850], -22, -4, Fric::BurstVelar, Place::Velar), // G
    Target {
        f: [500, 1500, 2500],
        g: [500, 1500, 2500],
        b: [200, 150, 200],
        av: OFF,
        ah: 0,
        af: OFF,
        fric: Fric::None,
        place: Place::Glottal,
        nasal: false,
    }, // HH
    fric([300, 1840, 2750], [60, 110, 150], -8, -3, Fric::Sh, Place::Postalveolar), // JH
    stop([200, 1990, 2850], OFF, 2, Fric::BurstVelar, Place::Velar), // K
    son([310, 1050, 2880], [60, 110, 280], -6),              // L
    nasal([480, 1270, 2130], [40, 200, 200], Place::Labial), // M
    nasal([480, 1340, 2470], [40, 300, 300], Place::Alveolar), // N
    nasal([480, 2000, 2900], [160, 150, 200], Place::Velar), // NG
    stop([200, 1100, 2150], OFF, -8, Fric::BurstLabial, Place::Labial), // P
    son([310, 1060, 1380], [70, 100, 120], -5),              // R
    fric([320, 1390, 2530], [200, 80, 200], OFF, -3, Fric::S, Place::Alveolar), // S
    fric([300, 1840, 2750], [200, 100, 300], OFF, 5, Fric::Sh, Place::Postalveolar), // SH
    stop([200, 1600, 2600], OFF, 2, Fric::BurstAlveolar, Place::Alveolar), // T
    fric([320, 1290, 2540], [200, 90, 200], OFF, -20, Fric::Th, Place::Dental), // TH
    fric([220, 1100, 2080], [60, 90, 120], -8, -22, Fric::F, Place::Labiodental), // V
    son([290, 610, 2150], [60, 80, 120], -7),                // W
    son([260, 2070, 3020], [50, 200, 400], -5),              // Y
    fric([240, 1520, 2480], [70, 60, 180], -8, -8, Fric::S, Place::Alveolar), // Z
    fric([300, 1840, 2750], [60, 110, 150], -8, -2, Fric::Sh, Place::Postalveolar), // ZH
];

const SCHWA: Target = v([500, 1500, 2500], [80, 100, 150]);
const SCHWA_I: Target = v([400, 1800, 2500], [70, 100, 150]);

/// The targets of `ph` with `stress` (unstressed `AH` is the schwa,
/// unstressed `IH` a centralised /ɪ/).
pub const fn target(ph: Ph, stress: Stress) -> Target {
    match (ph, stress) {
        (Ph::AH, Stress::Unstressed) => SCHWA,
        (Ph::IH, Stress::Unstressed) => SCHWA_I,
        _ => TABLE[ph as usize],
    }
}

/// Whether the phoneme is vocalic for the transition rules (vowels,
/// liquids, glides).
pub const fn vocalic(ph: Ph) -> bool {
    matches!(ph.class(), Class::Vowel | Class::Diphthong | Class::Liquid | Class::Glide)
}

/// The onset of formant `k` (0–2: F1–F3) at the boundary between a
/// consonant at `place` and a vocalic segment whose target there is `v`
/// (the locus equations; see the module documentation).
pub fn onset(place: Place, k: usize, v: f32, v2: f32) -> f32 {
    match (k, place) {
        (_, Place::Vocalic | Place::Glottal) => v,
        // F1 is low at any oral constriction.
        (0, _) => 0.45 * v + 120.0,
        (1, Place::Labial) => 0.80 * v + 120.0,
        (1, Place::Labiodental) => 0.75 * v + 250.0,
        (1, Place::Dental) => 0.55 * v + 750.0,
        (1, Place::Alveolar) => 0.45 * v + 1050.0,
        (1, Place::Postalveolar) => 0.35 * v + 1350.0,
        (1, Place::Velar) => velar_f2(v),
        (_, Place::Labial) => v - 150.0,
        (_, Place::Labiodental) => v - 100.0,
        (_, Place::Dental) => 0.6 * v + 1050.0,
        (_, Place::Alveolar) => 0.4 * v + 1650.0,
        (_, Place::Postalveolar) => 0.4 * v + 1450.0,
        // The velar pinch: F3 comes down towards the F2 onset.
        (_, Place::Velar) => 0.5 * velar_f2(v2) + 0.5 * v + 150.0,
    }
}

/// The velar F2 onset: high before front vowels, near the vowel before back
/// vowels.
fn velar_f2(v: f32) -> f32 {
    let x = if v > 1600.0 { 0.85 * v + 400.0 } else { 0.7 * v + 450.0 };
    if x > 2500.0 { 2500.0 } else { x }
}

/// Voice-onset time at 1× after a released stop (ms): the aspiration before
/// the next vocalic segment voices (Lisker & Abramson 1964).
pub const fn vot_ms(ph: Ph) -> u16 {
    match ph {
        Ph::P => 55,
        Ph::T => 65,
        Ph::K => 75,
        Ph::CH => 40,
        Ph::B => 6,
        Ph::D => 10,
        Ph::G => 18,
        Ph::JH => 8,
        _ => 0,
    }
}

/// A stop's burst duration at 1× (ms; shortened with the segment).
pub const fn burst_ms(ph: Ph) -> u16 {
    match ph {
        Ph::P => 6,
        Ph::B => 4,
        Ph::T | Ph::K => 10,
        Ph::D | Ph::G => 7,
        _ => 0,
    }
}

/// Fixed upper formants (Hz) and bandwidths: F4, F5, F6 of the cascade.
pub const F45: [u16; 3] = [3300, 3750, 4900];
/// B4, B5, B6 (Hz).
pub const B45: [u16; 3] = [250, 300, 1000];
/// Parallel P5 and P6 (Hz) for the high frication of /s z/ and alveolar
/// bursts (above the cascade's F5).
pub const P56: [u16; 2] = [4700, 6000];
/// Their bandwidths (Hz).
pub const PB56: [u16; 2] = [700, 1100];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phoneme::ALL;

    #[test]
    fn every_phoneme_has_targets() {
        for p in ALL {
            let t = target(p, Stress::Primary);
            assert!(t.f[0] >= 200 && t.f[0] < t.f[1] && t.f[1] < t.f[2], "{p:?}");
            assert!(t.g[0] < t.g[1] && t.g[1] < t.g[2], "{p:?}");
            if p.is_vowel() {
                assert_eq!(t.av, 0, "{p:?}");
            }
            if !p.is_voiced() {
                assert!(t.av == OFF, "{p:?} voiceless but voiced");
            }
            assert_eq!(t.nasal, matches!(p, Ph::M | Ph::N | Ph::NG));
            assert_eq!(t.g != t.f, p.class() == Class::Diphthong, "{p:?}");
        }
    }

    #[test]
    fn loci() {
        // Alveolar F2 onsets cluster near 1.5–2 kHz whatever the vowel.
        let a = onset(Place::Alveolar, 1, 870.0, 870.0);
        let b = onset(Place::Alveolar, 1, 2290.0, 2290.0);
        assert!((1400.0..1500.0).contains(&a) && (2000.0..2100.0).contains(&b));
        // Labial onsets are below the vowel's F2.
        assert!(onset(Place::Labial, 1, 1840.0, 1840.0) < 1840.0);
        // Velar pinch: F2 and F3 onsets close before a front vowel.
        let f2 = onset(Place::Velar, 1, 2290.0, 2290.0);
        let f3 = onset(Place::Velar, 2, 3010.0, 2290.0);
        assert!(f3 - f2 < 500.0, "{f2} {f3}");
    }
}
