//! Ticks to sample frames, exactly.
//!
//! A [`Clock`] maps a tick to the output frame on which an event lands:
//!
//! ```text
//! frame(t) = ⌊(acc0 + (t − tick0) · mul) / den⌋
//! metrical: mul = µs per quarter · rate,  den = ticks per quarter · 10⁶
//! SMPTE:    mul = rate · 1001 (29.97) or rate,  den = 30000 · tpf or fps · tpf
//! ```
//!
//! `den` is fixed by the division, so `acc0` (in units of 1/`den` frame,
//! 128 bits) carries the exact time of the last tempo change: every event
//! lands on the floor of its exact time in frames, however many tempo
//! changes came before (no rounding per change; the test computes the same
//! times independently). SMPTE files ignore tempo events (the division
//! fixes the time).
//!
//! A song is cut at [`MAX_SECONDS`]: an event later than that ends it (a
//! file of huge delta times is a day of silence at most, never an
//! overflow).
//!
//! The [`TempoMap`] keeps the tempo changes found while scanning a file
//! (up to [`TEMPO_MAX`]) for converting positions back to ticks and beats
//! (the UI's tempo view); playback itself follows the events.

use crate::smf::Division;

/// Default tempo: 120 BPM.
pub const DEFAULT_TEMPO: u32 = 500_000;
/// A song ends at the first event after this many seconds.
pub const MAX_SECONDS: u64 = 24 * 3600;
/// Tempo changes the [`TempoMap`] keeps.
pub const TEMPO_MAX: usize = 128;

/// Tick → frame conversion with the current tempo.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    rate: u32,
    division: Division,
    tick0: u64,
    acc0: u128,
    mul: u64,
    den: u64,
    tempo: u32,
}

impl Clock {
    /// A clock at `rate` Hz for `division`, at 120 BPM.
    #[must_use]
    pub fn new(rate: u32, division: Division) -> Self {
        let rate = rate.max(1);
        let (mul, den) = match division {
            Division::Metrical(tpq) => {
                (u64::from(DEFAULT_TEMPO) * u64::from(rate), u64::from(tpq.max(1)) * 1_000_000)
            }
            Division::Smpte { fps: 29, tpf } => (u64::from(rate) * 1001, 30_000 * u64::from(tpf.max(1))),
            Division::Smpte { fps, tpf } => (u64::from(rate), u64::from(fps.max(1)) * u64::from(tpf.max(1))),
        };
        Self { rate, division, tick0: 0, acc0: 0, mul, den, tempo: DEFAULT_TEMPO }
    }

    /// Output rate, Hz.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The frame of `tick` (ticks before the last tempo change are taken at
    /// the current tempo; callers go forward).
    #[must_use]
    pub fn frame(&self, tick: u64) -> u64 {
        // `den` ≥ 1 by construction (`max(1)`: no division panic path).
        u64::try_from(self.acc(tick) / u128::from(self.den.max(1))).unwrap_or(u64::MAX)
    }

    fn acc(&self, tick: u64) -> u128 {
        let dt = u128::from(tick.saturating_sub(self.tick0));
        self.acc0.saturating_add(dt * u128::from(self.mul))
    }

    /// Tempo `us` µs per quarter from `tick` (metrical files; 0 ignored).
    pub fn set_tempo(&mut self, tick: u64, us: u32) {
        if us == 0 {
            return;
        }
        if let Division::Metrical(_) = self.division
            && tick >= self.tick0
        {
            self.acc0 = self.acc(tick);
            self.tick0 = tick;
            self.tempo = us;
            self.mul = u64::from(us) * u64::from(self.rate);
        }
    }

    /// Current tempo, µs per quarter.
    #[must_use]
    pub fn tempo(&self) -> u32 {
        self.tempo
    }

    /// Current tempo in BPM.
    #[must_use]
    pub fn bpm(&self) -> f32 {
        60_000_000.0 / self.tempo as f32
    }

    /// The last frame a song may reach.
    #[must_use]
    pub fn max_frame(&self) -> u64 {
        MAX_SECONDS * u64::from(self.rate)
    }

    /// The anchor of the current tempo: (tick, frame).
    #[must_use]
    pub fn anchor(&self) -> (u64, u64) {
        (self.tick0, self.frame(self.tick0))
    }
}

/// One tempo segment.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TempoPoint {
    /// From this tick …
    pub tick: u64,
    /// … which is this frame …
    pub frame: u64,
    /// … at this tempo (µs per quarter).
    pub tempo: u32,
}

/// The tempo changes of a song (the first [`TEMPO_MAX`]).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempoMap {
    pts: [TempoPoint; TEMPO_MAX],
    n: u16,
    /// Changes past the capacity (not kept).
    pub dropped: u32,
    rate: u32,
    tpq: u16,
}

impl TempoMap {
    /// Empty (120 BPM from tick 0).
    #[must_use]
    pub fn new(rate: u32, division: Division) -> Self {
        let tpq = match division {
            Division::Metrical(t) => t,
            Division::Smpte { .. } => 0,
        };
        let mut m = Self { pts: [TempoPoint::default(); TEMPO_MAX], n: 1, dropped: 0, rate, tpq };
        m.pts[0] = TempoPoint { tick: 0, frame: 0, tempo: DEFAULT_TEMPO };
        m
    }

    /// Record the clock's state after a tempo change.
    pub fn push(&mut self, c: &Clock) {
        let (tick, frame) = c.anchor();
        let p = TempoPoint { tick, frame, tempo: c.tempo() };
        let n = usize::from(self.n);
        if self.pts[n - 1].tick == tick {
            self.pts[n - 1] = p;
        } else if n < TEMPO_MAX {
            self.pts[n] = p;
            self.n += 1;
        } else {
            self.dropped += 1;
        }
    }

    /// The tempo points.
    #[must_use]
    pub fn points(&self) -> &[TempoPoint] {
        &self.pts[..usize::from(self.n)]
    }

    fn at_frame(&self, frame: u64) -> TempoPoint {
        let pts = self.points();
        let i = pts.partition_point(|p| p.frame <= frame).max(1) - 1;
        pts[i]
    }

    /// Tempo (µs per quarter) at `frame`.
    #[must_use]
    pub fn tempo_at(&self, frame: u64) -> u32 {
        self.at_frame(frame).tempo
    }

    /// The tick sounding at `frame` (metrical files; SMPTE: 0).
    #[must_use]
    pub fn tick_at(&self, frame: u64) -> u64 {
        if self.tpq == 0 {
            return 0;
        }
        let p = self.at_frame(frame);
        let num = u128::from(frame - p.frame) * u128::from(self.tpq) * 1_000_000;
        let den = u128::from(p.tempo) * u128::from(self.rate.max(1));
        p.tick.saturating_add(u64::try_from(num / den).unwrap_or(u64::MAX))
    }

    /// Ticks per quarter (0 for SMPTE files).
    #[must_use]
    pub fn ticks_per_quarter(&self) -> u16 {
        self.tpq
    }
}
