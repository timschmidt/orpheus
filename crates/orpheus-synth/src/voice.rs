//! One voice: sources, filter, envelopes, LFO, glide; rendered in segments
//! of at most [`crate::CONTROL_PERIOD`] samples into a mono buffer, which
//! the engine pans and mixes.
//!
//! Control-rate work (pitch to phase increments, filter coefficients, the
//! filter and pitch envelopes, LFO, glide, gains) runs at the start of a
//! segment when the engine's sample clock is on a control tick or when an
//! event touched the voice (`dirty`), and advances by the samples since the
//! previous update. The oscillators, the amplitude envelope, the filter and
//! the FM operators run per sample.

use core::f32::consts::FRAC_PI_4;

use crate::CONTROL_PERIOD;
use crate::drum::{DrumParams, DrumVoice};
use crate::env::Env;
use crate::filter::{FilterMode, Svf};
use crate::fm::FmState;
use crate::lfo::Lfo;
use crate::math::{Rng, clampf, exp2, note_hz};
use crate::osc::Osc;
use crate::patch::{Kind, Patch, PatchCoef};
use crate::sample::{HitParams, SampleCache, SampleHit, SamplePlayer, Want};

/// Samples over which a stolen or cut voice fades out (5.3 ms at 48 kHz;
/// with 64 samples, stealing a loud saw left broadband clicks in the
/// spectrogram).
pub const FADE: u16 = 256;

/// What a part contributes to its voices' control updates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PartView<'a> {
    pub patch: &'a Patch,
    pub coef: &'a PatchCoef,
    /// Linear part volume.
    pub volume: f32,
    /// Constant-power pan gains.
    pub pan: (f32, f32),
    /// Pan position, −1 to 1.
    pub pan_pos: f32,
    /// Pitch bend, −1 to 1.
    pub bend: f32,
}

/// Engine-wide context of a segment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ctx<'a> {
    /// The sample cache of this render call.
    #[cfg_attr(not(feature = "samples"), allow(dead_code))]
    pub cache: Option<&'a SampleCache<'a>>,
    pub rate: f32,
    pub tempo: f32,
    /// Samples from the segment start to the next control tick.
    pub ahead: u32,
    /// The segment starts on a control tick.
    pub tick: bool,
}

#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
#[cfg_attr(not(feature = "samples"), allow(dead_code))]
pub(crate) enum Source {
    Sub([Osc; 2]),
    Fm(FmState),
    Drum(DrumVoice),
    Sample(SamplePlayer),
    Hit(SampleHit),
}

/// A voice slot.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Voice {
    pub active: bool,
    pub part: u8,
    pub note: u8,
    /// Gate closed (note-off seen, or a drum).
    pub released: bool,
    /// Note-off seen while the part's sustain pedal was down.
    pub sustained: bool,
    /// Allocation order (higher = younger).
    pub stamp: u64,
    /// Control update needed at the next segment.
    pub dirty: bool,
    /// Samples left of a fade-out (0: none).
    pub fade: u16,
    /// Kit choke group (0: none).
    pub group: u8,
    /// Kit entry pan, −1 to 1, added to the part's.
    pan: f32,
    vel: f32,
    vel_gain: f32,
    pitch: f32,
    target: f32,
    glide_step: f32,
    amp: Env,
    fenv: Env,
    penv: Env,
    lfo: Lfo,
    rng: Rng,
    svf: Svf,
    since: u32,
    fresh: bool,
    /// Inputs of the last pitch and cutoff computations (skipped when equal).
    last_semis: f32,
    last_cut: [f32; 3],
    /// Pan gains: target, current, per-sample step, samples of ramp left.
    gl: f32,
    gr: f32,
    cl: f32,
    cr: f32,
    dl: f32,
    dr: f32,
    ramp: u8,
    /// Gain without pan (for the sends).
    pub gain: f32,
    src: Source,
}

impl Voice {
    /// An idle slot.
    pub const IDLE: Self = Self {
        active: false,
        part: 0,
        note: 0,
        released: true,
        sustained: false,
        stamp: 0,
        dirty: false,
        fade: 0,
        group: 0,
        pan: 0.0,
        vel: 0.0,
        vel_gain: 0.0,
        pitch: 60.0,
        target: 60.0,
        glide_step: 0.0,
        amp: Env::IDLE,
        fenv: Env::IDLE,
        penv: Env::IDLE,
        lfo: Lfo::ZERO,
        rng: Rng::new(1),
        svf: Svf::ZERO,
        since: 0,
        fresh: false,
        last_semis: f32::NAN,
        last_cut: [f32::NAN; 3],
        gl: 0.0,
        gr: 0.0,
        cl: 0.0,
        cr: 0.0,
        dl: 0.0,
        dr: 0.0,
        ramp: 0,
        gain: 0.0,
        src: Source::Sub([Osc::new(crate::osc::Waveform::Sine, 1); 2]),
    };

    /// Start a melodic note (subtractive or FM) in this slot. `from` is the
    /// pitch to glide from (`None`: no glide).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        part: u8,
        p: &Patch,
        note: u8,
        velocity: u8,
        from: Option<f32>,
        stamp: u64,
        seed: u32,
        rate: f32,
    ) {
        let vel = f32::from(velocity.min(127)) * (1.0 / 127.0);
        let mut rng = Rng::new(seed);
        self.active = true;
        self.part = part;
        self.note = note;
        self.released = false;
        self.sustained = false;
        self.stamp = stamp;
        self.dirty = true;
        self.fade = 0;
        self.vel = vel;
        self.vel_gain = 1.0 - p.velocity + p.velocity * vel * vel;
        self.target = f32::from(note);
        self.set_glide(p, from, rate);
        self.amp.trigger();
        self.fenv.trigger();
        self.penv.trigger();
        if p.lfo.key_sync {
            self.lfo.reset(&mut rng);
        }
        self.svf.reset();
        self.since = 0;
        self.fresh = true;
        self.last_semis = f32::NAN;
        self.last_cut = [f32::NAN; 3];
        match &p.kind {
            Kind::Subtractive(s) => {
                let mut o =
                    [Osc::new(s.osc[0].wave, rng.next_u32()), Osc::new(s.osc[1].wave, rng.next_u32())];
                for (o, q) in o.iter_mut().zip(&s.osc) {
                    o.set_phase(q.phase);
                }
                self.src = Source::Sub(o);
            }
            Kind::Fm(f) => {
                let _ = f;
                let mut s = FmState::default();
                s.trigger();
                self.src = Source::Fm(s);
            }
            #[cfg(feature = "samples")]
            Kind::Sample(sp) => {
                self.src = Source::Sample(SamplePlayer::new(sp.slot, sp.looping, sp.start));
            }
            // Built without sample playback (the player until it has a
            // card sample source): the note does not sound.
            #[cfg(not(feature = "samples"))]
            Kind::Sample(_) => self.active = false,
            Kind::Drum(_) | Kind::Kit(_) => {}
        }
        self.group = 0;
        self.pan = 0.0;
        self.rng = rng;
    }

    /// Start a drum voice (`Ok`) or a kit sample hit (`Err`) in this slot,
    /// in choke group `group` with an extra `pan`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_drum(
        &mut self,
        part: u8,
        sound: Result<&DrumParams, &HitParams>,
        note: u8,
        vel_gain: f32,
        group: u8,
        pan: f32,
        stamp: u64,
        seed: u32,
        rate: f32,
    ) {
        *self = Self::IDLE;
        self.active = true;
        self.part = part;
        self.note = note;
        self.released = true;
        self.stamp = stamp;
        self.dirty = true;
        self.fresh = true;
        self.vel_gain = 1.0;
        self.group = group;
        self.pan = pan;
        self.src = match sound {
            Ok(d) => Source::Drum(DrumVoice::new(d, vel_gain, rate, seed)),
            #[cfg(feature = "samples")]
            Err(h) => Source::Hit(SampleHit::new(h, vel_gain, rate)),
            #[cfg(not(feature = "samples"))]
            Err(_) => {
                self.active = false;
                return;
            }
        };
    }

    fn set_glide(&mut self, p: &Patch, from: Option<f32>, rate: f32) {
        match from {
            Some(f) if p.glide > 0.0 => {
                self.pitch = f;
                self.glide_step = (self.target - f).abs() / (p.glide * rate);
            }
            _ => {
                self.pitch = self.target;
                self.glide_step = 0.0;
            }
        }
    }

    /// Mono: move to `note` without restarting the envelopes (legato), or
    /// with a retrigger from the current levels.
    pub fn change_note(
        &mut self,
        p: &Patch,
        note: u8,
        velocity: Option<u8>,
        retrigger: bool,
        glide: bool,
        rate: f32,
    ) {
        let from = self.pitch;
        self.note = note;
        self.target = f32::from(note);
        if let Some(v) = velocity {
            self.vel = f32::from(v.min(127)) * (1.0 / 127.0);
            self.vel_gain = 1.0 - p.velocity + p.velocity * self.vel * self.vel;
        }
        self.set_glide(p, glide.then_some(from), rate);
        self.released = false;
        self.sustained = false;
        if retrigger {
            self.amp.gate_on();
            self.fenv.gate_on();
            self.penv.gate_on();
            if let Source::Fm(f) = &mut self.src {
                f.retrigger();
            }
        }
        self.dirty = true;
    }

    /// Close the gate.
    pub fn release(&mut self, c: &PatchCoef) {
        if self.released {
            return;
        }
        self.released = true;
        self.amp.gate_off(&c.amp);
        self.fenv.gate_off(&c.filter);
        self.penv.gate_off(&c.pitch);
        if let Source::Fm(f) = &mut self.src {
            f.release(&c.op);
        }
    }

    /// Fade out over [`FADE`] samples and stop.
    pub fn cut(&mut self) {
        if self.fade == 0 {
            self.fade = FADE;
        }
    }

    /// Choke-group cut: a fast fade (τ 3 ms) for drums and hits, the
    /// 256-sample fade for anything else.
    pub fn choke(&mut self, rate: f32) {
        match &mut self.src {
            Source::Drum(d) => d.choke(rate),
            #[cfg(feature = "samples")]
            Source::Hit(h) => h.choke(rate),
            _ => self.cut(),
        }
    }

    /// Whether this is a drum voice or a kit hit (note-off does nothing).
    pub fn is_drum(&self) -> bool {
        matches!(self.src, Source::Drum(_) | Source::Hit(_))
    }

    /// Current level (envelope × velocity), for stealing.
    pub fn level(&self) -> f32 {
        match &self.src {
            Source::Drum(d) => d.level(),
            #[cfg(feature = "samples")]
            Source::Hit(h) => h.level(),
            _ => self.amp.level() * self.vel_gain,
        }
    }

    /// The sample play head, if this voice plays a sample.
    pub fn want(&self) -> Option<Want> {
        match &self.src {
            #[cfg(feature = "samples")]
            Source::Sample(p) => p.want(),
            #[cfg(feature = "samples")]
            Source::Hit(h) => h.player().want(),
            _ => None,
        }
    }

    /// Samples played as silence for a missing page since the last call.
    pub fn take_misses(&mut self) -> u32 {
        match &mut self.src {
            #[cfg(feature = "samples")]
            Source::Sample(p) => p.take_misses(),
            #[cfg(feature = "samples")]
            Source::Hit(h) => h.player_mut().take_misses(),
            _ => 0,
        }
    }

    fn control(&mut self, part: &PartView<'_>, ctx: &Ctx<'_>) {
        let n = self.since;
        self.since = 0;
        self.svf.flush();
        let p = part.patch;
        if let Source::Drum(_) | Source::Hit(_) = self.src {
            let g = part.volume;
            let pan = if self.pan == 0.0 { part.pan } else { pan_gains(part.pan_pos + self.pan) };
            self.gain = g;
            self.gl = g * pan.0;
            self.gr = g * pan.1;
            return;
        }
        if self.pitch != self.target {
            let d = self.glide_step * n as f32;
            let diff = self.target - self.pitch;
            if diff.abs() <= d || self.glide_step <= 0.0 {
                self.pitch = self.target;
            } else if diff > 0.0 {
                self.pitch += d;
            } else {
                self.pitch -= d;
            }
        }
        let filtered = p.filter.mode != FilterMode::Off;
        let fe = if filtered && p.filter.env_amount != 0.0 {
            self.fenv.advance(&part.coef.filter, n)
        } else {
            0.0
        };
        let pe = if p.pitch_env_amount != 0.0 { self.penv.advance(&part.coef.pitch, n) } else { 0.0 };
        let lfo = if p.lfo.is_active() {
            self.lfo.advance(p.lfo.wave, p.lfo.rate.hz(ctx.tempo) / ctx.rate, n, &mut self.rng)
        } else {
            0.0
        };
        let semis = self.pitch + part.bend * p.bend_range + pe * p.pitch_env_amount + lfo * p.lfo.pitch;
        let retune = semis != self.last_semis;
        self.last_semis = semis;
        match (&mut self.src, &p.kind) {
            (Source::Sub(osc), Kind::Subtractive(s)) => {
                let hz = if retune { note_hz(semis) / ctx.rate } else { 0.0 };
                for ((o, q), r) in osc.iter_mut().zip(&s.osc).zip(part.coef.osc_ratio) {
                    if retune {
                        o.set_dt(hz * r);
                    }
                    o.set_width(q.width + lfo * p.lfo.width);
                }
            }
            #[cfg(feature = "samples")]
            (Source::Sample(sp), Kind::Sample(s)) => {
                if sp.prepare(ctx.cache, ctx.rate) && retune {
                    let root = if s.root <= 127 { s.root } else { sp.root() };
                    sp.set_pitch(semis - f32::from(root) + s.tune);
                }
            }
            (Source::Fm(f), Kind::Fm(fp)) => {
                f.control(
                    fp,
                    &part.coef.fm,
                    &part.coef.op,
                    note_hz(semis),
                    self.vel,
                    (semis - 60.0) * (1.0 / 12.0),
                    ctx.rate,
                    n,
                    ctx.ahead,
                    ctx.tick,
                );
            }
            _ => {}
        }
        if filtered {
            let f = &p.filter;
            let oct = fe * f.env_amount
                + lfo * p.lfo.cutoff
                + f.key_track * (self.pitch - 60.0) * (1.0 / 12.0)
                + f.velocity * (self.vel - 1.0);
            let key = [oct, f.cutoff, f.resonance];
            if key != self.last_cut {
                self.last_cut = key;
                self.svf.set(f.cutoff * exp2(clampf(oct, -12.0, 12.0)), f.resonance, ctx.rate);
            }
        }
        let trem = 1.0 - p.lfo.amp * (0.5 - 0.5 * lfo);
        let g = self.vel_gain * p.level * part.volume * trem;
        self.gain = g;
        self.gl = g * part.pan.0;
        self.gr = g * part.pan.1;
    }

    /// Render `out.len()` (≤ [`crate::CONTROL_PERIOD`]) samples into `out`;
    /// returns whether the voice is still sounding.
    pub fn render(&mut self, part: &PartView<'_>, ctx: &Ctx<'_>, tick: bool, out: &mut [f32]) -> bool {
        if tick || self.dirty {
            self.control(part, ctx);
            self.dirty = false;
            if self.fresh {
                self.cl = self.gl;
                self.cr = self.gr;
                self.ramp = 0;
                self.fresh = false;
            } else {
                // A new gain ramps over one control period from wherever it is.
                const INV: f32 = 1.0 / CONTROL_PERIOD as f32;
                self.dl = (self.gl - self.cl) * INV;
                self.dr = (self.gr - self.cr) * INV;
                self.ramp = CONTROL_PERIOD as u8;
            }
        }
        self.since += out.len() as u32;
        out.fill(0.0);
        let p = part.patch;
        // The source, then (`shape`) the filter and the amplitude envelope
        // in one place for every melodic source (written once: the code
        // is paged on the player). `src`: the source itself still sounds.
        let (shape, src) = match (&mut self.src, &p.kind) {
            (Source::Drum(d), _) => (false, d.render(out, tick)),
            #[cfg(feature = "samples")]
            (Source::Hit(h), _) => (false, h.render(ctx.cache, ctx.rate, out, tick)),
            #[cfg(feature = "samples")]
            (Source::Sample(sp), Kind::Sample(_)) => {
                // The end of the sample ends the voice on the next control
                // tick (as drums), so the slot frees at the same sample
                // whatever the block sizes.
                if tick && sp.done() {
                    (false, false)
                } else {
                    sp.render(ctx.cache, out);
                    (true, true)
                }
            }
            (Source::Sub(osc), Kind::Subtractive(s)) => {
                for (o, q) in osc.iter_mut().zip(&s.osc) {
                    if q.level > 0.0 {
                        o.add_to(out, q.level);
                    }
                }
                if s.noise > 0.0 {
                    for x in out.iter_mut() {
                        *x += self.rng.bipolar() * s.noise;
                    }
                }
                (true, true)
            }
            (Source::Fm(f), Kind::Fm(_)) => {
                f.render(&part.coef.fm, out);
                (true, !f.finished(&part.coef.fm))
            }
            // The part's patch changed kind under a sounding voice.
            _ => (false, false),
        };
        let mut alive = if shape {
            self.svf.process(p.filter.mode, out);
            for x in out.iter_mut() {
                *x *= self.amp.step(&part.coef.amp);
            }
            !self.amp.is_idle() && src
        } else {
            src
        };
        if self.fade > 0 {
            for x in out.iter_mut() {
                *x *= f32::from(self.fade) * (1.0 / f32::from(FADE));
                self.fade = self.fade.saturating_sub(1);
            }
            if self.fade == 0 {
                alive = false;
            }
        }
        alive
    }
}

impl Voice {
    /// Mix `x` (this voice's rendered samples) into `l` and `r` with the
    /// pan gains, ramping them where a control update changed them.
    pub fn mix(&mut self, x: &[f32], l: &mut [f32], r: &mut [f32]) {
        let k = usize::from(self.ramp).min(x.len());
        let (mut cl, mut cr) = (self.cl, self.cr);
        for ((x, l), r) in x[..k].iter().zip(&mut l[..k]).zip(&mut r[..k]) {
            cl += self.dl;
            cr += self.dr;
            *l += x * cl;
            *r += x * cr;
        }
        self.ramp -= k as u8;
        if self.ramp == 0 {
            cl = self.gl;
            cr = self.gr;
        }
        for ((x, l), r) in x[k..].iter().zip(&mut l[k..]).zip(&mut r[k..]) {
            *l += x * cl;
            *r += x * cr;
        }
        self.cl = cl;
        self.cr = cr;
    }
}

/// Constant-power pan gains for `pan` in [−1, 1].
#[must_use]
pub(crate) fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (clampf(pan, -1.0, 1.0) + 1.0) * FRAC_PI_4;
    (orpheus_dsp::math::cos(a), orpheus_dsp::math::sin(a))
}
