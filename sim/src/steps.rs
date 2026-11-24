//! A fake accelerometer for the Steps app (WP SENS-1/UI-5c): the real
//! detector (`orpheus_steps::Detector`) fed at the player's 25 Hz with a
//! synthetic trace (walking, or lying still with sensor noise), and the
//! real day history. `set:walk=on|off`, `set:steps=N` (today's count),
//! `set:week=a,b,c,…` (today first), `set:sensor=off`.

use orpheus_steps::{Detector, History, Sensitivity};

/// Counting rate on the player (`firmware::sensor`).
pub const RATE: u32 = 25;
const PERIOD_MS: u64 = 1000 / RATE as u64;

pub struct FakeSteps {
    pub detector: Detector,
    pub history: History,
    pub sensor: bool,
    /// Someone walks with the player (else it lies on a table).
    pub walking: bool,
    /// Simulated time not yet turned into samples (ms), sample count.
    pending_ms: u64,
    n: u64,
    rng: u64,
    /// Today's count changed since the last `take_changed`.
    changed: bool,
}

impl Default for FakeSteps {
    fn default() -> Self {
        FakeSteps {
            detector: Detector::new(RATE as u16, Sensitivity::Medium),
            history: History::new(),
            sensor: true,
            walking: false,
            pending_ms: 0,
            n: 0,
            rng: 0x2545_F491_4F6C_DD1D,
            changed: false,
        }
    }
}

impl FakeSteps {
    fn noise(&mut self) -> i16 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        i16::try_from(self.rng % 9).unwrap_or(0) - 4
    }

    /// One sample at 25 Hz: 1.9 steps/s, ± 0.3 g with a second harmonic,
    /// gravity on z (the player held flat), or still with ± 4 counts.
    fn sample(&mut self) -> [i16; 3] {
        let (nx, ny, nz) = (self.noise(), self.noise(), self.noise());
        if !self.walking {
            return [nx, ny, 1024 + nz];
        }
        let t = self.n as f64 / f64::from(RATE);
        let phase = core::f64::consts::TAU * 1.9 * t;
        let v = 0.3 * (phase.sin() + 0.3 * (2.0 * phase + 0.7).sin());
        let sway = 0.08 * (phase / 2.0).sin();
        [
            (sway * 1024.0) as i16 + nx,
            (0.1 * phase.cos() * 1024.0) as i16 + ny,
            ((1.0 + v) * 1024.0) as i16 + nz,
        ]
    }

    /// Simulated time passes: the samples it holds go through the
    /// detector into today's count.
    pub fn advance(&mut self, ms: u64, today: Option<u32>) {
        if let Some(d) = today {
            self.history.roll(d);
        }
        if !self.sensor {
            return;
        }
        self.pending_ms += ms;
        while self.pending_ms >= PERIOD_MS {
            self.pending_ms -= PERIOD_MS;
            let s = self.sample();
            self.n += 1;
            let k = self.detector.feed(s[0], s[1], s[2]);
            if k > 0 {
                self.history.add(k);
                self.changed = true;
            }
        }
    }

    /// Set today's count.
    pub fn set_today(&mut self, n: u32) {
        let now = self.history.today();
        if n >= now {
            self.history.add(n - now);
        } else {
            self.history.reset_today();
            self.history.add(n);
        }
        self.changed = true;
    }

    /// Fill the week (today first) from a list of counts.
    pub fn set_week(&mut self, counts: &[u32], today: u32) {
        let mut h = History::new();
        for (k, c) in counts.iter().enumerate().rev() {
            h.roll(today.saturating_sub(u32::try_from(k).unwrap_or(0)));
            h.add(*c);
        }
        h.roll(today);
        self.history = h;
        self.changed = true;
    }

    pub fn take_changed(&mut self) -> bool {
        core::mem::take(&mut self.changed)
    }
}
