//! Automatic gain: a slow feed-forward gain towards a target peak with a
//! fast attack and a limiter (04 §5.2). Integer only.
//!
//! Per block: the block's peak decides the gain for the next one; within
//! a block the gain moves linearly from the old to the new value (no
//! zipper noise), and any sample that would still exceed full scale is
//! limited to it. Quiet input (below the noise floor) holds the gain, so
//! pauses are not pumped up into hiss.

/// Unity in Q12.
pub const UNITY: i32 = 4096;

/// AGC settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgcConfig {
    /// Peak the gain aims for (linear, 0–32767): −12 dBFS by default.
    pub target: u16,
    /// Largest gain, Q12 (+24 dB).
    pub max_gain: i32,
    /// Below this peak the gain holds (−50 dBFS).
    pub floor: u16,
    /// Rise, Q12 parts per second (≈ 3 dB/s).
    pub release_per_s: i32,
}

impl Default for AgcConfig {
    fn default() -> Self {
        AgcConfig { target: 8_231, max_gain: UNITY * 16, floor: 104, release_per_s: 1_413 }
    }
}

/// The AGC state.
#[derive(Clone, Copy, Debug)]
pub struct Agc {
    pub cfg: AgcConfig,
    gain: i32,
}

impl Agc {
    #[must_use]
    pub fn new(cfg: AgcConfig) -> Self {
        Agc { cfg, gain: UNITY }
    }

    /// The gain now, Q12.
    #[must_use]
    pub fn gain(&self) -> i32 {
        self.gain
    }

    /// The gain in tenths of a dB (for the screen).
    #[must_use]
    pub fn gain_db10(&self) -> i16 {
        // 20·log10(g/4096) = 6.0206·(log2 g − 12).
        let l = i64::from(crate::meter::log2_q8(self.gain.max(1) as u32) - 12 * 256);
        (l * 60_206 / 256_000) as i16
    }

    /// Process one block in place (interleaved; the gain is common to all
    /// channels). `frames` at `rate` gives the release step.
    pub fn process(&mut self, block: &mut [i16], frames: usize, rate: u32) {
        let peak = block.iter().map(|s| i32::from(s.unsigned_abs())).max().unwrap_or(0);
        let old = self.gain;
        let target = i32::from(self.cfg.target);
        let new = if peak == 0 || peak < i32::from(self.cfg.floor) {
            old
        } else if i64::from(peak) * i64::from(old) > i64::from(target) * i64::from(UNITY) {
            // Attack: straight to the gain that puts this peak on target.
            (i64::from(target) * i64::from(UNITY) / i64::from(peak)) as i32
        } else {
            // Release: slowly up.
            let step = i64::from(old) * i64::from(self.cfg.release_per_s) * frames as i64
                / (i64::from(UNITY) * i64::from(rate.max(1)));
            (i64::from(old) + step.max(1)) as i32
        };
        let new = new.min(self.cfg.max_gain).max(UNITY / 4);
        // Linear ramp from the old to the new gain, Q28 steps; one 64-bit
        // multiply and shift per sample.
        let n = block.len().max(1) as i64;
        let step = ((i64::from(new) - i64::from(old)) << 16) / n;
        let mut g = i64::from(old) << 16;
        for s in block.iter_mut() {
            let v = (i64::from(*s) * g) >> 28;
            *s = v.clamp(-32_767, 32_767) as i16;
            g += step;
        }
        self.gain = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeAdc, Signal};

    fn run(signal: Signal, secs: u32) -> (Agc, u16) {
        let mut adc = FakeAdc::new(16_000, 1, signal, 3);
        let mut agc = Agc::new(AgcConfig::default());
        let mut peak = 0u16;
        let mut b = [0i16; 320];
        for i in 0..secs * 50 {
            adc.fill(&mut b);
            agc.process(&mut b, 320, 16_000);
            if i >= (secs - 1) * 50 {
                peak = peak.max(b.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0));
            }
        }
        (agc, peak)
    }

    #[test]
    fn quiet_speech_comes_up_to_the_target() {
        let (agc, peak) = run(Signal::Tone { hz: 300, amp: 1_000 }, 12);
        assert!((7_000..=8_300).contains(&peak), "{peak}");
        assert!(agc.gain_db10() > 170, "{}", agc.gain_db10());
    }

    #[test]
    fn loud_input_is_brought_down_without_clipping() {
        let (agc, peak) = run(Signal::Tone { hz: 440, amp: 32_000 }, 3);
        assert!(peak <= 8_300, "{peak}");
        assert!(agc.gain() < UNITY);
    }

    #[test]
    fn silence_holds_the_gain() {
        let (agc, _) = run(Signal::Silence, 5);
        assert_eq!(agc.gain(), UNITY);
    }
}
