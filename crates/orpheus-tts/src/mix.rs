//! Speech in the audio output (TTS-3, 04 §8 "Output"): the speech voice
//! next to the music, after the DSP chain.
//!
//! Speech is rendered at the synthesiser's own 16 kHz ([`RATE`]) and comes
//! to the output through a [`Source`] (the firmware: a ring between the
//! speech task in thread mode and the audio task). [`Mixer`] puts it into
//! the output:
//!
//! * **Over music** ([`Mixer::mix_i16`], [`Mixer::mix_i32`]): resampled to
//!   the DAC's rate ([`Resampler`]), scaled by the speech volume
//!   ([`SPEECH_DB`], Settings › Accessibility › Speech volume) and added to
//!   the music, which is lowered while speech sounds when Music while
//!   speaking is [`MusicMode::Lower`] ([`DUCK_DB`]: down over
//!   [`ATTACK_MS`], held [`HOLD_MS`] after the last speech, back up over
//!   [`RELEASE_MS`]). [`MusicMode::Pause`] is the player's (the UI pauses
//!   the music while speech sounds); [`MusicMode::Keep`] leaves it.
//! * **Alone** ([`Mixer::alone`]): nothing else plays, the DAC runs at
//!   16 kHz and the samples go out as they are (scaled).
//! * **A stop** ([`Source::stopped`]): what is left of the stopped speech
//!   fades out over [`STOP_FADE`] input samples (3 ms) and is then dropped
//!   ([`Source::drop_stopped`]); the speech that follows is not touched.
//!
//! Integer gains (Q15 speech, Q16 music) and the resampler's f32: the same
//! samples on the host and the player.

use crate::resample::Resampler;

/// The rate speech is rendered and queued at.
pub const RATE: u32 = 16_000;

/// Settings › Accessibility › Speech volume: the speech's gain in dB for
/// each choice (Normal, −6, −12, −18 dB). Applied alone too, so the
/// setting means one thing.
pub const SPEECH_DB: [i8; 4] = [0, -6, -12, -18];

/// How far [`MusicMode::Lower`] lowers the music (dB).
pub const DUCK_DB: i8 = -12;
/// The music goes down over this when speech starts.
pub const ATTACK_MS: u32 = 20;
/// It stays down this long after the last speech (the reader's
/// utterances follow each other with short gaps).
pub const HOLD_MS: u32 = 300;
/// Then it comes back up over this.
pub const RELEASE_MS: u32 = 300;
/// A stopped utterance fades over this many 16 kHz samples (3 ms).
pub const STOP_FADE: u16 = 48;

/// Unity in the speech gain's Q15 (exact: Normal leaves the samples as
/// they are).
pub const Q15_ONE: u32 = 32_768;
/// Unity in the music gain's Q16.
pub const Q16_ONE: u32 = 65_536;

/// Settings › Accessibility › Music while speaking.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MusicMode {
    /// Lowered by [`DUCK_DB`] while speech sounds (the default).
    #[default]
    Lower,
    /// Paused while speech sounds (by the player; nothing to mix).
    Pause,
    /// Left as it is.
    Keep,
}

impl MusicMode {
    /// The setting's index (0 Lower, 1 Pause, 2 Keep).
    #[must_use]
    pub const fn from_index(v: u8) -> MusicMode {
        match v {
            1 => MusicMode::Pause,
            2 => MusicMode::Keep,
            _ => MusicMode::Lower,
        }
    }

    /// The music's gain while speech sounds (Q16).
    #[must_use]
    pub const fn duck_q16(self) -> u32 {
        match self {
            MusicMode::Lower => q16_of_db(DUCK_DB),
            MusicMode::Pause | MusicMode::Keep => Q16_ONE,
        }
    }
}

/// 10^(db/20) in Q16 for the levels used here (0, −6, −12, −18, −24 dB);
/// other values take the nearest lower 6 dB step.
#[must_use]
pub const fn q16_of_db(db: i8) -> u32 {
    match db {
        d if d >= 0 => Q16_ONE,
        -6..=-1 => 32_845,
        -12..=-7 => 16_462,
        -18..=-13 => 8_250,
        _ => 4_135,
    }
}

/// The speech gain of Speech volume choice `i` (Q15).
#[must_use]
#[allow(clippy::cast_lossless)]
pub const fn speech_q15(i: u8) -> u32 {
    let db = if (i as usize) < SPEECH_DB.len() { SPEECH_DB[i as usize] } else { 0 };
    q16_of_db(db) / 2
}

/// `x` scaled by the speech gain `g` (Q15; [`Q15_ONE`] leaves it).
#[must_use]
#[allow(clippy::cast_lossless)]
pub const fn scale(x: i16, g: u32) -> i16 {
    let g = if g > Q15_ONE { Q15_ONE } else { g };
    let v = (x as i32 * g as i32) >> 15;
    if v > 32_767 {
        32_767
    } else if v < -32_768 {
        -32_768
    } else {
        v as i16
    }
}

/// Where the speech samples come from (16 kHz, mono).
pub trait Source {
    /// The next sample, if one is ready.
    fn next(&mut self) -> Option<i16>;
    /// Speech is in progress: more will come even if none is ready now.
    fn sounding(&self) -> bool;
    /// A stop came since the last call (the fade starts).
    fn stopped(&mut self) -> bool;
    /// Drop what is left of the stopped speech (its fade is over).
    fn drop_stopped(&mut self);
}

/// The gains of one block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Gains {
    /// The speech's gain (Q15, [`speech_q15`]).
    pub speech: u32,
    /// The music's gain while speech sounds (Q16, [`MusicMode::duck_q16`]).
    pub duck: u32,
}

impl Gains {
    /// Speech at full level, the music not lowered.
    pub const UNITY: Gains = Gains { speech: Q15_ONE, duck: Q16_ONE };

    /// From the two settings' indices.
    #[must_use]
    pub const fn of(volume: u8, music: MusicMode) -> Gains {
        Gains { speech: speech_q15(volume), duck: music.duck_q16() }
    }
}

/// The mixer's state between blocks (see the module documentation).
#[derive(Clone, Copy, Debug)]
pub struct Mixer {
    rs: Resampler,
    /// The music's gain now (Q16).
    music: u32,
    /// Output frames left at the lowered level after the last speech.
    hold: u32,
    /// Input samples of the stop fade left (0: none).
    fade: u16,
    /// Output frames still to run through the resampler after the speech
    /// ended (its latency).
    tail: u32,
}

impl Mixer {
    /// A mixer for output at `rate` (16 kHz alone, else the DAC's).
    #[must_use]
    pub fn new(rate: u32) -> Mixer {
        Mixer { rs: Resampler::new(rate), music: Q16_ONE, hold: 0, fade: 0, tail: 0 }
    }

    /// The output rate.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rs.rate()
    }

    /// Output at another rate from now on (the resampler starts afresh).
    pub fn set_rate(&mut self, rate: u32) {
        if rate != self.rs.rate() {
            self.rs = Resampler::new(rate);
        }
    }

    /// The music's gain now (Q16; [`Q16_ONE`]: not lowered).
    #[must_use]
    pub fn music_gain(&self) -> u32 {
        self.music
    }

    /// Mixing still changes the output: speech sounds, its resampler
    /// rings out, or the music is not back at its level.
    #[must_use]
    pub fn busy(&self, src: &dyn Source) -> bool {
        src.sounding() || self.tail > 0 || self.music != Q16_ONE || self.hold > 0
    }

    /// One input sample from `src` (the stop fade applied; 0 when none is
    /// ready).
    fn input(&mut self, src: &mut dyn Source) -> Option<i32> {
        if src.stopped() {
            self.fade = STOP_FADE;
        }
        let x = src.next();
        if self.fade == 0 {
            return x.map(i32::from);
        }
        let k = i32::from(self.fade);
        self.fade -= 1;
        if self.fade == 0 {
            src.drop_stopped();
        }
        x.map(|v| i32::from(v) * k / i32::from(STOP_FADE))
    }

    /// The next speech sample at the output rate (16-bit scale, before the
    /// speech gain).
    fn speech_frame(&mut self, src: &mut dyn Source) -> f32 {
        while self.rs.needs_input() {
            let x = self.input(src);
            if x.is_some() {
                // 16 taps of the resampler (≈ 1 ms at 16 kHz) after the last.
                self.tail = self.rs.rate() / 1000 + 1;
            }
            self.rs.push(x.unwrap_or(0) as f32);
        }
        let y = self.rs.pull();
        self.tail = self.tail.saturating_sub(1);
        y
    }

    /// The music's gain for the next frame (`speaking`: speech in
    /// progress at the block's start).
    fn duck_step(&mut self, speaking: bool, g: Gains) -> u32 {
        let rate = self.rs.rate();
        if speaking {
            self.hold = rate * HOLD_MS / 1000;
            if self.music > g.duck {
                let step = (Q16_ONE - g.duck).div_ceil((rate * ATTACK_MS / 1000).max(1));
                self.music = self.music.saturating_sub(step).max(g.duck);
            } else {
                self.music = g.duck.min(self.music.max(g.duck));
            }
        } else if self.hold > 0 {
            self.hold -= 1;
        } else if self.music < Q16_ONE {
            let step = (Q16_ONE - g.duck.min(Q16_ONE)).div_ceil((rate * RELEASE_MS / 1000).max(1)).max(1);
            self.music = (self.music + step).min(Q16_ONE);
        }
        self.music
    }

    /// Mix speech into `buf` (interleaved 16-bit music, `ch` samples a
    /// frame) at the mixer's rate.
    pub fn mix_i16(&mut self, buf: &mut [i16], ch: usize, g: Gains, src: &mut dyn Source) {
        let ch = ch.max(1);
        let speaking = src.sounding();
        for f in buf.chunks_exact_mut(ch) {
            let m = self.duck_step(speaking, g);
            let s = self.speech_frame(src) * g.speech as f32 * (1.0 / 32_768.0);
            let s = s as i32;
            for x in f {
                let v = ((i32::from(*x) * m as i32) >> 16) + s;
                *x = v.clamp(-32_768, 32_767) as i16;
            }
        }
    }

    /// [`Mixer::mix_i16`] for left-justified 32-bit music (24 bits in the
    /// top bits, as the DSP chain's `i32` blocks).
    pub fn mix_i32(&mut self, buf: &mut [i32], ch: usize, g: Gains, src: &mut dyn Source) {
        let ch = ch.max(1);
        let speaking = src.sounding();
        for f in buf.chunks_exact_mut(ch) {
            let m = self.duck_step(speaking, g);
            let s = self.speech_frame(src) * g.speech as f32 * (1.0 / 32_768.0);
            let s = (s as i32).clamp(-32_768, 32_767) << 16;
            for x in f {
                let v = ((i64::from(*x) * i64::from(m)) >> 16) as i32;
                *x = v.saturating_add(s);
            }
        }
    }

    /// Speech alone at 16 kHz: the next output sample (`None`: none is
    /// ready; the caller pads with silence while [`Source::sounding`]).
    pub fn alone(&mut self, g: Gains, src: &mut dyn Source) -> Option<i16> {
        let x = self.input(src)?;
        Some(scale(x.clamp(-32_768, 32_767) as i16, g.speech))
    }

    /// The speech ended or the output changed: forget the resampler's
    /// history and put the music back at its level.
    pub fn reset(&mut self) {
        self.rs.reset();
        self.music = Q16_ONE;
        self.hold = 0;
        self.fade = 0;
        self.tail = 0;
    }
}

/// An earcon's sample `k` of `total` at `rate`: a sine glide from `hz.0`
/// to `hz.1` at `level` / 255 of full scale with 4 ms fades (the
/// simulator's `tone_sample` in f32; [`crate::speak`]'s sine).
#[must_use]
pub fn glide(hz: (u16, u16), level: u8, k: u32, total: u32, rate: u32) -> i16 {
    let (f0, f1) = (f32::from(hz.0), f32::from(hz.1));
    let r = rate.max(1) as f32;
    let secs = k as f32 / r;
    let dur = (total as f32 / r).max(1e-6);
    // Phase in turns of a linear glide: f0·t + (f1 − f0)·t²/(2T), wrapped
    // per term so f32 keeps its fraction.
    let a = f0 * secs;
    let b = (f1 - f0) * secs * secs / (2.0 * dur);
    let phase = (a - (a as u32) as f32) + (b - (b as i32) as f32);
    let phase = phase - (phase as i32) as f32;
    let phase = if phase < 0.0 { phase + 1.0 } else { phase };
    let fade = r * 0.004;
    let env = (k as f32 / fade).min(total.saturating_sub(k) as f32 / fade).min(1.0);
    let s = orpheus_synth::math::sin_turns(phase);
    let v = s * env * f32::from(level) / 255.0 * 32_767.0;
    v.clamp(-32_768.0, 32_767.0) as i16
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    /// Speech samples from a list; a stop after `stop_at` samples, with
    /// `after` new samples queued behind the stopped ones.
    struct List {
        s: Vec<i16>,
        at: usize,
        sounding: bool,
        stop: bool,
        drop_to: Option<usize>,
        drops: u32,
    }

    impl List {
        fn new(s: Vec<i16>) -> List {
            List { s, at: 0, sounding: true, stop: false, drop_to: None, drops: 0 }
        }
    }

    impl Source for List {
        fn next(&mut self) -> Option<i16> {
            let v = self.s.get(self.at).copied();
            if v.is_some() {
                self.at += 1;
            }
            v
        }
        fn sounding(&self) -> bool {
            self.sounding && self.at < self.s.len()
        }
        fn stopped(&mut self) -> bool {
            core::mem::take(&mut self.stop)
        }
        fn drop_stopped(&mut self) {
            self.drops += 1;
            if let Some(d) = self.drop_to {
                self.at = self.at.max(d);
            }
        }
    }

    fn tone(n: usize, hz: f32, amp: f32) -> Vec<i16> {
        (0..n)
            .map(|k| {
                let p = (k as f32 * hz / 16_000.0).fract();
                (orpheus_synth::math::sin_turns(p) * amp) as i16
            })
            .collect()
    }

    #[test]
    fn gains() {
        assert_eq!(speech_q15(0), Q15_ONE);
        assert_eq!(speech_q15(1), 16_422);
        assert_eq!(speech_q15(9), Q15_ONE, "out of range: Normal");
        assert_eq!(MusicMode::Keep.duck_q16(), Q16_ONE);
        assert_eq!(MusicMode::Pause.duck_q16(), Q16_ONE);
        assert_eq!(MusicMode::Lower.duck_q16(), 16_462);
        assert_eq!(MusicMode::from_index(1), MusicMode::Pause);
        assert_eq!(MusicMode::from_index(7), MusicMode::Lower);
        // 10^(-12/20) = 0.2512.
        assert!((f64::from(q16_of_db(-12)) / 65_536.0 - 0.2512).abs() < 0.001);
        assert!((f64::from(q16_of_db(-6)) / 65_536.0 - 0.5012).abs() < 0.001);
    }

    #[test]
    fn alone_passes_samples_with_the_gain() {
        let mut m = Mixer::new(RATE);
        let mut src = List::new(vec![1000, -2000, 32_000]);
        let g = Gains::of(1, MusicMode::Lower);
        let out: Vec<i16> = (0..4).filter_map(|_| m.alone(g, &mut src)).collect();
        assert_eq!(out, vec![501, -1003, 16_037]);
        let mut src = List::new(vec![1000, -2000, 32_767, -32_768]);
        let out: Vec<i16> = (0..4).filter_map(|_| m.alone(Gains::UNITY, &mut src)).collect();
        assert_eq!(out, vec![1000, -2000, 32_767, -32_768], "Normal: exactly the samples");
        assert_eq!(m.alone(Gains::UNITY, &mut src), None);
    }

    #[test]
    fn mixed_at_48k_keeps_the_level_and_length() {
        let mut m = Mixer::new(48_000);
        let speech = tone(1600, 1000.0, 10_000.0);
        let mut src = List::new(speech);
        let mut buf = vec![0i16; 2 * 5000];
        m.mix_i16(&mut buf, 2, Gains::UNITY, &mut src);
        // Both channels equal; the 1 kHz tone's peak ≈ 10,000 (±3 %).
        assert!(buf.chunks_exact(2).all(|f| f[0] == f[1]));
        let peak = buf.iter().map(|x| x.unsigned_abs()).max().unwrap();
        assert!((9_700..=10_300).contains(&peak), "{peak}");
        // 100 ms of 16 kHz speech is 100 ms at 48 kHz (+ the resampler's
        // 0.5 ms latency).
        let last = buf.chunks_exact(2).rposition(|f| f[0].unsigned_abs() > 300).unwrap();
        assert!((4790..=4830).contains(&last), "{last}");
    }

    #[test]
    fn music_is_lowered_held_and_released() {
        let rate = 48_000;
        let mut m = Mixer::new(rate);
        let g = Gains::of(0, MusicMode::Lower);
        let mut src = List::new(vec![0; 1600]);
        let mut block = vec![10_000i16; 2 * 2400];
        // Speaking: after the attack (20 ms = 960 frames) the music is at
        // −12 dB.
        m.mix_i16(&mut block, 2, g, &mut src);
        assert!(block[2 * 900] > block[2 * 2000], "going down");
        assert_eq!(block[2 * 2399], 2_511);
        // Speech over (its 1,600 samples were 4,800 frames): held 300 ms,
        // then back up within 300 ms.
        let mut src = List::new(vec![]);
        let mut held = vec![10_000i16; 2 * (rate as usize * 3 / 10 - 2400)];
        m.mix_i16(&mut held, 2, g, &mut src);
        assert!(held.iter().all(|&x| x == 2_511), "held");
        let mut up = vec![10_000i16; 2 * (rate as usize * 3 / 10 + 2600)];
        m.mix_i16(&mut up, 2, g, &mut src);
        assert!(up[0] < 3_000 && up[up.len() - 1] == 10_000, "{} {}", up[0], up[up.len() - 1]);
        assert!(!m.busy(&src));
    }

    #[test]
    fn keep_and_pause_leave_the_music() {
        for mode in [MusicMode::Keep, MusicMode::Pause] {
            let mut m = Mixer::new(44_100);
            let mut src = List::new(vec![0; 800]);
            let mut buf = vec![-7_000i16; 2 * 2205];
            m.mix_i16(&mut buf, 2, Gains::of(0, mode), &mut src);
            assert!(buf.iter().all(|&x| x == -7_000), "{mode:?}");
        }
    }

    #[test]
    fn i32_blocks_mix_like_i16() {
        let speech = tone(400, 700.0, 8_000.0);
        let (mut a, mut b) = (Mixer::new(44_100), Mixer::new(44_100));
        let (mut sa, mut sb) = (List::new(speech.clone()), List::new(speech));
        let mut x16 = vec![3_000i16; 2 * 1100];
        let mut x32: Vec<i32> = x16.iter().map(|&v| i32::from(v) << 16).collect();
        let g = Gains::of(2, MusicMode::Lower);
        a.mix_i16(&mut x16, 2, g, &mut sa);
        b.mix_i32(&mut x32, 2, g, &mut sb);
        for (p, q) in x16.iter().zip(&x32) {
            assert!((i32::from(*p) - (q >> 16)).abs() <= 1, "{p} {}", q >> 16);
        }
    }

    #[test]
    fn a_stop_fades_the_old_speech_then_drops_it() {
        let mut m = Mixer::new(RATE);
        // 200 samples of the old utterance, a new one from 1,000 on.
        let mut s = vec![20_000i16; 1000];
        s.extend(vec![-5_000i16; 100]);
        let mut src = List::new(s);
        src.drop_to = Some(1000);
        let g = Gains::UNITY;
        for _ in 0..100 {
            assert_eq!(m.alone(g, &mut src), Some(20_000));
        }
        src.stop = true;
        let faded: Vec<i16> = (0..STOP_FADE).filter_map(|_| m.alone(g, &mut src)).collect();
        assert_eq!(faded.len(), usize::from(STOP_FADE));
        assert!(faded.windows(2).all(|w| w[1] < w[0]), "falling");
        assert!(faded[0] > 19_000 && faded[faded.len() - 1] < 500, "{faded:?}");
        assert_eq!(src.drops, 1);
        // The new utterance follows untouched.
        assert_eq!(m.alone(g, &mut src), Some(-5_000));
    }

    #[test]
    fn loud_speech_over_loud_music_saturates() {
        let mut m = Mixer::new(RATE);
        let mut src = List::new(vec![32_767; 10]);
        let mut buf = vec![32_000i16; 10];
        m.mix_i16(&mut buf, 1, Gains { speech: Q15_ONE, duck: Q16_ONE }, &mut src);
        assert!(buf.iter().all(|&x| x == 32_767));
        let mut src = List::new(vec![-32_768; 10]);
        let mut buf = vec![i32::MIN + 5; 10];
        m.mix_i32(&mut buf, 1, Gains::UNITY, &mut src);
        assert!(buf.iter().all(|&x| x == i32::MIN));
    }

    #[test]
    fn glide_matches_a_reference_within_a_step() {
        // The simulator's f64 earcon (sim/src/speech.rs `tone_sample`).
        let reference = |k: u32, total: u32| {
            let (f0, f1) = (880.0f64, 1320.0f64);
            let secs = f64::from(k) / 16_000.0;
            let dur = f64::from(total) / 16_000.0;
            let phase = f0 * secs + (f1 - f0) * secs * secs / (2.0 * dur);
            let env = (f64::from(k) / 64.0).min(f64::from(total - k) / 64.0).min(1.0);
            let s = (phase.fract() * core::f64::consts::TAU).sin();
            (s * env * 70.0 / 255.0 * 32_767.0).round()
        };
        let total = 640;
        for k in 0..total {
            let a = f64::from(glide((880, 1320), 70, k, total, 16_000));
            assert!((a - reference(k, total)).abs() < 40.0, "{k}: {a} {}", reference(k, total));
        }
    }
}
