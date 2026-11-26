//! The engine: parts, voice allocation, sample-accurate events, mixing,
//! send effects, master limiter, rendering.
//!
//! # Rendering and events
//!
//! [`Engine::render_f32`] and [`Engine::render_i16`] fill an interleaved
//! stereo buffer and take the events of that block, each with a sample
//! `offset` from the block start. The block is cut into segments at every
//! event offset and at every control tick (each [`CONTROL_PERIOD`] samples
//! of the engine's running sample clock); an event applies before the
//! sample at its offset is computed, and a voice it touches gets its control
//! update on that same sample. So every event lands on its exact sample and
//! the output does not depend on how the caller cuts the stream into blocks
//! (both are tested).
//!
//! Events should be sorted by offset; one with a smaller offset than its
//! predecessor applies at the predecessor's offset. Offsets past the block
//! apply after its last sample (that is, at the start of the next block).
//!
//! # Voices
//!
//! [`MAX_VOICES`] slots (the limit can be lowered with
//! [`Engine::set_voice_limit`]) plus [`TAILS`] hidden slots where a stolen
//! voice fades out over 256 samples (5.3 ms at 48 kHz), so the new note can
//! start on its exact sample without a click. A note-on takes, in order: the slot already
//! playing the same note on the same part (retrigger), a free slot, the
//! quietest voice that is released or below −40 dB, else the oldest voice.
//! A mono part keeps one voice and a stack of the held notes (last-note
//! priority); with legato, notes played while another is held glide without
//! retriggering, and releasing the top note returns to the one below.

use crate::drum::DrumParams;
use crate::fx::{Chorus, ChorusParams, Delay, DelayParams, Limiter, Reverb, ReverbParams};
use crate::kit::{KIT_SLOTS, Kit, KitSound};
use crate::load::{Costs, Estimate, FX_ALL};
use crate::math::{Rng, clampf, to_i16};
use crate::patch::{Kind, Patch, PatchCoef};
use crate::sample::{HitParams, SampleCache, Want};
use crate::voice::{Ctx, PartView, Voice, pan_gains};

/// Voice slots.
pub const MAX_VOICES: usize = 16;
/// Parts (MIDI channels, sequencer tracks).
pub const PARTS: usize = 16;
/// Hidden slots for fading out stolen voices.
pub const TAILS: usize = 4;
/// Samples per control period (0.67 ms at 48 kHz). 16 cost about twice the
/// control work per voice-sample (pitch, filter design, envelopes) for no
/// audible gain.
pub const CONTROL_PERIOD: usize = 32;
/// Lowest and highest engine sample rate, Hz.
pub const RATE_RANGE: (u32, u32) = (8000, 48_000);
/// Held notes a mono part remembers.
pub const HELD: usize = 8;

// The index helpers below mask with these (identity for the values every
// caller has checked already): the compiler then sees the bounds, and the
// player's image has no panic path here (its location data would be
// resident, SYN-3).
const _: () = assert!(PARTS.is_power_of_two() && MAX_VOICES.is_power_of_two() && TAILS.is_power_of_two());

/// Part index of a checked part number.
#[inline(always)]
fn pi(part: u8) -> usize {
    usize::from(part) & (PARTS - 1)
}

/// A voice slot index from `alloc` (< the limit ≤ [`MAX_VOICES`]).
#[inline(always)]
fn vi(i: usize) -> usize {
    i & (MAX_VOICES - 1)
}

/// A per-part parameter changed by [`EventKind::Param`].
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Param {
    /// Part volume, 0–1.
    Volume,
    /// Part pan, −1 (left) to 1 (right).
    Pan,
    /// Filter cutoff, Hz.
    Cutoff,
    /// Filter resonance, 0–1.
    Resonance,
    /// Pitch bend, −1 to 1 (× the patch's bend range).
    Bend,
    /// Glide time, seconds.
    Glide,
    /// LFO rate, Hz.
    LfoRate,
    /// LFO pitch depth, semitones.
    LfoPitch,
    /// LFO cutoff depth, octaves.
    LfoCutoff,
    /// Sustain pedal: ≥ 0.5 down.
    Sustain,
    /// Patch level, 0–1.
    Level,
    /// Send to the delay, 0–1.
    DelaySend,
    /// Send to the chorus, 0–1.
    ChorusSend,
    /// Send to the reverb, 0–1.
    ReverbSend,
    /// Drum hits of the part from now on: pitch offset, semitones (−48 to
    /// 48), added to the kit entry's or drum patch's tune. The Sequencer's
    /// per-step locks (SEQ-2) set it just before a note-on and back to 0
    /// just after, at the same sample. Sounding hits do not change.
    DrumTune,
    /// Drum hits from now on: decay scale (0.05–20), multiplying the
    /// entry's (1: unchanged).
    DrumDecay,
    /// Drum hits from now on: tone offset (−1 to 1), added to the entry's.
    DrumTone,
}

/// An effect parameter changed by [`EventKind::Fx`].
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FxParam {
    /// Delay time, seconds (sets beats to 0).
    DelayTime,
    /// Delay time in beats (0: use the time in seconds).
    DelayBeats,
    /// Delay feedback, 0–0.95.
    DelayFeedback,
    /// Delay damping, 0–0.95.
    DelayDamp,
    /// High pass of the delay's repeats, Hz (below 20: off).
    DelayLowCut,
    /// Delay wet level, 0–1.
    DelayLevel,
    /// Chorus rate, Hz.
    ChorusRate,
    /// Chorus depth, 0–1.
    ChorusDepth,
    /// Chorus wet level, 0–1.
    ChorusLevel,
    /// Reverb decay time, seconds.
    ReverbDecay,
    /// Reverb damping, 0–0.9.
    ReverbDamp,
    /// Reverb wet level, 0–1.
    ReverbLevel,
}

/// What an event does.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum EventKind {
    /// Start a note (velocity 0 is a note-off).
    NoteOn {
        /// Part, 0–15.
        part: u8,
        /// MIDI note, 0–127.
        note: u8,
        /// Velocity, 1–127.
        velocity: u8,
    },
    /// Release a note.
    NoteOff {
        /// Part.
        part: u8,
        /// MIDI note.
        note: u8,
    },
    /// Change a part parameter.
    Param {
        /// Part.
        part: u8,
        /// Which.
        param: Param,
        /// New value (clamped to the parameter's range; NaN ignored).
        value: f32,
    },
    /// Change an effect parameter.
    Fx {
        /// Which.
        param: FxParam,
        /// New value.
        value: f32,
    },
    /// Release every note of a part (`0xFF`: every part).
    AllNotesOff {
        /// Part.
        part: u8,
    },
    /// Fade every voice out within 256 samples and clear the effects.
    AllSoundOff,
    /// Tempo, BPM (20–300).
    Tempo(f32),
    /// Master gain, 0–2 (ramps over one control period from its sample).
    Master(f32),
}

/// An event at a sample offset within a render block.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct Event {
    /// Sample (frame) offset from the start of the block.
    pub offset: u32,
    /// What happens.
    pub kind: EventKind,
}

impl Event {
    /// Note-on at `offset`.
    #[must_use]
    pub const fn note_on(offset: u32, part: u8, note: u8, velocity: u8) -> Self {
        Self { offset, kind: EventKind::NoteOn { part, note, velocity } }
    }

    /// Note-off at `offset`.
    #[must_use]
    pub const fn note_off(offset: u32, part: u8, note: u8) -> Self {
        Self { offset, kind: EventKind::NoteOff { part, note } }
    }

    /// Part parameter change at `offset`.
    #[must_use]
    pub const fn param(offset: u32, part: u8, param: Param, value: f32) -> Self {
        Self { offset, kind: EventKind::Param { part, param, value } }
    }
}

/// Read-only view of a sounding voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceInfo {
    /// Slot (≥ [`MAX_VOICES`]: a fading tail).
    pub slot: usize,
    /// Part.
    pub part: u8,
    /// Note.
    pub note: u8,
    /// Gate closed.
    pub released: bool,
    /// Fading out (stolen or cut).
    pub fading: bool,
    /// Current level (envelope × velocity).
    pub level: f32,
    /// Allocation order.
    pub stamp: u64,
}

#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
struct Part {
    patch: Patch,
    coef: PatchCoef,
    volume: f32,
    pan: f32,
    pan_g: (f32, f32),
    bend: f32,
    sends: [f32; 3],
    sustain: bool,
    held: [u8; HELD],
    n_held: u8,
    /// The last note played (glide start), [`NO_NOTE`] for none (a
    /// plain byte: the engine is valid all-zero, see [`Engine::init`]).
    last_note: u8,
    /// Drum hit offsets: tune (semitones), decay scale, tone
    /// ([`Param::DrumTune`] …).
    drum: [f32; 3],
}

/// Neutral drum offsets.
const DRUM_NEUTRAL: [f32; 3] = [0.0, 1.0, 0.0];
/// No last note (`Part::last_note`).
const NO_NOTE: u8 = 0xFF;

impl Part {
    /// A part holding [`Patch::INIT`], in place.
    fn init(&mut self, rate: f32) {
        self.patch = Patch::INIT;
        self.coef = PatchCoef::new(&Patch::INIT, rate);
        self.volume = 0.8;
        self.pan = 0.0;
        self.pan_g = pan_gains(0.0);
        self.bend = 0.0;
        self.sends = [0.0; 3];
        self.sustain = false;
        self.held = [0; HELD];
        self.n_held = 0;
        self.last_note = NO_NOTE;
        self.drum = DRUM_NEUTRAL;
    }

    /// The last note played, if any (glide start).
    fn last(&self) -> Option<f32> {
        (self.last_note != NO_NOTE).then(|| f32::from(self.last_note))
    }

    fn view(&self) -> PartView<'_> {
        PartView {
            patch: &self.patch,
            coef: &self.coef,
            volume: self.volume,
            pan: self.pan_g,
            pan_pos: self.pan,
            bend: self.bend,
        }
    }

    fn hold(&mut self, note: u8) {
        self.unhold(note);
        if usize::from(self.n_held) == HELD {
            self.held.copy_within(1.., 0);
            self.n_held -= 1;
        }
        self.held[usize::from(self.n_held) & (HELD - 1)] = note;
        self.n_held += 1;
    }

    fn unhold(&mut self, note: u8) {
        let n = usize::from(self.n_held).min(HELD);
        if let Some(i) = self.held.iter().take(n).position(|&h| h == note) {
            for k in i + 1..n {
                self.held[(k - 1) & (HELD - 1)] = self.held[k & (HELD - 1)];
            }
            self.n_held -= 1;
        }
    }

    fn top(&self) -> Option<u8> {
        self.n_held.checked_sub(1).and_then(|i| self.held.get(usize::from(i)).copied())
    }
}

#[derive(bytemuck::Zeroable, Clone, Copy, Debug)]
struct Bus {
    l: [f32; CONTROL_PERIOD],
    r: [f32; CONTROL_PERIOD],
    send: [[f32; CONTROL_PERIOD]; 3],
    voice: [f32; CONTROL_PERIOD],
    /// An effect fading out renders here first ([`Engine::set_fx_enabled`]).
    wet: [[f32; CONTROL_PERIOD]; 2],
}

/// Samples over which an effect switched off fades out.
pub const FX_FADE: u16 = 256;

/// Add `wl`/`wr` to `l`/`r` with a gain falling from `fade` / [`FX_FADE`]
/// by one step per sample; returns the samples of the fade left.
fn add_fading_wet(wl: &[f32], wr: &[f32], l: &mut [f32], r: &mut [f32], mut fade: u16) -> u16 {
    for (((l, r), a), b) in l.iter_mut().zip(r.iter_mut()).zip(wl).zip(wr) {
        let g = f32::from(fade) * (1.0 / f32::from(FX_FADE));
        *l += a * g;
        *r += b * g;
        fade = fade.saturating_sub(1);
    }
    fade
}

/// `d` with a part's drum offsets (unchanged, bit for bit, when they are
/// neutral).
fn drum_offset(d: DrumParams, off: [f32; 3]) -> DrumParams {
    if off == DRUM_NEUTRAL {
        return d;
    }
    DrumParams { tune: d.tune + off[0], decay: d.decay * off[1], tone: d.tone + off[2], ..d }.sanitized()
}

/// A sample hit with a part's drum offsets.
fn hit_offset(h: HitParams, off: [f32; 3]) -> HitParams {
    if off == DRUM_NEUTRAL {
        return h;
    }
    HitParams { tune: h.tune + off[0], decay: h.decay * off[1], tone: h.tone + off[2], ..h }.sanitized()
}

/// The synthesis engine. All memory is inside this value
/// ([`crate::ram`] lists it): no allocation, no global state.
#[derive(bytemuck::Zeroable, Clone, Debug)]
pub struct Engine {
    rate: u32,
    rate_f: f32,
    parts: [Part; PARTS],
    voices: [Voice; MAX_VOICES],
    tails: [Voice; TAILS],
    voice_limit: usize,
    clock: u64,
    stamp: u64,
    rng: Rng,
    tempo: f32,
    master: f32,
    master_cur: f32,
    master_step: f32,
    master_ramp: u8,
    bus: Bus,
    out: [[f32; 2]; CONTROL_PERIOD],
    delay: Delay,
    chorus: Chorus,
    reverb: Reverb,
    limiter: Limiter,
    kits: [Kit; KIT_SLOTS],
    misses: u32,
    /// The effects enabled (`load::FX_*`; the load governor switches them).
    fx_on: u8,
    /// Samples left of an effect's fade-out after it was switched off.
    fx_fade: [u16; 3],
}

impl Engine {
    /// An engine at `rate` Hz (clamped to [`RATE_RANGE`]) whose random
    /// sources (noise, sample-and-hold) start from `seed`. Every part holds
    /// [`Patch::INIT`]; the effects have their defaults with no sends.
    ///
    /// The value is about 62 KB: on the player use [`Engine::init`] on
    /// memory that holds an engine already, or on zeroed memory (an
    /// all-zero `Engine` is a valid value: `bytemuck::Zeroable`).
    #[must_use]
    pub fn new(rate: u32, seed: u32) -> Self {
        let mut e = <Self as bytemuck::Zeroable>::zeroed();
        e.init(rate, seed);
        e
    }

    /// Make this engine what [`Engine::new`]`(rate, seed)` returns, in
    /// place: field by field, no temporary larger than a part (≈ 1 KiB),
    /// so the player can build it in the decoder arena (zeroed first) from
    /// a task's small stack. `new` is this on a zeroed value: one code path.
    pub fn init(&mut self, rate: u32, seed: u32) {
        let rate = rate.clamp(RATE_RANGE.0, RATE_RANGE.1);
        let rate_f = rate as f32;
        self.rate = rate;
        self.rate_f = rate_f;
        for p in &mut self.parts {
            p.init(rate_f);
        }
        self.voices.fill(Voice::IDLE);
        self.tails.fill(Voice::IDLE);
        self.voice_limit = MAX_VOICES;
        self.clock = 0;
        self.stamp = 0;
        self.rng = Rng::new(seed);
        self.tempo = 120.0;
        self.master = 1.0;
        self.master_cur = 1.0;
        self.master_step = 0.0;
        self.master_ramp = 0;
        self.bus.l.fill(0.0);
        self.bus.r.fill(0.0);
        for s in &mut self.bus.send {
            s.fill(0.0);
        }
        self.bus.voice.fill(0.0);
        for w in &mut self.bus.wet {
            w.fill(0.0);
        }
        self.out.fill([0.0; 2]);
        self.delay.init();
        self.chorus.init();
        self.reverb.init();
        self.limiter = Limiter::new(rate_f);
        for k in &mut self.kits {
            *k = Kit::new(crate::kit::STANDARD);
        }
        self.misses = 0;
        self.fx_on = FX_ALL;
        self.fx_fade = [0; 3];
        self.delay.set(&DelayParams::DEFAULT, rate_f, self.tempo);
        self.chorus.set(&ChorusParams::DEFAULT, rate_f);
        self.reverb.set_rate(rate_f);
    }

    /// Silence everything and restart the clock and the random sources from
    /// `seed`; patches, part volumes, pans, sends and effect settings stay.
    pub fn reset(&mut self, seed: u32) {
        self.voices.fill(Voice::IDLE);
        self.tails.fill(Voice::IDLE);
        for p in &mut self.parts {
            p.bend = 0.0;
            p.sustain = false;
            p.n_held = 0;
            p.last_note = NO_NOTE;
            p.drum = DRUM_NEUTRAL;
        }
        self.clock = 0;
        self.stamp = 0;
        self.rng = Rng::new(seed);
        self.delay.clear();
        self.chorus.clear();
        self.reverb.clear();
        self.limiter = Limiter::new(self.rate_f);
        self.master_cur = self.master;
        self.master_ramp = 0;
        self.misses = 0;
        self.fx_on &= !self.fading();
        self.fx_fade = [0; 3];
    }

    /// [`Engine::reset`], then set the sample clock to `clock`: a player
    /// that restarts from silence at frame `clock` of a piece (a MIDI seek)
    /// keeps the control ticks on the same samples as a render from the
    /// start, so the two outputs agree once the state they carry agrees.
    pub fn reset_at(&mut self, seed: u32, clock: u64) {
        self.reset(seed);
        self.clock = clock;
    }

    /// Sample rate, Hz.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Samples rendered since creation or [`Engine::reset`].
    #[must_use]
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// Install `patch` on `part` (ignored past [`PARTS`]). Values are
    /// clamped; the part's sounding voices fade out within 256 samples.
    pub fn set_patch(&mut self, part: u8, patch: &Patch) {
        let Some(p) = self.parts.get_mut(usize::from(part)) else { return };
        p.patch = patch.sanitized();
        p.coef = PatchCoef::new(&p.patch, self.rate_f);
        p.n_held = 0;
        for v in self.voices.iter_mut().chain(self.tails.iter_mut()) {
            if v.active && v.part == part {
                v.cut();
            }
        }
    }

    /// The patch of `part`, as installed (sanitised).
    #[must_use]
    pub fn patch(&self, part: u8) -> Option<&Patch> {
        self.parts.get(usize::from(part)).map(|p| &p.patch)
    }

    /// Install `kit` in kit slot `slot` (ignored past [`KIT_SLOTS`]); the
    /// kit patches of that slot play it from their next note (sounding
    /// hits play out).
    pub fn set_kit(&mut self, slot: u8, kit: &Kit) {
        if let Some(k) = self.kits.get_mut(usize::from(slot)) {
            *k = *kit;
        }
    }

    /// The kit in `slot`.
    #[must_use]
    pub fn kit(&self, slot: u8) -> Option<&Kit> {
        self.kits.get(usize::from(slot))
    }

    /// The play heads of the voices playing samples, for
    /// [`SampleCache::service`]: fills `out`, returns how many.
    pub fn sample_wants(&self, out: &mut [Want]) -> usize {
        let mut n = 0;
        for w in self.voices.iter().chain(&self.tails).filter(|v| v.active).filter_map(Voice::want) {
            if n == out.len() {
                break;
            }
            out[n] = w;
            n += 1;
        }
        n
    }

    /// Samples played as silence because a voice's sample page was not in
    /// the cache, since creation or [`Engine::reset`].
    #[must_use]
    pub fn sample_misses(&self) -> u32 {
        self.misses
    }

    /// Use at most `n` voice slots (1–16). Voices above the limit play out.
    pub fn set_voice_limit(&mut self, n: usize) {
        self.voice_limit = n.clamp(1, MAX_VOICES);
    }

    /// Enable the send effects of `mask` (`load::FX_*`) and switch the
    /// others off (the load governor, [`crate::load`]). One switched off
    /// fades out over [`FX_FADE`] samples and is then cleared, and its
    /// sends are ignored while it is off; one switched on starts silent.
    /// With every effect on (the default) the render is unchanged.
    pub fn set_fx_enabled(&mut self, mask: u8) {
        for i in 0..3 {
            let bit = 1u8 << i;
            let on = self.fx_on & bit != 0 && self.fx_fade[i] == 0;
            if mask & bit != 0 && !on {
                // Back on (also in the middle of its fade: the rest of
                // the tail plays at full level).
                self.fx_on |= bit;
                self.fx_fade[i] = 0;
            } else if mask & bit == 0 && on {
                self.fx_fade[i] = FX_FADE;
            }
        }
    }

    /// The effects enabled and not fading out (`load::FX_*`).
    #[must_use]
    pub fn fx_enabled(&self) -> u8 {
        self.fx_on & !self.fading()
    }

    /// The effects fading out.
    fn fading(&self) -> u8 {
        (0..3).filter(|&i| self.fx_fade[i] != 0).fold(0, |m, i| m | 1 << i)
    }

    /// What rendering costs now by the table `c` (cycles per frame): the
    /// sounding voices by class, the effects that run (enabled, and fed by
    /// a sounding voice's part or still ringing). Out of line: the player
    /// calls it before and after every piece, and one copy is a line of
    /// the code cache less than one inlined into each caller (SYN-3).
    #[must_use]
    #[inline(never)]
    pub fn estimate(&self, c: &Costs) -> Estimate {
        let mut e = Estimate { base: c.empty, fx: c.fx, ..Estimate::default() };
        let mut fed = 0u8;
        for v in self.voices.iter().chain(&self.tails).filter(|v| v.active) {
            let p = &self.parts[pi(v.part)];
            let cost = c.voice(v.class(&p.patch));
            e.voices += cost;
            e.n += 1;
            if v.fade > 0 {
                e.fading += cost;
                e.n_fading += 1;
            }
            for (i, s) in p.sends.iter().enumerate() {
                if *s > 0.0 {
                    fed |= 1 << i;
                }
            }
        }
        let ringing = u8::from(!self.delay.is_idle())
            | u8::from(!self.chorus.is_idle()) << 1
            | u8::from(!self.reverb.is_idle()) << 2;
        e.wanted = fed | ringing;
        if !cfg!(feature = "delay") {
            e.wanted &= !crate::load::FX_DELAY;
        }
        // (An effect fading out still costs.)
        e.running = e.wanted & self.fx_on;
        e
    }

    /// Fade out voices until at most `keep` sound (not counting those
    /// fading already): released ones first, oldest first, then the
    /// oldest held ones. Returns how many it faded (the load governor;
    /// out of line: rare, kept out of the player's per-piece code).
    #[inline(never)]
    pub fn shed(&mut self, keep: usize) -> usize {
        let mut cut = 0;
        loop {
            let live = self.voices.iter().filter(|v| v.active && v.fade == 0);
            if live.count() <= keep {
                return cut;
            }
            let pick = self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.active && v.fade == 0)
                .min_by_key(|(_, v)| (!v.released, v.stamp))
                .map(|(i, _)| i);
            let Some(i) = pick else { return cut };
            self.voices[vi(i)].cut();
            cut += 1;
        }
    }

    /// Delay settings.
    pub fn set_delay(&mut self, p: &DelayParams) {
        self.delay.set(p, self.rate_f, self.tempo);
    }

    /// Chorus settings.
    pub fn set_chorus(&mut self, p: &ChorusParams) {
        self.chorus.set(p, self.rate_f);
    }

    /// Reverb settings.
    pub fn set_reverb(&mut self, p: &ReverbParams) {
        self.reverb.set(p);
    }

    /// Current effect settings.
    #[must_use]
    pub fn fx(&self) -> (DelayParams, ChorusParams, ReverbParams) {
        (self.delay.params(), self.chorus.params(), self.reverb.params())
    }

    /// Tempo, BPM.
    #[must_use]
    pub fn tempo(&self) -> f32 {
        self.tempo
    }

    /// Current limiter gain (1 = not limiting).
    #[must_use]
    pub fn limiter_gain(&self) -> f32 {
        self.limiter.gain()
    }

    /// Sounding voices (including fading tails).
    #[must_use]
    pub fn active_voices(&self) -> usize {
        self.voices.iter().chain(&self.tails).filter(|v| v.active).count()
    }

    /// The sounding voices.
    pub fn voices(&self) -> impl Iterator<Item = VoiceInfo> + '_ {
        self.voices.iter().chain(&self.tails).enumerate().filter(|(_, v)| v.active).map(|(slot, v)| {
            VoiceInfo {
                slot,
                part: v.part,
                note: v.note,
                released: v.released,
                fading: v.fade > 0,
                level: v.level(),
                stamp: v.stamp,
            }
        })
    }

    /// Apply an event now (between render calls).
    pub fn apply(&mut self, e: EventKind) {
        match e {
            EventKind::NoteOn { part, note, velocity } => self.note_on(part, note, velocity),
            EventKind::NoteOff { part, note } => self.note_off(part, note),
            EventKind::Param { part, param, value } => self.set_param(part, param, value),
            EventKind::Fx { param, value } => self.set_fx(param, value),
            EventKind::AllNotesOff { part } => {
                for p in 0..PARTS as u8 {
                    if part == 0xFF || part == p {
                        self.release_part(p);
                    }
                }
            }
            EventKind::AllSoundOff => {
                for v in self.voices.iter_mut().chain(self.tails.iter_mut()) {
                    if v.active {
                        v.cut();
                    }
                }
                for p in &mut self.parts {
                    p.n_held = 0;
                }
                self.delay.clear();
                self.chorus.clear();
                self.reverb.clear();
            }
            EventKind::Tempo(bpm) => {
                if !bpm.is_nan() {
                    self.tempo = clampf(bpm, 20.0, 300.0);
                    let d = self.delay.params();
                    self.delay.set(&d, self.rate_f, self.tempo);
                }
            }
            EventKind::Master(g) => {
                if !g.is_nan() {
                    self.master = clampf(g, 0.0, 2.0);
                    self.master_step = (self.master - self.master_cur) * (1.0 / CONTROL_PERIOD as f32);
                    self.master_ramp = CONTROL_PERIOD as u8;
                }
            }
        }
    }

    fn set_param(&mut self, part: u8, param: Param, value: f32) {
        if value.is_nan() {
            return;
        }
        let Some(p) = self.parts.get_mut(usize::from(part)) else { return };
        let v = value;
        // Drum offsets act at the next note-on only: no voice changes.
        let drum = match param {
            Param::DrumTune => Some((0, clampf(v, -48.0, 48.0))),
            Param::DrumDecay => Some((1, clampf(v, 0.05, 20.0))),
            Param::DrumTone => Some((2, clampf(v, -1.0, 1.0))),
            _ => None,
        };
        if let Some((i, x)) = drum {
            p.drum[i] = x;
            return;
        }
        match param {
            Param::Volume => p.volume = clampf(v, 0.0, 1.0),
            Param::Pan => {
                p.pan = clampf(v, -1.0, 1.0);
                p.pan_g = pan_gains(p.pan);
            }
            Param::Cutoff => p.patch.filter.cutoff = clampf(v, crate::filter::MIN_CUTOFF, 24_000.0),
            Param::Resonance => p.patch.filter.resonance = clampf(v, 0.0, 1.0),
            Param::Bend => p.bend = clampf(v, -1.0, 1.0),
            Param::Glide => p.patch.glide = clampf(v, 0.0, 5.0),
            Param::LfoRate => p.patch.lfo.rate = crate::lfo::LfoRate::Hz(clampf(v, 0.0, 50.0)),
            Param::LfoPitch => p.patch.lfo.pitch = clampf(v, -24.0, 24.0),
            Param::LfoCutoff => p.patch.lfo.cutoff = clampf(v, -8.0, 8.0),
            Param::Level => p.patch.level = clampf(v, 0.0, 1.0),
            Param::DelaySend => p.sends[0] = clampf(v, 0.0, 1.0),
            Param::ChorusSend => p.sends[1] = clampf(v, 0.0, 1.0),
            Param::ReverbSend => p.sends[2] = clampf(v, 0.0, 1.0),
            Param::DrumTune | Param::DrumDecay | Param::DrumTone => {}
            Param::Sustain => {
                let down = v >= 0.5;
                p.sustain = down;
                if !down {
                    let coef = p.coef;
                    let mono = p.patch.mono && p.n_held > 0;
                    for vo in self.voices.iter_mut() {
                        if vo.active && vo.part == part && vo.sustained && !mono {
                            vo.sustained = false;
                            vo.release(&coef);
                        }
                    }
                }
            }
        }
        for vo in self.voices.iter_mut().chain(self.tails.iter_mut()) {
            if vo.active && vo.part == part {
                vo.dirty = true;
            }
        }
    }

    fn set_fx(&mut self, param: FxParam, v: f32) {
        if v.is_nan() {
            return;
        }
        let (mut d, mut c, mut r) = self.fx();
        match param {
            FxParam::DelayTime => {
                d.time = v;
                d.beats = 0.0;
            }
            FxParam::DelayBeats => d.beats = v,
            FxParam::DelayFeedback => d.feedback = v,
            FxParam::DelayDamp => d.damp = v,
            FxParam::DelayLowCut => d.low_cut = v,
            FxParam::DelayLevel => d.level = v,
            FxParam::ChorusRate => c.rate = v,
            FxParam::ChorusDepth => c.depth = v,
            FxParam::ChorusLevel => c.level = v,
            FxParam::ReverbDecay => r.decay = v,
            FxParam::ReverbDamp => r.damp = v,
            FxParam::ReverbLevel => r.level = v,
        }
        self.delay.set(&d, self.rate_f, self.tempo);
        self.chorus.set(&c, self.rate_f);
        self.reverb.set(&r);
    }

    fn release_part(&mut self, part: u8) {
        let Some(p) = self.parts.get_mut(usize::from(part)) else { return };
        p.n_held = 0;
        p.sustain = false;
        let coef = p.coef;
        for v in &mut self.voices {
            if v.active && v.part == part {
                v.sustained = false;
                v.release(&coef);
            }
        }
    }

    fn vel_gain(p: &Patch, velocity: u8) -> f32 {
        let v = f32::from(velocity.min(127)) * (1.0 / 127.0);
        1.0 - p.velocity + p.velocity * v * v
    }

    fn note_on(&mut self, part: u8, note: u8, velocity: u8) {
        if velocity == 0 {
            return self.note_off(part, note);
        }
        let Some(p) = self.parts.get(usize::from(part)) else { return };
        if note > 127 {
            return;
        }
        let off = p.drum;
        match p.patch.kind {
            Kind::Drum(d) => self.drum_on(part, Ok(&drum_offset(d, off)), note, velocity, 0, 0.0),
            Kind::Kit(slot) => {
                let Some(e) = self.kits.get(usize::from(slot)).and_then(|k| k.find(note)).copied() else {
                    return;
                };
                match e.decode() {
                    Some(KitSound::Drum(d)) => {
                        self.drum_on(part, Ok(&drum_offset(d, off)), note, velocity, e.choke, e.pan_f());
                    }
                    Some(KitSound::Sample(h)) => {
                        self.drum_on(part, Err(&hit_offset(h, off)), note, velocity, e.choke, e.pan_f());
                    }
                    None => {}
                }
            }
            _ if p.patch.mono => self.mono_on(part, note, velocity),
            _ => self.poly_on(part, note, velocity),
        }
    }

    fn next_stamp(&mut self) -> u64 {
        self.stamp += 1;
        self.stamp
    }

    /// Start a drum (`Ok`) or a sample hit (`Err`); a choke group ≠ 0
    /// first chokes the part's sounding voices of that group.
    fn drum_on(
        &mut self,
        part: u8,
        sound: Result<&DrumParams, &HitParams>,
        note: u8,
        velocity: u8,
        group: u8,
        pan: f32,
    ) {
        let p = &self.parts[pi(part)];
        let g = Self::vel_gain(&p.patch, velocity);
        if group != 0 {
            for v in self.voices.iter_mut().chain(self.tails.iter_mut()) {
                if v.active && v.part == part && v.group == group {
                    v.choke(self.rate_f);
                }
            }
        }
        let i = self.alloc(part, note);
        let stamp = self.next_stamp();
        let seed = self.rng.next_u32();
        self.voices[vi(i)].start_drum(part, sound, note, g, group, pan, stamp, seed, self.rate_f);
    }

    fn poly_on(&mut self, part: u8, note: u8, velocity: u8) {
        let p = &self.parts[pi(part)];
        let from = if p.patch.glide > 0.0 { p.last() } else { None };
        let i = self.alloc(part, note);
        let stamp = self.next_stamp();
        let seed = self.rng.next_u32();
        let p = &mut self.parts[pi(part)];
        self.voices[vi(i)].start(part, &p.patch, note, velocity, from, stamp, seed, self.rate_f);
        p.last_note = note;
    }

    fn mono_voice(&self, part: u8) -> Option<usize> {
        self.voices.iter().position(|v| v.active && v.fade == 0 && v.part == part && !v.is_drum())
    }

    fn mono_on(&mut self, part: u8, note: u8, velocity: u8) {
        let rate = self.rate_f;
        let had_held = self.parts[pi(part)].n_held > 0;
        self.parts[pi(part)].hold(note);
        let found = self.mono_voice(part);
        let p = &self.parts[pi(part)];
        let glide = p.patch.glide > 0.0;
        if let Some(i) = found {
            let v = &mut self.voices[vi(i)];
            if p.patch.legato && had_held && !v.released {
                v.change_note(&p.patch, note, None, false, glide, rate);
            } else {
                v.change_note(&p.patch, note, Some(velocity), true, glide && !p.patch.legato, rate);
            }
            v.stamp = self.stamp + 1;
            self.stamp += 1;
        } else {
            let from = if glide && !p.patch.legato { p.last() } else { None };
            let i = self.alloc(part, note);
            let stamp = self.next_stamp();
            let seed = self.rng.next_u32();
            let p = &self.parts[pi(part)];
            self.voices[vi(i)].start(part, &p.patch, note, velocity, from, stamp, seed, rate);
        }
        self.parts[pi(part)].last_note = note;
    }

    fn note_off(&mut self, part: u8, note: u8) {
        let Some(p) = self.parts.get_mut(usize::from(part)) else { return };
        if p.patch.ignores_note_off() {
            return;
        }
        let rate = self.rate_f;
        if p.patch.mono {
            p.unhold(note);
            let top = p.top();
            let Some(i) =
                self.voices.iter().position(|v| v.active && v.fade == 0 && v.part == part && !v.is_drum())
            else {
                return;
            };
            let p = &self.parts[pi(part)];
            let v = &mut self.voices[vi(i)];
            if v.note != note {
                return;
            }
            match top {
                Some(t) => {
                    v.change_note(&p.patch, t, None, !p.patch.legato, p.patch.glide > 0.0, rate);
                    self.parts[pi(part)].last_note = t;
                }
                None if p.sustain => v.sustained = true,
                None => v.release(&p.coef),
            }
            return;
        }
        let (sustain, coef) = (p.sustain, p.coef);
        for v in &mut self.voices {
            if v.active && v.part == part && v.note == note && !v.released && !v.is_drum() {
                if sustain {
                    v.sustained = true;
                } else {
                    v.release(&coef);
                }
            }
        }
    }

    fn move_to_tail(&mut self, i: usize) {
        let j = self.tails.iter().position(|t| !t.active).unwrap_or_else(|| {
            let mut best = 0;
            for (k, t) in self.tails.iter().enumerate() {
                if t.fade < self.tails[best & (TAILS - 1)].fade {
                    best = k;
                }
            }
            best
        });
        let j = j & (TAILS - 1);
        self.tails[j] = self.voices[vi(i)];
        self.tails[j].cut();
        self.voices[vi(i)].active = false;
    }

    /// A slot for a new note on `part`/`note` (see the module docs).
    fn alloc(&mut self, part: u8, note: u8) -> usize {
        let lim = self.voice_limit.min(MAX_VOICES);
        if let Some(i) = self.voices[..lim]
            .iter()
            .position(|v| v.active && v.fade == 0 && v.part == part && v.note == note)
        {
            self.move_to_tail(i);
            return i;
        }
        if let Some(i) = self.voices[..lim].iter().position(|v| !v.active) {
            return i;
        }
        let mut quiet: Option<(usize, f32, u64)> = None;
        let mut oldest = (0, u64::MAX);
        for (i, v) in self.voices[..lim].iter().enumerate() {
            let level = v.level();
            if v.released || v.fade > 0 || level < 0.01 {
                let better = match quiet {
                    None => true,
                    Some((_, l, s)) => level < l || (level == l && v.stamp < s),
                };
                if better {
                    quiet = Some((i, level, v.stamp));
                }
            }
            if v.stamp < oldest.1 {
                oldest = (i, v.stamp);
            }
        }
        let i = quiet.map_or(oldest.0, |q| q.0);
        self.move_to_tail(i);
        i
    }

    /// Render one segment of `n` ≤ [`CONTROL_PERIOD`] frames into `self.out`.
    fn segment(&mut self, n: usize, cache: Option<&SampleCache<'_>>) {
        let n = n.min(CONTROL_PERIOD);
        let tick = self.clock % CONTROL_PERIOD as u64 == 0;
        let ahead = (CONTROL_PERIOD - (self.clock % CONTROL_PERIOD as u64) as usize) as u32;
        let ctx = Ctx { cache, rate: self.rate_f, tempo: self.tempo, ahead, tick };
        let Self { voices, tails, parts, bus, misses, .. } = self;
        bus.l[..n].fill(0.0);
        bus.r[..n].fill(0.0);
        for s in &mut bus.send {
            s[..n].fill(0.0);
        }
        for v in voices.iter_mut().chain(tails.iter_mut()) {
            if !v.active {
                continue;
            }
            let part = &parts[pi(v.part)];
            let buf = &mut bus.voice[..n];
            let alive = v.render(&part.view(), &ctx, tick, buf);
            v.mix(buf, &mut bus.l[..n], &mut bus.r[..n]);
            for (send, s) in bus.send.iter_mut().zip(part.sends) {
                let g = s * v.gain;
                if g != 0.0 {
                    for (o, x) in send[..n].iter_mut().zip(buf.iter()) {
                        *o += x * g;
                    }
                }
            }
            *misses = misses.wrapping_add(v.take_misses());
            if !alive {
                v.active = false;
            }
        }
        let Self { bus, delay, chorus, reverb, fx_on, fx_fade, .. } = self;
        let Bus { l, r, send, wet, .. } = bus;
        if tick {
            delay.flush();
            reverb.flush();
            self.limiter.flush();
        }
        // The effects in order; one switched off renders into `wet` and is
        // added fading ([`Engine::set_fx_enabled`]); with all on and none
        // fading this is the plain sum, bit for bit.
        let [wl, wr] = wet;
        for (i, (fade, send)) in fx_fade.iter_mut().zip(send.iter()).enumerate() {
            let bit = 1u8 << i;
            if *fx_on & bit == 0 {
                continue;
            }
            let (dl, dr) = if *fade > 0 {
                wl[..n].fill(0.0);
                wr[..n].fill(0.0);
                (&mut wl[..n], &mut wr[..n])
            } else {
                (&mut l[..n], &mut r[..n])
            };
            let input = &send[..n];
            match i {
                0 => delay.process(input, dl, dr),
                1 => chorus.process(input, dl, dr),
                _ => reverb.process(input, dl, dr),
            }
            if *fade > 0 {
                *fade = add_fading_wet(&wl[..n], &wr[..n], &mut l[..n], &mut r[..n], *fade);
                if *fade == 0 {
                    *fx_on &= !bit;
                    match i {
                        0 => delay.clear(),
                        1 => chorus.clear(),
                        _ => reverb.clear(),
                    }
                }
            }
        }
        let Bus { l, r, .. } = &self.bus;
        for ((o, l), r) in self.out[..n].iter_mut().zip(&l[..n]).zip(&r[..n]) {
            if self.master_ramp > 0 {
                self.master_cur += self.master_step;
                self.master_ramp -= 1;
                if self.master_ramp == 0 {
                    self.master_cur = self.master;
                }
            }
            let m = self.master_cur;
            let (a, b) = self.limiter.frame(l * m, r * m);
            *o = [a, b];
        }
        self.clock += n as u64;
    }

    fn run(
        &mut self,
        frames: usize,
        events: &[Event],
        cache: Option<&SampleCache<'_>>,
        mut sink: impl FnMut(usize, [f32; 2]),
    ) {
        let mut pos = 0;
        let mut ei = 0;
        loop {
            while let Some(e) = events.get(ei) {
                if (e.offset as usize).min(frames) > pos {
                    break;
                }
                self.apply(e.kind);
                ei += 1;
            }
            if pos >= frames {
                break;
            }
            let next = events.get(ei).map_or(frames, |e| (e.offset as usize).min(frames));
            let to_tick = CONTROL_PERIOD - (self.clock % CONTROL_PERIOD as u64) as usize;
            let end = next.min(pos + to_tick);
            let n = (end - pos).min(CONTROL_PERIOD);
            self.segment(n, cache);
            for (i, f) in self.out[..n].iter().enumerate() {
                sink(pos + i, *f);
            }
            pos = end;
        }
    }

    /// Render `out.len() / 2` interleaved stereo frames (an odd last sample
    /// is set to 0), applying `events` at their offsets. Samples are in
    /// [−1, 1]. Sample voices are silent (no cache: see
    /// [`Engine::render_f32_with`]).
    pub fn render_f32(&mut self, out: &mut [f32], events: &[Event]) {
        self.f32_out(out, events, None);
    }

    /// The same as 16-bit samples (rounded, no dither).
    pub fn render_i16(&mut self, out: &mut [i16], events: &[Event]) {
        self.i16_out(out, events, None);
    }

    /// [`Engine::render_f32`] with sample voices playing from `cache`.
    /// Call [`SampleCache::service`] with [`Engine::sample_wants`] between
    /// blocks to stream samples that are not wholly pinned.
    pub fn render_f32_with(&mut self, out: &mut [f32], events: &[Event], cache: &SampleCache<'_>) {
        self.f32_out(out, events, Some(cache));
    }

    /// [`Engine::render_i16`] with sample voices playing from `cache`.
    pub fn render_i16_with(&mut self, out: &mut [i16], events: &[Event], cache: &SampleCache<'_>) {
        self.i16_out(out, events, Some(cache));
    }

    fn f32_out(&mut self, out: &mut [f32], events: &[Event], cache: Option<&SampleCache<'_>>) {
        let frames = out.len() / 2;
        if out.len() % 2 == 1 {
            out[out.len() - 1] = 0.0;
        }
        self.run(frames, events, cache, |i, [l, r]| {
            if let Some([a, b]) = out.get_mut(2 * i..2 * i + 2) {
                *a = l;
                *b = r;
            }
        });
    }

    fn i16_out(&mut self, out: &mut [i16], events: &[Event], cache: Option<&SampleCache<'_>>) {
        let frames = out.len() / 2;
        if out.len() % 2 == 1 {
            out[out.len() - 1] = 0;
        }
        self.run(frames, events, cache, |i, [l, r]| {
            if let Some([a, b]) = out.get_mut(2 * i..2 * i + 2) {
                *a = to_i16(l);
                *b = to_i16(r);
            }
        });
    }
}
