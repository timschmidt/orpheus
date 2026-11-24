//! Level meters on the output samples (what goes to the DAC): peak and RMS
//! per channel, and a count of samples at full scale, accumulated until the
//! UI takes them (≤ 15 times per second, 05 §7.1 "Levels").

use crate::math;

/// What the meter computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterMode {
    #[default]
    Off,
    /// Peak and clip count (cheap; 04 §5.1 "peak detector always").
    Peak,
    /// Peak, clip count and RMS (while the Levels view is visible).
    PeakRms,
}

/// Levels since the last [`Meter::take`]. Linear values are Q15
/// (32768 = full scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Levels {
    pub peak: [u16; 2],
    /// 0 unless the mode is [`MeterMode::PeakRms`].
    pub rms: [u16; 2],
    /// Samples at the 16-bit limits (any channel).
    pub clipped: u32,
    /// Frames measured.
    pub frames: u32,
}

/// Q15 level → tenths of a dBFS (−960 for silence). For the UI.
#[must_use]
pub fn db10(v: u16) -> i16 {
    if v == 0 {
        return -960;
    }
    let db = math::lin_to_db(f32::from(v) / 32768.0) * 10.0;
    let r = if db >= 0.0 { db + 0.5 } else { db - 0.5 };
    (r as i32).clamp(-960, 0) as i16
}

#[derive(Debug, Clone, Default)]
pub struct Meter {
    mode: MeterMode,
    /// Peak |x| in 16-bit units (0 … 32768).
    peak: [u32; 2],
    sq: [u64; 2],
    clipped: u32,
    frames: u32,
}

impl Meter {
    #[must_use]
    pub const fn new() -> Self {
        Self { mode: MeterMode::Off, peak: [0; 2], sq: [0; 2], clipped: 0, frames: 0 }
    }

    pub fn set_mode(&mut self, mode: MeterMode) {
        self.mode = mode;
        let _ = self.take();
    }

    #[must_use]
    pub fn mode(&self) -> MeterMode {
        self.mode
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.mode != MeterMode::Off
    }

    /// Measure interleaved samples already reduced to 16-bit integers (`v`
    /// maps each stored sample to that range).
    pub fn measure<S: Copy>(&mut self, buf: &[S], channels: usize, v: impl Fn(S) -> i32) {
        let ch = channels.clamp(1, 2);
        let rms = self.mode == MeterMode::PeakRms;
        if self.mode == MeterMode::Off {
            return;
        }
        let mut n = 0u32;
        for f in buf.chunks_exact(ch) {
            for (c, &s) in f.iter().enumerate() {
                let x = v(s);
                let a = x.unsigned_abs();
                self.peak[c] = self.peak[c].max(a);
                if a >= 32767 {
                    self.clipped = self.clipped.saturating_add(1);
                }
                if rms {
                    self.sq[c] = self.sq[c].wrapping_add(u64::from(a * a));
                }
            }
            n += 1;
        }
        if ch == 1 {
            self.peak[1] = self.peak[0];
            self.sq[1] = self.sq[0];
        }
        self.frames = self.frames.saturating_add(n);
    }

    /// The levels so far; starts a new period.
    pub fn take(&mut self) -> Levels {
        let mut l = Levels { clipped: self.clipped, frames: self.frames, ..Levels::default() };
        for c in 0..2 {
            l.peak[c] = self.peak[c].min(32768) as u16;
            if self.mode == MeterMode::PeakRms && self.frames > 0 {
                let ms = self.sq[c] / u64::from(self.frames);
                l.rms[c] = ms.isqrt().min(32768) as u16;
            }
        }
        self.peak = [0; 2];
        self.sq = [0; 2];
        self.clipped = 0;
        self.frames = 0;
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_wave_levels() {
        let mut m = Meter::new();
        m.set_mode(MeterMode::PeakRms);
        let buf: [i16; 200] = core::array::from_fn(|i| if (i / 2) % 2 == 0 { 16384 } else { -16384 });
        m.measure(&buf, 2, i32::from);
        let l = m.take();
        assert_eq!(l.peak, [16384, 16384]);
        assert_eq!(l.rms, [16384, 16384]);
        assert_eq!(l.frames, 100);
        assert_eq!(db10(16384), -60);
        assert_eq!(db10(32768), 0);
    }
}
