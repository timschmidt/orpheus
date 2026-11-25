//! Fuzz-target body of `orpheus-synth` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`synth_events`] — the engine over arbitrary event streams: notes on any
//! part and note number, every part and effect parameter with any `f32`
//! (NaN, infinities, huge values), tempo, master, all-notes/all-sound off,
//! patches with arbitrary field values (also NaN), voice limits, resets,
//! render blocks of 0–4096 frames in `f32` and `i16`, events at any offset
//! and in any order. Oracles: no panic; every output sample finite and
//! within [−1, 1]; at most 16 + 4 voices sound; every voice level finite.

#![forbid(unsafe_code)]

use orpheus_synth::lfo::LfoRate;
use orpheus_synth::{
    DrumKind, DrumParams, Engine, Event, EventKind, FxParam, Kind, MAX_VOICES, Param, Patch, TAILS, Waveform,
    presets,
};

const PARAMS: [Param; 14] = [
    Param::Volume,
    Param::Pan,
    Param::Cutoff,
    Param::Resonance,
    Param::Bend,
    Param::Glide,
    Param::LfoRate,
    Param::LfoPitch,
    Param::LfoCutoff,
    Param::Sustain,
    Param::Level,
    Param::DelaySend,
    Param::ChorusSend,
    Param::ReverbSend,
];

const FX: [FxParam; 11] = [
    FxParam::DelayTime,
    FxParam::DelayBeats,
    FxParam::DelayFeedback,
    FxParam::DelayDamp,
    FxParam::DelayLevel,
    FxParam::ChorusRate,
    FxParam::ChorusDepth,
    FxParam::ChorusLevel,
    FxParam::ReverbDecay,
    FxParam::ReverbDamp,
    FxParam::ReverbLevel,
];

const DRUMS: [DrumKind; 6] =
    [DrumKind::Kick, DrumKind::Snare, DrumKind::ClosedHat, DrumKind::OpenHat, DrumKind::Clap, DrumKind::Tom];

/// A preset (melodic, drum or kit) with one field set to `v`.
fn patch(sel: u8, field: u8, v: f32) -> Patch {
    let n = presets::MELODIC.len() as u8;
    let mut p = match sel % (n + 2) {
        i if i < n => presets::MELODIC[usize::from(i)].1,
        i if i == n => Patch::drum(DrumParams::new(DRUMS[usize::from(field) % DRUMS.len()])),
        _ => Patch::KIT,
    };
    match field % 30 {
        0 => p.filter.cutoff = v,
        1 => p.filter.resonance = v,
        2 => p.filter.env_amount = v,
        3 => p.filter.key_track = v,
        4 => p.filter.velocity = v,
        5 => p.amp_env.attack = v,
        6 => p.amp_env.decay = v,
        7 => p.amp_env.sustain = v,
        8 => p.amp_env.release = v,
        9 => p.filter_env.attack = v,
        10 => p.pitch_env.decay = v,
        11 => p.pitch_env_amount = v,
        12 => p.lfo.rate = LfoRate::Hz(v),
        13 => p.lfo.rate = LfoRate::Sync(v),
        14 => p.lfo.pitch = v,
        15 => p.lfo.cutoff = v,
        16 => p.lfo.amp = v,
        17 => p.lfo.width = v,
        18 => p.glide = v,
        19 => p.level = v,
        20 => p.velocity = v,
        21 => p.bend_range = v,
        22 => {
            p.mono = v > 0.0;
            p.legato = v > 1.0;
        }
        f => match &mut p.kind {
            Kind::Subtractive(s) => match f {
                23 => s.osc[0].semitones = v,
                24 => s.osc[0].width = v,
                25 => s.osc[1].wave = Waveform::Table(v.to_bits() as u8),
                26 => {
                    s.osc[0].wave =
                        [Waveform::Pink, Waveform::White, Waveform::Triangle][v.to_bits() as usize % 3];
                }
                27 => s.noise = v,
                _ => s.osc[1].cents = v,
            },
            Kind::Fm(fm) => match f {
                23 => fm.op[usize::from(field) % 4].ratio = v,
                24 => fm.op[usize::from(field) % 4].level = v,
                25 => fm.feedback = v,
                26 => fm.algorithm = v.to_bits() as u8,
                27 => fm.ops = v.to_bits() as u8,
                _ => fm.key_scale = v,
            },
            Kind::Drum(d) => match f {
                23 => d.tune = v,
                24 => d.decay = v,
                25 => d.tone = v,
                26 => d.snap = v,
                _ => d.level = v,
            },
            Kind::Kit => {}
        },
    }
    p
}

fn check(e: &Engine, out: &[f32]) {
    for (i, x) in out.iter().enumerate() {
        assert!(x.is_finite() && x.abs() <= 1.0, "sample {i} = {x}");
    }
    assert!(e.active_voices() <= MAX_VOICES + TAILS, "{} voices", e.active_voices());
    for v in e.voices() {
        assert!(v.level.is_finite(), "voice level {v:?}");
    }
}

/// Input: a 4-byte header (rate selector, seed, voice limit, preset
/// selector), then 8-byte records `[op, part, a, b, x0, x1, x2, x3]` (x as
/// a little-endian `f32` or `u16`; see the match below). At most 200 000
/// frames are rendered per input.
pub fn synth_events(data: &[u8]) {
    if data.len() < 4 {
        return;
    }
    let rate = [8000, 22_050, 44_100, 48_000, 96_000, 0][usize::from(data[0]) % 6];
    let mut e = Engine::new(rate, u32::from(data[1]));
    e.set_voice_limit(usize::from(data[2]));
    for part in 0..16u8 {
        e.set_patch(part, &patch(data[3].wrapping_add(part), 255, 0.0));
    }
    let mut pending: Vec<Event> = Vec::new();
    let mut fbuf = vec![0.0f32; 2 * 4096 + 1];
    let mut ibuf = vec![0i16; 2 * 4096 + 1];
    let mut budget: usize = 200_000;
    for r in data[4..].chunks_exact(8) {
        let x = f32::from_le_bytes([r[4], r[5], r[6], r[7]]);
        let off16 = u32::from(u16::from_le_bytes([r[4], r[5]]));
        let off8 = u32::from(r[3]) * 4;
        let ev = |offset, kind| Event { offset, kind };
        match r[0] % 12 {
            0 => pending.push(ev(off16, EventKind::NoteOn { part: r[1], note: r[2], velocity: r[3] })),
            1 => pending.push(ev(off16, EventKind::NoteOff { part: r[1], note: r[2] })),
            2 => pending.push(ev(
                off8,
                EventKind::Param { part: r[1], param: PARAMS[usize::from(r[2]) % PARAMS.len()], value: x },
            )),
            3 => pending.push(ev(off8, EventKind::Fx { param: FX[usize::from(r[2]) % FX.len()], value: x })),
            4 => pending.push(ev(off8, EventKind::Tempo(x))),
            5 => pending.push(ev(off8, EventKind::Master(x))),
            6 => pending.push(ev(off16, EventKind::AllNotesOff { part: r[1] })),
            7 => pending.push(ev(off16, EventKind::AllSoundOff)),
            op @ (8 | 9) => {
                let frames = (usize::from(u16::from_le_bytes([r[2], r[3]])) % 4097).min(budget);
                budget -= frames;
                // An odd length exercises the trailing-sample rule.
                let len = 2 * frames + usize::from(r[1] & 1);
                pending.truncate(64);
                if op == 8 {
                    e.render_f32(&mut fbuf[..len], &pending);
                    check(&e, &fbuf[..len]);
                } else {
                    e.render_i16(&mut ibuf[..len], &pending);
                    check(&e, &[]);
                }
                pending.clear();
            }
            10 => e.set_patch(r[1], &patch(r[2], r[3], x)),
            _ => {
                if r[1] & 1 == 0 {
                    e.reset(x.to_bits());
                } else {
                    e.set_voice_limit(usize::from(r[2]));
                }
            }
        }
        if budget == 0 {
            break;
        }
    }
    let frames = 1024.min(budget);
    pending.truncate(64);
    e.render_f32(&mut fbuf[..2 * frames], &pending);
    check(&e, &fbuf[..2 * frames]);
}
