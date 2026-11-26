//! Two- to four-operator FM (phase modulation), DX-style.
//!
//! Each operator is a sine oscillator at `ratio × note frequency + detune`
//! with its own envelope. An [`Algorithm`] says which operators modulate
//! which (a modulator always has a higher index than what it modulates, so
//! the operators are computed from the highest down) and which are carriers
//! (summed to the output, scaled by 1/carriers). The highest active
//! operator can feed back into itself; the feedback uses the mean of its
//! last two outputs, which keeps it from oscillating at Nyquist.
//!
//! A modulator's `level` is its modulation index in radians (0–16); a
//! carrier's is its amplitude (0–1). FM is not band-limited; patches keep
//! indices moderate at high notes, and `key_scale` lowers modulator levels
//! with pitch (per octave above C4).
//!
//! The topologies of [`ALGORITHMS`] are generic graph shapes (stack,
//! branches, parallel), numbered our own way; operator 1 is index 0.

use crate::env::{Adsr, Env, EnvCoef};
use crate::math::{clampf, sin_phase, turns_to_phase};

/// Operators per voice.
pub const OPS: usize = 4;

/// One operator's settings.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct OpParams {
    /// Frequency ratio to the note (0.0625–32).
    pub ratio: f32,
    /// Fixed offset, Hz (−100 to 100).
    pub detune: f32,
    /// Modulator: index in radians (0–16). Carrier: amplitude (0–1).
    pub level: f32,
    /// Velocity sensitivity of the level, 0–1.
    pub velocity: f32,
    /// Envelope.
    pub env: Adsr,
}

impl OpParams {
    /// An operator at `ratio` and `level` with envelope `env`.
    #[must_use]
    pub const fn new(ratio: f32, level: f32, env: Adsr) -> Self {
        Self { ratio, detune: 0.0, level, velocity: 0.5, env }
    }

    /// A silent operator.
    pub const OFF: Self = Self::new(1.0, 0.0, Adsr::GATE);

    fn sanitized(&self) -> Self {
        Self {
            ratio: clampf(self.ratio, 0.0625, 32.0),
            detune: clampf(self.detune, -100.0, 100.0),
            level: clampf(self.level, 0.0, 16.0),
            velocity: clampf(self.velocity, 0.0, 1.0),
            env: self.env.sanitized(),
        }
    }
}

/// FM settings.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct FmParams {
    /// Index into [`ALGORITHMS`] (wrapped).
    pub algorithm: u8,
    /// Operators in use, 2–4 (operators 1..=ops).
    pub ops: u8,
    /// Feedback of the highest active operator, 0–1 (1 = index π).
    pub feedback: f32,
    /// Modulator level change per octave above C4, −1 to 1 (−0.3: 30 % less
    /// per octave).
    pub key_scale: f32,
    /// Operator settings.
    pub op: [OpParams; OPS],
}

impl FmParams {
    /// The settings with every field in range.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        Self {
            algorithm: self.algorithm % ALGORITHMS.len() as u8,
            ops: self.ops.clamp(2, OPS as u8),
            feedback: clampf(self.feedback, 0.0, 1.0),
            key_scale: clampf(self.key_scale, -1.0, 1.0),
            op: [
                self.op[0].sanitized(),
                self.op[1].sanitized(),
                self.op[2].sanitized(),
                self.op[3].sanitized(),
            ],
        }
    }
}

/// An operator graph: bit `j` of `mods[i]` means operator `j` modulates
/// operator `i` (only `j > i` counts); bit `i` of `carriers` makes operator
/// `i` an output.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Algorithm {
    /// Modulators of each operator.
    pub mods: [u8; OPS],
    /// Carrier mask.
    pub carriers: u8,
}

/// The algorithms (operator numbers 1–4 as in a DX chart; arrows modulate):
///
/// | # | Graph | Carriers |
/// |---|---|---|
/// | 0 | 4→3→2→1 (stack; with 2 ops: 2→1) | 1 |
/// | 1 | (3 + 4)→2→1 | 1 |
/// | 2 | 4→3→1, 2→1 | 1 |
/// | 3 | 2→1, 4→3 (two pairs) | 1, 3 |
/// | 4 | 4→1, 4→2, 4→3 (one modulator, three carriers) | 1, 2, 3 |
/// | 5 | 2→1, 3, 4 | 1, 3, 4 |
/// | 6 | 1, 2, 3, 4 (additive) | all |
/// | 7 | (2 + 3 + 4)→1 | 1 |
pub const ALGORITHMS: [Algorithm; 8] = [
    Algorithm { mods: [0b0010, 0b0100, 0b1000, 0], carriers: 0b0001 },
    Algorithm { mods: [0b0010, 0b1100, 0, 0], carriers: 0b0001 },
    Algorithm { mods: [0b0110, 0, 0b1000, 0], carriers: 0b0001 },
    Algorithm { mods: [0b0010, 0, 0b1000, 0], carriers: 0b0101 },
    Algorithm { mods: [0b1000, 0b1000, 0b1000, 0], carriers: 0b0111 },
    Algorithm { mods: [0b0010, 0, 0, 0], carriers: 0b1101 },
    Algorithm { mods: [0, 0, 0, 0], carriers: 0b1111 },
    Algorithm { mods: [0b1110, 0, 0, 0], carriers: 0b0001 },
];

const TURN: f32 = 1.0 / core::f32::consts::TAU;

/// One operator's running state (28 bytes).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq)]
pub struct OpState {
    phase: u32,
    inc: u32,
    /// Output scale: level (carrier) or index/2π (modulator, in turns), with
    /// velocity and key scaling.
    scale: f32,
    /// Envelope level, interpolated per sample towards `target`.
    e: f32,
    de: f32,
    target: f32,
    pub(crate) env: Env,
}

/// What an FM patch's voices share (in the part's coefficients): the
/// operator graph as a matrix, the carrier weights, the feedback.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq)]
pub struct FmCoef {
    /// `m[i][j]`: operator `j` modulates operator `i` (0 or 1).
    m: [[f32; OPS]; OPS],
    /// Carrier weights (1/carriers or 0).
    w: [f32; OPS],
    fb_scale: f32,
    carriers: u8,
    n: u8,
}

impl FmCoef {
    /// The graph of `p` (sanitised).
    #[must_use]
    pub fn new(p: &FmParams) -> Self {
        let alg = &ALGORITHMS[usize::from(p.algorithm) % ALGORITHMS.len()];
        let n = usize::from(p.ops.clamp(2, OPS as u8));
        let active = (1u8 << n) - 1;
        let carriers = if alg.carriers & active == 0 { 1 } else { alg.carriers & active };
        let cw = 1.0 / carriers.count_ones() as f32;
        let mut c =
            Self { fb_scale: clampf(p.feedback, 0.0, 1.0) * 0.25, carriers, n: n as u8, ..Self::default() };
        for i in 0..OPS {
            c.w[i] = if carriers & (1 << i) != 0 { cw } else { 0.0 };
            for j in 0..OPS {
                c.m[i][j] = if j > i && alg.mods[i] & active & (1 << j) != 0 { 1.0 } else { 0.0 };
            }
        }
        c
    }

    fn is_carrier(&self, i: usize) -> bool {
        self.carriers & (1 << i) != 0
    }
}

/// The FM voice's running state.
///
/// The operator envelopes run at control rate, one period ahead: each
/// control update advances them to the next tick and the per-sample level
/// moves there in a straight line (DX-style envelope stepping; an attack
/// of 0 becomes a ramp of at most one control period, 0.67 ms).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq)]
pub struct FmState {
    pub(crate) op: [OpState; OPS],
    fb: [f32; 2],
    /// Samples the current envelope ramps were set for.
    ramp: u32,
    /// The envelopes have been advanced to the next tick since the trigger.
    primed: bool,
}

impl FmState {
    /// Start a note: phases to 0, envelopes from 0.
    pub fn trigger(&mut self) {
        for o in &mut self.op {
            o.phase = 0;
            o.e = 0.0;
            o.de = 0.0;
            o.target = 0.0;
            o.env.trigger();
        }
        self.fb = [0.0; 2];
        self.ramp = 0;
        self.primed = false;
    }

    /// Retrigger the envelopes from their levels (legato off, stolen voice).
    pub fn retrigger(&mut self) {
        for o in &mut self.op {
            o.env.gate_on();
        }
    }

    /// Close the operator envelopes' gates.
    pub fn release(&mut self, c: &[EnvCoef; OPS]) {
        for (o, c) in self.op.iter_mut().zip(c) {
            o.env.gate_off(c);
        }
    }

    /// Control update: operator frequencies and scales for note frequency
    /// `hz` (Hz), velocity `vel` (0–1), octaves above C4 `oct`; `elapsed`
    /// samples since the last update, `ahead` samples to the next tick,
    /// `tick`: this update is on a tick. The envelopes advance to the next
    /// tick on ticks (and once after the trigger); an update between ticks
    /// (an event) only re-aims the ramps.
    #[allow(clippy::too_many_arguments)]
    pub fn control(
        &mut self,
        p: &FmParams,
        g: &FmCoef,
        c: &[EnvCoef; OPS],
        hz: f32,
        vel: f32,
        oct: f32,
        rate: f32,
        elapsed: u32,
        ahead: u32,
        tick: bool,
    ) {
        let advance = tick || !self.primed;
        self.primed = true;
        let ks = clampf(1.0 + p.key_scale * oct, 0.0, 4.0);
        let snap = elapsed == self.ramp;
        let inv = 1.0 / ahead.max(1) as f32;
        for (i, (o, q)) in self.op.iter_mut().zip(&p.op).enumerate() {
            let dt = (hz * q.ratio + q.detune) / rate;
            o.inc = if dt > 0.0 && dt < 0.5 { crate::osc::inc_for(dt) } else { 0 };
            let v = 1.0 - q.velocity + q.velocity * vel;
            o.scale = if g.is_carrier(i) { q.level * v } else { q.level * v * ks * TURN };
            // The ramp set last time has run to its end: land exactly on it.
            if snap {
                o.e = o.target;
            }
            if advance {
                o.target = o.env.advance(&c[i], ahead);
            }
            o.de = (o.target - o.e) * inv;
        }
        self.ramp = ahead;
    }

    /// Whether every carrier has finished.
    #[must_use]
    pub fn finished(&self, g: &FmCoef) -> bool {
        (0..OPS).filter(|&i| g.is_carrier(i)).all(|i| self.op[i].env.is_idle() && self.op[i].e == 0.0)
    }

    /// Render `out.len()` samples (overwrites `out`).
    pub fn render(&mut self, g: &FmCoef, out: &mut [f32]) {
        // Written out per operator count: at the firmware's opt-level "s"
        // a generic loop over the graph is not unrolled (it cost 2× more).
        let (m, w, k) = (&g.m, g.w, g.fb_scale);
        let [o0, o1, o2, o3] = &mut self.op;
        let [mut f0, mut f1] = self.fb;
        match g.n {
            2 => {
                for s in out.iter_mut() {
                    let v1 = op(o1, k * (f0 + f1));
                    f1 = f0;
                    f0 = v1;
                    let y1 = v1 * o1.scale;
                    let y0 = op(o0, m[0][1] * y1) * o0.scale;
                    *s = w[0] * y0 + w[1] * y1;
                }
            }
            3 => {
                for s in out.iter_mut() {
                    let v2 = op(o2, k * (f0 + f1));
                    f1 = f0;
                    f0 = v2;
                    let y2 = v2 * o2.scale;
                    let y1 = op(o1, m[1][2] * y2) * o1.scale;
                    let y0 = op(o0, m[0][1] * y1 + m[0][2] * y2) * o0.scale;
                    *s = w[0] * y0 + w[1] * y1 + w[2] * y2;
                }
            }
            _ => {
                for s in out.iter_mut() {
                    let v3 = op(o3, k * (f0 + f1));
                    f1 = f0;
                    f0 = v3;
                    let y3 = v3 * o3.scale;
                    let y2 = op(o2, m[2][3] * y3) * o2.scale;
                    let y1 = op(o1, m[1][2] * y2 + m[1][3] * y3) * o1.scale;
                    let y0 = op(o0, m[0][1] * y1 + m[0][2] * y2 + m[0][3] * y3) * o0.scale;
                    *s = w[0] * y0 + w[1] * y1 + w[2] * y2 + w[3] * y3;
                }
            }
        }
        self.fb = [f0, f1];
    }
}

/// One operator sample with phase modulation `pm` (turns): the enveloped
/// sine before the output scale.
#[inline(always)]
fn op(o: &mut OpState, pm: f32) -> f32 {
    o.e += o.de;
    let v = sin_phase(o.phase.wrapping_add(turns_to_phase(pm))) * o.e;
    o.phase = o.phase.wrapping_add(o.inc);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulators_are_above_their_targets() {
        for a in &ALGORITHMS {
            for (i, m) in a.mods.iter().enumerate() {
                assert_eq!(m & ((1 << (i + 1)) - 1), 0, "{a:?}");
                assert_eq!(a.carriers & m, 0, "a carrier also modulates: {a:?}");
            }
        }
    }

    #[test]
    fn two_op_stack_is_bounded() {
        let env = Adsr::new(0.0, 0.5, 0.5, 0.2);
        let p = FmParams {
            algorithm: 0,
            ops: 2,
            feedback: 1.0,
            key_scale: 0.0,
            op: [OpParams::new(1.0, 1.0, env), OpParams::new(2.0, 8.0, env), OpParams::OFF, OpParams::OFF],
        };
        let c = [EnvCoef::new(&env, 48_000.0); OPS];
        let g = FmCoef::new(&p);
        let mut s = FmState::default();
        s.trigger();
        let mut b = [0.0f32; 4800];
        let k = crate::CONTROL_PERIOD;
        for chunk in b.chunks_mut(k) {
            s.control(&p, &g, &c, 440.0, 1.0, 0.0, 48_000.0, k as u32, k as u32, true);
            s.render(&g, chunk);
        }
        assert!(b.iter().all(|v| v.abs() <= 1.0 && v.is_finite()));
        assert!(b.iter().any(|v| v.abs() > 0.5));
    }
}
