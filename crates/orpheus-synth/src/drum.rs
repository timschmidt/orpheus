//! Drum voices built only from oscillators, noise, filters and exponential
//! decays, after the published descriptions of the classic analogue drum
//! machines' circuits (bridged-T resonators pinged by a pulse, six square
//! oscillators at inharmonic ratios for the metal sounds, filtered noise
//! with shaped envelopes). Facts about those circuits (frequencies, which
//! oscillators feed which sound) are used; no code or samples are copied.
//!
//! | Kind | Sound sources | Shaping | `tone` | `snap` |
//! |---|---|---|---|---|
//! | Kick | sine, pitch drop ×4 → ×1 (τ 22 ms), noise click (τ 1.2 ms) | amplitude τ 140 ms | drive into a soft clip | click |
//! | Snare | two sines (185 and 330 Hz) with a small pitch drop; noise | tones τ 55 ms; noise τ 90 ms | noise high pass 1.2–5 kHz | noise share |
//! | Closed / open hat | six band-limited squares at inharmonic ratios, noise | high-passed at 6–10 kHz; τ 15 / 110 ms | high-pass cutoff | noise share |
//! | Clap | noise | band-passed; three bursts 9 ms apart (τ 2.5 ms) then a tail τ 55 ms | band 1.0–2.6 kHz | — |
//! | Tom | sine with a pitch drop ×1.6 → ×1 (τ 45 ms), a little noise | τ 160 ms | — | noise share |
//! | Rim | two sines (455 and 1667 Hz, the rim-shot resonators), noise click | τ 9 ms, high-passed, driven | high partial and high pass | click |
//! | Cowbell | two band-limited squares (540 and 800 Hz, two of the metal oscillators) | band-passed, then a low pass at 3.5 kHz; a fast (τ 12 ms) plus a slow (τ 110 ms) decay | band 1.1–3.3 kHz | fast-decay share |
//! | Cymbal | the six metal squares and noise | two bands: band pass at 3.4 kHz (τ 0.9 s) and high pass at 7 kHz (τ 0.35 s) | balance from low (ride) to high (crash) | noise (sizzle) |
//! | Clave | sine at 2.5 kHz, noise click | τ 18 ms | a second partial at ×2.76 (wood block) | click |
//! | Shaker | noise | high pass 4–10 kHz; attack 0–12 ms, then τ 45 ms | high-pass cutoff | attack (1 = instant) |
//!
//! `tune` (semitones) moves every frequency, `decay` scales every time
//! constant except clicks and bursts (0.1–4), `tone` and `snap` are 0–1. The
//! six metal frequencies are the ones usually quoted for the classic analogue
//! hi-hat circuit (205.3, 304.4, 369.6, 522.7, 540, 800 Hz; here ×1.6 for the
//! hats and cymbal). A drum voice ends when its level falls below −80 dB,
//! decided on control ticks only.

use crate::filter::Svf;
use crate::math::{Rng, clampf, decay_coef, exp2, sin_phase, sin_turns, soft_clip};
use crate::osc::{blep, inc_for};

/// Drum voice type.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DrumKind {
    /// Bass drum.
    Kick,
    /// Snare drum.
    Snare,
    /// Closed hi-hat.
    ClosedHat,
    /// Open hi-hat.
    OpenHat,
    /// Hand clap.
    Clap,
    /// Tom (pitch by `tune`; congas and bongos are tuned toms).
    Tom,
    /// Rim shot (side stick).
    Rim,
    /// Cowbell.
    Cowbell,
    /// Cymbal: `tone` from ride (dark, long) to crash (bright).
    Cymbal,
    /// Clave (`tone` toward a wood block).
    Clave,
    /// Shaker, maracas, cabasa.
    Shaker,
}

impl DrumKind {
    /// Every kind, in declaration order (the binary code is the index).
    pub const ALL: [Self; 11] = [
        Self::Kick,
        Self::Snare,
        Self::ClosedHat,
        Self::OpenHat,
        Self::Clap,
        Self::Tom,
        Self::Rim,
        Self::Cowbell,
        Self::Cymbal,
        Self::Clave,
        Self::Shaker,
    ];

    /// The kind with binary code `i` (its index in [`DrumKind::ALL`]).
    #[must_use]
    pub fn from_code(i: u8) -> Option<Self> {
        Self::ALL.get(usize::from(i)).copied()
    }

    /// The binary code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// Drum voice settings.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct DrumParams {
    /// Voice type.
    pub kind: DrumKind,
    /// Pitch offset, semitones (−24 to 24).
    pub tune: f32,
    /// Time scale of the decays, 0.1–4.
    pub decay: f32,
    /// Colour, 0–1 (see the module table).
    pub tone: f32,
    /// Noise, click or attack, 0–1 (see the module table).
    pub snap: f32,
    /// Level, 0–1.
    pub level: f32,
}

impl DrumParams {
    /// Defaults for `kind`.
    #[must_use]
    pub const fn new(kind: DrumKind) -> Self {
        Self { kind, tune: 0.0, decay: 1.0, tone: 0.5, snap: 0.5, level: 0.8 }
    }

    /// The settings with every field in range.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        Self {
            kind: self.kind,
            tune: clampf(self.tune, -24.0, 24.0),
            decay: clampf(self.decay, 0.1, 4.0),
            tone: clampf(self.tone, 0.0, 1.0),
            snap: clampf(self.snap, 0.0, 1.0),
            level: clampf(self.level, 0.0, 1.0),
        }
    }
}

const HAT: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

/// A drum voice's running state (about 150 bytes).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct DrumVoice {
    kind: DrumKind,
    t: u32,
    ph: [u32; 6],
    inc: [u32; 6],
    /// Base and peak phase increments (cycles per sample) for the pitch drop.
    f0: f32,
    f1: f32,
    amp: f32,
    amp_c: f32,
    pitch: f32,
    pitch_c: f32,
    noise: f32,
    noise_c: f32,
    /// Second envelope (cowbell, cymbal: the slow decay; shaker: the decay
    /// after the attack).
    aux: f32,
    aux_c: f32,
    /// One-pole low-pass state and coefficient (cymbal: its complement is
    /// the high band).
    lp1: f32,
    lp1_c: f32,
    burst_c: f32,
    burst_gap: u32,
    tail_start: u32,
    svf: Svf,
    tone: f32,
    snap: f32,
    level: f32,
    rng: Rng,
}

/// Level below which a drum voice ends (−80 dB).
const END: f32 = 1.0e-4;

impl DrumVoice {
    /// A voice that starts `p` at velocity gain `vel` (0–1) at `rate` Hz.
    #[must_use]
    pub fn new(p: &DrumParams, vel: f32, rate: f32, seed: u32) -> Self {
        let p = p.sanitized();
        let tune = exp2(p.tune * (1.0 / 12.0));
        let d = p.decay;
        let mut v = Self {
            kind: p.kind,
            t: 0,
            ph: [0; 6],
            inc: [0; 6],
            f0: 0.0,
            f1: 0.0,
            amp: 1.0,
            amp_c: 0.0,
            pitch: 1.0,
            pitch_c: 0.0,
            noise: 1.0,
            noise_c: 0.0,
            aux: 0.0,
            aux_c: 0.0,
            lp1: 0.0,
            lp1_c: 0.0,
            burst_c: 0.0,
            burst_gap: 0,
            tail_start: 0,
            svf: Svf::default(),
            tone: p.tone,
            snap: p.snap,
            level: p.level * vel,
            rng: Rng::new(seed),
        };
        match p.kind {
            DrumKind::Kick => {
                v.f0 = 50.0 * tune / rate;
                v.f1 = v.f0 * 3.0;
                v.amp_c = decay_coef(0.14 * d, rate);
                v.pitch_c = decay_coef(0.022 * d, rate);
                v.noise_c = decay_coef(0.0012, rate);
            }
            DrumKind::Snare => {
                v.f0 = 185.0 * tune / rate;
                v.f1 = 330.0 * tune / rate;
                v.amp_c = decay_coef(0.055 * d, rate);
                v.pitch_c = decay_coef(0.01, rate);
                v.noise_c = decay_coef(0.09 * d, rate);
                v.svf.set(1200.0 + 3800.0 * p.tone, 0.2, rate);
            }
            DrumKind::ClosedHat | DrumKind::OpenHat => {
                let tau = if p.kind == DrumKind::ClosedHat { 0.015 } else { 0.11 };
                for (i, f) in HAT.iter().enumerate() {
                    v.inc[i] = inc_for(clampf(f * 1.6 * tune / rate, 0.0, 0.45));
                }
                v.amp_c = decay_coef(tau * d, rate);
                v.noise_c = v.amp_c;
                v.svf.set(6000.0 + 4000.0 * p.tone, 0.3, rate);
            }
            DrumKind::Clap => {
                v.svf.set(1000.0 * exp2(1.4 * p.tone) * tune, 0.45, rate);
                v.burst_c = decay_coef(0.0025, rate);
                v.burst_gap = (0.009 * rate) as u32;
                v.tail_start = 3 * v.burst_gap;
                v.amp_c = decay_coef(0.055 * d, rate);
                v.noise = 1.0;
            }
            DrumKind::Tom => {
                v.f0 = 110.0 * tune / rate;
                v.f1 = v.f0 * 0.6;
                v.amp_c = decay_coef(0.16 * d, rate);
                v.pitch_c = decay_coef(0.045 * d, rate);
                v.noise_c = decay_coef(0.02, rate);
            }
            DrumKind::Rim => {
                v.inc[0] = inc_for(clampf(455.0 * tune / rate, 0.0, 0.45));
                v.inc[1] = inc_for(clampf(1667.0 * tune / rate, 0.0, 0.45));
                v.amp_c = decay_coef(0.009 * d, rate);
                v.noise_c = decay_coef(0.0012, rate);
                v.svf.set((300.0 + 1200.0 * p.tone) * tune, 0.1, rate);
            }
            DrumKind::Cowbell => {
                v.inc[0] = inc_for(clampf(540.0 * tune / rate, 0.0, 0.45));
                v.inc[1] = inc_for(clampf(800.0 * tune / rate, 0.0, 0.45));
                v.amp_c = decay_coef(0.012, rate);
                v.noise = 0.0;
                v.aux = 1.0;
                v.aux_c = decay_coef(0.11 * d, rate);
                v.svf.set(1100.0 * exp2(1.6 * p.tone) * tune, 0.55, rate);
                // Then a one-pole low pass at 3.5 kHz takes the edge off
                // the squares' upper harmonics.
                v.lp1_c = decay_coef(1.0 / (core::f32::consts::TAU * 3500.0 * tune), rate);
            }
            DrumKind::Cymbal => {
                for (i, f) in HAT.iter().enumerate() {
                    v.inc[i] = inc_for(clampf(f * 1.6 * tune / rate, 0.0, 0.45));
                }
                v.amp_c = decay_coef(0.9 * d, rate);
                v.aux = 1.0;
                v.aux_c = decay_coef(0.35 * d, rate);
                v.noise_c = decay_coef(0.25 * d, rate);
                v.svf.set(3400.0 * tune, 0.35, rate);
                // A one-pole low pass at 7 kHz; the high band is x − lp.
                v.lp1_c = decay_coef(1.0 / (core::f32::consts::TAU * 7000.0 * tune), rate);
            }
            DrumKind::Clave => {
                v.inc[0] = inc_for(clampf(2500.0 * tune / rate, 0.0, 0.45));
                v.inc[1] = inc_for(clampf(2500.0 * 2.76 * tune / rate, 0.0, 0.45));
                v.amp_c = decay_coef(0.018 * d, rate);
                v.noise_c = decay_coef(0.0008, rate);
            }
            DrumKind::Shaker => {
                // amp: the attack's complement (1 → 0 within the attack
                // time); aux: the decay. The voice is (1 − amp) · aux.
                let attack = 0.012 * (1.0 - p.snap);
                v.amp = if attack * rate >= 1.0 { 1.0 } else { 0.0 };
                v.amp_c = decay_coef(attack * 0.3, rate);
                v.noise = 0.0;
                v.aux = 1.0;
                v.aux_c = decay_coef(0.045 * d, rate);
                v.svf.set((4000.0 + 6000.0 * p.tone) * tune, 0.15, rate);
            }
        }
        v
    }

    /// Voice type.
    #[must_use]
    pub fn kind(&self) -> DrumKind {
        self.kind
    }

    /// Current level (for voice stealing).
    #[must_use]
    pub fn level(&self) -> f32 {
        self.amp.max(self.noise).max(self.aux) * self.level
    }

    /// Fade out quickly (choke group): τ 3 ms from now.
    pub fn choke(&mut self, rate: f32) {
        let c = decay_coef(0.003, rate);
        self.amp_c = self.amp_c.min(c);
        self.noise_c = self.noise_c.min(c);
        self.aux_c = self.aux_c.min(c);
        if self.kind == DrumKind::Clap && self.t < self.tail_start {
            // No more bursts: straight into the (choked) tail.
            self.tail_start = self.t;
            self.amp = self.noise;
        }
        if self.kind == DrumKind::Shaker {
            // The attack is over: decay from the present level.
            self.aux *= 1.0 - self.amp;
            self.amp = 0.0;
        }
    }

    /// Render `out.len()` samples (overwrites `out`); returns whether the
    /// voice is still sounding. `tick`: the segment starts on a control
    /// tick; the voice clears tiny state and decides whether it has ended
    /// only there, so the result does not depend on how the caller cuts the
    /// stream into segments.
    pub fn render(&mut self, out: &mut [f32], tick: bool) -> bool {
        if tick {
            self.svf.flush();
            self.amp = crate::math::flush(self.amp);
            self.noise = crate::math::flush(self.noise);
            self.pitch = crate::math::flush(self.pitch);
            self.aux = crate::math::flush(self.aux);
            self.lp1 = crate::math::flush(self.lp1);
            let env = self.amp.max(self.noise).max(self.aux);
            let sounding =
                (self.kind == DrumKind::Clap && self.t < self.tail_start) || (env >= END && self.level > 0.0);
            if !sounding {
                out.fill(0.0);
                return false;
            }
        }
        let g = self.level;
        match self.kind {
            DrumKind::Kick => self.kick(out, g),
            DrumKind::Snare => self.snare(out, g),
            DrumKind::ClosedHat | DrumKind::OpenHat => self.hat(out, g),
            DrumKind::Clap => self.clap(out, g),
            DrumKind::Tom => self.tom(out, g),
            DrumKind::Rim => self.rim(out, g),
            DrumKind::Cowbell => self.cowbell(out, g),
            DrumKind::Cymbal => self.cymbal(out, g),
            DrumKind::Clave => self.clave(out, g),
            DrumKind::Shaker => self.shaker(out, g),
        }
        true
    }

    // One method per kind (not inlined): each inner loop is its own
    // symbol, for the cycle estimate and for placing hot code (SYN-3).
    #[inline(never)]
    fn kick(&mut self, out: &mut [f32], g: f32) {
        let drive = 1.0 + 3.0 * self.tone;
        let click = 0.6 * self.snap;
        for o in out.iter_mut() {
            let dt = self.f0 + self.f1 * self.pitch;
            let p = (self.ph[0] >> 8) as f32 * (1.0 / 16_777_216.0);
            self.ph[0] = self.ph[0].wrapping_add(inc_for(clampf(dt, 0.0, 0.45)));
            let s = sin_turns(p) * self.amp * drive + self.rng.bipolar() * self.noise * click;
            *o = soft_clip(s) * g;
            self.amp *= self.amp_c;
            self.pitch *= self.pitch_c;
            self.noise *= self.noise_c;
        }
    }

    #[inline(never)]
    fn snare(&mut self, out: &mut [f32], g: f32) {
        let body = 0.55 * (1.0 - 0.6 * self.snap);
        let nz = 0.3 + 0.7 * self.snap;
        for o in out.iter_mut() {
            let bend = 1.0 + 0.25 * self.pitch;
            let p0 = (self.ph[0] >> 8) as f32 * (1.0 / 16_777_216.0);
            let p1 = (self.ph[1] >> 8) as f32 * (1.0 / 16_777_216.0);
            self.ph[0] = self.ph[0].wrapping_add(inc_for(clampf(self.f0 * bend, 0.0, 0.45)));
            self.ph[1] = self.ph[1].wrapping_add(inc_for(clampf(self.f1 * bend, 0.0, 0.45)));
            let tones = (sin_turns(p0) + 0.6 * sin_turns(p1)) * self.amp * body;
            let n = self.rng.bipolar() * self.noise;
            let (lp, bp) = self.svf.tick(n);
            let hp = n - 1.6 * bp - lp;
            *o = soft_clip(tones + hp * nz) * g;
            self.amp *= self.amp_c;
            self.pitch *= self.pitch_c;
            self.noise *= self.noise_c;
        }
    }

    #[inline(never)]
    fn hat(&mut self, out: &mut [f32], g: f32) {
        let metal = 1.0 - 0.7 * self.snap;
        let nz = 0.2 + 0.8 * self.snap;
        let dts = self.inc.map(|i| i as f32 * (1.0 / 4_294_967_296.0));
        let invs = dts.map(|d| if d > 0.0 { 1.0 / d } else { 0.0 });
        for o in out.iter_mut() {
            let mut m = 0.0;
            for i in 0..6 {
                m += square(self.ph[i], dts[i], invs[i]);
                self.ph[i] = self.ph[i].wrapping_add(self.inc[i]);
            }
            let s = (m * (1.0 / 6.0) * metal + self.rng.bipolar() * nz) * self.amp;
            let (lp, bp) = self.svf.tick(s);
            *o = soft_clip(1.4 * (s - 1.4 * bp - lp)) * g;
            self.amp *= self.amp_c;
        }
        self.noise = self.amp;
    }

    #[inline(never)]
    fn clap(&mut self, out: &mut [f32], g: f32) {
        for o in out.iter_mut() {
            if self.t < self.tail_start && self.t % self.burst_gap.max(1) == 0 {
                self.noise = 1.0;
            }
            let e = if self.t >= self.tail_start {
                let e = self.amp * 0.7;
                self.amp *= self.amp_c;
                e
            } else {
                self.noise
            };
            self.noise *= self.burst_c;
            let n = self.rng.bipolar() * e;
            let (_, bp) = self.svf.tick(n);
            *o = soft_clip(2.2 * bp) * g;
            self.t = self.t.wrapping_add(1);
        }
        if self.t >= self.tail_start {
            self.noise = 0.0;
        }
    }

    #[inline(never)]
    fn tom(&mut self, out: &mut [f32], g: f32) {
        let nz = 0.08 * self.snap;
        for o in out.iter_mut() {
            let dt = self.f0 + self.f1 * self.pitch;
            let p = (self.ph[0] >> 8) as f32 * (1.0 / 16_777_216.0);
            self.ph[0] = self.ph[0].wrapping_add(inc_for(clampf(dt, 0.0, 0.45)));
            let s = sin_turns(p) * self.amp * 0.9 + self.rng.bipolar() * self.noise * nz;
            *o = soft_clip(s) * g;
            self.amp *= self.amp_c;
            self.pitch *= self.pitch_c;
            self.noise *= self.noise_c;
        }
    }

    #[inline(never)]
    fn rim(&mut self, out: &mut [f32], g: f32) {
        let click = 0.5 * self.snap;
        let hi = 0.35 + 0.65 * self.tone;
        for o in out.iter_mut() {
            let s = (0.7 * sin_phase(self.ph[0]) + hi * sin_phase(self.ph[1])) * self.amp
                + self.rng.bipolar() * self.noise * click;
            self.ph[0] = self.ph[0].wrapping_add(self.inc[0]);
            self.ph[1] = self.ph[1].wrapping_add(self.inc[1]);
            *o = soft_clip(1.6 * self.svf.tick_hp(s)) * g;
            self.amp *= self.amp_c;
            self.noise *= self.noise_c;
        }
    }

    #[inline(never)]
    fn cowbell(&mut self, out: &mut [f32], g: f32) {
        let fast = 0.3 + 0.6 * self.snap;
        let slow = 1.0 - 0.5 * self.snap;
        let dts = [self.inc[0], self.inc[1]].map(|i| i as f32 * (1.0 / 4_294_967_296.0));
        let invs = dts.map(|d| if d > 0.0 { 1.0 / d } else { 0.0 });
        for o in out.iter_mut() {
            let m = square(self.ph[0], dts[0], invs[0]) + square(self.ph[1], dts[1], invs[1]);
            self.ph[0] = self.ph[0].wrapping_add(self.inc[0]);
            self.ph[1] = self.ph[1].wrapping_add(self.inc[1]);
            let (_, bp) = self.svf.tick(m * 0.5);
            self.lp1 = bp + (self.lp1 - bp) * self.lp1_c;
            *o = soft_clip(2.6 * self.lp1 * (fast * self.amp + slow * self.aux)) * g;
            self.amp *= self.amp_c;
            self.aux *= self.aux_c;
        }
    }

    #[inline(never)]
    fn cymbal(&mut self, out: &mut [f32], g: f32) {
        let low = 0.9 * (1.0 - self.tone);
        let high = 0.45 + 0.75 * self.tone;
        let nz = 0.6 * self.snap;
        let dts = self.inc.map(|i| i as f32 * (1.0 / 4_294_967_296.0));
        let invs = dts.map(|d| if d > 0.0 { 1.0 / d } else { 0.0 });
        for o in out.iter_mut() {
            let mut m = 0.0;
            for i in 0..6 {
                m += square(self.ph[i], dts[i], invs[i]);
                self.ph[i] = self.ph[i].wrapping_add(self.inc[i]);
            }
            let x = m * (1.0 / 6.0) + self.rng.bipolar() * nz * self.noise;
            let (_, bp) = self.svf.tick(x);
            self.lp1 = x + (self.lp1 - x) * self.lp1_c;
            let hp = x - self.lp1;
            *o = soft_clip(1.3 * (bp * low * self.amp + hp * high * self.aux)) * g;
            self.amp *= self.amp_c;
            self.aux *= self.aux_c;
            self.noise *= self.noise_c;
        }
    }

    #[inline(never)]
    fn clave(&mut self, out: &mut [f32], g: f32) {
        let click = 0.3 * self.snap;
        let wood = 0.6 * self.tone;
        for o in out.iter_mut() {
            let s = (sin_phase(self.ph[0]) + wood * sin_phase(self.ph[1])) * self.amp
                + self.rng.bipolar() * self.noise * click;
            self.ph[0] = self.ph[0].wrapping_add(self.inc[0]);
            self.ph[1] = self.ph[1].wrapping_add(self.inc[1]);
            *o = soft_clip(0.9 * s) * g;
            self.amp *= self.amp_c;
            self.noise *= self.noise_c;
        }
    }

    #[inline(never)]
    fn shaker(&mut self, out: &mut [f32], g: f32) {
        for o in out.iter_mut() {
            let e = (1.0 - self.amp) * self.aux;
            let x = self.svf.tick_hp(self.rng.bipolar() * e);
            *o = soft_clip(1.5 * x) * g;
            self.amp *= self.amp_c;
            self.aux *= self.aux_c;
        }
    }
}

/// A band-limited square (polyBLEP) at the 32-bit phase `ph`, ±1.
#[inline]
fn square(ph: u32, dt: f32, inv: f32) -> f32 {
    let p = (ph >> 8) as f32 * (1.0 / 16_777_216.0);
    let q = if p >= 0.5 { p - 0.5 } else { p + 0.5 };
    let naive = if p < 0.5 { 1.0 } else { -1.0 };
    naive + 2.0 * blep(p, dt, inv) - 2.0 * blep(q, dt, inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drum_sounds_bounded_and_ends() {
        for kind in DrumKind::ALL {
            let mut v = DrumVoice::new(&DrumParams::new(kind), 1.0, 48_000.0, 5);
            let mut peak = 0.0f32;
            let mut blocks = 0;
            loop {
                let mut b = [0.0f32; 16];
                let alive = v.render(&mut b, true);
                peak = b.iter().fold(peak, |m, x| m.max(x.abs()));
                assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
                blocks += 1;
                if !alive || blocks > 48_000 * 12 / 16 {
                    break;
                }
            }
            assert!(peak > 0.2, "{kind:?} peak {peak}");
            let limit = if kind == DrumKind::Cymbal { 10 } else { 3 };
            assert!(blocks < 48_000 * limit / 16, "{kind:?} lasts {blocks} blocks");
        }
    }

    #[test]
    fn codes_round_trip() {
        for k in DrumKind::ALL {
            assert_eq!(DrumKind::from_code(k.code()), Some(k));
        }
        assert_eq!(DrumKind::from_code(11), None);
    }
}
