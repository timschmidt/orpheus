//! Send effects (delay, chorus, reverb) and the master soft limiter.
//!
//! Each send effect takes a mono bus (the sum of the voices' sends) and adds
//! its wet stereo output to the master bus. Memory is fixed (sized for
//! 48 kHz, the highest engine rate):
//!
//! | Effect | Line | RAM |
//! |---|---|---|
//! | Delay | 12 288 × `i16` (256 ms at 48 kHz, 279 ms at 44.1 kHz) | 24 KiB |
//! | Chorus | 1 024 × `f32` | 4 KiB |
//! | Reverb | 8 192 × `i16` shared by 4 combs and 4 all-passes | 16 KiB |
//! | Limiter | — | 12 bytes |
//!
//! The `i16` lines store the signal at 1/8192 per step (±4.0 full scale)
//! and truncate toward zero when writing, so a decaying feedback loop
//! reaches exact silence instead of a limit cycle. An effect whose input has
//! been zero and whose lines hold only zeros skips its work entirely
//! (exactly equivalent: its output would be zero).
//!
//! * **Delay**: one line, left tap at the delay time, right tap `spread`
//!   later; one-pole low pass in the feedback path.
//! * **Chorus**: two taps (left and right in quadrature) swept by a triangle
//!   LFO around 7 ms.
//! * **Reverb**: Schroeder's design (parallel combs into series all-passes,
//!   M. R. Schroeder, "Natural sounding artificial reverberation", 1962)
//!   with a low pass in each comb's feedback (Moorer, 1979) — the structure
//!   Freeverb also uses — written from those descriptions: comb delays
//!   29.7, 37.1, 41.1, 43.7 ms (Schroeder's), all-passes 5.0 and 1.7 ms on
//!   the left, 5.3 and 1.9 ms on the right (decorrelation), all-pass gain
//!   0.7, comb gains set from the decay time (RT60).
//! * **Limiter**: a peak follower (instant attack, 100 ms release) sets the
//!   target gain threshold / peak above −0.45 dBFS; the applied gain follows
//!   the target down with a 1 ms time constant (an instant gain drop
//!   splatters: visible as broadband lines in the spectrogram of a dense
//!   mix) and up with the follower's release; [`soft_clip`] (identity below
//!   −1.9 dBFS) catches the overshoot of that millisecond, so the output
//!   never exceeds full scale.

use crate::math::{clampf, decay_coef, exp2, flush, soft_clip};

const Q: f32 = 8192.0;
const DQ: f32 = 1.0 / Q;

#[inline]
fn quant(x: f32) -> i16 {
    (x * Q) as i16
}

/// Delay line length, samples.
pub const DELAY_LEN: usize = 12_288;
/// Chorus line length, samples.
pub const CHORUS_LEN: usize = 1024;
/// Reverb memory, samples.
pub const REVERB_LEN: usize = 8192;

/// Delay settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayParams {
    /// Delay time, seconds. Times beyond the line (256 ms at 48 kHz) are
    /// clamped to it, also when they come from `beats`.
    pub time: f32,
    /// Tempo sync: delay time in beats (0 = use `time`).
    pub beats: f32,
    /// Right tap after the left, seconds (0–0.05).
    pub spread: f32,
    /// Feedback, 0–0.95.
    pub feedback: f32,
    /// Damping of the repeats, 0 (bright) to 0.95 (dark).
    pub damp: f32,
    /// Wet level, 0–1.
    pub level: f32,
}

impl DelayParams {
    /// Defaults: half a beat (250 ms at 120 BPM), a little spread, 35 %
    /// feedback.
    pub const DEFAULT: Self =
        Self { time: 0.3, beats: 0.5, spread: 0.011, feedback: 0.35, damp: 0.3, level: 0.6 };
}

/// The delay.
#[derive(Clone, Debug)]
pub struct Delay {
    line: [i16; DELAY_LEN],
    pos: usize,
    time: usize,
    spread: usize,
    fb: f32,
    damp: f32,
    lp: f32,
    level: f32,
    zero_run: usize,
    params: DelayParams,
}

impl Delay {
    /// A silent delay with the default settings.
    #[must_use]
    pub fn new() -> Self {
        let mut d = Self {
            line: [0; DELAY_LEN],
            pos: 0,
            time: 1,
            spread: 0,
            fb: 0.0,
            damp: 0.0,
            lp: 0.0,
            level: 0.0,
            zero_run: DELAY_LEN,
            params: DelayParams::DEFAULT,
        };
        d.set(&DelayParams::DEFAULT, 48_000.0, 120.0);
        d
    }

    /// Current settings.
    #[must_use]
    pub fn params(&self) -> DelayParams {
        self.params
    }

    /// Apply settings at `rate` Hz and `bpm`.
    pub fn set(&mut self, p: &DelayParams, rate: f32, bpm: f32) {
        let beats = clampf(p.beats, 0.0, 16.0);
        let t = if beats > 0.0 { beats * 60.0 / bpm } else { clampf(p.time, 0.0, 10.0) };
        self.time = clampf(t * rate, 1.0, (DELAY_LEN - 2) as f32) as usize;
        self.spread = (clampf(p.spread, 0.0, 0.05) * rate) as usize;
        self.spread = self.spread.min(DELAY_LEN - 2 - self.time);
        self.fb = clampf(p.feedback, 0.0, 0.95);
        self.damp = clampf(p.damp, 0.0, 0.95);
        self.level = clampf(p.level, 0.0, 1.0);
        self.params = DelayParams {
            time: clampf(p.time, 0.0, 10.0),
            beats,
            spread: clampf(p.spread, 0.0, 0.05),
            feedback: self.fb,
            damp: self.damp,
            level: self.level,
        };
    }

    /// Clear the line.
    pub fn clear(&mut self) {
        self.line = [0; DELAY_LEN];
        self.lp = 0.0;
        self.zero_run = DELAY_LEN;
    }

    /// Whether the last call did no work.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.zero_run >= DELAY_LEN
    }

    /// Process the mono send `input`, adding the wet signal to `l` and `r`.
    pub fn process(&mut self, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        if self.zero_run >= DELAY_LEN && input.iter().all(|&x| x == 0.0) {
            self.lp = 0.0;
            return;
        }
        let wrap = |i: usize| if i >= DELAY_LEN { i - DELAY_LEN } else { i };
        let (time, spread) = (self.time, self.spread);
        for ((x, l), r) in input.iter().zip(l.iter_mut()).zip(r.iter_mut()) {
            let y = f32::from(self.line[wrap(self.pos + DELAY_LEN - time)]) * DQ;
            let y2 = f32::from(self.line[wrap(self.pos + DELAY_LEN - time - spread)]) * DQ;
            self.lp = y + (self.lp - y) * self.damp;
            let q = quant(*x + self.lp * self.fb);
            self.line[self.pos] = q;
            self.zero_run = if q == 0 { self.zero_run + 1 } else { 0 };
            *l += y * self.level;
            *r += y2 * self.level;
            self.pos = wrap(self.pos + 1);
        }
    }

    /// Clear tiny state (on control ticks).
    pub fn flush(&mut self) {
        self.lp = flush(self.lp);
    }
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

/// Chorus settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChorusParams {
    /// Sweep rate, Hz (0.05–5).
    pub rate: f32,
    /// Sweep depth, 0–1 (1 = ±2.5 ms).
    pub depth: f32,
    /// Wet level, 0–1.
    pub level: f32,
}

impl ChorusParams {
    /// Defaults.
    pub const DEFAULT: Self = Self { rate: 0.6, depth: 0.5, level: 0.7 };
}

/// The chorus.
#[derive(Clone, Debug)]
pub struct Chorus {
    line: [f32; CHORUS_LEN],
    pos: usize,
    phase: u32,
    inc: u32,
    base: f32,
    depth: f32,
    level: f32,
    zero_run: usize,
    params: ChorusParams,
}

impl Chorus {
    /// A silent chorus with the default settings.
    #[must_use]
    pub fn new() -> Self {
        let mut c = Self {
            line: [0.0; CHORUS_LEN],
            pos: 0,
            phase: 0,
            inc: 0,
            base: 0.0,
            depth: 0.0,
            level: 0.0,
            zero_run: CHORUS_LEN,
            params: ChorusParams::DEFAULT,
        };
        c.set(&ChorusParams::DEFAULT, 48_000.0);
        c
    }

    /// Current settings.
    #[must_use]
    pub fn params(&self) -> ChorusParams {
        self.params
    }

    /// Apply settings at `rate` Hz.
    pub fn set(&mut self, p: &ChorusParams, rate: f32) {
        let r = clampf(p.rate, 0.05, 5.0);
        let d = clampf(p.depth, 0.0, 1.0);
        self.inc = crate::osc::inc_for(r / rate);
        self.base = 0.007 * rate;
        self.depth = d * 0.0025 * rate;
        self.level = clampf(p.level, 0.0, 1.0);
        self.params = ChorusParams { rate: r, depth: d, level: self.level };
    }

    /// Clear the line.
    pub fn clear(&mut self) {
        self.line = [0.0; CHORUS_LEN];
        self.zero_run = CHORUS_LEN;
    }

    #[inline]
    fn tap(&self, d: f32) -> f32 {
        let d = clampf(d, 1.0, (CHORUS_LEN - 2) as f32);
        let i = d as usize;
        let fr = d - i as f32;
        let a = self.line[(self.pos + CHORUS_LEN - i) % CHORUS_LEN];
        let b = self.line[(self.pos + CHORUS_LEN - i - 1) % CHORUS_LEN];
        a + (b - a) * fr
    }

    /// Process the mono send `input`, adding the wet signal to `l` and `r`.
    pub fn process(&mut self, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        if self.zero_run >= CHORUS_LEN && input.iter().all(|&x| x == 0.0) {
            self.phase = self.phase.wrapping_add(self.inc.wrapping_mul(input.len() as u32));
            return;
        }
        for ((x, l), r) in input.iter().zip(l.iter_mut()).zip(r.iter_mut()) {
            self.line[self.pos] = *x;
            self.zero_run = if *x == 0.0 { self.zero_run + 1 } else { 0 };
            self.phase = self.phase.wrapping_add(self.inc);
            let tri = |ph: u32| {
                let p = (ph >> 8) as f32 * (1.0 / 16_777_216.0);
                if p < 0.5 { 4.0 * p - 1.0 } else { 3.0 - 4.0 * p }
            };
            *l += self.tap(self.base + self.depth * tri(self.phase)) * self.level;
            *r += self.tap(self.base + self.depth * tri(self.phase.wrapping_add(1 << 30))) * self.level;
            self.pos = (self.pos + 1) % CHORUS_LEN;
        }
    }
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

/// Reverb settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    /// Decay time to −60 dB, seconds (0.2–10).
    pub decay: f32,
    /// High-frequency damping, 0–0.9.
    pub damp: f32,
    /// Wet level, 0–1.
    pub level: f32,
}

impl ReverbParams {
    /// Defaults: a 1.6 s room.
    pub const DEFAULT: Self = Self { decay: 1.6, damp: 0.4, level: 0.5 };
}

const COMB_MS: [f32; 4] = [29.7, 37.1, 41.1, 43.7];
const AP_MS: [f32; 4] = [5.0, 1.7, 5.3, 1.9];
const AP_G: f32 = 0.7;

#[derive(Clone, Copy, Debug, Default)]
struct Line {
    start: usize,
    len: usize,
    pos: usize,
}

/// The reverb.
#[derive(Clone, Debug)]
pub struct Reverb {
    mem: [i16; REVERB_LEN],
    comb: [Line; 4],
    ap: [Line; 4],
    g: [f32; 4],
    lp: [f32; 4],
    damp: f32,
    level: f32,
    longest: usize,
    zero_run: usize,
    rate: f32,
    params: ReverbParams,
}

impl Reverb {
    /// A silent reverb with the default settings at 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut r = Self {
            mem: [0; REVERB_LEN],
            comb: [Line::default(); 4],
            ap: [Line::default(); 4],
            g: [0.0; 4],
            lp: [0.0; 4],
            damp: 0.0,
            level: 0.0,
            longest: 1,
            zero_run: REVERB_LEN,
            rate: 48_000.0,
            params: ReverbParams::DEFAULT,
        };
        r.set_rate(48_000.0);
        r
    }

    /// Lay out the lines for `rate` Hz (≤ 48 kHz) and clear them.
    pub fn set_rate(&mut self, rate: f32) {
        let rate = clampf(rate, 8000.0, 48_000.0);
        self.rate = rate;
        let mut start = 0;
        let mut longest = 1;
        let mut lay = |ms: f32| {
            let len = ((ms * 0.001 * rate) as usize).max(1);
            let l = Line { start, len, pos: 0 };
            start += len;
            longest = longest.max(len);
            l
        };
        let comb = COMB_MS.map(&mut lay);
        let ap = AP_MS.map(&mut lay);
        debug_assert!(start <= REVERB_LEN);
        self.comb = comb;
        self.ap = ap;
        self.longest = longest;
        self.mem = [0; REVERB_LEN];
        self.lp = [0.0; 4];
        self.zero_run = REVERB_LEN;
        let p = self.params;
        self.set(&p);
    }

    /// Current settings.
    #[must_use]
    pub fn params(&self) -> ReverbParams {
        self.params
    }

    /// Apply settings.
    pub fn set(&mut self, p: &ReverbParams) {
        let decay = clampf(p.decay, 0.2, 10.0);
        for (g, c) in self.g.iter_mut().zip(&self.comb) {
            // 10^(−3·L/(RT60·fs)).
            *g = exp2(-3.0 * core::f32::consts::LOG2_10 * c.len as f32 / (decay * self.rate));
        }
        self.damp = clampf(p.damp, 0.0, 0.9);
        self.level = clampf(p.level, 0.0, 1.0);
        self.params = ReverbParams { decay, damp: self.damp, level: self.level };
    }

    /// Clear the lines.
    pub fn clear(&mut self) {
        self.mem = [0; REVERB_LEN];
        self.lp = [0.0; 4];
        self.zero_run = REVERB_LEN;
    }

    /// Process the mono send `input`, adding the wet signal to `l` and `r`.
    pub fn process(&mut self, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        if self.zero_run >= self.longest && input.iter().all(|&x| x == 0.0) {
            self.lp = [0.0; 4];
            return;
        }
        for ((x, l), r) in input.iter().zip(l.iter_mut()).zip(r.iter_mut()) {
            let x = *x * 0.25;
            let mut sum = 0.0;
            let mut nonzero = false;
            for k in 0..4 {
                let c = &mut self.comb[k];
                let at = c.start + c.pos;
                let y = f32::from(self.mem[at]) * DQ;
                self.lp[k] = y + (self.lp[k] - y) * self.damp;
                let q = quant(x + self.lp[k] * self.g[k]);
                self.mem[at] = q;
                nonzero |= q != 0;
                c.pos = if c.pos + 1 == c.len { 0 } else { c.pos + 1 };
                sum += y;
            }
            let mut out = [sum * 0.5; 2];
            for (ch, o) in out.iter_mut().enumerate() {
                for a in &mut self.ap[2 * ch..2 * ch + 2] {
                    let at = a.start + a.pos;
                    let d = f32::from(self.mem[at]) * DQ;
                    let w = *o + AP_G * d;
                    *o = d - AP_G * w;
                    let q = quant(w);
                    self.mem[at] = q;
                    nonzero |= q != 0;
                    a.pos = if a.pos + 1 == a.len { 0 } else { a.pos + 1 };
                }
            }
            self.zero_run = if nonzero { 0 } else { self.zero_run + 1 };
            *l += out[0] * self.level;
            *r += out[1] * self.level;
        }
    }

    /// Clear tiny state (on control ticks).
    pub fn flush(&mut self) {
        for v in &mut self.lp {
            *v = flush(*v);
        }
    }
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

/// The master soft limiter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limiter {
    env: f32,
    gain: f32,
    rel: f32,
    att: f32,
    thr: f32,
}

impl Limiter {
    /// Threshold (−0.45 dBFS).
    pub const THRESHOLD: f32 = 0.95;

    /// A limiter at `rate` Hz.
    #[must_use]
    pub fn new(rate: f32) -> Self {
        Self {
            env: 0.0,
            gain: 1.0,
            rel: decay_coef(0.1, rate),
            att: decay_coef(0.001, rate),
            thr: Self::THRESHOLD,
        }
    }

    /// Limit one stereo frame.
    #[inline]
    pub fn frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        // Non-finite input (cannot happen with sanitised patches) is muted.
        let l = if l.abs() <= 1.0e6 { l } else { 0.0 };
        let r = if r.abs() <= 1.0e6 { r } else { 0.0 };
        let peak = l.abs().max(r.abs());
        self.env = if peak > self.env { peak } else { self.env * self.rel };
        let target = if self.env > self.thr { self.thr / self.env } else { 1.0 };
        self.gain = if target < self.gain { target + (self.gain - target) * self.att } else { target };
        (soft_clip(l * self.gain), soft_clip(r * self.gain))
    }

    /// End of a segment: clear a tiny envelope.
    pub fn flush(&mut self) {
        self.env = flush(self.env);
    }

    /// Current gain reduction factor (1 = none).
    #[must_use]
    pub fn gain(&self) -> f32 {
        self.gain
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn delay_repeats_at_its_time_and_decays_to_silence() {
        let mut d = Delay::new();
        d.set(
            &DelayParams { time: 0.01, beats: 0.0, spread: 0.0, feedback: 0.5, damp: 0.0, level: 1.0 },
            1000.0,
            120.0,
        );
        let mut input = [0.0f32; 16];
        input[0] = 1.0;
        let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
        d.process(&input, &mut l, &mut r);
        assert_eq!(l[10], 1.0);
        assert!(l.iter().enumerate().all(|(i, &v)| i == 10 || v == 0.0));
        let zero = [0.0f32; 16];
        for _ in 0..2000 {
            d.process(&zero, &mut [0.0; 16], &mut [0.0; 16]);
        }
        assert!(d.is_idle());
    }

    #[test]
    fn reverb_decays_to_exact_silence() {
        let mut v = Reverb::new();
        let mut input = [0.0f32; 16];
        input[0] = 1.0;
        let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
        v.process(&input, &mut l, &mut r);
        let zero = [0.0f32; 16];
        let mut heard = false;
        for _ in 0..(48_000 * 12 / 16) {
            let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
            v.process(&zero, &mut l, &mut r);
            heard |= l.iter().chain(&r).any(|x| x.abs() > 1e-3);
        }
        assert!(heard);
        assert!(v.zero_run >= v.longest, "reverb still running after 12 s");
    }

    #[test]
    fn limiter_bounds_output() {
        let mut lim = Limiter::new(48_000.0);
        for i in 0..10_000 {
            let x = (i as f32 * 0.37).sin() * 8.0;
            let (l, r) = lim.frame(x, -x);
            assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
        }
        let (l, _) = lim.frame(f32::NAN, 0.0);
        assert_eq!(l, 0.0);
    }
}
