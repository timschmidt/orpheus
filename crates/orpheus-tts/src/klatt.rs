//! The formant synthesiser's signal path at 16 kHz: a cascade/parallel
//! formant synthesiser after Klatt (1980), "Software for a cascade/parallel
//! formant synthesizer", JASA 67(3), written from the paper's description
//! (no code copied), with a KLGLOTT88-like voicing source (Klatt & Klatt
//! 1990, JASA 87(2)).
//!
//! ```text
//!            ┌── AV · glottal flow derivative (polynomial, polyBLEP) ─┐
//!  voicing ──┤                                                        ├─► tilt ─┐
//!            └── AH · noise (×½ while the glottis is closed) ─────────┘         │
//!                                                                               ▼
//!   cascade: nasal pole ─► nasal zero ─► R6 ─► R5 ─► R4 ─► R3 ─► R2 ─► R1 ────► (+) ─► out
//!                                                                               ▲
//!   parallel: AF · noise ─► ±A2·P2 ∓ A3·P3 ± A4·P4 ∓ A5·P5 ± A6·P6 + AB·bypass ┘
//! ```
//!
//! * **Resonator** (Klatt 1980, eq. 1–3): `y[n] = A·x[n] + B·y[n−1] +
//!   C·y[n−2]` with `C = −e^(−2π·BW·T)`, `B = 2e^(−π·BW·T)·cos(2π·F·T)`,
//!   `A = 1 − B − C` (unity gain at DC: the cascade's formant levels then
//!   follow from the frequencies, as in natural vowels). The parallel
//!   resonators are normalised to about unity gain at their peak instead,
//!   so their amplitudes are spectrum levels.
//! * **Antiresonator** (the nasal zero): the inverse, `y = A'x[n] +
//!   B'x[n−1] + C'x[n−2]`, `A' = 1/A`, `B' = −B/A`, `C' = −C/A`. With the
//!   zero on the nasal pole they cancel (an oral sound).
//! * **Voicing source**: the flow of KLGLOTT88, `U(x) = (27/4)(x² − x³)`
//!   over the open phase `x = t/(OQ·T0) ∈ [0, 1]` (peak flow 1 at
//!   x = 2/3), zero while closed; its derivative `(27/4)(2x − 3x²)/OQ` is
//!   the excitation (the lip radiation's differentiation folded in). The
//!   step at the closure instant is band-limited with a two-sample polyBLEP
//!   at its fractional position (the phase is continuous, so F0 is not
//!   quantised to whole samples). F0 and OQ change at the start of a
//!   period. A one-pole low-pass gives the spectral tilt (TL); aspiration
//!   noise is added at half amplitude in the closed phase (Klatt 1980's
//!   modulation of AH by the glottal cycle). Before the tilt, a one-zero
//!   pre-emphasis `x[n] − 0.65·x[n−1]` lifts the source by ≈ 8 dB at
//!   2–3 kHz relative to F1: the polynomial pulse alone left F2 and F3 of
//!   /i/ ≈ 28 dB under F1, natural voices ≈ 20 dB (Peterson & Barney
//!   1952's formant levels).
//! * **F6**: at 16 kHz the cascade has a fixed sixth formant (4.9 kHz ×
//!   the voice's scale, as Klatt (1980) suggests for higher sampling
//!   rates), so voiced sounds have energy above 4 kHz.
//! * **Noise**: uniform white noise (xorshift32), the sum of two draws
//!   (triangular, less peaky).
//!
//! Everything is `f32` with IEEE basic operations only (no fused
//! multiply-add, no `libm`), so the samples are the same on the host and on
//! the player (orpheus-synth's rule, 04 §9). Parameters change once per
//! control tick ([`TICK`] samples, 1 ms): a filter's coefficients are
//! redesigned only when its frequency or bandwidth changed; amplitudes ramp
//! linearly over the tick. Each filter runs over the whole tick with its
//! state in registers (on the M4 the per-sample loop spilled 120 loads and
//! stores when all filters ran sample by sample; now 7).

use orpheus_synth::math::{Rng, clampf, exp2, flush, sin_turns};

/// Internal sample rate (Hz).
pub const FS: u32 = 16_000;
/// Samples per control tick (1 ms).
pub const TICK: usize = 16;

const T: f32 = 1.0 / FS as f32;
const LOG2_E: f32 = core::f32::consts::LOG2_E;
const PI: f32 = core::f32::consts::PI;

/// e^(−π·bw·T).
#[inline]
fn pole_radius(bw: f32) -> f32 {
    exp2(-PI * bw * T * LOG2_E)
}

/// cos(2π·f·T) and sin(2π·f·T).
#[inline]
fn cos_sin(f: f32) -> (f32, f32) {
    let p = f * T;
    (sin_turns(p + 0.25), sin_turns(p))
}

/// A two-pole digital resonator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resonator {
    a: f32,
    b: f32,
    c: f32,
    y1: f32,
    y2: f32,
    /// The (f, bw) the coefficients were designed for (NaN: none yet).
    key: [f32; 2],
}

impl Default for Resonator {
    fn default() -> Resonator {
        Resonator { a: 0.0, b: 0.0, c: 0.0, y1: 0.0, y2: 0.0, key: [f32::NAN; 2] }
    }
}

impl Resonator {
    /// Klatt's design: centre `f`, bandwidth `bw` (Hz), unity gain at DC.
    /// Nothing is recomputed when `f` and `bw` are unchanged.
    pub fn set(&mut self, f: f32, bw: f32) {
        if self.key == [f, bw] {
            return;
        }
        self.key = [f, bw];
        let r = pole_radius(bw);
        let (cs, _) = cos_sin(f);
        self.c = -(r * r);
        self.b = 2.0 * r * cs;
        self.a = 1.0 - self.b - self.c;
    }

    /// The same poles with about unity gain at the peak (the parallel
    /// branch): `A ≈ (1 − r)·2·sin θ` (the peak gain of the poles alone is
    /// ≈ 1/((1 − r)·2 sin θ) for r near 1).
    pub fn set_peak(&mut self, f: f32, bw: f32) {
        if self.key == [f, bw] {
            return;
        }
        self.key = [f, bw];
        let r = pole_radius(bw);
        let (cs, sn) = cos_sin(f);
        self.c = -(r * r);
        self.b = 2.0 * r * cs;
        self.a = (1.0 - r) * 2.0 * sn;
    }

    /// One sample.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.a * x + self.b * self.y1 + self.c * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// Filters one tick in place (the state stays in registers: the
    /// samples are the same as [`Resonator::tick`]'s).
    #[inline]
    pub fn block(&mut self, buf: &mut [f32; TICK]) {
        let (a, b, c) = (self.a, self.b, self.c);
        let (mut y1, mut y2) = (self.y1, self.y2);
        for v in buf.iter_mut() {
            let y = a * *v + b * y1 + c * y2;
            y2 = y1;
            y1 = y;
            *v = y;
        }
        self.y1 = y1;
        self.y2 = y2;
    }

    /// Flushes tiny state to zero (keeps it out of the subnormal range).
    pub fn flush(&mut self) {
        self.y1 = flush(self.y1);
        self.y2 = flush(self.y2);
    }

    /// Largest state magnitude.
    pub fn level(&self) -> f32 {
        self.y1.abs().max(self.y2.abs())
    }

    /// Clears the state.
    pub fn clear(&mut self) {
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// A two-zero antiresonator (the nasal zero).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AntiResonator {
    a: f32,
    b: f32,
    c: f32,
    x1: f32,
    x2: f32,
    key: [f32; 2],
}

impl Default for AntiResonator {
    fn default() -> AntiResonator {
        AntiResonator { a: 0.0, b: 0.0, c: 0.0, x1: 0.0, x2: 0.0, key: [f32::NAN; 2] }
    }
}

impl AntiResonator {
    /// Klatt's design: the inverse of [`Resonator::set`] (unity gain at DC).
    pub fn set(&mut self, f: f32, bw: f32) {
        if self.key == [f, bw] {
            return;
        }
        self.key = [f, bw];
        let r = pole_radius(bw);
        let (cs, _) = cos_sin(f);
        let c = -(r * r);
        let b = 2.0 * r * cs;
        let a = 1.0 - b - c;
        let inv = 1.0 / a;
        self.a = inv;
        self.b = -b * inv;
        self.c = -c * inv;
    }

    /// One sample.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.a * x + self.b * self.x1 + self.c * self.x2;
        self.x2 = self.x1;
        self.x1 = x;
        y
    }

    /// Filters one tick in place.
    #[inline]
    pub fn block(&mut self, buf: &mut [f32; TICK]) {
        let (a, b, c) = (self.a, self.b, self.c);
        let (mut x1, mut x2) = (self.x1, self.x2);
        for v in buf.iter_mut() {
            let x = *v;
            *v = a * x + b * x1 + c * x2;
            x2 = x1;
            x1 = x;
        }
        self.x1 = x1;
        self.x2 = x2;
    }

    /// Flushes tiny state.
    pub fn flush(&mut self) {
        self.x1 = flush(self.x1);
        self.x2 = flush(self.x2);
    }

    /// Clears the state.
    pub fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
    }
}

/// Peak of the flow derivative at the closure: (27/4)·(2 − 3) = −27/4.
const CLOSE_STEP: f32 = 6.75;

/// The KLGLOTT88-like voicing source (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glottis {
    /// Position in the period, [0, 1).
    phase: f32,
    /// F0/FS of the running period.
    dt: f32,
    /// Open quotient of the running period.
    oq: f32,
    inv_oq: f32,
    /// What the next period uses.
    next_dt: f32,
    next_oq: f32,
    /// The sample being built (one sample of delay for the polyBLEP).
    pending: f32,
}

impl Default for Glottis {
    fn default() -> Glottis {
        Glottis {
            phase: 0.0,
            dt: 0.0075,
            oq: 0.6,
            inv_oq: 1.0 / 0.6,
            next_dt: 0.0075,
            next_oq: 0.6,
            pending: 0.0,
        }
    }
}

impl Glottis {
    /// F0 (Hz, 40–600) and open quotient (0.3–0.8) from the next period on.
    pub fn set(&mut self, f0: f32, oq: f32) {
        self.next_dt = clampf(f0, 40.0, 600.0) * T;
        self.next_oq = clampf(oq, 0.3, 0.8);
    }

    /// Whether the glottis is open (for the aspiration's modulation).
    pub fn open(&self) -> bool {
        self.phase < self.oq
    }

    /// One sample of the flow derivative (peak magnitude 6.75/OQ).
    #[inline]
    pub fn tick(&mut self) -> f32 {
        let p0 = self.phase;
        let mut p = p0 + self.dt;
        if p >= 1.0 {
            p -= 1.0;
            self.dt = self.next_dt;
            self.oq = self.next_oq;
            self.inv_oq = 1.0 / self.oq;
        }
        let oq = self.oq;
        let mut now = if p < oq {
            let x = p * self.inv_oq;
            6.75 * x * (2.0 - 3.0 * x) * self.inv_oq
        } else {
            0.0
        };
        if p0 < oq && p >= oq {
            // The closure step (from −6.75/OQ up to 0) fell inside this
            // sample, `d` samples ago: two-sample polyBLEP.
            let h = CLOSE_STEP * self.inv_oq;
            let d = (p - oq) / self.dt;
            let e = 1.0 - d;
            now -= h * e * e * 0.5;
            self.pending += h * d * d * 0.5;
        }
        self.phase = p;
        let out = self.pending;
        self.pending = now;
        out
    }
}

/// The synthesiser's parameters for one control tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Fundamental frequency (Hz).
    pub f0: f32,
    /// Open quotient.
    pub oq: f32,
    /// Voicing amplitude (linear).
    pub av: f32,
    /// Aspiration amplitude into the cascade (linear).
    pub ah: f32,
    /// Frication amplitude into the parallel branch (linear).
    pub af: f32,
    /// Spectral tilt: the one-pole low-pass coefficient (0 = flat).
    pub tilt: f32,
    /// Cascade formants F1–F6 (Hz).
    pub f: [f32; 6],
    /// Their bandwidths (Hz).
    pub b: [f32; 6],
    /// Nasal pole and zero (Hz; equal = oral).
    pub fnp: f32,
    /// Nasal zero (Hz).
    pub fnz: f32,
    /// Parallel resonator frequencies P2–P6 (Hz).
    pub pf: [f32; 5],
    /// Parallel bandwidths (Hz).
    pub pb: [f32; 5],
    /// Parallel amplitudes A2–A6 and the bypass AB (linear).
    pub pa: [f32; 6],
}

impl Frame {
    /// Silence with neutral formants.
    pub const SILENT: Frame = Frame {
        f0: 110.0,
        oq: 0.6,
        av: 0.0,
        ah: 0.0,
        af: 0.0,
        tilt: 0.0,
        f: [500.0, 1500.0, 2500.0, 3300.0, 3750.0, 4900.0],
        b: [60.0, 90.0, 150.0, 250.0, 300.0, 1000.0],
        fnp: 270.0,
        fnz: 270.0,
        pf: [1500.0, 2500.0, 3300.0, 4500.0, 6000.0],
        pb: [200.0, 300.0, 400.0, 600.0, 1000.0],
        pa: [0.0; 6],
    };
}

/// The source pre-emphasis coefficient.
const PRE: f32 = 0.65;

/// Bandwidth of the nasal pole and zero (Hz).
const BN: f32 = 100.0;

/// The signal path's state (see the module documentation).
#[derive(Clone, Copy, Debug)]
pub struct Dsp {
    glottis: Glottis,
    rng: Rng,
    pre_x: f32,
    tilt_y: f32,
    tilt: f32,
    np: Resonator,
    nz: AntiResonator,
    cascade: [Resonator; 6],
    parallel: [Resonator; 5],
    pa: [f32; 6],
    /// Amplitudes at the start of the tick and their per-sample steps.
    av: f32,
    ah: f32,
    af: f32,
    dav: f32,
    dah: f32,
    daf: f32,
    /// The parallel branch has state worth running.
    par_live: bool,
    /// The cascade has state worth running.
    cas_live: bool,
}

impl Dsp {
    /// A quiet synthesiser; `seed` seeds the noise.
    pub fn new(seed: u32) -> Dsp {
        let mut d = Dsp {
            glottis: Glottis::default(),
            rng: Rng::new(seed),
            pre_x: 0.0,
            tilt_y: 0.0,
            tilt: 0.0,
            np: Resonator::default(),
            nz: AntiResonator::default(),
            cascade: [Resonator::default(); 6],
            parallel: [Resonator::default(); 5],
            pa: [0.0; 6],
            av: 0.0,
            ah: 0.0,
            af: 0.0,
            dav: 0.0,
            dah: 0.0,
            daf: 0.0,
            par_live: false,
            cas_live: false,
        };
        d.set_frame(&Frame::SILENT);
        d.av = 0.0;
        d.dav = 0.0;
        d
    }

    /// Clears all state (after a stop).
    pub fn reset(&mut self, seed: u32) {
        *self = Dsp::new(seed);
    }

    /// Designs the coefficients of `fr` and ramps the amplitudes to it over
    /// the next tick.
    pub fn set_frame(&mut self, fr: &Frame) {
        // The amplitudes reached at the end of the last tick.
        let n = TICK as f32;
        self.av += self.dav * n;
        self.ah += self.dah * n;
        self.af += self.daf * n;
        let inv = 1.0 / n;
        self.dav = (fr.av - self.av) * inv;
        self.dah = (fr.ah - self.ah) * inv;
        self.daf = (fr.af - self.af) * inv;
        self.glottis.set(fr.f0, fr.oq);
        self.tilt = clampf(fr.tilt, 0.0, 0.97);
        let voiced = fr.av > 0.0 || fr.ah > 0.0 || self.av > 0.0 || self.ah > 0.0;
        if voiced || self.cas_live {
            self.np.set(fr.fnp, BN);
            self.nz.set(fr.fnz, BN);
            for k in 0..6 {
                self.cascade[k].set(fr.f[k], fr.b[k]);
            }
            let lvl = self.cascade.iter().fold(self.np.level(), |m, r| m.max(r.level()));
            self.cas_live = voiced || lvl > 1e-6;
            if !self.cas_live {
                for r in &mut self.cascade {
                    r.clear();
                }
                self.np.clear();
                self.nz.clear();
                self.tilt_y = 0.0;
                self.pre_x = 0.0;
            }
        }
        let fric = fr.af > 0.0 || self.af > 0.0;
        if fric || self.par_live {
            for k in 0..5 {
                self.parallel[k].set_peak(fr.pf[k], fr.pb[k]);
            }
            self.pa = fr.pa;
            let lvl = self.parallel.iter().fold(0.0f32, |m, r| m.max(r.level()));
            self.par_live = fric || lvl > 1e-6;
            if !self.par_live {
                for r in &mut self.parallel {
                    r.clear();
                }
            }
        }
        for r in &mut self.cascade {
            r.flush();
        }
        for r in &mut self.parallel {
            r.flush();
        }
        self.np.flush();
        self.nz.flush();
        self.tilt_y = flush(self.tilt_y);
        self.pre_x = flush(self.pre_x);
    }

    /// Renders one tick into `out` (overwrites it). The source runs per
    /// sample into a buffer; each filter then runs over the whole tick
    /// with its state in registers (the same arithmetic, in the same
    /// order, as filtering sample by sample).
    pub fn render(&mut self, out: &mut [f32; TICK]) {
        if !self.cas_live && !self.par_live {
            out.fill(0.0);
            // Keep the source running so its phase is continuous.
            for _ in 0..TICK {
                self.glottis.tick();
            }
            return;
        }
        let (mut av, mut ah, mut af) = (self.av, self.ah, self.af);
        let a = self.tilt;
        let b = 1.0 - a;
        let mut cas = [0.0f32; TICK];
        let mut fric = [0.0f32; TICK];
        let (mut pre_x, mut tilt_y) = (self.pre_x, self.tilt_y);
        for i in 0..TICK {
            let open = self.glottis.open();
            let g = self.glottis.tick();
            if self.cas_live {
                let noise = self.rng.bipolar() + self.rng.bipolar();
                let asp = if open { ah * noise } else { ah * 0.5 * noise };
                let x = av * g + asp;
                let e = x - PRE * pre_x;
                pre_x = x;
                tilt_y = b * e + a * tilt_y;
                cas[i] = tilt_y;
            }
            if self.par_live {
                fric[i] = af * (self.rng.bipolar() + self.rng.bipolar());
            }
            av += self.dav;
            ah += self.dah;
            af += self.daf;
        }
        self.pre_x = pre_x;
        self.tilt_y = tilt_y;
        if self.cas_live {
            self.np.block(&mut cas);
            self.nz.block(&mut cas);
            for k in (0..6).rev() {
                self.cascade[k].block(&mut cas);
            }
        }
        if self.par_live {
            // y + (((((A2·P2 − A3·P3) + A4·P4) − A5·P5) + A6·P6) + AB·x),
            // accumulated resonator by resonator.
            let mut acc = [0.0f32; TICK];
            let mut tmp = fric;
            self.parallel[0].block(&mut tmp);
            for i in 0..TICK {
                acc[i] = self.pa[0] * tmp[i];
            }
            for k in 1..5 {
                let mut tmp = fric;
                self.parallel[k].block(&mut tmp);
                let g = self.pa[k];
                if k % 2 == 1 {
                    for i in 0..TICK {
                        acc[i] -= g * tmp[i];
                    }
                } else {
                    for i in 0..TICK {
                        acc[i] += g * tmp[i];
                    }
                }
            }
            for i in 0..TICK {
                cas[i] += acc[i] + self.pa[5] * fric[i];
            }
        }
        *out = cas;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn resonator_peaks_at_its_frequency() {
        let mut r = Resonator::default();
        r.set(1000.0, 80.0);
        // Impulse response energy at 1 kHz versus 3 kHz by correlation.
        let mut ir = [0.0f32; 1024];
        let mut x = 1.0;
        for v in ir.iter_mut() {
            *v = r.tick(x);
            x = 0.0;
        }
        let mag = |f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, v) in ir.iter().enumerate() {
                let w = core::f64::consts::TAU * f64::from(f) * n as f64 / 16000.0;
                re += f64::from(*v) * w.cos();
                im -= f64::from(*v) * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(mag(1000.0) > 10.0 * mag(3000.0));
        // Unity DC gain.
        let dc: f32 = ir.iter().sum();
        assert!((dc - 1.0).abs() < 1e-3, "{dc}");
        // Peak-normalised: about unity at the peak.
        let mut p = Resonator::default();
        p.set_peak(2000.0, 200.0);
        let mut x = 1.0;
        for v in ir.iter_mut() {
            *v = p.tick(x);
            x = 0.0;
        }
        let m = {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, v) in ir.iter().enumerate() {
                let w = core::f64::consts::TAU * 2000.0 * n as f64 / 16000.0;
                re += f64::from(*v) * w.cos();
                im -= f64::from(*v) * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!((m - 1.0).abs() < 0.1, "peak gain {m}");
    }

    #[test]
    fn antiresonator_cancels_resonator() {
        let mut r = Resonator::default();
        let mut z = AntiResonator::default();
        r.set(450.0, 100.0);
        z.set(450.0, 100.0);
        let mut x = 1.0;
        for n in 0..64 {
            let y = z.tick(r.tick(x));
            let want = if n == 0 { 1.0 } else { 0.0 };
            assert!((y - want).abs() < 1e-4, "{n}: {y}");
            x = 0.0;
        }
    }

    #[test]
    fn glottal_source_has_its_period_and_no_dc() {
        let mut g = Glottis::default();
        g.set(100.0, 0.6);
        let mut v = std::vec::Vec::new();
        for _ in 0..16_000 {
            v.push(g.tick());
        }
        let mean: f32 = v[160..].iter().sum::<f32>() / (v.len() - 160) as f32;
        assert!(mean.abs() < 0.02, "mean {mean}");
        // Closures (large positive steps after the negative peak) every 160 samples.
        let mut closes = std::vec::Vec::new();
        for n in 1..v.len() {
            if v[n - 1] < -5.0 && v[n] > -5.0 {
                closes.push(n);
            }
        }
        assert!(closes.len() >= 98, "{}", closes.len());
        // (The first period still runs at the default F0: the new one
        // starts at the next period.)
        for w in closes.windows(2).skip(1) {
            assert!((159..=161).contains(&(w[1] - w[0])), "{:?}", w);
        }
    }
}
