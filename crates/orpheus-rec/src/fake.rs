//! A fake ADC for tests and the simulator: tones, noise, silence and
//! speech-like bursts, deterministic from a seed.

/// What the fake input plays.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Signal {
    Silence,
    /// A sine of `hz` with peak `amp` (the right channel a fifth higher).
    Tone {
        hz: u32,
        amp: i16,
    },
    /// White noise, peak `amp`.
    Noise(i16),
    /// A tone of `hz` at `amp` with some noise, on for `on_ms`, then quiet
    /// (noise at 1 % of `amp`) for `off_ms`: talking with pauses.
    Bursts {
        hz: u32,
        amp: i16,
        on_ms: u32,
        off_ms: u32,
    },
}

/// `sin(x)` for any `x` (range reduction and a Taylor series; no `libm`).
#[must_use]
pub fn sin(x: f64) -> f64 {
    const TAU: f64 = core::f64::consts::TAU;
    const PI: f64 = core::f64::consts::PI;
    let mut r = x - TAU * (x / TAU) as i64 as f64;
    if r > PI {
        r -= TAU;
    } else if r < -PI {
        r += TAU;
    }
    // Fold into [-π/2, π/2].
    if r > PI / 2.0 {
        r = PI - r;
    } else if r < -PI / 2.0 {
        r = -PI - r;
    }
    let r2 = r * r;
    let mut term = r;
    let mut sum = r;
    let mut k = 1.0;
    for _ in 0..9 {
        term *= -r2 / ((k + 1.0) * (k + 2.0));
        sum += term;
        k += 2.0;
    }
    sum
}

/// The fake converter.
#[derive(Clone, Debug)]
pub struct FakeAdc {
    pub rate: u32,
    pub channels: u8,
    pub signal: Signal,
    /// Frames produced.
    pub t: u64,
    seed: u32,
}

impl FakeAdc {
    #[must_use]
    pub fn new(rate: u32, channels: u8, signal: Signal, seed: u32) -> Self {
        FakeAdc { rate, channels: channels.clamp(1, 2), signal, t: 0, seed: seed | 1 }
    }

    fn noise(&mut self) -> i32 {
        // xorshift32
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x >> 16) as i32 - 32_768
    }

    fn tone(&self, hz: u32, ch: u8) -> f64 {
        let f = f64::from(hz) * if ch == 1 { 1.5 } else { 1.0 };
        sin(core::f64::consts::TAU * f * self.t as f64 / f64::from(self.rate))
    }

    /// The next frame's sample of channel `ch`.
    fn sample(&mut self, ch: u8) -> i16 {
        let v = match self.signal {
            Signal::Silence => 0,
            Signal::Tone { hz, amp } => (self.tone(hz, ch) * f64::from(amp)) as i32,
            Signal::Noise(amp) => self.noise() * i32::from(amp) / 32_768,
            Signal::Bursts { hz, amp, on_ms, off_ms } => {
                let period = u64::from(on_ms + off_ms).max(1) * u64::from(self.rate) / 1000;
                let on = (self.t % period.max(1)) < u64::from(on_ms) * u64::from(self.rate) / 1000;
                let n = self.noise() * i32::from(amp) / 32_768;
                if on { (self.tone(hz, ch) * f64::from(amp) * 0.9) as i32 + n / 10 } else { n / 100 }
            }
        };
        v.clamp(-32_768, 32_767) as i16
    }

    /// Fill `out` with interleaved frames.
    pub fn fill(&mut self, out: &mut [i16]) {
        let ch = usize::from(self.channels);
        for f in out.chunks_exact_mut(ch) {
            for (c, s) in f.iter_mut().enumerate() {
                *s = self.sample(c as u8);
            }
            self.t += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_is_accurate() {
        for i in -40..40 {
            let x = f64::from(i) * 0.37;
            // sin² + sin(x + π/2)² = 1
            let s = sin(x);
            let c = sin(x + core::f64::consts::FRAC_PI_2);
            assert!((s * s + c * c - 1.0).abs() < 1e-9, "{x}");
        }
        assert!((sin(core::f64::consts::FRAC_PI_6) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn tone_peaks_at_amp() {
        let mut a = FakeAdc::new(48_000, 1, Signal::Tone { hz: 1000, amp: 10_000 }, 1);
        let mut b = [0i16; 480];
        a.fill(&mut b);
        let peak = b.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!((9_990..=10_000).contains(&peak), "{peak}");
    }
}
