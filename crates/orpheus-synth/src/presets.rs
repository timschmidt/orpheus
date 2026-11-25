//! Factory patches (our own sounds; SYN-2 moves presets and kits to data
//! files). They are also the material of the golden renders.

use crate::drum::{DrumKind, DrumParams};
use crate::env::Adsr;
use crate::filter::FilterMode;
use crate::fm::{FmParams, OpParams};
use crate::lfo::{LfoParams, LfoRate, LfoWave};
use crate::osc::Waveform;
use crate::patch::{FilterParams, Kind, OscParams, Patch, SubParams};

const fn sub(a: OscParams, b: OscParams, noise: f32) -> Kind {
    Kind::Subtractive(SubParams { osc: [a, b], noise })
}

const fn osc(wave: Waveform, semitones: f32, cents: f32, level: f32) -> OscParams {
    OscParams { wave, semitones, cents, level, width: 0.5, phase: 0.0 }
}

const LFO_OFF: LfoParams = Patch::INIT.lfo;

/// Two detuned saws through a resonant low pass with an envelope; vibrato.
pub const SAW_LEAD: Patch = Patch {
    kind: sub(osc(Waveform::Saw, 0.0, -7.0, 0.6), osc(Waveform::Saw, 0.0, 7.0, 0.6), 0.0),
    filter: FilterParams {
        mode: FilterMode::LowPass,
        cutoff: 1800.0,
        resonance: 0.35,
        env_amount: 2.0,
        key_track: 0.5,
        velocity: 1.0,
    },
    amp_env: Adsr::new(0.005, 0.25, 0.75, 0.25),
    filter_env: Adsr::new(0.002, 0.4, 0.2, 0.3),
    lfo: LfoParams { wave: LfoWave::Sine, rate: LfoRate::Hz(5.5), pitch: 0.12, ..LFO_OFF },
    ..Patch::INIT
};

/// Square plus a sine an octave below; mono, legato, short glide.
pub const SQUARE_BASS: Patch = Patch {
    kind: sub(osc(Waveform::Pulse, 0.0, 0.0, 0.7), osc(Waveform::Sine, -12.0, 0.0, 0.6), 0.0),
    filter: FilterParams {
        mode: FilterMode::LowPass,
        cutoff: 350.0,
        resonance: 0.45,
        env_amount: 3.0,
        key_track: 0.3,
        velocity: 1.0,
    },
    amp_env: Adsr::new(0.001, 0.3, 0.6, 0.08),
    filter_env: Adsr::new(0.0, 0.22, 0.0, 0.1),
    glide: 0.06,
    mono: true,
    legato: true,
    ..Patch::INIT
};

/// Two pulses with LFO pulse-width modulation, slow attack.
pub const PWM_PAD: Patch = Patch {
    kind: sub(
        OscParams { width: 0.5, ..osc(Waveform::Pulse, 0.0, -5.0, 0.5) },
        OscParams { width: 0.35, ..osc(Waveform::Pulse, 12.0, 5.0, 0.3) },
        0.0,
    ),
    filter: FilterParams { mode: FilterMode::LowPass, cutoff: 2200.0, resonance: 0.1, ..FilterParams::OFF },
    amp_env: Adsr::new(0.35, 0.5, 0.8, 1.0),
    lfo: LfoParams { wave: LfoWave::Triangle, rate: LfoRate::Hz(0.8), width: 0.3, ..LFO_OFF },
    level: 0.6,
    ..Patch::INIT
};

/// Triangle and breath noise.
pub const TRI_FLUTE: Patch = Patch {
    kind: sub(osc(Waveform::Triangle, 0.0, 0.0, 0.9), osc(Waveform::Sine, 12.0, 0.0, 0.1), 0.04),
    filter: FilterParams {
        mode: FilterMode::LowPass,
        cutoff: 6000.0,
        resonance: 0.0,
        key_track: 0.5,
        ..FilterParams::OFF
    },
    amp_env: Adsr::new(0.06, 0.1, 0.9, 0.12),
    lfo: LfoParams { wave: LfoWave::Sine, rate: LfoRate::Hz(4.8), pitch: 0.08, amp: 0.1, ..LFO_OFF },
    ..Patch::INIT
};

/// Wavetable organ (table 0), linear gate envelope.
pub const ORGAN: Patch = Patch {
    kind: sub(osc(Waveform::Table(0), 0.0, 0.0, 0.7), osc(Waveform::Table(0), 12.0, 2.0, 0.3), 0.0),
    amp_env: Adsr::new(0.003, 0.0, 1.0, 0.04).linear(),
    lfo: LfoParams { wave: LfoWave::Sine, rate: LfoRate::Hz(6.5), amp: 0.15, ..LFO_OFF },
    ..Patch::INIT
};

/// Wavetable voice (table 3) with a slow filter swell.
pub const VOCAL: Patch = Patch {
    kind: sub(osc(Waveform::Table(3), 0.0, -3.0, 0.6), osc(Waveform::Table(2), 0.0, 3.0, 0.5), 0.0),
    filter: FilterParams {
        mode: FilterMode::LowPass,
        cutoff: 900.0,
        resonance: 0.2,
        env_amount: 1.5,
        ..FilterParams::OFF
    },
    amp_env: Adsr::new(0.12, 0.3, 0.8, 0.5),
    filter_env: Adsr::new(0.4, 0.6, 0.6, 0.5),
    lfo: LfoParams { wave: LfoWave::Sine, rate: LfoRate::Hz(5.0), pitch: 0.1, ..LFO_OFF },
    ..Patch::INIT
};

/// Plucked string: saw and a noise burst into a closing low pass.
pub const PLUCK: Patch = Patch {
    kind: sub(osc(Waveform::Saw, 0.0, 0.0, 0.8), OscParams::OFF, 0.3),
    filter: FilterParams {
        mode: FilterMode::LowPass,
        cutoff: 250.0,
        resonance: 0.2,
        env_amount: 5.0,
        key_track: 0.8,
        velocity: 1.5,
    },
    amp_env: Adsr::new(0.0, 0.6, 0.0, 0.25),
    filter_env: Adsr::new(0.0, 0.18, 0.0, 0.2),
    ..Patch::INIT
};

/// FM electric piano: two pairs, a body and a bright tine.
pub const FM_EPIANO: Patch = Patch {
    kind: Kind::Fm(FmParams {
        algorithm: 3,
        ops: 4,
        feedback: 0.0,
        key_scale: -0.3,
        op: [
            OpParams::new(1.0, 0.8, Adsr::new(0.0, 1.8, 0.0, 0.35)),
            OpParams::new(1.0, 1.6, Adsr::new(0.0, 1.0, 0.2, 0.3)),
            OpParams::new(1.0, 0.3, Adsr::new(0.0, 0.7, 0.0, 0.3)),
            OpParams::new(14.0, 0.9, Adsr::new(0.0, 0.12, 0.0, 0.1)),
        ],
    }),
    amp_env: Adsr::new(0.0, 0.0, 1.0, 0.35),
    ..Patch::INIT
};

/// FM bell: inharmonic 2-op stack, long decay.
pub const FM_BELL: Patch = Patch {
    kind: Kind::Fm(FmParams {
        algorithm: 0,
        ops: 2,
        feedback: 0.0,
        key_scale: -0.2,
        op: [
            OpParams::new(1.0, 0.9, Adsr::new(0.0, 3.0, 0.0, 1.5)),
            OpParams::new(3.5, 3.0, Adsr::new(0.0, 2.0, 0.0, 1.0)),
            OpParams::OFF,
            OpParams::OFF,
        ],
    }),
    amp_env: Adsr::new(0.0, 0.0, 1.0, 1.5),
    velocity: 0.5,
    ..Patch::INIT
};

/// FM bass: 2-op with feedback, mono.
pub const FM_BASS: Patch = Patch {
    kind: Kind::Fm(FmParams {
        algorithm: 0,
        ops: 2,
        feedback: 0.45,
        key_scale: 0.0,
        op: [
            OpParams::new(1.0, 0.9, Adsr::new(0.0, 0.8, 0.5, 0.08)),
            OpParams::new(1.0, 2.6, Adsr::new(0.0, 0.25, 0.15, 0.08)),
            OpParams::OFF,
            OpParams::OFF,
        ],
    }),
    amp_env: Adsr::new(0.0, 0.0, 1.0, 0.08),
    mono: true,
    ..Patch::INIT
};

/// Game effect: a square sweeping down two octaves.
pub const SWEEP_FX: Patch = Patch {
    kind: sub(osc(Waveform::Pulse, 0.0, 0.0, 0.6), OscParams::OFF, 0.0),
    amp_env: Adsr::new(0.0, 0.35, 0.0, 0.05),
    pitch_env: Adsr::new(0.0, 0.3, 0.0, 0.1),
    pitch_env_amount: 24.0,
    velocity: 0.0,
    ..Patch::INIT
};

/// Noise hit through a band pass that falls.
pub const NOISE_HIT: Patch = Patch {
    kind: sub(OscParams::OFF, OscParams::OFF, 0.9),
    filter: FilterParams {
        mode: FilterMode::BandPass,
        cutoff: 600.0,
        resonance: 0.6,
        env_amount: 3.0,
        ..FilterParams::OFF
    },
    amp_env: Adsr::new(0.0, 0.25, 0.0, 0.1),
    filter_env: Adsr::new(0.0, 0.2, 0.0, 0.1),
    ..Patch::INIT
};

/// Pink noise, held (the signal generator's speaker check).
pub const PINK: Patch = Patch {
    kind: sub(osc(Waveform::Pink, 0.0, 0.0, 1.0), OscParams::OFF, 0.0),
    amp_env: Adsr::new(0.01, 0.0, 1.0, 0.01).linear(),
    velocity: 0.0,
    ..Patch::INIT
};

/// The melodic presets by name.
pub const MELODIC: [(&str, Patch); 12] = [
    ("saw-lead", SAW_LEAD),
    ("square-bass", SQUARE_BASS),
    ("pwm-pad", PWM_PAD),
    ("tri-flute", TRI_FLUTE),
    ("organ", ORGAN),
    ("vocal", VOCAL),
    ("pluck", PLUCK),
    ("fm-epiano", FM_EPIANO),
    ("fm-bell", FM_BELL),
    ("fm-bass", FM_BASS),
    ("sweep-fx", SWEEP_FX),
    ("noise-hit", NOISE_HIT),
];

/// The drum voices by name.
pub const DRUMS: [(&str, DrumParams); 6] = [
    ("kick", DrumParams::new(DrumKind::Kick)),
    ("snare", DrumParams::new(DrumKind::Snare)),
    ("closed-hat", DrumParams::new(DrumKind::ClosedHat)),
    ("open-hat", DrumParams::new(DrumKind::OpenHat)),
    ("clap", DrumParams::new(DrumKind::Clap)),
    ("tom", DrumParams::new(DrumKind::Tom)),
];
