//! Simple drum voices: kick, snare, closed and open hat, clap, tom, built
//! only from oscillators, noise, the SVF and exponential decays. (The full
//! drum kit, cymbals and percussion are SYN-2.)
//!
//! | Kind | Sound sources | Shaping |
//! |---|---|---|
//! | Kick | sine, pitch drop ×4 → ×1 (τ 22 ms), noise click (τ 1.2 ms) | amplitude τ 140 ms; `tone` drives a soft clip |
//! | Snare | two sines (185 and 330 Hz) with a small pitch drop; noise | tones τ 55 ms; noise high-passed at 1.2–5 kHz (`tone`), τ 90 ms; `snap` sets the noise share |
//! | Closed / open hat | six band-limited squares at inharmonic ratios, noise | high-passed at 6–10 kHz; τ 15 / 110 ms; a closed hat chokes the open ones of its part |
//! | Clap | noise | band-passed at 1.0–2.6 kHz; three bursts 9 ms apart (τ 2.5 ms) then a tail τ 55 ms |
//! | Tom | sine with a pitch drop ×1.6 → ×1 (τ 45 ms), a little noise | τ 160 ms |
//!
//! `tune` (semitones) moves every frequency, `decay` scales every time
//! constant (0.1–4), `tone` and `snap` are 0–1. The six hat frequencies are
//! the ones usually quoted for the classic analogue hi-hat circuit (205.3,
//! 304.4, 369.6, 522.7, 540, 800 Hz; a fact about that circuit, not code).
//! A drum voice ends when its level falls below −80 dB.

use crate::filter::Svf;
use crate::math::{Rng, clampf, decay_coef, exp2, sin_turns, soft_clip};
use crate::osc::{blep, inc_for};

/// Drum voice type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrumKind {
    /// Bass drum.
    Kick,
    /// Snare drum.
    Snare,
    /// Closed hi-hat (chokes the open hats of its part).
    ClosedHat,
    /// Open hi-hat.
    OpenHat,
    /// Hand clap.
    Clap,
    /// Tom (pitch by `tune`).
    Tom,
}

/// Drum voice settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrumParams {
    /// Voice type.
    pub kind: DrumKind,
    /// Pitch offset, semitones (−24 to 24).
    pub tune: f32,
    /// Time scale of the decays, 0.1–4.
    pub decay: f32,
    /// Colour, 0–1 (kick: drive; snare and hats: filter; clap: band).
    pub tone: f32,
    /// Noise or click share, 0–1.
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

    /// A General MIDI percussion note (channel 10) mapped to a drum voice of
    /// this set, or `None` for notes it does not have (SYN-2 completes the
    /// kit).
    #[must_use]
    pub fn from_gm_note(note: u8) -> Option<Self> {
        let k = |kind| Some(Self::new(kind));
        let tom = |tune: f32| Some(Self { tune, ..Self::new(DrumKind::Tom) });
        match note {
            35 => Some(Self { tune: -2.0, ..Self::new(DrumKind::Kick) }),
            36 => k(DrumKind::Kick),
            38 => k(DrumKind::Snare),
            40 => Some(Self { tone: 0.8, snap: 0.7, ..Self::new(DrumKind::Snare) }),
            39 => k(DrumKind::Clap),
            42 | 44 => k(DrumKind::ClosedHat),
            46 => k(DrumKind::OpenHat),
            41 => tom(-7.0),
            43 => tom(-4.0),
            45 => tom(-2.0),
            47 => tom(0.0),
            48 => tom(3.0),
            50 => tom(5.0),
            _ => None,
        }
    }
}

const HAT: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

/// A drum voice's running state (about 120 bytes).
#[derive(Clone, Copy, Debug, PartialEq)]
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
        self.amp.max(self.noise) * self.level
    }

    /// Fade out quickly (hat choke): τ 3 ms from now.
    pub fn choke(&mut self, rate: f32) {
        let c = decay_coef(0.003, rate);
        self.amp_c = self.amp_c.min(c);
        self.noise_c = self.noise_c.min(c);
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
            let env = self.amp.max(self.noise);
            let sounding =
                (self.kind == DrumKind::Clap && self.t < self.tail_start) || (env >= END && self.level > 0.0);
            if !sounding {
                out.fill(0.0);
                return false;
            }
        }
        let g = self.level;
        match self.kind {
            DrumKind::Kick => {
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
            DrumKind::Snare => {
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
            DrumKind::ClosedHat | DrumKind::OpenHat => {
                let metal = 1.0 - 0.7 * self.snap;
                let nz = 0.2 + 0.8 * self.snap;
                let dts = self.inc.map(|i| i as f32 * (1.0 / 4_294_967_296.0));
                let invs = dts.map(|d| if d > 0.0 { 1.0 / d } else { 0.0 });
                for o in out.iter_mut() {
                    let mut m = 0.0;
                    for i in 0..6 {
                        let p = (self.ph[i] >> 8) as f32 * (1.0 / 16_777_216.0);
                        let (dt, inv) = (dts[i], invs[i]);
                        let q = if p >= 0.5 { p - 0.5 } else { p + 0.5 };
                        let naive = if p < 0.5 { 1.0 } else { -1.0 };
                        m += naive + 2.0 * blep(p, dt, inv) - 2.0 * blep(q, dt, inv);
                        self.ph[i] = self.ph[i].wrapping_add(self.inc[i]);
                    }
                    let s = (m * (1.0 / 6.0) * metal + self.rng.bipolar() * nz) * self.amp;
                    let (lp, bp) = self.svf.tick(s);
                    *o = soft_clip(1.4 * (s - 1.4 * bp - lp)) * g;
                    self.amp *= self.amp_c;
                }
                self.noise = self.amp;
            }
            DrumKind::Clap => {
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
            DrumKind::Tom => {
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
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drum_sounds_bounded_and_ends() {
        for kind in [
            DrumKind::Kick,
            DrumKind::Snare,
            DrumKind::ClosedHat,
            DrumKind::OpenHat,
            DrumKind::Clap,
            DrumKind::Tom,
        ] {
            let mut v = DrumVoice::new(&DrumParams::new(kind), 1.0, 48_000.0, 5);
            let mut peak = 0.0f32;
            let mut blocks = 0;
            loop {
                let mut b = [0.0f32; 16];
                let alive = v.render(&mut b, true);
                peak = b.iter().fold(peak, |m, x| m.max(x.abs()));
                assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
                blocks += 1;
                if !alive || blocks > 48_000 * 4 / 16 {
                    break;
                }
            }
            assert!(peak > 0.2, "{kind:?} peak {peak}");
            assert!(blocks < 48_000 * 3 / 16, "{kind:?} lasts {blocks} blocks");
        }
    }
}
