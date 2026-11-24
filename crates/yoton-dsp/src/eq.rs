//! Parametric equaliser and bass/treble tone control.
//!
//! Parameters are integers (settings-friendly): frequency in Hz, Q in
//! hundredths, gain in millibels (hundredths of a dB). A change moves the
//! band's parameters (log-frequency, Q and gain in dB) linearly from where
//! they are to the new values over [`EQ_RAMP_MS`]; each processed chunk
//! designs one intermediate section and the cascade interpolates the
//! coefficients per frame between designs, so nothing steps.
//!
//! A band whose frequency is at or above 0.49 × the stream rate is treated as
//! flat (it does not exist at that rate).

use crate::biquad::{self, BiquadCascade, Coeffs, Shape, SoftCascade};
use crate::math;

/// Ramp length of an equaliser or tone change.
pub const EQ_RAMP_MS: u32 = 30;

/// Most bands the equaliser runs.
pub const MAX_BANDS: usize = 10;

/// One band as the user sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub shape: Shape,
    pub freq_hz: u32,
    /// Q × 100 (70 = 0.70).
    pub q100: u16,
    /// Gain in millibels (hundredths of a dB).
    pub gain_mb: i16,
}

impl Band {
    #[must_use]
    pub const fn peaking(freq_hz: u32, q100: u16, gain_mb: i16) -> Self {
        Self { shape: Shape::Peaking, freq_hz, q100, gain_mb }
    }

    /// The band with gain 0 (as it is when the equaliser is off).
    #[must_use]
    pub const fn flat(self) -> Self {
        Self { gain_mb: 0, ..self }
    }
}

/// Centre frequencies of the five-band editor (05 §7.1 "Equaliser screen").
pub const EQ5_FREQS: [u32; 5] = [60, 230, 910, 3600, 14000];
/// Q of the five-band editor's bells: two octaves apart, Q = 0.7 overlaps
/// neighbours at about −3 dB.
pub const EQ5_Q100: u16 = 70;

/// Presets of the equaliser screen (05 §7.1). `Custom` bands come from the
/// settings store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Flat,
    Rock,
    Pop,
    Jazz,
    Classical,
    Electronic,
    HipHop,
    BassBoost,
    Vocal,
}

impl Preset {
    pub const ALL: [Self; 9] = [
        Self::Flat,
        Self::Rock,
        Self::Pop,
        Self::Jazz,
        Self::Classical,
        Self::Electronic,
        Self::HipHop,
        Self::BassBoost,
        Self::Vocal,
    ];

    /// Gains in dB for the five bands of [`EQ5_FREQS`]. yotonrs's own
    /// choices (moderate, ≤ 6 dB, so the automatic pre-amp stays small).
    #[must_use]
    pub const fn gains_db(self) -> [i8; 5] {
        match self {
            Self::Flat => [0, 0, 0, 0, 0],
            Self::Rock => [4, 2, -2, 2, 4],
            Self::Pop => [-1, 2, 4, 2, -1],
            Self::Jazz => [3, 1, 0, 2, 3],
            Self::Classical => [3, 2, -1, 2, 3],
            Self::Electronic => [5, 2, 0, 2, 4],
            Self::HipHop => [6, 3, 0, 1, 2],
            Self::BassBoost => [6, 3, 0, 0, 0],
            Self::Vocal => [-2, 0, 4, 3, 0],
        }
    }

    /// The five bands of this preset.
    #[must_use]
    pub fn bands(self) -> [Band; 5] {
        let g = self.gains_db();
        core::array::from_fn(|i| Band::peaking(EQ5_FREQS[i], EQ5_Q100, i16::from(g[i]) * 100))
    }
}

/// A band's parameters as the ramp interpolates them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Params {
    log2_freq: f32,
    q: f32,
    gain_db: f32,
}

impl Params {
    fn of(b: &Band, rate: u32) -> Self {
        let exists = b.freq_hz > 0 && (b.freq_hz as f32) < 0.49 * rate as f32;
        Self {
            log2_freq: math::log2(b.freq_hz.max(1) as f32),
            q: f32::from(b.q100.max(1)) / 100.0,
            gain_db: if exists { f32::from(b.gain_mb) / 100.0 } else { 0.0 },
        }
    }

    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        Self {
            log2_freq: a.log2_freq + (b.log2_freq - a.log2_freq) * t,
            q: a.q + (b.q - a.q) * t,
            gain_db: a.gain_db + (b.gain_db - a.gain_db) * t,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BandState {
    /// What the user set.
    set: Band,
    /// Shape the section has now (a shape change goes through 0 dB).
    shape: Shape,
    from: Params,
    to: Params,
    /// Frames into the ramp and its length; `t == len` when settled.
    t: u32,
    len: u32,
}

/// An `N`-band equaliser over any [`BiquadCascade`].
///
/// RAM: 48 bytes per band plus the cascade (100 bytes per section for
/// [`SoftCascade`]); 1496 bytes for ten bands.
#[derive(Debug, Clone)]
pub struct Equalizer<const N: usize, C: BiquadCascade = SoftCascade<N>> {
    bands: [BandState; N],
    n: usize,
    rate: u32,
    cascade: C,
}

impl<const N: usize> Default for Equalizer<N, SoftCascade<N>> {
    fn default() -> Self {
        Self::new(SoftCascade::new())
    }
}

impl<const N: usize, C: BiquadCascade> Equalizer<N, C> {
    /// No bands, rate 44.1 kHz until [`Equalizer::set_rate`].
    #[must_use]
    pub fn new(cascade: C) -> Self {
        let b = Band::peaking(1000, 70, 0);
        let p = Params::of(&b, 44100);
        let st = BandState { set: b, shape: b.shape, from: p, to: p, t: 0, len: 0 };
        Self { bands: [st; N], n: 0, rate: 44100, cascade }
    }

    /// The bands as set.
    pub fn bands(&self) -> impl Iterator<Item = Band> + '_ {
        self.bands[..self.n].iter().map(|b| b.set)
    }

    /// The stream rate changed: redesign everything at once (no ramp; the
    /// pipeline mutes around a rate switch) and forget the history.
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
        for i in 0..self.n {
            let b = &mut self.bands[i];
            b.shape = b.set.shape;
            b.to = Params::of(&b.set, self.rate);
            b.from = b.to;
            b.t = 0;
            b.len = 0;
            let c = section(b.shape, &b.to, self.rate);
            self.cascade.set(i, c, 0);
        }
        self.cascade.reset();
    }

    /// Replace all bands (at most `N`; extra bands are ignored). Bands that
    /// disappear ramp to flat first, so the number of bands only shrinks
    /// once they are flat.
    pub fn set_bands(&mut self, bands: &[Band]) {
        let m = bands.len().min(N);
        for (i, b) in bands.iter().take(m).enumerate() {
            self.set_band(i, *b);
        }
        for i in m..self.n {
            let b = self.bands[i].set.flat();
            self.set_band(i, b);
        }
    }

    /// Set band `i` (`i < N`), growing the band count if needed.
    pub fn set_band(&mut self, i: usize, band: Band) {
        if i >= N {
            return;
        }
        while self.n <= i {
            // New bands start flat at their own frequency.
            let st = &mut self.bands[self.n];
            let flat = band.flat();
            st.set = flat;
            st.shape = flat.shape;
            st.to = Params::of(&flat, self.rate);
            st.from = st.to;
            st.t = 0;
            st.len = 0;
            self.cascade.set(self.n, Coeffs::IDENTITY, 0);
            self.n += 1;
        }
        let rate = self.rate;
        let st = &mut self.bands[i];
        if st.set == band {
            return;
        }
        st.set = band;
        let cur = current(st);
        st.from = cur;
        st.t = 0;
        st.len = ramp_frames(rate);
        st.to = if band.shape == st.shape || cur.gain_db == 0.0 {
            st.shape = band.shape;
            Params::of(&band, rate)
        } else {
            // Ramp the old shape to flat; `step` continues to the new one.
            Params { gain_db: 0.0, ..cur }
        };
        if st.to == st.from || (st.to.gain_db == 0.0 && st.from.gain_db == 0.0 && st.shape == band.shape) {
            // Nothing audible moves (e.g. retuning a flat band).
            st.from = st.to;
            st.len = 0;
            let c = section(st.shape, &st.to, rate);
            self.cascade.set(i, c, 0);
        }
    }

    /// Whether processing changes anything.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.bands[..self.n].iter().any(|b| b.to.gain_db != 0.0 || b.t < b.len)
            || (0..self.n).any(|i| self.cascade.ramping(i))
    }

    /// Whether any band boosts (now or at its target).
    #[must_use]
    pub fn may_boost(&self) -> bool {
        self.bands[..self.n].iter().any(|b| b.to.gain_db > 0.0 || b.from.gain_db > 0.0 && b.t < b.len)
    }

    /// Bands that cost cycles in the next chunk.
    #[must_use]
    pub fn busy_sections(&self) -> usize {
        self.bands[..self.n]
            .iter()
            .enumerate()
            .filter(|(i, b)| b.to.gain_db != 0.0 || b.t < b.len || self.cascade.ramping(*i))
            .count()
    }

    /// Magnitude in dB at `freq` of the response the settings ask for
    /// (targets, not the ramp position). For UI curves and the automatic
    /// pre-amp.
    #[must_use]
    pub fn response_db(&self, freq: f32) -> f32 {
        let mut m = 1.0;
        for b in &self.bands[..self.n] {
            let p = Params::of(&b.set, self.rate);
            m *= section(b.set.shape, &p, self.rate).magnitude(freq, self.rate);
        }
        math::lin_to_db(m)
    }

    /// Largest boost of the target response over the audio band, in mB
    /// (0 if the response nowhere exceeds unity).
    #[must_use]
    pub fn peak_boost_mb(&self) -> i32 {
        if !self.bands[..self.n].iter().any(|b| b.set.gain_mb > 0) {
            return 0;
        }
        let top = (0.45 * self.rate as f32).min(20000.0);
        let mut worst: f32 = 0.0;
        // 48 log-spaced points 20 Hz … top, plus every band's own frequency.
        let l0 = math::log2(20.0);
        let l1 = math::log2(top);
        for k in 0..48 {
            let f = math::exp2(l0 + (l1 - l0) * k as f32 / 47.0);
            worst = worst.max(self.response_db(f));
        }
        for b in &self.bands[..self.n] {
            let f = (b.set.freq_hz as f32).min(top);
            worst = worst.max(self.response_db(f));
        }
        (worst * 100.0 + 0.5) as i32
    }

    /// Advance the ramps by one chunk of `frames` frames and hand the
    /// cascade the coefficients for the end of the chunk.
    fn step(&mut self, frames: u32) {
        for i in 0..self.n {
            let st = &mut self.bands[i];
            if st.t >= st.len {
                continue;
            }
            st.t = (st.t + frames).min(st.len);
            let p = Params::lerp(&st.from, &st.to, st.t as f32 / st.len as f32);
            let p = if st.t == st.len { st.to } else { p };
            self.cascade.set(i, section(st.shape, &p, self.rate), frames);
            if st.t == st.len && st.shape != st.set.shape {
                // Reached flat in the old shape: continue in the new one.
                st.shape = st.set.shape;
                st.from = Params { gain_db: 0.0, ..Params::of(&st.set, self.rate) };
                st.to = Params::of(&st.set, self.rate);
                st.t = 0;
            }
        }
    }

    /// Run over a chunk (interleaved Q4.27, at most [`crate::CHUNK`] frames).
    pub fn process(&mut self, buf: &mut [i32], channels: usize) {
        let frames = (buf.len() / channels.max(1)) as u32;
        self.step(frames);
        self.cascade.process(buf, channels);
    }

    /// Forget the signal history; ramps finish at once.
    pub fn reset(&mut self) {
        for i in 0..self.n {
            let st = &mut self.bands[i];
            st.shape = st.set.shape;
            st.to = Params::of(&st.set, self.rate);
            st.from = st.to;
            st.t = 0;
            st.len = 0;
            self.cascade.set(i, section(st.shape, &st.to, self.rate), 0);
        }
        self.cascade.reset();
    }

    /// The cascade (for engines that report their own statistics).
    pub fn cascade(&self) -> &C {
        &self.cascade
    }
}

fn current(st: &BandState) -> Params {
    if st.len == 0 { st.to } else { Params::lerp(&st.from, &st.to, st.t as f32 / st.len as f32) }
}

fn ramp_frames(rate: u32) -> u32 {
    (rate * EQ_RAMP_MS / 1000).max(1)
}

fn section(shape: Shape, p: &Params, rate: u32) -> Coeffs {
    biquad::design(shape, math::exp2(p.log2_freq), p.q, p.gain_db, rate)
}

/// Bass shelf corner frequency.
pub const BASS_HZ: u32 = 120;
/// Treble shelf corner frequency.
pub const TREBLE_HZ: u32 = 6000;
/// Shelf Q (cookbook slope S = 1: monotonic, no overshoot).
pub const SHELF_Q100: u16 = 71;

/// Bass and treble shelves: a two-band [`Equalizer`] with a low shelf at
/// [`BASS_HZ`] and a high shelf at [`TREBLE_HZ`].
#[derive(Debug, Clone)]
pub struct Tone<C: BiquadCascade = SoftCascade<2>> {
    eq: Equalizer<2, C>,
}

impl Default for Tone<SoftCascade<2>> {
    fn default() -> Self {
        Self::new(SoftCascade::new())
    }
}

impl<C: BiquadCascade> Tone<C> {
    #[must_use]
    pub fn new(cascade: C) -> Self {
        let mut eq = Equalizer::new(cascade);
        eq.set_band(0, Band { shape: Shape::LowShelf, freq_hz: BASS_HZ, q100: SHELF_Q100, gain_mb: 0 });
        eq.set_band(1, Band { shape: Shape::HighShelf, freq_hz: TREBLE_HZ, q100: SHELF_Q100, gain_mb: 0 });
        Self { eq }
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.eq.set_rate(rate);
    }

    /// Bass gain in mB (the UI offers ±12 dB).
    pub fn set_bass(&mut self, mb: i16) {
        self.eq.set_band(0, Band { shape: Shape::LowShelf, freq_hz: BASS_HZ, q100: SHELF_Q100, gain_mb: mb });
    }

    /// Treble gain in mB.
    pub fn set_treble(&mut self, mb: i16) {
        self.eq
            .set_band(1, Band { shape: Shape::HighShelf, freq_hz: TREBLE_HZ, q100: SHELF_Q100, gain_mb: mb });
    }

    #[must_use]
    pub fn equalizer(&self) -> &Equalizer<2, C> {
        &self.eq
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.eq.is_active()
    }

    #[must_use]
    pub fn may_boost(&self) -> bool {
        self.eq.may_boost()
    }

    pub fn process(&mut self, buf: &mut [i32], channels: usize) {
        self.eq.process(buf, channels);
    }

    pub fn reset(&mut self) {
        self.eq.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_equalizer_is_inactive() {
        let mut eq = Equalizer::<5>::default();
        eq.set_bands(&Preset::Flat.bands());
        assert!(!eq.is_active());
        eq.set_bands(&Preset::Rock.bands());
        assert!(eq.is_active());
        assert!(eq.may_boost());
        let pk = eq.peak_boost_mb();
        assert!((350..=600).contains(&pk), "{pk}");
    }

    #[test]
    fn band_above_nyquist_is_flat() {
        let mut eq = Equalizer::<1>::default();
        eq.set_rate(22050);
        eq.set_band(0, Band::peaking(14000, 70, 600));
        assert!(!eq.is_active());
    }
}
