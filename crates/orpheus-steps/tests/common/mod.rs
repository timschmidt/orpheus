//! Synthetic accelerometer traces for the detector tests: a person walking
//! or running with the player in the hand or a pocket, the player on a
//! table, shaken, or on a bus. Deterministic (seeded); raw counts at
//! 1024 per g, clipped to the ± 2 g range of the part.

#![allow(dead_code)]

use std::f64::consts::TAU;

/// xorshift64*.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Roughly normal, mean 0, standard deviation 1.
    pub fn gauss(&mut self) -> f64 {
        (0..6).map(|_| self.f()).sum::<f64>() * 2.0_f64.sqrt() - 6.0 / 2.0 * 2.0_f64.sqrt()
    }
}

type V = [f64; 3];

fn norm(v: V) -> V {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn add(a: V, b: V, k: f64) -> V {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

/// Rotate `v` about unit axis `k` by `t` radians (Rodrigues).
fn rot(v: V, k: V, t: f64) -> V {
    let (s, c) = t.sin_cos();
    let kv = cross(k, v);
    let d = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
    [
        v[0] * c + kv[0] * s + k[0] * d * (1.0 - c),
        v[1] * c + kv[1] * s + k[1] * d * (1.0 - c),
        v[2] * c + kv[2] * s + k[2] * d * (1.0 - c),
    ]
}

/// A body frame in device coordinates: up (against gravity), lateral,
/// forward, for a random orientation.
fn frame(rng: &mut Rng) -> (V, V, V) {
    let up = norm([rng.gauss(), rng.gauss(), rng.gauss()]);
    let tmp = if up[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let lat = norm(cross(up, tmp));
    let fwd = cross(up, lat);
    (up, lat, fwd)
}

fn counts(a: V, noise: f64, rng: &mut Rng) -> [i16; 3] {
    let mut o = [0i16; 3];
    for (o, v) in o.iter_mut().zip(a) {
        let c = (v * 1024.0 + rng.gauss() * noise).round();
        *o = c.clamp(-2048.0, 2047.0) as i16;
    }
    o
}

/// How the player is carried.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Carry {
    Hand,
    /// Trouser pocket: the thigh swings at half the step rate and the
    /// player turns in the pocket.
    Pocket,
}

/// Gait parameters.
#[derive(Clone, Copy, Debug)]
pub struct Gait {
    /// Steps per second.
    pub cadence: f64,
    /// Vertical acceleration amplitude (g).
    pub amp: f64,
    /// Second-harmonic share (heel strike and push-off as two bumps).
    pub harmonic: f64,
    /// Step-to-step timing jitter (fraction).
    pub jitter: f64,
    pub carry: Carry,
}

pub const WALK: Gait = Gait { cadence: 1.9, amp: 0.30, harmonic: 0.35, jitter: 0.05, carry: Carry::Hand };
pub const SLOW: Gait = Gait { cadence: 1.5, amp: 0.15, harmonic: 0.25, jitter: 0.08, carry: Carry::Hand };
pub const BRISK: Gait = Gait { cadence: 2.2, amp: 0.45, harmonic: 0.4, jitter: 0.04, carry: Carry::Hand };
pub const RUN: Gait = Gait { cadence: 2.8, amp: 1.1, harmonic: 0.5, jitter: 0.04, carry: Carry::Hand };
pub const POCKET: Gait = Gait { cadence: 1.8, amp: 0.40, harmonic: 0.45, jitter: 0.06, carry: Carry::Pocket };

/// A trace and the number of steps really taken in it.
pub struct Trace {
    pub samples: Vec<[i16; 3]>,
    pub steps: u32,
}

impl Trace {
    pub fn new() -> Trace {
        Trace { samples: Vec::new(), steps: 0 }
    }

    pub fn then(mut self, other: Trace) -> Trace {
        self.samples.extend(other.samples);
        self.steps += other.steps;
        self
    }
}

/// `secs` of walking with gait `g` at `rate` Hz.
pub fn walk(rate: u32, secs: f64, g: Gait, seed: u64) -> Trace {
    let mut rng = Rng::new(seed);
    let (up, lat, fwd) = frame(&mut rng);
    let turn_axis = norm([rng.gauss(), rng.gauss(), rng.gauss()]);
    let n = (secs * f64::from(rate)) as usize;
    let dt = 1.0 / f64::from(rate);
    let mut phase = 0.0f64;
    let mut step_len = 1.0 + g.jitter * rng.gauss();
    let mut steps = 0u32;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 * dt;
        let v = g.amp * ((TAU * phase).sin() + g.harmonic * (2.0 * TAU * phase + 0.7).sin());
        let sway = 0.25 * g.amp * (TAU * phase / 2.0).sin();
        let surge = 0.35 * g.amp * (TAU * phase + 1.2).sin();
        let (mut u, mut l, mut f) = (up, lat, fwd);
        let mut extra = 0.0;
        if g.carry == Carry::Pocket {
            // The thigh: ± 25° at half the step rate, and the player
            // slowly turning in the pocket.
            let swing = 0.45 * (TAU * phase / 2.0).sin();
            u = rot(u, lat, swing);
            f = rot(f, lat, swing);
            let turn = 0.4 * (TAU * 0.07 * t).sin();
            u = rot(u, turn_axis, turn);
            l = rot(l, turn_axis, turn);
            f = rot(f, turn_axis, turn);
            extra = 0.15 * g.amp * (TAU * phase / 2.0 + 0.3).cos();
        }
        let a = add(add(add(u, u, v), l, sway), f, surge + extra);
        out.push(counts(a, 4.0, &mut rng));
        phase += g.cadence * dt / step_len;
        if phase >= 1.0 {
            phase -= 1.0;
            steps += 1;
            step_len = 1.0 + g.jitter * rng.gauss();
        }
    }
    Trace { samples: out, steps }
}

/// The player lying still for `secs` (sensor noise, a little drift).
pub fn rest(rate: u32, secs: f64, seed: u64) -> Trace {
    let mut rng = Rng::new(seed);
    let (up, _, _) = frame(&mut rng);
    let n = (secs * f64::from(rate)) as usize;
    let samples =
        (0..n).map(|i| counts(add(up, up, 0.002 * (i as f64 / 3000.0).sin()), 3.0, &mut rng)).collect();
    Trace { samples, steps: 0 }
}

/// Picked up, turned over, put down again: a few large irregular moves
/// with rest between them.
pub fn handling(rate: u32, secs: f64, seed: u64) -> Trace {
    let mut rng = Rng::new(seed);
    let (mut up, lat, _) = frame(&mut rng);
    let n = (secs * f64::from(rate)) as usize;
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        // Rest 1–4 s.
        let still = (f64::from(rate) * (1.0 + 3.0 * rng.f())) as usize;
        for _ in 0..still.min(n - i) {
            out.push(counts(up, 3.0, &mut rng));
            i += 1;
        }
        // One move: 0.3–0.8 s of a jerk and a turn.
        let len = (f64::from(rate) * (0.3 + 0.5 * rng.f())) as usize;
        let turn = rng.gauss();
        for k in 0..len.min(n - i) {
            let p = k as f64 / len as f64;
            let jerk = 0.8 * (TAU * p).sin() * rng.gauss().abs();
            let u = rot(up, lat, turn * p);
            out.push(counts(add(u, lat, jerk), 6.0, &mut rng));
            i += 1;
        }
        up = rot(up, lat, turn);
    }
    Trace { samples: out, steps: 0 }
}

/// Shaking the player in the hand at `hz` (4–7 Hz is typical), in bursts
/// of 2–5 s.
pub fn shake(rate: u32, secs: f64, hz: f64, amp: f64, seed: u64) -> Trace {
    let mut rng = Rng::new(seed);
    let (up, lat, _) = frame(&mut rng);
    let n = (secs * f64::from(rate)) as usize;
    let dt = 1.0 / f64::from(rate);
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let burst = (f64::from(rate) * (2.0 + 3.0 * rng.f())) as usize;
        let f = hz * (0.85 + 0.3 * rng.f());
        for k in 0..burst.min(n - i) {
            let t = k as f64 * dt;
            let s = amp * (TAU * f * t).sin() + 0.2 * amp * (TAU * 2.0 * f * t).sin();
            out.push(counts(add(add(up, lat, s), up, 0.3 * s), 5.0, &mut rng));
            i += 1;
        }
        let pause = (f64::from(rate) * (0.5 + rng.f())) as usize;
        for _ in 0..pause.min(n - i) {
            out.push(counts(up, 3.0, &mut rng));
            i += 1;
        }
    }
    Trace { samples: out, steps: 0 }
}

/// A bus ride with the player on the lap: engine vibration, slow sway in
/// bends and braking, and potholes.
pub fn bus(rate: u32, secs: f64, seed: u64) -> Trace {
    let mut rng = Rng::new(seed);
    let (up, lat, fwd) = frame(&mut rng);
    let n = (secs * f64::from(rate)) as usize;
    let dt = 1.0 / f64::from(rate);
    let mut bump = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 * dt;
        if rng.f() < 0.2 / f64::from(rate) {
            // A pothole about every 5 s.
            bump = 0.3 + 0.3 * rng.f();
        }
        let vib = 0.04 * (TAU * 11.3 * t).sin() + 0.02 * rng.gauss();
        let v = vib + bump * (TAU * 6.0 * t).sin();
        bump *= 0.75;
        let sway = 0.12 * (TAU * 0.23 * t).sin();
        let brake = 0.1 * (TAU * 0.05 * t).sin();
        out.push(counts(add(add(add(up, up, v), lat, sway), fwd, brake), 4.0, &mut rng));
    }
    Trace { samples: out, steps: 0 }
}

/// Every sample through a detector; the steps counted.
pub fn count(d: &mut orpheus_steps::Detector, t: &Trace) -> u32 {
    t.samples.iter().map(|s| d.feed(s[0], s[1], s[2])).sum()
}

/// `got` within `pct` % of `want`.
pub fn within(got: u32, want: u32, pct: u32) -> bool {
    u64::from(got.abs_diff(want)) * 100 <= u64::from(want) * u64::from(pct)
}
