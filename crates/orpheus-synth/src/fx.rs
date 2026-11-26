//! Send effects (delay, chorus, reverb) and the master soft limiter.
//!
//! Each send effect takes a mono bus (the sum of the voices' sends) and adds
//! its wet stereo output to the master bus. Memory is fixed (sized for
//! 48 kHz, the highest engine rate):
//!
//! | Effect | Line | RAM |
//! |---|---|---|
//! | Delay | 12 288 × `i16`: 256 ms at full rate, up to 1.02 s at ¼ rate | 24 KiB |
//! | Chorus | 1 024 × `f32` | 4 KiB |
//! | Reverb | 4 096 × `i16` shared by 8 feedback lines and 3 diffusers at half rate (±1.0 at 1/32768; louder saturates) | 8 KiB |
//! | Limiter | — | 20 bytes |
//!
//! The `i16` lines store the signal at 1/8192 per step (±4.0 full scale)
//! and truncate toward zero when writing, so a decaying feedback loop
//! reaches exact silence instead of a limit cycle. An effect whose input has
//! been zero and whose lines hold only zeros skips its work entirely
//! (exactly equivalent: its output would be zero).
//!
//! * **Delay**: one line, left tap at the delay time, right tap `spread`
//!   later; in the feedback path a one-pole low pass (`damp`) and a one-pole
//!   high pass (`low_cut`), so repeats get darker and thinner. Times up to
//!   the line (256 ms at 48 kHz) are kept at full rate and are exact to the
//!   sample (tempo sync rounds beats × 60 / BPM × rate to the nearest
//!   sample). Longer times (to 1.02 s at 48 kHz: a quarter note at 59 BPM)
//!   store every k-th sample (k = 2…4, the mean of k inputs) and read back
//!   with linear interpolation at the exact time, with a two-pole low pass
//!   at 0.4 × rate / k before the line and on each tap (against aliasing and
//!   the interpolation's images; their delay is subtracted from the read
//!   position): the echo of a click is a short smear whose centre of mass
//!   is within a sample of the delay time; repeats lose the top above
//!   that cutoff (4.8 kHz for one second at 48 kHz), as a long tape or
//!   bucket-brigade delay does. Changing k clears the line.
//! * **Chorus**: two taps (left and right in quadrature) swept by a triangle
//!   LFO around 7 ms.
//! * **Reverb**: a feedback delay network (Jot and Chaigne, "Digital delay
//!   networks for designing artificial reverberators", 1991; Stautner and
//!   Puckette, 1982) at half the engine's rate (the mean of each input
//!   pair in, linear interpolation out: half the cycles and half the
//!   memory; a damped tail has little above 12 kHz): three series all-pass
//!   diffusers (4.8, 3.6, 2.7 ms,
//!   gain 0.6) into eight delay lines of 11.3–28.9 ms (mutually prime
//!   lengths), each with a one-pole low pass (`damp`) and a gain set from
//!   the decay time (RT60), mixed by an 8 × 8 Hadamard matrix (lossless:
//!   the decay comes from the gains alone, the same for every line per
//!   second). Left and right are two orthogonal sign patterns of the line
//!   outputs. Written from those descriptions. The lines hold ±1.0 at
//!   1/32768 and are written *rounded* while a value is above a threshold
//!   T = 0.5 / (1 − g) + 1 steps (g: the gain of the shortest line; above T
//!   one pass loses more than rounding can add back, so rounding cannot
//!   sustain a limit cycle) and *truncated toward zero* below it, so the
//!   tail decays at the set rate down to a few steps and then drains to
//!   exact zeros (the tail of a single loud hit keeps the set rate for its
//!   first 30 dB at decays up to 2.5 s, its first 15 dB at 10 s). (Truncation alone, as the first version did, is biased
//!   toward zero: a 0.8 s decay measured 0.48 s, the tail of a single hit
//!   being only tens of steps; error feedback did not help — its shaped
//!   noise still biased the high frequencies.)
//! * **Limiter**: a peak follower (instant attack, 100 ms release) sets the
//!   target gain threshold / peak above −0.45 dBFS; the applied gain follows
//!   the target down with a 1 ms time constant (an instant gain drop
//!   splatters: visible as broadband lines in the spectrogram of a dense
//!   mix) and up with the follower's release; [`soft_clip`] (identity below
//!   −1.9 dBFS) catches the overshoot of that millisecond, so the output
//!   never exceeds full scale.

use crate::filter::Svf;
use crate::math::{clampf, decay_coef, exp2, flush, soft_clip};

#[cfg_attr(not(feature = "delay"), allow(dead_code))]
const Q: f32 = 8192.0;
#[cfg_attr(not(feature = "delay"), allow(dead_code))]
const DQ: f32 = 1.0 / Q;

#[inline]
#[cfg_attr(not(feature = "delay"), allow(dead_code))]
fn quant(x: f32) -> i16 {
    sat16(x * Q)
}

/// `x as i16` (saturating, NaN → 0), through `i32`: the same values, but
/// one `vcvt` and one `ssat` on the M4 where the direct cast costs three
/// compares and three FPSCR reads.
#[inline]
pub(crate) fn sat16(x: f32) -> i16 {
    (x as i32).clamp(-32_768, 32_767) as i16
}

/// Delay line length, samples.
pub const DELAY_LEN: usize = 12_288;
/// Longest delay as a multiple of the line (the line then stores every
/// fourth sample).
pub const DELAY_MAX_K: usize = 4;
/// Chorus line length, samples.
pub const CHORUS_LEN: usize = 1024;
/// Reverb memory, samples.
pub const REVERB_LEN: usize = 4096;

/// Delay settings.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct DelayParams {
    /// Delay time, seconds. Times beyond 4 lines (1.02 s at 48 kHz) are
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
    /// High pass of the repeats, Hz (0 = off; 20–2000).
    pub low_cut: f32,
    /// Wet level, 0–1.
    pub level: f32,
}

impl DelayParams {
    /// Defaults: half a beat (250 ms at 120 BPM), a little spread, 35 %
    /// feedback, repeats cut below 120 Hz.
    pub const DEFAULT: Self =
        Self { time: 0.3, beats: 0.5, spread: 0.011, feedback: 0.35, damp: 0.3, low_cut: 120.0, level: 0.6 };
}

/// The delay.
#[derive(bytemuck::Zeroable, Clone, Debug)]
pub struct Delay {
    line: [i16; DELAY_LEN],
    pos: usize,
    /// Delay of the left and right taps, samples.
    time: usize,
    spread: usize,
    /// Samples per line sample (1–4) and the inputs summed so far.
    k: usize,
    cnt: usize,
    acc: f32,
    inv_k: f32,
    /// k > 1: per `cnt`, the read offset behind the newest line sample and
    /// the interpolation fraction, for the left and right taps.
    taps: [[(usize, f32); DELAY_MAX_K]; 2],
    fb: f32,
    damp: f32,
    lp: f32,
    hp_c: f32,
    hp: f32,
    level: f32,
    zero_run: usize,
    /// k > 1: low passes before the line and on the two taps.
    aa: [Svf; 3],
    params: DelayParams,
}

impl Delay {
    /// A silent delay with the default settings.
    #[must_use]
    pub fn new() -> Self {
        let mut d = <Self as bytemuck::Zeroable>::zeroed();
        d.init();
        d
    }

    /// [`Delay::new`] in place (no copy of the 24 KiB line through the
    /// stack: the player builds the engine in the decoder arena).
    pub fn init(&mut self) {
        self.line.fill(0);
        self.pos = 0;
        self.time = 1;
        self.spread = 0;
        self.k = 1;
        self.cnt = 0;
        self.acc = 0.0;
        self.inv_k = 1.0;
        self.taps = [[(0, 0.0); DELAY_MAX_K]; 2];
        self.fb = 0.0;
        self.damp = 0.0;
        self.lp = 0.0;
        self.hp_c = 1.0;
        self.hp = 0.0;
        self.level = 0.0;
        self.zero_run = DELAY_LEN;
        self.aa = [Svf::ZERO; 3];
        self.params = DelayParams::DEFAULT;
        self.set(&DelayParams::DEFAULT, 48_000.0, 120.0);
    }

    /// Current settings.
    #[must_use]
    pub fn params(&self) -> DelayParams {
        self.params
    }

    /// The delay of the left tap in samples and the line's decimation.
    #[must_use]
    pub fn timing(&self) -> (usize, usize) {
        (self.time, self.k)
    }

    /// Apply settings at `rate` Hz and `bpm`.
    pub fn set(&mut self, p: &DelayParams, rate: f32, bpm: f32) {
        // Built without the delay (the player's MIDI build, SYN-3: no
        // MIDI controller feeds it): the settings are kept, nothing sounds.
        #[cfg(not(feature = "delay"))]
        {
            let _ = (rate, bpm);
            self.params = *p;
            return;
        }
        #[cfg(feature = "delay")]
        self.set_on(p, rate, bpm);
    }

    #[cfg(feature = "delay")]
    fn set_on(&mut self, p: &DelayParams, rate: f32, bpm: f32) {
        let beats = clampf(p.beats, 0.0, 16.0);
        let t = if beats > 0.0 { beats * 60.0 / bpm } else { clampf(p.time, 0.0, 10.0) };
        let max = (DELAY_MAX_K * (DELAY_LEN - 4)) as f32;
        let time = clampf(t * rate + 0.5, 1.0, max) as usize;
        let k = if time <= DELAY_LEN - 2 { 1 } else { time.div_ceil(DELAY_LEN - 4).min(DELAY_MAX_K) };
        if k != self.k {
            self.k = k;
            self.clear();
        }
        self.time = time;
        self.inv_k = 1.0 / k as f32;
        let spread = (clampf(p.spread, 0.0, 0.05) * rate) as usize;
        let room = if k == 1 { DELAY_LEN - 2 } else { k * (DELAY_LEN - 4) };
        self.spread = spread.min(room - time);
        if k > 1 {
            for f in &mut self.aa {
                f.set(0.4 * rate / k as f32, 0.0, rate);
            }
            // The two low passes in the echo path (Q ½, trapezoidal) delay
            // it by k/(2g) = 1/g samples each at low frequencies, g =
            // tan(π·fc/rate): the line is read that much earlier.
            let comp = 2.0 / crate::math::tan_lo(core::f32::consts::PI * 0.4 / k as f32);
            for (tap, d) in self.taps.iter_mut().zip([time, time + self.spread]) {
                for (c, t) in tap.iter_mut().enumerate().take(k) {
                    // Back from the newest line sample (which holds the k
                    // inputs centred (k + 1)/2 + c samples ago), in line
                    // samples.
                    let b = (d as f32 - comp - c as f32 - (k + 1) as f32 * 0.5) / k as f32;
                    let bi = b as usize;
                    *t = (bi, b - bi as f32);
                }
            }
        }
        self.fb = clampf(p.feedback, 0.0, 0.95);
        self.damp = clampf(p.damp, 0.0, 0.95);
        let low_cut = if p.low_cut >= 20.0 { clampf(p.low_cut, 20.0, 2000.0) } else { 0.0 };
        self.hp_c =
            if low_cut > 0.0 { decay_coef(1.0 / (core::f32::consts::TAU * low_cut), rate) } else { 1.0 };
        self.level = clampf(p.level, 0.0, 1.0);
        self.params = DelayParams {
            time: clampf(p.time, 0.0, 10.0),
            beats,
            spread: clampf(p.spread, 0.0, 0.05),
            feedback: self.fb,
            damp: self.damp,
            low_cut,
            level: self.level,
        };
    }

    /// Clear the line.
    pub fn clear(&mut self) {
        self.line.fill(0);
        self.lp = 0.0;
        self.hp = 0.0;
        self.acc = 0.0;
        self.cnt = 0;
        self.zero_run = DELAY_LEN;
        for f in &mut self.aa {
            f.reset();
        }
    }

    /// Whether the last call did no work.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.zero_run >= DELAY_LEN
    }

    /// The feedback filter: low pass (damping), then high pass (low cut).
    #[inline]
    #[cfg_attr(not(feature = "delay"), allow(dead_code))]
    fn filter(&mut self, y: f32) -> f32 {
        self.lp = y + (self.lp - y) * self.damp;
        if self.hp_c >= 1.0 {
            return self.lp;
        }
        self.hp = self.lp + (self.hp - self.lp) * self.hp_c;
        self.lp - self.hp
    }

    /// Process the mono send `input`, adding the wet signal to `l` and `r`
    /// (nothing without the `delay` feature).
    pub fn process(&mut self, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        #[cfg(not(feature = "delay"))]
        {
            let _ = (input, l, r);
            return;
        }
        #[cfg(feature = "delay")]
        self.process_on(input, l, r);
    }

    #[cfg(feature = "delay")]
    fn process_on(&mut self, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        if self.zero_run >= DELAY_LEN && input.iter().all(|&x| x == 0.0) {
            self.lp = 0.0;
            self.hp = 0.0;
            for f in &mut self.aa {
                f.reset();
            }
            // Keep the decimation phase: it decides where later inputs land.
            self.cnt = (self.cnt + input.len()) % self.k;
            return;
        }
        let wrap = |i: usize| if i >= DELAY_LEN { i - DELAY_LEN } else { i };
        if self.k == 1 {
            let (time, spread) = (self.time, self.spread);
            for ((x, l), r) in input.iter().zip(l.iter_mut()).zip(r.iter_mut()) {
                let y = f32::from(self.line[wrap(self.pos + DELAY_LEN - time)]) * DQ;
                let y2 = f32::from(self.line[wrap(self.pos + DELAY_LEN - time - spread)]) * DQ;
                let f = self.filter(y);
                let q = quant(*x + f * self.fb);
                self.line[self.pos] = q;
                self.zero_run = if q == 0 { self.zero_run + 1 } else { 0 };
                *l += y * self.level;
                *r += y2 * self.level;
                self.pos = wrap(self.pos + 1);
            }
            return;
        }
        let read = |line: &[i16; DELAY_LEN], newest: usize, (back, fr): (usize, f32)| {
            let a = f32::from(line[wrap(newest + DELAY_LEN - back)]);
            let b = f32::from(line[wrap(newest + DELAY_LEN - back - 1)]);
            (a + (b - a) * fr) * DQ
        };
        for ((x, l), r) in input.iter().zip(l.iter_mut()).zip(r.iter_mut()) {
            let newest = wrap(self.pos + DELAY_LEN - 1);
            let y = self.aa[1].tick(read(&self.line, newest, self.taps[0][self.cnt])).0;
            let y2 = self.aa[2].tick(read(&self.line, newest, self.taps[1][self.cnt])).0;
            let f = self.filter(y);
            self.acc += self.aa[0].tick(*x + f * self.fb).0;
            self.cnt += 1;
            if self.cnt == self.k {
                let q = quant(self.acc * self.inv_k);
                self.line[self.pos] = q;
                self.zero_run = if q == 0 { self.zero_run + 1 } else { 0 };
                self.pos = wrap(self.pos + 1);
                self.acc = 0.0;
                self.cnt = 0;
            }
            *l += y * self.level;
            *r += y2 * self.level;
        }
    }

    /// Clear tiny state (on control ticks).
    pub fn flush(&mut self) {
        self.lp = flush(self.lp);
        self.hp = flush(self.hp);
        self.acc = flush(self.acc);
        for f in &mut self.aa {
            f.flush();
        }
    }
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

/// Chorus settings.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
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
#[derive(bytemuck::Zeroable, Clone, Debug)]
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
        let mut c = <Self as bytemuck::Zeroable>::zeroed();
        c.init();
        c
    }

    /// [`Chorus::new`] in place.
    pub fn init(&mut self) {
        self.line.fill(0.0);
        self.pos = 0;
        self.phase = 0;
        self.inc = 0;
        self.base = 0.0;
        self.depth = 0.0;
        self.level = 0.0;
        self.zero_run = CHORUS_LEN;
        self.params = ChorusParams::DEFAULT;
        self.set(&ChorusParams::DEFAULT, 48_000.0);
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
        self.line.fill(0.0);
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
            self.line[self.pos % CHORUS_LEN] = *x;
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
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
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

/// Feedback lines.
pub const REVERB_LINES: usize = 8;
const LINE_MS: [f32; REVERB_LINES] = [11.3, 13.1, 15.7, 17.9, 20.3, 22.9, 25.7, 28.9];
const DIFF_MS: [f32; 3] = [4.77, 3.59, 2.73];
const DIFF_G: f32 = 0.6;
const IN_GAIN: f32 = 0.3;
const OUT_GAIN: f32 = 0.3;
/// 1/√8: the Hadamard matrix's normalisation.
const H8: f32 = 0.353_553_38;
/// Reverb line scale: ±1.0 at 1/32768.
const RQ: f32 = 32_768.0;
const RDQ: f32 = 1.0 / RQ;
/// The input's signs into the lines: orthogonal to both output patterns,
/// and spread over four lines by the first mix (a Hadamard row would land
/// in one line).
const IN_SIGN: [f32; REVERB_LINES] = [1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, 1.0];
/// Network steps per block (32 input samples).
const BLOCK_STEPS: usize = 17;

#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default)]
struct Line {
    start: usize,
    len: usize,
    pos: usize,
}

impl Line {
    /// The next `out.len()` (≤ `len`) samples from the current position
    /// (two copies at most: the run to the end, the run from the start).
    #[inline]
    fn load(&self, mem: &[i16], out: &mut [i16]) {
        // (Through `get`: the ranges are in the line by construction; the
        // player's image then has no panic path here, SYN-3.)
        let line = mem.get(self.start..self.start + self.len).unwrap_or_default();
        let n = out.len().min(self.len.saturating_sub(self.pos));
        let (a, b) = out.split_at_mut(n);
        if let Some(src) = line.get(self.pos..self.pos + n) {
            a.copy_from_slice(src);
        }
        if let Some(src) = line.get(..b.len()) {
            b.copy_from_slice(src);
        }
    }

    /// Write `vals` where [`Line::load`] read and move on past them.
    #[inline]
    fn store(&mut self, mem: &mut [i16], vals: &[i16]) {
        let left = self.len.saturating_sub(self.pos);
        let n = vals.len().min(left);
        let (a, b) = vals.split_at(n);
        if let Some(line) = mem.get_mut(self.start..self.start + self.len) {
            if let Some(dst) = line.get_mut(self.pos..self.pos + n) {
                dst.copy_from_slice(a);
            }
            if let Some(dst) = line.get_mut(..b.len()) {
                dst.copy_from_slice(b);
            }
        }
        self.pos = if n == left { b.len() } else { self.pos + n };
    }
}

/// In-place fast Walsh–Hadamard transform of 8 values (unnormalised),
/// written out (24 additions in registers).
#[inline]
fn hadamard8(v: &mut [f32; REVERB_LINES]) {
    let [a, b, c, d, e, f, g, h] = *v;
    let (a, b, c, d, e, f, g, h) = (a + b, a - b, c + d, c - d, e + f, e - f, g + h, g - h);
    let (a, b, c, d, e, f, g, h) = (a + c, b + d, a - c, b - d, e + g, f + h, e - g, f - h);
    *v = [a + e, b + f, c + g, d + h, a - e, b - f, c - g, d - h];
}

/// The reverb.
#[derive(bytemuck::Zeroable, Clone, Debug)]
pub struct Reverb {
    mem: [i16; REVERB_LEN],
    line: [Line; REVERB_LINES],
    diff: [Line; 3],
    g: [f32; REVERB_LINES],
    lp: [f32; REVERB_LINES],
    /// Steps (1/32768) above which line writes round rather than truncate.
    round_above: i32,
    damp: f32,
    level: f32,
    longest: usize,
    /// Input samples per block: fewer network steps than the shortest line.
    chunk: usize,
    /// Network steps whose writes were all zero.
    zero_run: usize,
    /// The network's rate: half the engine's.
    rate: f32,
    /// Second sample of an input pair next; the pair's first input; the
    /// network's last two outputs (left, right).
    odd: bool,
    first: f32,
    prev: [f32; 2],
    cur: [f32; 2],
    params: ReverbParams,
}

impl Reverb {
    /// A silent reverb with the default settings at 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut r = <Self as bytemuck::Zeroable>::zeroed();
        r.init();
        r
    }

    /// [`Reverb::new`] in place.
    pub fn init(&mut self) {
        self.mem.fill(0);
        self.line = [Line::default(); REVERB_LINES];
        self.diff = [Line::default(); 3];
        self.g = [0.0; REVERB_LINES];
        self.lp = [0.0; REVERB_LINES];
        self.round_above = 1;
        self.damp = 0.0;
        self.level = 0.0;
        self.longest = 1;
        self.chunk = 2;
        self.zero_run = REVERB_LEN;
        self.rate = 24_000.0;
        self.odd = false;
        self.first = 0.0;
        self.prev = [0.0; 2];
        self.cur = [0.0; 2];
        self.params = ReverbParams::DEFAULT;
        self.set_rate(48_000.0);
    }

    /// Lay out the lines for an engine at `rate` Hz (≤ 48 kHz; the network
    /// runs at half of it) and clear them.
    pub fn set_rate(&mut self, rate: f32) {
        let rate = clampf(rate, 8000.0, 48_000.0) * 0.5;
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
        let line = LINE_MS.map(&mut lay);
        let diff = DIFF_MS.map(&mut lay);
        debug_assert!(start <= REVERB_LEN);
        let shortest = line.iter().chain(&diff).map(|l| l.len).min().unwrap_or(1);
        self.chunk = 2 * (BLOCK_STEPS.min(shortest) - 1).max(1);
        self.line = line;
        self.diff = diff;
        self.longest = longest;
        self.clear();
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
        let gain = |len: usize| exp2(-3.0 * core::f32::consts::LOG2_10 * len as f32 / (decay * self.rate));
        for (g, c) in self.g.iter_mut().zip(&self.line) {
            // 10^(−3·L/(RT60·fs)), times the Hadamard normalisation.
            *g = gain(c.len) * H8;
        }
        let shortest = self.line.iter().map(|c| c.len).min().unwrap_or(1);
        // ⌈0.5 / (1 − g) + 1⌉ steps.
        let t = clampf(0.5 / (1.0 - gain(shortest)) + 1.0, 1.0, 30_000.0);
        self.round_above = if (t as i32) as f32 == t { t as i32 } else { t as i32 + 1 };
        self.damp = clampf(p.damp, 0.0, 0.9);
        self.level = clampf(p.level, 0.0, 1.0);
        self.params = ReverbParams { decay, damp: self.damp, level: self.level };
    }

    /// Clear the lines.
    pub fn clear(&mut self) {
        self.mem.fill(0);
        self.lp = [0.0; REVERB_LINES];
        self.zero_run = REVERB_LEN;
        self.first = 0.0;
        self.prev = [0.0; 2];
        self.cur = [0.0; 2];
    }

    /// Whether the lines hold only zeros (the tail has ended).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.zero_run >= self.longest && self.cur == [0.0; 2] && self.prev == [0.0; 2]
    }

    /// Run the network for the inputs `xs` (at most [`BLOCK_STEPS`]),
    /// writing each step's (left, right) to `outs`. Line by line over the
    /// block rather than step by step: every line and diffuser is longer
    /// than a block, so no step reads what another step of the same block
    /// wrote, and each value is computed exactly as one step at a time would
    /// (the same operations in the same order per value).
    fn steps(&mut self, xs: &[f32], outs: &mut [[f32; 2]]) {
        let m = xs.len().min(self.chunk / 2 + 1).min(BLOCK_STEPS);
        // OR of every value written in the block (zero: the block wrote
        // only zeros).
        let mut any = 0i32;
        // Input diffusion: series all-passes, each over the block.
        let mut d = [0.0f32; BLOCK_STEPS];
        for (d, x) in d.iter_mut().zip(&xs[..m]) {
            *d = *x * IN_GAIN;
        }
        let mut cells = [0i16; BLOCK_STEPS];
        for a in &mut self.diff {
            a.load(&self.mem, &mut cells[..m]);
            for (cell, d) in cells[..m].iter_mut().zip(&mut d[..m]) {
                let z = f32::from(*cell) * RDQ;
                let w = *d + DIFF_G * z;
                *d = z - DIFF_G * w;
                let q = sat16(w * RQ);
                *cell = q;
                any |= i32::from(q);
            }
            a.store(&mut self.mem, &cells[..m]);
        }
        // Read the lines, damp, scale: v[k][j] for line k at step j.
        let mut y = [[0.0f32; BLOCK_STEPS]; REVERB_LINES];
        let mut v = [[0.0f32; BLOCK_STEPS]; REVERB_LINES];
        let damp = self.damp;
        for k in 0..REVERB_LINES {
            let c = self.line[k];
            let (g, mut lp) = (self.g[k], self.lp[k]);
            c.load(&self.mem, &mut cells[..m]);
            for ((cell, yk), vk) in cells[..m].iter().zip(&mut y[k][..m]).zip(&mut v[k][..m]) {
                let s = f32::from(*cell) * RDQ;
                lp = s + (lp - s) * damp;
                *yk = s;
                *vk = lp * g;
            }
            self.lp[k] = lp;
        }
        // Mix each step.
        for j in 0..m {
            let mut col = [v[0][j], v[1][j], v[2][j], v[3][j], v[4][j], v[5][j], v[6][j], v[7][j]];
            hadamard8(&mut col);
            for (vk, c) in v.iter_mut().zip(col) {
                vk[j] = c;
            }
        }
        // Write back where each step read; round above the threshold,
        // truncate toward zero below.
        // In integers: r = trunc(2z) (exact: |z| < 2^23); trunc(z) is r / 2
        // and z rounded half away from zero is (r ± 1) / 2 — the same values
        // as rounding in f32, with no float compare.
        let t2 = 2 * self.round_above;
        for k in 0..REVERB_LINES {
            let c = &mut self.line[k];
            let sign = IN_SIGN[k] * (2.0 * RQ);
            for ((cell, vk), dj) in cells[..m].iter_mut().zip(&v[k][..m]).zip(&d[..m]) {
                let r = (*vk * (2.0 * RQ) + *dj * sign) as i32;
                let q = if r.abs() >= t2 { (r + ((r >> 31) | 1)) / 2 } else { r / 2 };
                let q = q.clamp(-32_768, 32_767) as i16;
                *cell = q;
                any |= i32::from(q);
            }
            c.store(&mut self.mem, &cells[..m]);
        }
        // Steps of all-zero writes, counted by whole blocks (a block with
        // any non-zero write restarts the count): when it reaches the
        // longest line, every line holds zeros, whatever the block sizes.
        self.zero_run = if any != 0 { 0 } else { self.zero_run + m };
        let g = OUT_GAIN * self.level;
        for (j, o) in outs.iter_mut().enumerate().take(m) {
            let s = |k: usize| y[k][j];
            let left = (s(0) - s(1)) + (s(2) - s(3)) + (s(4) - s(5)) + (s(6) - s(7));
            let right = (s(0) + s(1)) - (s(2) + s(3)) + (s(4) + s(5)) - (s(6) + s(7));
            *o = [left * g, right * g];
        }
    }

    /// Process the mono send `input`, adding the wet signal to `l` and `r`.
    /// The network takes the mean of each pair of inputs and its outputs
    /// are interpolated linearly back to the full rate (two samples later).
    pub fn process(&mut self, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        if self.is_idle() && input.iter().all(|&x| x == 0.0) {
            self.lp = [0.0; REVERB_LINES];
            // Keep the pairing: it decides which inputs are averaged.
            self.odd ^= input.len() % 2 == 1;
            return;
        }
        let chunk = self.chunk.max(1);
        for ((input, l), r) in input.chunks(chunk).zip(l.chunks_mut(chunk)).zip(r.chunks_mut(chunk)) {
            // The network inputs this chunk completes.
            let mut xs = [0.0f32; BLOCK_STEPS];
            let mut m = 0;
            let (mut odd, mut first) = (self.odd, self.first);
            for x in input {
                if odd {
                    if let Some(s) = xs.get_mut(m) {
                        *s = (first + *x) * 0.5;
                    }
                    m += 1;
                } else {
                    first = *x;
                }
                odd = !odd;
            }
            let mut outs = [[0.0f32; 2]; BLOCK_STEPS];
            self.steps(&xs[..m.min(BLOCK_STEPS)], &mut outs);
            let mut j = 0;
            let (mut odd, mut prev, mut cur) = (self.odd, self.prev, self.cur);
            for (l, r) in l.iter_mut().zip(r.iter_mut()) {
                if odd {
                    *l += cur[0];
                    *r += cur[1];
                    prev = cur;
                    cur = outs[j.min(BLOCK_STEPS - 1)];
                    j += 1;
                } else {
                    *l += (prev[0] + cur[0]) * 0.5;
                    *r += (prev[1] + cur[1]) * 0.5;
                }
                odd = !odd;
            }
            if let Some(x) = input.last()
                && odd
            {
                // The chunk ends inside a pair: its first input waits.
                self.first = *x;
            }
            (self.odd, self.prev, self.cur) = (odd, prev, cur);
        }
    }

    /// Clear tiny state (on control ticks).
    pub fn flush(&mut self) {
        for v in &mut self.lp {
            *v = flush(*v);
        }
        self.first = flush(self.first);
    }
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

/// The master soft limiter.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
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
            &DelayParams {
                time: 0.01,
                beats: 0.0,
                spread: 0.0,
                feedback: 0.5,
                damp: 0.0,
                low_cut: 0.0,
                level: 1.0,
            },
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
    fn reverb_blocks_do_not_change_the_output_at_any_rate() {
        // The network runs line by line over blocks no longer than its
        // shortest line; cutting the input differently must not change a
        // sample, also at 8 kHz where a diffuser is only 10 steps long.
        for rate in [8000.0, 22_050.0, 48_000.0] {
            let run = |cut: usize| {
                let mut v = Reverb::new();
                v.set_rate(rate);
                v.set(&ReverbParams { decay: 3.0, damp: 0.3, level: 1.0 });
                let n = 20_000;
                let x: std::vec::Vec<f32> = (0..n)
                    .map(|i| if i % 3001 < 40 { ((i * 7) % 13) as f32 * 0.05 - 0.3 } else { 0.0 })
                    .collect();
                let (mut l, mut r) = (std::vec![0.0f32; n], std::vec![0.0f32; n]);
                let mut at = 0;
                while at < n {
                    let k = cut.min(n - at);
                    v.process(&x[at..at + k], &mut l[at..at + k], &mut r[at..at + k]);
                    at += k;
                }
                (l, r)
            };
            let a = run(1);
            assert!(a.0.iter().any(|&s| s != 0.0));
            for cut in [3, 32, 1000] {
                assert!(run(cut) == a, "rate {rate}, blocks of {cut}");
            }
        }
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
