//! The General MIDI sound set: our own 32 synthesised sounds for the 128
//! GM programs, and the drum kits for channel 10.
//!
//! Every sound is built from `orpheus-synth`'s building blocks (subtractive
//! oscillators and filter, 2–4 operator FM, envelopes, LFO); nothing is
//! sampled or copied from another sound set. Several programs of a family
//! share a sound ([`SOUND_OF`]); the 32 sounds are the families' typical
//! timbres, chosen so that a General MIDI file keeps its arrangement
//! (bass stays bass, pads stay pads, plucked stays plucked) at a CPU cost
//! the player can carry (mostly one or two oscillators through the filter,
//! 2–3 operator FM; 4 operators only for the electric piano).
//!
//! | # | Sound | Built from | GM programs (0-based) |
//! |---|---|---|---|
//! | 0 | piano | FM, 3 operators: a decaying 1:1 pair + an octave partial | 0–3 |
//! | 1 | e-piano | FM, 4 operators (the `fm-epiano` preset, louder) | 4, 5 |
//! | 2 | harpsichord | narrow pulse + saw an octave up, fast filter decay | 6, 7 |
//! | 3 | bell | FM 1:3.5, long decay | 8–10, 14, 31, 98, 112 |
//! | 4 | mallet | FM 1:4, short index decay | 11–13, 108, 113–115 |
//! | 5 | pluck | saw + noise burst into a closing low pass (the `pluck` preset, louder) | 15, 104–107 |
//! | 6 | organ | drawbar wavetable, 8' + 4' (the `organ` preset, softer) | 16–18 |
//! | 7 | church organ | drawbar 8' + hollow 16', slow speech | 19 |
//! | 8 | reed organ | two detuned pulses, tremolo | 20–23, 109 |
//! | 9 | nylon guitar | triangle + quiet saw, plucked filter | 24 |
//! | 10 | steel guitar | saw + pulse, brighter pluck | 25–28 |
//! | 11 | drive guitar | detuned saws + sub square, resonant | 29, 30, 84 |
//! | 12 | bass | sine + saw, plucked filter, mono | 32–35 |
//! | 13 | slap bass | FM 1:1 with feedback, fast index decay, mono | 36, 37 |
//! | 14 | synth bass | resonant saw (the `acid-bass` preset) | 38 |
//! | 15 | synth bass 2 | FM with feedback (the `fm-bass` preset, louder) | 39 |
//! | 16 | solo strings | two saws, vibrato, bowed attack | 40–43, 110 |
//! | 17 | strings | ensemble (the `strings` preset) | 44, 48–51 |
//! | 18 | harp | triangle + octave sine, plucked | 45, 46 |
//! | 19 | timpani | sine with a pitch drop and a noise thud | 47, 116–118 |
//! | 20 | choir | vocal wavetables (the `vocal` preset) | 52–54, 85, 91, 101 |
//! | 21 | brass | two saws, filter swell (the `brass` preset) | 55–59, 61 |
//! | 22 | horn | rounder brass, slower | 60, 62, 63 |
//! | 23 | sax | pulse + saw, breath, vibrato | 64–67 |
//! | 24 | reed | square + hollow wavetable (clarinet, oboe) | 68–71, 111 |
//! | 25 | flute | triangle and breath (the `tri-flute` preset, softer) | 72–79, 82, 83 |
//! | 26 | square lead | two pulses | 80 |
//! | 27 | saw lead | the `saw-lead` preset | 81, 86, 87 |
//! | 28 | pad | PWM pad (the `pwm-pad` preset) | 88, 90, 93, 95, 99, 100, 103 |
//! | 29 | warm pad | slow detuned saws, low filter | 89, 92, 94, 96, 97, 102 |
//! | 30 | noise fx | filtered noise swell | 119–122, 125–127 |
//! | 31 | chirp | sine trill | 123, 124 |
//!
//! Channel 10 plays the kit in the engine's kit slot 0; its program picks
//! the factory kit ([`kit_for`]): 16–23 `punch` (power), 24–31 `analog`
//! (electronic, 808), 32–47 `tight` (jazz, brush), otherwise `standard`
//! (32 GM percussion notes).
//!
//! The set is also a sound bank ([`write_bank`], `data/gm-sounds.osb`):
//! the 32 sounds as `OSB1` patch records in this order, then the factory
//! kits, so the Sequencer and user banks can start from them.

use orpheus_synth::bank::{BankError, Writer};
use orpheus_synth::filter::FilterMode;
use orpheus_synth::kit::FACTORY;
use orpheus_synth::lfo::{LfoParams, LfoRate, LfoWave};
use orpheus_synth::presets;
use orpheus_synth::{
    Adsr, FilterParams, FmParams, Kind, Kit, OpParams, OscParams, Patch, SubParams, Waveform,
};

/// Sounds in the set.
pub const SOUNDS: usize = 32;

const fn sub(a: OscParams, b: OscParams, noise: f32) -> Kind {
    Kind::Subtractive(SubParams { osc: [a, b], noise })
}

const fn osc(wave: Waveform, semitones: f32, cents: f32, level: f32) -> OscParams {
    OscParams { wave, semitones, cents, level, width: 0.5, phase: 0.0 }
}

const fn pulse(width: f32, semitones: f32, cents: f32, level: f32) -> OscParams {
    OscParams { wave: Waveform::Pulse, semitones, cents, level, width, phase: 0.0 }
}

const fn lp(cutoff: f32, resonance: f32, env_amount: f32, key_track: f32, velocity: f32) -> FilterParams {
    FilterParams { mode: FilterMode::LowPass, cutoff, resonance, env_amount, key_track, velocity }
}

const LFO_OFF: LfoParams = Patch::INIT.lfo;

const fn vibrato(rate: f32, depth: f32) -> LfoParams {
    LfoParams { wave: LfoWave::Sine, rate: LfoRate::Hz(rate), pitch: depth, ..LFO_OFF }
}

const fn op(ratio: f32, level: f32, env: Adsr) -> OpParams {
    OpParams::new(ratio, level, env)
}

/// 0 — piano: a 1:1 FM pair whose index falls quickly (the hammer's
/// brightness), plus a quiet octave partial; long free decay.
pub const PIANO: Patch = Patch {
    kind: Kind::Fm(FmParams {
        algorithm: 5,
        ops: 3,
        feedback: 0.0,
        key_scale: -0.35,
        op: [
            op(1.0, 1.0, Adsr::new(0.0, 3.2, 0.0, 0.35)),
            OpParams { velocity: 0.8, ..op(1.0, 1.7, Adsr::new(0.0, 0.9, 0.12, 0.3)) },
            op(2.0, 0.18, Adsr::new(0.0, 1.4, 0.0, 0.3)),
            OpParams::OFF,
        ],
    }),
    amp_env: Adsr::new(0.0, 0.0, 1.0, 0.35),
    velocity: 0.8,
    level: 1.0,
    ..Patch::INIT
};

/// 1 — e-piano: the `fm-epiano` preset, louder (it sits with the piano).
pub const E_PIANO: Patch = Patch { level: 1.0, ..presets::FM_EPIANO };

/// 5 — pluck: the `pluck` preset, louder.
pub const PLUCK: Patch = Patch { level: 1.0, ..presets::PLUCK };

/// 6 — organ: the `organ` preset, softer (it never decays).
pub const ORGAN: Patch = Patch { level: 0.5, ..presets::ORGAN };

/// 15 — synth bass 2: the `fm-bass` preset, louder.
pub const FM_BASS: Patch = Patch { level: 1.0, ..presets::FM_BASS };

/// 25 — flute: the `tri-flute` preset, softer.
pub const FLUTE: Patch = Patch { level: 0.55, ..presets::TRI_FLUTE };

/// 2 — harpsichord (and clavinet): narrow pulse and a saw an octave up into
/// a low pass that snaps shut.
pub const HARPSICHORD: Patch = Patch {
    kind: sub(pulse(0.18, 0.0, 0.0, 0.55), osc(Waveform::Saw, 12.0, 3.0, 0.3), 0.0),
    filter: lp(900.0, 0.15, 3.0, 0.7, 0.5),
    amp_env: Adsr::new(0.0, 1.4, 0.0, 0.18),
    filter_env: Adsr::new(0.0, 0.25, 0.25, 0.15),
    velocity: 0.4,
    ..Patch::INIT
};

/// 3 — bell: inharmonic 1:3.5 FM, long decay (glockenspiel, celesta,
/// music box, tubular bells, crystal).
pub const BELL: Patch = Patch {
    kind: Kind::Fm(FmParams {
        algorithm: 0,
        ops: 2,
        feedback: 0.0,
        key_scale: -0.3,
        op: [
            op(1.0, 0.8, Adsr::new(0.0, 2.6, 0.0, 1.2)),
            op(3.5, 2.4, Adsr::new(0.0, 1.6, 0.0, 0.8)),
            OpParams::OFF,
            OpParams::OFF,
        ],
    }),
    amp_env: Adsr::new(0.0, 0.0, 1.0, 1.2),
    velocity: 0.6,
    ..Patch::INIT
};

/// 4 — mallet: 1:4 FM with a quick index decay (marimba, xylophone,
/// vibraphone, kalimba, agogo, steel drum, wood block).
pub const MALLET: Patch = Patch {
    kind: Kind::Fm(FmParams {
        algorithm: 0,
        ops: 2,
        feedback: 0.0,
        key_scale: -0.4,
        op: [
            op(1.0, 0.85, Adsr::new(0.0, 0.9, 0.0, 0.25)),
            op(4.0, 1.6, Adsr::new(0.0, 0.09, 0.0, 0.05)),
            OpParams::OFF,
            OpParams::OFF,
        ],
    }),
    amp_env: Adsr::new(0.0, 0.0, 1.0, 0.25),
    velocity: 0.7,
    ..Patch::INIT
};

/// 7 — church organ: the drawbar table at 8' and the hollow table at 16',
/// a slower speech.
pub const CHURCH_ORGAN: Patch = Patch {
    kind: sub(osc(Waveform::Table(0), 0.0, 0.0, 0.55), osc(Waveform::Table(2), -12.0, 1.5, 0.45), 0.0),
    amp_env: Adsr::new(0.06, 0.0, 1.0, 0.35).linear(),
    velocity: 0.0,
    level: 0.4,
    ..Patch::INIT
};

/// 8 — reed organ: two detuned pulses (accordion, harmonica, bagpipe)
/// with a little tremolo.
pub const REED_ORGAN: Patch = Patch {
    kind: sub(pulse(0.3, 0.0, -6.0, 0.45), pulse(0.3, 0.0, 6.0, 0.45), 0.0),
    filter: lp(2600.0, 0.1, 0.0, 0.4, 0.5),
    amp_env: Adsr::new(0.03, 0.1, 0.9, 0.12),
    lfo: LfoParams { wave: LfoWave::Sine, rate: LfoRate::Hz(5.5), amp: 0.08, ..LFO_OFF },
    level: 0.45,
    ..Patch::INIT
};

/// 9 — nylon guitar: triangle with a quiet saw, plucked low pass.
pub const NYLON: Patch = Patch {
    kind: sub(osc(Waveform::Triangle, 0.0, 0.0, 0.8), osc(Waveform::Saw, 0.0, 2.0, 0.25), 0.0),
    filter: lp(450.0, 0.1, 3.0, 0.8, 1.0),
    amp_env: Adsr::new(0.0, 1.6, 0.0, 0.2),
    filter_env: Adsr::new(0.0, 0.3, 0.1, 0.2),
    ..Patch::INIT
};

/// 10 — steel guitar: saw and pulse, a brighter pluck (steel, jazz, clean,
/// muted).
pub const STEEL: Patch = Patch {
    kind: sub(osc(Waveform::Saw, 0.0, 0.0, 0.8), pulse(0.3, 0.0, 4.0, 0.4), 0.08),
    filter: lp(700.0, 0.2, 4.0, 0.8, 1.2),
    amp_env: Adsr::new(0.0, 1.8, 0.0, 0.2),
    filter_env: Adsr::new(0.0, 0.22, 0.05, 0.15),
    level: 1.0,
    ..Patch::INIT
};

/// 11 — drive guitar: detuned saws over a square an octave down through a
/// resonant low pass, sustained (overdrive, distortion, charang).
pub const DRIVE: Patch = Patch {
    kind: sub(osc(Waveform::Saw, 0.0, -8.0, 0.5), osc(Waveform::Saw, 0.0, 8.0, 0.5), 0.0),
    filter: lp(2200.0, 0.35, 1.0, 0.4, 0.8),
    amp_env: Adsr::new(0.003, 0.4, 0.75, 0.15),
    filter_env: Adsr::new(0.0, 0.3, 0.3, 0.2),
    lfo: vibrato(5.0, 0.05),
    level: 0.55,
    ..Patch::INIT
};

/// 12 — bass: sine body with a saw edge, plucked filter, one voice.
pub const BASS: Patch = Patch {
    kind: sub(osc(Waveform::Sine, 0.0, 0.0, 0.85), osc(Waveform::Saw, 0.0, 0.0, 0.35), 0.0),
    filter: lp(500.0, 0.15, 2.0, 0.3, 1.0),
    amp_env: Adsr::new(0.002, 0.9, 0.45, 0.08),
    filter_env: Adsr::new(0.0, 0.2, 0.0, 0.1),
    mono: true,
    level: 1.0,
    ..Patch::INIT
};

/// 13 — slap bass: 1:1 FM with feedback and a fast index decay.
pub const SLAP_BASS: Patch = Patch {
    kind: Kind::Fm(FmParams {
        algorithm: 0,
        ops: 2,
        feedback: 0.3,
        key_scale: 0.0,
        op: [
            op(1.0, 0.85, Adsr::new(0.0, 0.7, 0.4, 0.08)),
            op(1.0, 3.2, Adsr::new(0.0, 0.12, 0.2, 0.08)),
            OpParams::OFF,
            OpParams::OFF,
        ],
    }),
    amp_env: Adsr::new(0.0, 0.0, 1.0, 0.08),
    mono: true,
    level: 1.0,
    ..Patch::INIT
};

/// 16 — solo strings: two saws close together, a bowed attack, vibrato
/// (violin to contrabass, fiddle).
pub const SOLO_STRINGS: Patch = Patch {
    kind: sub(osc(Waveform::Saw, 0.0, -4.0, 0.55), osc(Waveform::Saw, 0.0, 4.0, 0.45), 0.0),
    filter: lp(2400.0, 0.15, 0.6, 0.5, 0.8),
    amp_env: Adsr::new(0.09, 0.3, 0.85, 0.3),
    filter_env: Adsr::new(0.1, 0.4, 0.5, 0.3),
    lfo: vibrato(5.6, 0.14),
    level: 0.6,
    ..Patch::INIT
};

/// 18 — harp: triangle and an octave sine, plucked (pizzicato, harp).
pub const HARP: Patch = Patch {
    kind: sub(osc(Waveform::Triangle, 0.0, 0.0, 0.85), osc(Waveform::Sine, 12.0, 0.0, 0.25), 0.0),
    filter: lp(3000.0, 0.0, 1.0, 0.5, 0.6),
    amp_env: Adsr::new(0.0, 1.1, 0.0, 0.4),
    filter_env: Adsr::new(0.0, 0.3, 0.0, 0.3),
    ..Patch::INIT
};

/// 19 — timpani: a sine falling from a fifth above, a noise thud
/// (timpani, taiko, melodic tom, synth drum).
pub const TIMPANI: Patch = Patch {
    kind: sub(osc(Waveform::Sine, 0.0, 0.0, 0.9), osc(Waveform::Triangle, 12.0, 0.0, 0.15), 0.12),
    filter: lp(900.0, 0.1, 1.5, 0.6, 0.5),
    amp_env: Adsr::new(0.0, 1.3, 0.0, 0.5),
    filter_env: Adsr::new(0.0, 0.08, 0.0, 0.1),
    pitch_env: Adsr::new(0.0, 0.07, 0.0, 0.05),
    pitch_env_amount: 7.0,
    velocity: 0.8,
    ..Patch::INIT
};

/// 22 — horn: rounder, slower brass (French horn, synth brass).
pub const HORN: Patch = Patch {
    kind: sub(osc(Waveform::Saw, 0.0, -4.0, 0.55), osc(Waveform::Triangle, 0.0, 4.0, 0.45), 0.0),
    filter: lp(500.0, 0.1, 2.0, 0.6, 1.0),
    amp_env: Adsr::new(0.07, 0.3, 0.85, 0.25),
    filter_env: Adsr::new(0.12, 0.4, 0.5, 0.25),
    lfo: vibrato(5.0, 0.05),
    ..Patch::INIT
};

/// 23 — sax: pulse and saw with a breathy edge, filter swell, vibrato.
pub const SAX: Patch = Patch {
    kind: sub(pulse(0.35, 0.0, 0.0, 0.55), osc(Waveform::Saw, 0.0, 5.0, 0.35), 0.03),
    filter: lp(1400.0, 0.25, 1.5, 0.6, 1.0),
    amp_env: Adsr::new(0.03, 0.25, 0.8, 0.12),
    filter_env: Adsr::new(0.04, 0.3, 0.4, 0.15),
    lfo: vibrato(5.0, 0.1),
    level: 0.6,
    ..Patch::INIT
};

/// 24 — reed: square and the hollow table (clarinet, oboe, bassoon, shanai).
pub const REED: Patch = Patch {
    kind: sub(pulse(0.5, 0.0, 0.0, 0.5), osc(Waveform::Table(2), 0.0, 3.0, 0.5), 0.02),
    filter: lp(2000.0, 0.15, 0.8, 0.6, 0.8),
    amp_env: Adsr::new(0.04, 0.2, 0.85, 0.1),
    filter_env: Adsr::new(0.05, 0.3, 0.5, 0.1),
    lfo: vibrato(4.8, 0.06),
    level: 0.45,
    ..Patch::INIT
};

/// 26 — square lead: two pulses a few cents apart, open filter.
pub const SQUARE_LEAD: Patch = Patch {
    kind: sub(pulse(0.5, 0.0, -4.0, 0.5), pulse(0.45, 0.0, 4.0, 0.5), 0.0),
    filter: lp(4000.0, 0.1, 0.5, 0.5, 0.5),
    amp_env: Adsr::new(0.004, 0.2, 0.85, 0.12),
    lfo: vibrato(5.5, 0.1),
    level: 0.42,
    ..Patch::INIT
};

/// 29 — warm pad: slow, detuned saws under a low filter.
pub const WARM_PAD: Patch = Patch {
    kind: sub(osc(Waveform::Saw, 0.0, -10.0, 0.5), osc(Waveform::Saw, -12.0, 10.0, 0.4), 0.0),
    filter: lp(900.0, 0.15, 1.0, 0.3, 0.3),
    amp_env: Adsr::new(0.6, 0.8, 0.85, 1.5),
    filter_env: Adsr::new(1.0, 1.0, 0.6, 1.5),
    lfo: LfoParams { wave: LfoWave::Triangle, rate: LfoRate::Hz(0.3), cutoff: 0.4, ..LFO_OFF },
    level: 0.85,
    ..Patch::INIT
};

/// 30 — noise fx: band-passed noise that swells and fades (seashore,
/// breath, helicopter, applause, gunshot, reverse cymbal: one family).
pub const NOISE_FX: Patch = Patch {
    kind: sub(OscParams::OFF, OscParams::OFF, 0.9),
    filter: FilterParams {
        mode: FilterMode::BandPass,
        cutoff: 1200.0,
        resonance: 0.3,
        env_amount: 1.5,
        key_track: 0.5,
        velocity: 0.5,
    },
    amp_env: Adsr::new(0.25, 1.0, 0.4, 0.8),
    filter_env: Adsr::new(0.4, 0.8, 0.3, 0.8),
    lfo: LfoParams { wave: LfoWave::Triangle, rate: LfoRate::Hz(0.5), cutoff: 0.8, ..LFO_OFF },
    level: 1.0,
    ..Patch::INIT
};

/// 31 — chirp: a sine trilling a whole tone at 12 Hz (bird, telephone).
pub const CHIRP: Patch = Patch {
    kind: sub(osc(Waveform::Sine, 12.0, 0.0, 0.7), OscParams::OFF, 0.0),
    amp_env: Adsr::new(0.01, 0.3, 0.6, 0.1),
    lfo: LfoParams { wave: LfoWave::Square, rate: LfoRate::Hz(12.0), pitch: 1.0, ..LFO_OFF },
    level: 0.9,
    ..Patch::INIT
};

/// The 32 sounds with their names, in bank order.
pub const SET: [(&str, Patch); SOUNDS] = [
    ("piano", PIANO),
    ("e-piano", E_PIANO),
    ("harpsichord", HARPSICHORD),
    ("bell", BELL),
    ("mallet", MALLET),
    ("pluck", PLUCK),
    ("organ", ORGAN),
    ("church-organ", CHURCH_ORGAN),
    ("reed-organ", REED_ORGAN),
    ("nylon-guitar", NYLON),
    ("steel-guitar", STEEL),
    ("drive-guitar", DRIVE),
    ("bass", BASS),
    ("slap-bass", SLAP_BASS),
    ("synth-bass", presets::ACID_BASS),
    ("synth-bass-2", FM_BASS),
    ("solo-strings", SOLO_STRINGS),
    ("strings", presets::STRINGS),
    ("harp", HARP),
    ("timpani", TIMPANI),
    ("choir", presets::VOCAL),
    ("brass", presets::BRASS),
    ("horn", HORN),
    ("sax", SAX),
    ("reed", REED),
    ("flute", FLUTE),
    ("square-lead", SQUARE_LEAD),
    ("saw-lead", presets::SAW_LEAD),
    ("pad", presets::PWM_PAD),
    ("warm-pad", WARM_PAD),
    ("noise-fx", NOISE_FX),
    ("chirp", CHIRP),
];

/// The sound of each GM program (0-based).
pub const SOUND_OF: [u8; 128] = [
    // Piano
    0, 0, 0, 0, 1, 1, 2, 2, //
    // Chromatic percussion
    3, 3, 3, 4, 4, 4, 3, 5, //
    // Organ
    6, 6, 6, 7, 8, 8, 8, 8, //
    // Guitar
    9, 10, 10, 10, 10, 11, 11, 3, //
    // Bass
    12, 12, 12, 12, 13, 13, 14, 15, //
    // Strings
    16, 16, 16, 16, 17, 18, 18, 19, //
    // Ensemble
    17, 17, 17, 17, 20, 20, 20, 21, //
    // Brass
    21, 21, 21, 21, 22, 21, 22, 22, //
    // Reed
    23, 23, 23, 23, 24, 24, 24, 24, //
    // Pipe
    25, 25, 25, 25, 25, 25, 25, 25, //
    // Synth lead
    26, 27, 25, 25, 11, 20, 27, 27, //
    // Synth pad
    28, 29, 28, 20, 29, 28, 29, 28, //
    // Synth effects
    29, 29, 3, 28, 28, 20, 29, 28, //
    // Ethnic
    5, 5, 5, 5, 4, 8, 16, 24, //
    // Percussive
    3, 4, 4, 4, 19, 19, 19, 30, //
    // Sound effects
    30, 30, 30, 31, 31, 30, 30, 30,
];

/// The sound for GM program `program` (0–127; higher wraps).
#[must_use]
pub fn sound_of(program: u8) -> u8 {
    SOUND_OF[usize::from(program & 0x7F)]
}

/// The patches of [`SET`] without their names (a reference into `SET`
/// links the names too; on the player they would be resident strings).
static PATCHES: [Patch; SOUNDS] = {
    let mut a = [Patch::INIT; SOUNDS];
    let mut i = 0;
    while i < SOUNDS {
        a[i] = SET[i].1;
        i += 1;
    }
    a
};

/// The factory kits' entries without their names (as [`PATCHES`]).
static KIT_ENTRIES: [&[orpheus_synth::KitEntry]; FACTORY.len()] = {
    let mut a: [&[orpheus_synth::KitEntry]; FACTORY.len()] = [&[]; FACTORY.len()];
    let mut i = 0;
    while i < FACTORY.len() {
        a[i] = FACTORY[i].1;
        i += 1;
    }
    a
};

/// Sound `i`'s patch (`i` < [`SOUNDS`], else the piano).
#[must_use]
pub fn patch(i: u8) -> &'static Patch {
    PATCHES.get(usize::from(i)).unwrap_or(&PATCHES[0])
}

/// The factory kit (index into `orpheus_synth::kit::FACTORY`) for a
/// program change on the drum channel.
#[must_use]
pub fn kit_for(program: u8) -> u8 {
    match program & 0x7F {
        16..=23 => 2, // power → punch
        24..=31 => 1, // electronic, 808 → analog
        32..=47 => 3, // jazz, brush → tight
        _ => 0,       // standard, room, orchestra, SFX → standard
    }
}

/// Factory kit `i` (index into `FACTORY`, else `standard`).
#[must_use]
pub fn kit(i: u8) -> Kit {
    Kit::new(KIT_ENTRIES.get(usize::from(i)).copied().unwrap_or(KIT_ENTRIES[0]))
}

/// Write the set as an `OSB1` sound bank: the 32 sounds, then the factory
/// kits. Returns the bank's length.
pub fn write_bank(buf: &mut [u8]) -> Result<usize, BankError> {
    let mut w = Writer::new(buf)?;
    for (name, p) in &SET {
        w.patch(name, p)?;
    }
    for (name, k) in &FACTORY {
        w.kit(name, &Kit::new(k))?;
    }
    Ok(w.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_used_and_every_program_mapped() {
        let mut used = [false; SOUNDS];
        for &s in &SOUND_OF {
            assert!(usize::from(s) < SOUNDS);
            used[usize::from(s)] = true;
        }
        assert!(used.iter().all(|&u| u), "unused sounds: {used:?}");
    }

    #[test]
    fn names_are_unique_and_fit_the_bank() {
        for (i, (a, _)) in SET.iter().enumerate() {
            assert!(a.len() <= orpheus_synth::bank::NAME_LEN);
            for (b, _) in &SET[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
