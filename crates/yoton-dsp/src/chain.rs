//! The chain: every stage in its fixed order, run in place over the
//! decoder's block.

use yoton_audio::rate::{RatePlan, rate_plan};
use yoton_audio::{PcmBlock, SampleKind, Unsupported};

use crate::biquad::{BiquadCascade, SoftCascade};
use crate::cost::{self, Cost, StageId};
use crate::crossfeed::{Crossfeed, CrossfeedParams};
use crate::decimate::Decimator;
use crate::dither::{DitherMode, Ditherer, round16};
use crate::eq::{Band, Equalizer, MAX_BANDS, Preset, Tone};
use crate::fade::Fade;
use crate::fixed::UNITY;
use crate::gain::{Gain, RgSettings, RgTags, replaygain_mb};
use crate::limiter::Limiter;
use crate::matrix::{ChannelMode, Matrix};
use crate::meter::{Levels, Meter, MeterMode};

/// Frames per internal chunk (input frames; fewer after decimation). The
/// work buffer is `CHUNK × 2` words = 512 bytes.
pub const CHUNK: usize = 64;

/// Largest boost (mB) the gains and filters together may apply before the
/// headroom guard lowers the gain stage: 18 dB, 6 dB under the 24 dB the
/// Q4.27 format has before anything wraps.
pub const HEADROOM_MB: i32 = 1800;

/// Word length at the DAC FIFO (04 §3.2, `yoton_audio::rate::WidthPlan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutWidth {
    /// 16-bit FIFO: `I32` blocks leave the chain as `s16 << 16`.
    #[default]
    Bits16,
    /// 24-bit FIFO: `I32` blocks keep 24 bits.
    Bits24,
}

/// The stream the chain is set up for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u8,
    pub kind: SampleKind,
    /// Significant bits in the source.
    pub bits: u8,
}

/// The equaliser's pre-amp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqPreamp {
    /// Minus the peak of the EQ + tone response (05 §7.1: "pre-amp row
    /// avoids clipping automatically").
    Auto,
    /// A fixed gain, mB.
    Manual(i32),
}

/// A sample type of [`PcmBlock`].
trait Pcm: Copy {
    const WIDE: bool;
    /// To Q4.27.
    fn load(self) -> i32;
    /// From a 16-bit integer.
    fn from16(q: i32) -> Self;
    /// From Q4.27, rounded to 24 bits (wide types only).
    fn from24(x: i32) -> Self;
    /// As a 16-bit integer, for the meter.
    fn level(self) -> i32;
}

impl Pcm for i16 {
    const WIDE: bool = false;
    #[inline(always)]
    fn load(self) -> i32 {
        i32::from(self) << 12
    }
    #[inline(always)]
    fn from16(q: i32) -> Self {
        q as i16
    }
    #[inline(always)]
    fn from24(x: i32) -> Self {
        round16(x) as i16
    }
    #[inline(always)]
    fn level(self) -> i32 {
        i32::from(self)
    }
}

impl Pcm for i32 {
    const WIDE: bool = true;
    #[inline(always)]
    fn load(self) -> i32 {
        self >> 4
    }
    #[inline(always)]
    fn from16(q: i32) -> Self {
        q << 16
    }
    #[inline(always)]
    fn from24(x: i32) -> Self {
        (x.saturating_add(8) >> 4).clamp(-(1 << 23), (1 << 23) - 1) << 8
    }
    #[inline(always)]
    fn level(self) -> i32 {
        self >> 16
    }
}

/// The DSP chain. `E` runs the equaliser's sections ([`SoftCascade`] by
/// default; the hardware IIR engine of WP DSP-1 when it exists).
///
/// Order (04 §5.1; deviations explained in the crate documentation):
/// decimator → gain (ReplayGain, pre-amps) → equaliser → bass/treble →
/// crossfeed → channel mode and balance → limiter → fade → 16/24-bit
/// output (dither) → meter.
#[derive(Debug, Clone)]
pub struct Chain<E: BiquadCascade = SoftCascade<MAX_BANDS>> {
    fmt: Format,
    out_rate: u32,
    out_width: OutWidth,
    direct_high: bool,
    work: [i32; CHUNK * 2],
    decim: Decimator,
    gain: Gain,
    eq: Equalizer<MAX_BANDS, E>,
    tone: Tone,
    xfeed: Crossfeed,
    matrix: Matrix,
    limiter: Limiter,
    limiter_on: bool,
    fade: Fade,
    dither: Ditherer,
    meter: Meter,
    rg: RgSettings,
    tags: RgTags,
    preamp: EqPreamp,
}

impl Default for Chain<SoftCascade<MAX_BANDS>> {
    fn default() -> Self {
        Self::new(OutWidth::Bits16, false)
    }
}

impl Chain<SoftCascade<MAX_BANDS>> {
    /// A chain with the software equaliser. `out_width` and `direct_high`
    /// (the DAC takes 88.2/96 kHz directly, U-AUD-2) describe the DAC.
    #[must_use]
    pub fn new(out_width: OutWidth, direct_high: bool) -> Self {
        Self::with_cascade(SoftCascade::new(), out_width, direct_high)
    }
}

impl<E: BiquadCascade> Chain<E> {
    /// A chain whose equaliser runs on `cascade`.
    #[must_use]
    pub fn with_cascade(cascade: E, out_width: OutWidth, direct_high: bool) -> Self {
        let mut c = Self {
            fmt: Format { rate: 44100, channels: 2, kind: SampleKind::I16, bits: 16 },
            out_rate: 44100,
            out_width,
            direct_high,
            work: [0; CHUNK * 2],
            decim: Decimator::new(),
            gain: Gain::new(),
            eq: Equalizer::new(cascade),
            tone: Tone::default(),
            xfeed: Crossfeed::new(),
            matrix: Matrix::new(),
            limiter: Limiter::new(),
            limiter_on: true,
            fade: Fade::new(),
            dither: Ditherer::new(DitherMode::Tpdf),
            meter: Meter::new(),
            rg: RgSettings::default(),
            tags: RgTags::default(),
            preamp: EqPreamp::Auto,
        };
        c.apply_rate();
        c
    }

    // ------------------------------------------------------------ stream

    /// Set up for a stream. Returns the rate the DAC must run at. Called
    /// again with the same format (gapless track change) it keeps all state.
    ///
    /// # Errors
    /// `Unsupported::Channels` for more than two channels,
    /// `Unsupported::Rate` for a rate the output path cannot play.
    pub fn set_format(&mut self, fmt: Format) -> Result<u32, Unsupported> {
        if !(1..=2).contains(&fmt.channels) {
            return Err(Unsupported::Channels(fmt.channels));
        }
        let (factor, out) = match rate_plan(fmt.rate, self.direct_high) {
            RatePlan::Native(r) | RatePlan::DirectHigh(r) => (1, r),
            RatePlan::Decimate { factor, rate } => (factor, rate),
            RatePlan::Unsupported => return Err(Unsupported::Rate(fmt.rate)),
        };
        if fmt == self.fmt && out == self.out_rate {
            return Ok(out);
        }
        let rate_changed = out != self.out_rate || factor != self.decim.factor();
        self.fmt = fmt;
        self.out_rate = out;
        if rate_changed {
            self.decim.set_factor(factor);
            self.apply_rate();
        } else {
            self.reset();
        }
        Ok(out)
    }

    fn apply_rate(&mut self) {
        let r = self.out_rate;
        self.gain.set_rate(r);
        self.eq.set_rate(r);
        self.tone.set_rate(r);
        self.xfeed.set_rate(r);
        self.matrix.set_rate(r);
        self.limiter.set_rate(r);
        self.fade.set_rate(r);
        self.dither.set(self.dither.mode(), r);
        self.update_gain(true);
        self.reset();
    }

    /// The format set last.
    #[must_use]
    pub fn format(&self) -> Format {
        self.fmt
    }

    /// The DAC rate for the current format.
    #[must_use]
    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }

    /// Forget all signal history and finish all ramps (seek, flush). The
    /// pipeline fades around a seek (04 §2.2), so nothing is audible.
    pub fn reset(&mut self) {
        self.decim.reset();
        self.gain.reset();
        self.eq.reset();
        self.tone.reset();
        self.xfeed.reset();
        self.matrix.reset();
        self.limiter.reset();
        self.dither.reset();
    }

    // -------------------------------------------------------- parameters

    /// ReplayGain settings (ramped).
    pub fn set_replaygain(&mut self, s: RgSettings) {
        self.rg = s;
        self.update_gain(false);
    }

    /// The current track's gain tags (ramped; call at the track boundary).
    pub fn set_track_gains(&mut self, t: RgTags) {
        self.tags = t;
        self.update_gain(false);
    }

    /// The equaliser's bands (up to [`MAX_BANDS`]; fewer bands ramp the
    /// rest to flat).
    pub fn set_eq(&mut self, bands: &[Band]) {
        self.eq.set_bands(bands);
        self.update_gain(false);
    }

    /// One band of the equaliser.
    pub fn set_eq_band(&mut self, i: usize, band: Band) {
        self.eq.set_band(i, band);
        self.update_gain(false);
    }

    /// One of the built-in presets (five bands).
    pub fn set_eq_preset(&mut self, p: Preset) {
        self.set_eq(&p.bands());
    }

    pub fn set_eq_preamp(&mut self, p: EqPreamp) {
        self.preamp = p;
        self.update_gain(false);
    }

    /// Bass shelf gain, mB.
    pub fn set_bass(&mut self, mb: i16) {
        self.tone.set_bass(mb);
        self.update_gain(false);
    }

    /// Treble shelf gain, mB.
    pub fn set_treble(&mut self, mb: i16) {
        self.tone.set_treble(mb);
        self.update_gain(false);
    }

    /// Crossfeed on (with these parameters) or off.
    pub fn set_crossfeed(&mut self, p: Option<CrossfeedParams>) {
        self.xfeed.set(p);
    }

    /// −100 (left only) … +100 (right only).
    pub fn set_balance(&mut self, b: i8) {
        self.matrix.set_balance(b);
    }

    pub fn set_channel_mode(&mut self, m: ChannelMode) {
        self.matrix.set_mode(m);
    }

    /// Allow the limiter (on by default). It engages only while a stage can
    /// raise the level.
    pub fn set_limiter(&mut self, on: bool) {
        self.limiter_on = on;
    }

    pub fn set_dither(&mut self, m: DitherMode) {
        self.dither.set(m, self.out_rate);
    }

    /// Restart the dither generator with `seed` (tests).
    pub fn seed_dither(&mut self, seed: u32) {
        self.dither.seed(seed);
    }

    pub fn set_meter(&mut self, m: MeterMode) {
        self.meter.set_mode(m);
    }

    /// Levels since the last call.
    pub fn take_levels(&mut self) -> Levels {
        self.meter.take()
    }

    /// Fade linearly to `level` (Q3.28; 0 silent, [`UNITY`] full) over `ms`.
    pub fn fade_to(&mut self, level: i32, ms: u32) {
        self.fade.to(level, ms);
    }

    pub fn fade_out(&mut self, ms: u32) {
        self.fade.to(0, ms);
    }

    pub fn fade_in(&mut self, ms: u32) {
        self.fade.to(UNITY, ms);
    }

    /// The last fade has finished.
    #[must_use]
    pub fn fade_done(&self) -> bool {
        self.fade.done()
    }

    /// Frames the limiter reduced since the last call (diagnostics).
    pub fn take_limited(&mut self) -> u32 {
        self.limiter.take_engaged()
    }

    /// Gain the gain stage is heading for, mB (ReplayGain + pre-amps after
    /// the headroom guard).
    #[must_use]
    pub fn gain_mb(&self) -> i32 {
        self.gain.mb()
    }

    /// The equaliser (for its response curve in the UI).
    #[must_use]
    pub fn equalizer(&self) -> &Equalizer<MAX_BANDS, E> {
        &self.eq
    }

    /// Largest boost of EQ and tone together, mB.
    fn filter_boost_mb(&self) -> i32 {
        let te = self.tone.equalizer();
        let eq_up = self.eq.bands().any(|b| b.gain_mb > 0);
        let tone_up = te.bands().any(|b| b.gain_mb > 0);
        if !eq_up && !tone_up {
            return 0;
        }
        if !tone_up {
            return self.eq.peak_boost_mb();
        }
        if !eq_up {
            return te.peak_boost_mb();
        }
        // Both: evaluate the product on the same grid.
        let top = (0.45 * self.out_rate as f32).min(20000.0);
        let (l0, l1) = (crate::math::log2(20.0), crate::math::log2(top));
        let mut worst: f32 = 0.0;
        for k in 0..64 {
            let f = crate::math::exp2(l0 + (l1 - l0) * k as f32 / 63.0);
            worst = worst.max(self.eq.response_db(f) + te.response_db(f));
        }
        for b in self.eq.bands() {
            let f = (b.freq_hz as f32).min(top);
            worst = worst.max(self.eq.response_db(f) + te.response_db(f));
        }
        (worst * 100.0 + 0.5) as i32
    }

    fn update_gain(&mut self, immediate: bool) {
        let rg = replaygain_mb(&self.rg, &self.tags);
        let boost = self.filter_boost_mb().max(0);
        let pre = match self.preamp {
            EqPreamp::Auto => -boost,
            EqPreamp::Manual(mb) => mb,
        };
        let mut total = rg + pre;
        if total + boost > HEADROOM_MB {
            total = HEADROOM_MB - boost;
        }
        if total != self.gain.mb() || immediate {
            self.gain.set_mb(total, immediate);
        }
    }

    // -------------------------------------------------------- processing

    fn limiter_active(&self) -> bool {
        (self.limiter_on
            && (self.gain.may_boost()
                || self.eq.may_boost()
                || self.tone.may_boost()
                || (self.fmt.channels == 2 && self.xfeed.is_active())
                || self.decim.is_active()))
            || !self.limiter.is_idle()
    }

    /// Bit set of the stages that run on the next block (bit = [`StageId`]
    /// as `u16`), for diagnostics and the cost model.
    #[must_use]
    pub fn active(&self) -> u16 {
        let stereo = self.fmt.channels == 2;
        let bit = |id: StageId, on: bool| if on { 1u16 << id as u16 } else { 0 };
        let mut m = 0u16;
        let set = |m: &mut u16, id: StageId, on: bool| *m |= bit(id, on);
        set(&mut m, StageId::Decimator, self.decim.is_active());
        set(&mut m, StageId::Gain, self.gain.is_active());
        set(&mut m, StageId::Eq, self.eq.is_active());
        set(&mut m, StageId::Tone, self.tone.is_active());
        set(&mut m, StageId::Crossfeed, stereo && self.xfeed.is_active());
        set(&mut m, StageId::Matrix, stereo && self.matrix.is_active());
        set(&mut m, StageId::Limiter, self.limiter_active());
        set(&mut m, StageId::Fade, self.fade.is_active());
        let processing = m != 0;
        let wide16 =
            self.fmt.kind == SampleKind::I32 && self.out_width == OutWidth::Bits16 && self.fmt.bits > 16;
        set(&mut m, StageId::Output, processing || wide16);
        set(&mut m, StageId::Meter, self.meter.is_active());
        m
    }

    /// Estimated cost of the next block, Cortex-M4 cycles per output frame
    /// (see [`crate::cost`]).
    #[must_use]
    pub fn cost(&self) -> Cost {
        let act = self.active();
        let on = |id: StageId| act & (1 << id as u16) != 0;
        let ch = u32::from(self.fmt.channels);
        let mut c = Cost::default();
        let f = u32::from(self.decim.factor());
        if on(StageId::Decimator) {
            c.add(StageId::Decimator, cost::decimator(f, ch));
        }
        // Converting in and out of the work buffer.
        if on(StageId::Output) {
            c.add(StageId::Output, cost::convert(f, ch) + cost::output(self.dither_cost_mode(), ch));
        }
        if on(StageId::Gain) {
            c.add(StageId::Gain, cost::GAIN_PER_SAMPLE * ch);
        }
        if on(StageId::Eq) {
            c.add(StageId::Eq, cost::biquads(self.eq.busy_sections() as u32, ch));
        }
        if on(StageId::Tone) {
            c.add(StageId::Tone, cost::biquads(self.tone.equalizer().busy_sections() as u32, ch));
        }
        if on(StageId::Crossfeed) {
            c.add(StageId::Crossfeed, cost::CROSSFEED_PER_FRAME);
        }
        if on(StageId::Matrix) {
            c.add(StageId::Matrix, cost::MATRIX_PER_FRAME);
        }
        if on(StageId::Limiter) {
            c.add(StageId::Limiter, cost::limiter(ch));
        }
        if on(StageId::Fade) {
            c.add(StageId::Fade, cost::GAIN_PER_SAMPLE * ch);
        }
        if on(StageId::Meter) {
            c.add(StageId::Meter, cost::meter(self.meter.mode(), ch));
        }
        c
    }

    fn dither_cost_mode(&self) -> DitherMode {
        if self.fmt.kind == SampleKind::I32 && self.out_width == OutWidth::Bits24 {
            DitherMode::Off
        } else {
            self.dither.mode()
        }
    }

    /// Process one decoder block in place. Returns the number of valid
    /// samples now at the start of the block (fewer than before when
    /// decimating; a trailing partial frame is dropped). When no stage is
    /// active the block is not touched (bit-exact).
    pub fn process(&mut self, block: &mut PcmBlock<'_>) -> usize {
        match block {
            PcmBlock::I16(s) => self.run(s),
            PcmBlock::I32(s) => self.run(s),
        }
    }

    /// [`Chain::process`] for an `i16` slice.
    pub fn process_i16(&mut self, s: &mut [i16]) -> usize {
        self.run(s)
    }

    /// [`Chain::process`] for a left-justified `i32` slice.
    pub fn process_i32(&mut self, s: &mut [i32]) -> usize {
        self.run(s)
    }

    fn run<S: Pcm>(&mut self, s: &mut [S]) -> usize {
        let ch = usize::from(self.fmt.channels.clamp(1, 2));
        let act = self.active();
        let on = |id: StageId| act & (1 << id as u16) != 0;
        if !on(StageId::Output) {
            if self.meter.is_active() {
                self.meter.measure(s, ch, S::level);
            }
            return s.len();
        }
        let wide24 = S::WIDE && self.out_width == OutWidth::Bits24;
        let stereo = ch == 2;
        let frames = s.len() / ch;
        let (mut rd, mut wr) = (0, 0);
        while rd < frames {
            let n_in = (frames - rd).min(CHUNK);
            let src = &s[rd * ch..(rd + n_in) * ch];
            let work = &mut self.work[..n_in * ch];
            for (w, &x) in work.iter_mut().zip(src) {
                *w = x.load();
            }
            let n = if on(StageId::Decimator) { self.decim.process(work, ch) } else { n_in };
            let work = &mut self.work[..n * ch];
            if on(StageId::Gain) {
                self.gain.process(work, ch);
            }
            if on(StageId::Eq) {
                self.eq.process(work, ch);
            }
            if on(StageId::Tone) {
                self.tone.process(work, ch);
            }
            if stereo && on(StageId::Crossfeed) {
                self.xfeed.process(work);
            }
            if stereo && on(StageId::Matrix) {
                self.matrix.process(work);
            }
            if on(StageId::Limiter) {
                self.limiter.process(work, ch);
            }
            if on(StageId::Fade) {
                self.fade.process(work, ch);
            }
            let dst = &mut s[wr * ch..(wr + n) * ch];
            if wide24 {
                for (d, &x) in dst.iter_mut().zip(work.iter()) {
                    *d = S::from24(x);
                }
            } else {
                self.dither.quantize(work, ch);
                for (d, &q) in dst.iter_mut().zip(work.iter()) {
                    *d = S::from16(q);
                }
            }
            if self.meter.is_active() {
                self.meter.measure(dst, ch, S::level);
            }
            rd += n_in;
            wr += n;
        }
        wr * ch
    }
}

/// Bytes of RAM a chain occupies (all state is inside the struct).
#[must_use]
pub const fn ram_bytes() -> usize {
    core::mem::size_of::<Chain>()
}
