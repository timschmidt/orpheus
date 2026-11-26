//! Scores and the golden-render scenarios.
//!
//! A [`Score`] is a list of events at absolute times; [`drive`] feeds it to
//! an engine in blocks of any size, converting the times to offsets within
//! each block. The [`SCENARIOS`] are the material of the golden renders
//! (`tests/golden.rs`), of the block-size independence test and of the
//! `render` example (WAV files for listening).

use crate::engine::{Engine, Event, EventKind, FxParam, Param};
use crate::fx::{ChorusParams, DelayParams, ReverbParams};
use crate::lfo::{LfoParams, LfoRate, LfoWave};
use crate::patch::Patch;
use crate::presets;

/// Events a score holds.
pub const SCORE_MAX: usize = 512;

/// An event at an absolute frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timed {
    /// Frame from the start.
    pub at: u64,
    /// What happens.
    pub kind: EventKind,
}

/// Events at absolute times, kept sorted (stable: equal times keep their
/// insertion order).
#[derive(Clone, Debug)]
pub struct Score {
    ev: [Timed; SCORE_MAX],
    n: usize,
    rate: f32,
}

impl Score {
    /// An empty score at `rate` Hz.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self { ev: [Timed { at: 0, kind: EventKind::AllSoundOff }; SCORE_MAX], n: 0, rate: rate as f32 }
    }

    /// Add `kind` at `at` frames (dropped when full).
    pub fn push_at(&mut self, at: u64, kind: EventKind) {
        if self.n == SCORE_MAX {
            return;
        }
        let mut i = self.n;
        while i > 0 && self.ev[i - 1].at > at {
            self.ev[i] = self.ev[i - 1];
            i -= 1;
        }
        self.ev[i] = Timed { at, kind };
        self.n += 1;
    }

    /// Add `kind` at `t` seconds.
    pub fn push(&mut self, t: f32, kind: EventKind) {
        self.push_at((t * self.rate) as u64, kind);
    }

    /// A note from `t` for `dur` seconds.
    pub fn note(&mut self, t: f32, dur: f32, part: u8, note: u8, velocity: u8) {
        self.push(t, EventKind::NoteOn { part, note, velocity });
        self.push(t + dur, EventKind::NoteOff { part, note });
    }

    /// A part parameter at `t` seconds.
    pub fn param(&mut self, t: f32, part: u8, param: Param, value: f32) {
        self.push(t, EventKind::Param { part, param, value });
    }

    /// The events, sorted.
    #[must_use]
    pub fn events(&self) -> &[Timed] {
        &self.ev[..self.n]
    }
}

/// Feed `score` to `engine` over `frames` frames in blocks of `block`
/// frames (shortened where a block would hold more than 32 events): `f`
/// renders each block, given the engine, the frame range and the block's
/// events with relative offsets.
pub fn drive(
    engine: &mut Engine,
    score: &Score,
    frames: usize,
    block: usize,
    mut f: impl FnMut(&mut Engine, core::ops::Range<usize>, &[Event]),
) {
    let block = block.max(1);
    let ev = score.events();
    let mut ei = 0;
    let mut pos = 0;
    while pos < frames {
        let mut end = (pos + block).min(frames);
        let mut local = [Event { offset: 0, kind: EventKind::AllSoundOff }; 32];
        let mut n = 0;
        while ei < ev.len() && (ev[ei].at as usize) < end {
            if n == local.len() {
                end = (ev[ei].at as usize).max(pos + 1);
                break;
            }
            local[n] = Event { offset: (ev[ei].at as usize).saturating_sub(pos) as u32, kind: ev[ei].kind };
            n += 1;
            ei += 1;
        }
        f(engine, pos..end, &local[..n]);
        pos = end;
    }
}

/// How a scenario sets up the engine and writes its score.
#[derive(Clone, Copy, Debug)]
pub enum Setup {
    /// [`presets::MELODIC`]`[i]` playing the standard phrase.
    Melodic(usize),
    /// [`presets::DRUMS`]`[i]`: two hits.
    Drum(usize),
    /// [`crate::kit::FACTORY`]`[i]` in kit slot 0 playing two bars.
    Kit(usize),
    /// Custom.
    Custom(fn(&mut Engine, &mut Score)),
}

/// A golden-render scenario.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    /// Name (the key in `tests/goldens.txt`).
    pub name: &'static str,
    /// Sample rate.
    pub rate: u32,
    /// Length, seconds.
    pub seconds: f32,
    /// Set-up.
    pub setup: Setup,
}

impl Scenario {
    /// Frames to render.
    #[must_use]
    pub fn frames(&self) -> usize {
        (self.seconds * self.rate as f32) as usize
    }

    /// A fresh engine (seed 1) set up for the scenario, and its score.
    #[must_use]
    pub fn build(&self) -> (Engine, Score) {
        let mut e = Engine::new(self.rate, 1);
        let mut s = Score::new(self.rate);
        match self.setup {
            Setup::Melodic(i) => {
                e.set_patch(0, &presets::MELODIC[i].1);
                phrase(&mut s, 0);
            }
            Setup::Drum(i) => {
                e.set_patch(0, &Patch::drum(presets::DRUMS[i].1));
                s.note(0.0, 0.1, 0, 60, 127);
                s.note(0.25, 0.1, 0, 60, 80);
            }
            Setup::Kit(i) => {
                let (name, kit) = crate::kit::FACTORY[i];
                e.set_kit(0, &crate::kit::Kit::new(kit));
                e.set_patch(9, &Patch::KIT);
                e.set_reverb(&ReverbParams { decay: 1.1, damp: 0.5, level: 0.5 });
                s.param(0.0, 9, Param::ReverbSend, 0.12);
                if name == "percussion" {
                    percussion_beat(&mut s);
                } else {
                    kit_beat(&mut s);
                }
            }
            Setup::Custom(f) => f(&mut e, &mut s),
        }
        (e, s)
    }

    /// Render to interleaved `i16` in blocks of `block` frames.
    pub fn render_i16(&self, out: &mut [i16], block: usize) {
        let (mut e, s) = self.build();
        let frames = self.frames().min(out.len() / 2);
        drive(&mut e, &s, frames, block, |e, r, ev| e.render_i16(&mut out[2 * r.start..2 * r.end], ev));
    }
}

/// C4, E4, G4 overlapping at falling velocities, C5 loud, then C7.
fn phrase(s: &mut Score, part: u8) {
    s.note(0.00, 0.35, part, 60, 100);
    s.note(0.08, 0.30, part, 64, 70);
    s.note(0.16, 0.25, part, 67, 45);
    s.note(0.30, 0.25, part, 72, 127);
    s.note(0.45, 0.15, part, 96, 110);
}

fn kit_groove(e: &mut Engine, s: &mut Score) {
    e.set_patch(9, &Patch::KIT);
    let step = 60.0 / 120.0 / 4.0;
    for i in 0..16u8 {
        let t = f32::from(i) * step;
        if i % 4 == 0 {
            s.note(t, 0.05, 9, 36, 120);
        }
        if i % 8 == 4 {
            s.note(t, 0.05, 9, 38, 110);
        }
        if i == 12 {
            s.note(t, 0.05, 9, 39, 90);
        }
        if i == 14 {
            s.note(t, 0.05, 9, 46, 90);
        } else if i % 2 == 0 {
            s.note(t, 0.05, 9, 42, 70 + 8 * (i % 4));
        }
        if i == 15 {
            s.note(t, 0.05, 9, 42, 60);
        }
    }
    s.note(1.0, 0.1, 9, 45, 100);
    s.note(1.15, 0.1, 9, 48, 100);
}

/// Sixteenth steps at 110 BPM.
const STEP: f32 = 60.0 / 110.0 / 4.0;

/// Two bars for a drum kit: kick, snare and clap, eighth hats with an open
/// hat cut by the next closed one (choke group), rim, cowbell, clave and
/// shaker where the kit has them, a tom fill and a crash. Notes a kit does
/// not have are silent.
fn kit_beat(s: &mut Score) {
    let mut hit = |bar: u8, step: u8, note: u8, vel: u8| {
        s.note((f32::from(bar) * 16.0 + f32::from(step)) * STEP, 0.05, 9, note, vel);
    };
    for bar in 0..2u8 {
        let kicks: &[u8] = if bar == 0 { &[0, 7, 8] } else { &[0, 7, 10] };
        for &k in kicks {
            hit(bar, k, 36, 120);
        }
        hit(bar, 4, 38, 110);
        hit(bar, 12, 38, 115);
        if bar == 1 {
            hit(bar, 12, 39, 100);
            hit(bar, 11, 38, 40);
        }
        for st in (0..16u8).step_by(2) {
            if bar == 0 && st == 14 {
                hit(bar, st, 46, 95);
            } else if !(bar == 1 && st >= 12) {
                hit(bar, st, 42, if st % 4 == 0 { 100 } else { 70 });
            }
        }
        hit(bar, 3, 37, 80);
        hit(bar, 2 + 8 * bar, 56, 70);
        hit(bar, 6, 75, 75);
        for st in (1..16u8).step_by(2) {
            hit(bar, st, 70, 45 + 4 * (st % 4));
        }
    }
    // Fill and crash.
    for (st, n) in [(12u8, 50u8), (13, 48), (14, 47), (15, 45)] {
        hit(1, st, n, 105);
    }
    hit(2, 0, 36, 120);
    hit(2, 0, 49, 110);
    hit(0, 0, 51, 60);
}

/// Two bars of hand percussion: a 3-2 son clave, conga tumbao (muted and
/// open in one choke group), bongos, cowbell, shaker sixteenths, kick.
fn percussion_beat(s: &mut Score) {
    let mut hit = |bar: u8, step: u8, note: u8, vel: u8| {
        s.note((f32::from(bar) * 16.0 + f32::from(step)) * STEP, 0.05, 9, note, vel);
    };
    for bar in 0..2u8 {
        let clave: &[u8] = if bar == 0 { &[0, 6, 12] } else { &[4, 8] };
        for &c in clave {
            hit(bar, c, 75, 100);
        }
        for (st, n, v) in [
            (0u8, 62u8, 70u8),
            (2, 62, 60),
            (4, 62, 80),
            (6, 63, 100),
            (7, 63, 90),
            (8, 62, 70),
            (10, 62, 60),
            (12, 62, 80),
            (14, 64, 100),
            (15, 64, 95),
        ] {
            hit(bar, st, n, v);
        }
        for (st, n) in [(1u8, 60u8), (3, 61), (5, 60), (9, 60), (11, 61), (13, 60)] {
            hit(bar, st, n, 75);
        }
        for st in (0..16u8).step_by(4) {
            hit(bar, st, 56, if st == 0 { 90 } else { 65 });
        }
        for st in 0..16u8 {
            hit(bar, st, 70, if st % 4 == 0 { 80 } else { 50 });
        }
        hit(bar, 0, 36, 110);
        hit(bar, 8, 36, 90);
        hit(bar, 8, 67, 70);
        hit(bar, 10, 68, 70);
    }
    hit(2, 0, 36, 110);
    hit(2, 0, 51, 80);
}

fn glide_mono(e: &mut Engine, s: &mut Score) {
    e.set_patch(0, &presets::SQUARE_BASS);
    s.note(0.0, 0.30, 0, 36, 110);
    s.note(0.25, 0.30, 0, 43, 90); // legato: glides
    s.note(0.50, 0.40, 0, 48, 100); // legato again
    s.note(0.60, 0.10, 0, 41, 100); // held above 48: returns to 48 after
    s.note(1.00, 0.15, 0, 36, 127); // detached: retriggers
}

fn steal(e: &mut Engine, s: &mut Score) {
    let mut p = presets::SAW_LEAD;
    p.amp_env.release = 2.0;
    e.set_patch(0, &p);
    for i in 0..24u8 {
        s.note(f32::from(i) * 0.02, 0.6, 0, 40 + 2 * i, 60 + i * 2);
    }
}

fn fx_delay(e: &mut Engine, s: &mut Score) {
    e.set_patch(0, &presets::PLUCK);
    e.set_delay(&DelayParams { beats: 0.5, feedback: 0.5, ..DelayParams::DEFAULT });
    s.param(0.0, 0, Param::DelaySend, 0.6);
    s.note(0.0, 0.1, 0, 64, 110);
    s.note(0.2, 0.1, 0, 71, 90);
    s.push(0.9, EventKind::Fx { param: FxParam::DelayFeedback, value: 0.2 });
}

/// A tempo-synced dotted eighth at 120 BPM (375 ms: the line at half rate)
/// with feedback, low cut and damping.
fn fx_delay_sync(e: &mut Engine, s: &mut Score) {
    e.set_patch(0, &presets::PLUCK);
    e.set_delay(&DelayParams {
        beats: 0.75,
        feedback: 0.45,
        damp: 0.35,
        low_cut: 250.0,
        ..DelayParams::DEFAULT
    });
    s.push(0.0, EventKind::Tempo(120.0));
    s.param(0.0, 0, Param::DelaySend, 0.7);
    s.param(0.0, 0, Param::Pan, -0.3);
    for (i, n) in [64u8, 67, 71, 74].iter().enumerate() {
        s.note(0.5 * i as f32, 0.1, 0, *n, 100);
    }
}

/// A long delay: one beat at 70 BPM (857 ms: the line at quarter rate).
fn fx_delay_long(e: &mut Engine, s: &mut Score) {
    e.set_patch(9, &Patch::KIT);
    e.set_delay(&DelayParams {
        beats: 1.0,
        feedback: 0.5,
        damp: 0.2,
        low_cut: 150.0,
        ..DelayParams::DEFAULT
    });
    s.push(0.0, EventKind::Tempo(70.0));
    s.param(0.0, 9, Param::DelaySend, 0.6);
    s.note(0.0, 0.05, 9, 37, 120);
    s.note(0.43, 0.05, 9, 56, 100);
}

fn fx_chorus(e: &mut Engine, s: &mut Score) {
    e.set_patch(0, &presets::PWM_PAD);
    e.set_chorus(&ChorusParams::DEFAULT);
    s.param(0.0, 0, Param::ChorusSend, 0.8);
    for n in [57, 60, 64] {
        s.note(0.0, 1.0, 0, n, 90);
    }
}

fn fx_reverb(e: &mut Engine, s: &mut Score) {
    e.set_patch(9, &Patch::KIT);
    e.set_reverb(&ReverbParams { decay: 1.2, ..ReverbParams::DEFAULT });
    s.param(0.0, 9, Param::ReverbSend, 0.7);
    s.note(0.0, 0.1, 9, 38, 120);
    s.note(0.3, 0.1, 9, 39, 110);
}

/// A long hall on strings and a clave: the tail's diffusion and decay.
fn fx_reverb_hall(e: &mut Engine, s: &mut Score) {
    e.set_patch(0, &presets::STRINGS);
    e.set_patch(9, &Patch::KIT);
    e.set_reverb(&ReverbParams { decay: 4.0, damp: 0.3, level: 0.7 });
    s.param(0.0, 0, Param::ReverbSend, 0.6);
    s.param(0.0, 9, Param::ReverbSend, 0.8);
    for n in [55u8, 62, 67, 71] {
        s.note(0.0, 0.8, 0, n, 85);
    }
    s.note(1.2, 0.05, 9, 75, 120);
    s.note(1.5, 0.05, 9, 38, 110);
}

fn lfo_sync(e: &mut Engine, s: &mut Score) {
    let mut p = presets::SAW_LEAD;
    p.lfo = LfoParams { wave: LfoWave::Triangle, rate: LfoRate::Sync(2.0), cutoff: 2.0, ..p.lfo };
    e.set_patch(0, &p);
    s.push(0.0, EventKind::Tempo(100.0));
    s.note(0.0, 0.9, 0, 48, 100);
}

fn param_sweep(e: &mut Engine, s: &mut Score) {
    e.set_patch(0, &presets::SAW_LEAD);
    s.note(0.0, 0.9, 0, 52, 100);
    for i in 0..180u16 {
        let t = f32::from(i) * 0.005;
        let x = f32::from(i) / 180.0;
        s.param(t, 0, Param::Cutoff, 200.0 + 7800.0 * x * x);
        s.param(t, 0, Param::Pan, 2.0 * x - 1.0);
    }
    s.param(0.5, 0, Param::Bend, 1.0);
    s.param(0.7, 0, Param::Bend, -0.5);
}

/// Four seconds of everything: kit, FM bass, pad with chorus, lead with
/// delay, reverb on the snare, a game effect.
fn demo_song(e: &mut Engine, s: &mut Score) {
    e.set_patch(9, &Patch::KIT);
    e.set_patch(1, &presets::FM_BASS);
    e.set_patch(2, &presets::PWM_PAD);
    e.set_patch(3, &presets::SAW_LEAD);
    e.set_patch(4, &presets::SWEEP_FX);
    e.set_patch(5, &presets::FM_EPIANO);
    s.param(0.0, 9, Param::ReverbSend, 0.25);
    s.param(0.0, 2, Param::ChorusSend, 0.6);
    s.param(0.0, 2, Param::Volume, 0.5);
    s.param(0.0, 3, Param::DelaySend, 0.4);
    s.param(0.0, 3, Param::Pan, 0.3);
    s.param(0.0, 5, Param::Pan, -0.4);
    s.param(0.0, 5, Param::ReverbSend, 0.3);
    let step = 0.125; // sixteenths at 120 BPM
    let bass = [36u8, 36, 48, 36, 43, 36, 46, 48];
    for bar in 0..2u8 {
        let t0 = f32::from(bar) * 2.0;
        for i in 0..16u8 {
            let t = t0 + f32::from(i) * step;
            if i % 4 == 0 {
                s.note(t, 0.05, 9, 36, 120);
            }
            if i % 8 == 4 {
                s.note(t, 0.05, 9, 38, 105);
            }
            s.note(t, 0.03, 9, if i == 14 { 46 } else { 42 }, 60 + 10 * (i % 2));
            if i % 2 == 0 {
                s.note(t, step * 1.6, 1, bass[usize::from(i / 2)] - 12, 100);
            }
        }
        let chord: [u8; 3] = if bar == 0 { [60, 63, 67] } else { [58, 62, 65] };
        for n in chord {
            s.note(t0, 1.9, 2, n, 80);
        }
        for (k, n) in [72u8, 75, 79, 77].iter().enumerate() {
            s.note(t0 + 0.5 * k as f32, 0.3, 3, *n, 90);
        }
        s.note(t0 + 1.0, 0.4, 5, 84, 70);
        s.note(t0 + 1.0, 0.4, 5, 79, 60);
    }
    s.note(3.5, 0.3, 4, 84, 100);
    s.push(3.9, EventKind::Master(0.5));
}

const fn m(name: &'static str, i: usize) -> Scenario {
    Scenario { name, rate: 48_000, seconds: 0.9, setup: Setup::Melodic(i) }
}

const fn d(name: &'static str, i: usize) -> Scenario {
    Scenario { name, rate: 48_000, seconds: 0.6, setup: Setup::Drum(i) }
}

const fn c(name: &'static str, seconds: f32, f: fn(&mut Engine, &mut Score)) -> Scenario {
    Scenario { name, rate: 48_000, seconds, setup: Setup::Custom(f) }
}

const fn k(name: &'static str, i: usize) -> Scenario {
    Scenario { name, rate: 48_000, seconds: 33.0 * STEP + 0.6, setup: Setup::Kit(i) }
}

/// The golden scenarios.
pub const SCENARIOS: [Scenario; 45] = [
    m("phrase/saw-lead", 0),
    m("phrase/square-bass", 1),
    m("phrase/pwm-pad", 2),
    m("phrase/tri-flute", 3),
    m("phrase/organ", 4),
    m("phrase/vocal", 5),
    m("phrase/pluck", 6),
    m("phrase/fm-epiano", 7),
    m("phrase/fm-bell", 8),
    m("phrase/fm-bass", 9),
    m("phrase/sweep-fx", 10),
    m("phrase/noise-hit", 11),
    d("drum/kick", 0),
    d("drum/snare", 1),
    d("drum/closed-hat", 2),
    d("drum/open-hat", 3),
    d("drum/clap", 4),
    d("drum/tom", 5),
    c("kit-groove", 1.5, kit_groove),
    c("glide-mono", 1.3, glide_mono),
    c("steal", 1.0, steal),
    c("fx-delay", 1.5, fx_delay),
    c("fx-chorus", 1.2, fx_chorus),
    c("fx-reverb", 2.0, fx_reverb),
    m("phrase/acid-bass", 12),
    m("phrase/strings", 13),
    m("phrase/brass", 14),
    m("phrase/sub-bass", 15),
    d("drum/rim", 6),
    d("drum/cowbell", 7),
    Scenario { name: "drum/cymbal", rate: 48_000, seconds: 2.5, setup: Setup::Drum(8) },
    d("drum/clave", 9),
    d("drum/shaker", 10),
    k("kit/standard", 0),
    k("kit/analog", 1),
    k("kit/punch", 2),
    k("kit/tight", 3),
    k("kit/percussion", 4),
    c("fx-delay-sync", 3.5, fx_delay_sync),
    c("fx-delay-long", 4.0, fx_delay_long),
    c("fx-reverb-hall", 5.0, fx_reverb_hall),
    c("lfo-sync", 1.0, lfo_sync),
    c("param-sweep", 1.0, param_sweep),
    c("demo-song", 4.0, demo_song),
    Scenario { name: "demo-song@44k1", rate: 44_100, seconds: 4.0, setup: Setup::Custom(demo_song) },
];

/// FNV-1a (64-bit) of the samples' little-endian bytes: the golden hash.
#[must_use]
pub fn hash_i16(samples: &[i16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for s in samples {
        for b in s.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}
