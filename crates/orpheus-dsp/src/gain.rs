//! ReplayGain resolution and the software gain stage (ReplayGain + pre-amp
//! + EQ pre-amp: the only software gains, 04 §3.3).

use crate::fixed::{Ramp, UNITY, mul, q28};
use crate::math;

/// Ramp length of a gain change: the same as the equaliser's, so the
/// automatic pre-amp tracks the boost it compensates.
pub const GAIN_RAMP_MS: u32 = crate::eq::EQ_RAMP_MS;

/// Which ReplayGain value to use. "Auto" (album when playing an album in
/// order, track when shuffling) is resolved by the playback service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RgMode {
    #[default]
    Off,
    Track,
    Album,
}

/// The user's ReplayGain settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgSettings {
    pub mode: RgMode,
    /// Pre-amp added to the tag gain, mB.
    pub preamp_mb: i32,
    /// Lower the gain so `peak × gain ≤ 1` when the tag has a peak.
    pub prevent_clip: bool,
    /// Gain for files without tags (mB), when the mode is not `Off`.
    pub fallback_mb: i32,
}

impl Default for RgSettings {
    fn default() -> Self {
        Self { mode: RgMode::Off, preamp_mb: 0, prevent_clip: true, fallback_mb: 0 }
    }
}

/// A track's gain tags, in the units of `orpheus-meta::Gains` (gains in mB,
/// peaks in millionths of full scale). R128 gains (reference −23 LUFS) are
/// converted by the caller: ReplayGain 2 gain = R128 gain + 500 mB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RgTags {
    pub track_gain_mb: Option<i32>,
    pub track_peak_ppm: Option<u32>,
    pub album_gain_mb: Option<i32>,
    pub album_peak_ppm: Option<u32>,
}

/// The ReplayGain gain to apply, mB (0 when off).
#[must_use]
pub fn replaygain_mb(s: &RgSettings, t: &RgTags) -> i32 {
    let (gain, peak) = match s.mode {
        RgMode::Off => return 0,
        RgMode::Track => (
            t.track_gain_mb.or(t.album_gain_mb),
            if t.track_gain_mb.is_some() { t.track_peak_ppm } else { t.album_peak_ppm },
        ),
        RgMode::Album => (
            t.album_gain_mb.or(t.track_gain_mb),
            if t.album_gain_mb.is_some() { t.album_peak_ppm } else { t.track_peak_ppm },
        ),
    };
    let mut g = match gain {
        Some(g) => g.clamp(-6000, 6000) + s.preamp_mb,
        None => s.fallback_mb,
    };
    if s.prevent_clip
        && gain.is_some()
        && let Some(p) = peak.filter(|p| *p > 0)
    {
        // gain ≤ −20·log10(peak).
        let lim = -math::lin_to_db(p as f32 / 1e6) * 100.0;
        let lim = if lim >= 0.0 { (lim + 0.5) as i32 } else { (lim - 0.5) as i32 };
        g = g.min(lim);
    }
    g
}

/// mB → Q3.28 linear gain (saturates at +18 dB).
#[must_use]
pub fn mb_to_q28(mb: i32) -> i32 {
    if mb == 0 { UNITY } else { q28(math::db_to_lin(mb as f32 / 100.0)) }
}

/// One gain for all channels, ramped per frame.
#[derive(Debug, Clone)]
pub struct Gain {
    ramp: Ramp<1>,
    mb: i32,
    rate: u32,
}

impl Default for Gain {
    fn default() -> Self {
        Self::new()
    }
}

impl Gain {
    #[must_use]
    pub const fn new() -> Self {
        Self { ramp: Ramp::new([UNITY]), mb: 0, rate: 44100 }
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
    }

    /// Target gain in mB, reached over [`GAIN_RAMP_MS`] (at once if
    /// `immediate`).
    pub fn set_mb(&mut self, mb: i32, immediate: bool) {
        self.mb = mb;
        let frames = if immediate { 0 } else { self.rate * GAIN_RAMP_MS / 1000 };
        self.ramp.to([mb_to_q28(mb)], frames);
    }

    #[must_use]
    pub fn mb(&self) -> i32 {
        self.mb
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.ramp.ramping() || self.ramp.cur[0] != UNITY
    }

    /// No ramp moves: [`Self::is_active`] stays as it is until a setter.
    #[must_use]
    pub fn settled(&self) -> bool {
        !self.ramp.ramping()
    }

    #[must_use]
    pub fn may_boost(&self) -> bool {
        self.ramp.cur[0] > UNITY || self.ramp.target[0] > UNITY
    }

    pub fn process(&mut self, buf: &mut [i32], channels: usize) {
        let ch = channels.max(1);
        if !self.ramp.ramping() {
            let g = self.ramp.cur[0];
            for x in buf.iter_mut() {
                *x = mul(*x, g);
            }
            return;
        }
        for f in buf.chunks_exact_mut(ch) {
            let [g] = self.ramp.tick();
            for x in f {
                *x = mul(*x, g);
            }
        }
    }

    /// Finish any ramp at once.
    pub fn reset(&mut self) {
        let t = self.ramp.target;
        self.ramp.jump(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution() {
        let tags = RgTags {
            track_gain_mb: Some(-648),
            track_peak_ppm: Some(988_525),
            album_gain_mb: Some(210),
            album_peak_ppm: Some(1_200_000),
        };
        let mut s = RgSettings { mode: RgMode::Track, ..RgSettings::default() };
        assert_eq!(replaygain_mb(&s, &tags), -648);
        s.mode = RgMode::Album;
        // +2.10 dB but peak 1.2 → limited to −1.58 dB.
        assert_eq!(replaygain_mb(&s, &tags), -158);
        s.prevent_clip = false;
        s.preamp_mb = 300;
        assert_eq!(replaygain_mb(&s, &tags), 510);
        s.fallback_mb = -600;
        assert_eq!(replaygain_mb(&s, &RgTags::default()), -600);
        s.mode = RgMode::Off;
        assert_eq!(replaygain_mb(&s, &tags), 0);
    }
}
