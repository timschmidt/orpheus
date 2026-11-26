//! Tap tempo: the tempo from the last few taps of a key.

use crate::model::{TEMPO_MAX, TEMPO_MIN};

/// Taps remembered.
pub const TAPS: usize = 4;
/// A pause longer than this starts a new count, ms.
pub const RESET_MS: u64 = 2_000;

/// The last taps' times.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TapTempo {
    taps: [u64; TAPS],
    n: u8,
}

impl TapTempo {
    /// No taps.
    #[must_use]
    pub const fn new() -> Self {
        TapTempo { taps: [0; TAPS], n: 0 }
    }

    /// A tap at `ms`: from the second tap on, the tempo of the mean
    /// interval of the last [`TAPS`] taps, in 1/100 BPM (clamped to
    /// 40–240 BPM). A pause over [`RESET_MS`] (or time going back) starts
    /// again.
    pub fn tap(&mut self, ms: u64) -> Option<u16> {
        let n = usize::from(self.n);
        if n > 0 && (ms <= self.taps[n - 1] || ms - self.taps[n - 1] > RESET_MS) {
            self.n = 0;
        }
        let n = usize::from(self.n);
        if n == TAPS {
            self.taps.copy_within(1.., 0);
            self.taps[TAPS - 1] = ms;
        } else {
            self.taps[n] = ms;
            self.n += 1;
        }
        let n = usize::from(self.n);
        if n < 2 {
            return None;
        }
        let span = self.taps[n - 1] - self.taps[0];
        let per = span * 100 / (n as u64 - 1);
        // 60,000 ms a minute, × 100 for hundredths, × 100 for `per`.
        let t = (600_000_000 + per / 2) / per.max(1);
        Some(t.clamp(u64::from(TEMPO_MIN), u64::from(TEMPO_MAX)) as u16)
    }

    /// Taps counted.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.n as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_give_the_tempo() {
        let mut t = TapTempo::new();
        assert_eq!(t.tap(1000), None);
        assert_eq!(t.tap(1500), Some(12_000));
        assert_eq!(t.tap(2000), Some(12_000));
        // Uneven taps: the mean of the last four.
        // 1600 ms over three intervals: 112.5 BPM.
        assert_eq!(t.tap(2600), Some(11_250));
        // The first tap drops out: 1500 … 3300 is 600 ms a beat.
        assert_eq!(t.tap(3300), Some(10_000));
        // A long pause starts again.
        assert_eq!(t.tap(9000), None);
        assert_eq!(t.tap(9100), Some(24_000), "clamped");
        assert_eq!(t.tap(12_000), None);
        assert_eq!(t.tap(13_900), Some(4_000), "clamped");
    }
}
