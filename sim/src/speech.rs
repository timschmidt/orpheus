//! The simulator's speech service (A11Y-1): the screen reader's
//! [`SpeechService`] rendered through `orpheus-tts` in simulated time.
//!
//! Utterances wait in the toolkit's [`UtteranceQueue`] (the fixed queue the
//! firmware's speech voice will use) and are spoken one after the other at
//! 16 kHz mono (04 §8: speech alone may run the DAC at 16 kHz); earcons are
//! rendered from their [`Tone`]. [`SimSpeech::advance`] renders as much
//! audio as simulated time passed, so a key that stops speech (`Stop::Now`)
//! cuts it where a listener would hear it cut. Everything the reader asked
//! for is logged with its time ([`SimSpeech::log`]); the audio timeline
//! (silence included) is kept for hashing ([`SimSpeech::audio_hash`]) and
//! WAV export ([`SimSpeech::wav`]).

use orpheus_tts::{Config as TtsConfig, Options, Speaker, Speed, Stop, Voice};
use orpheus_ui::a11y::{Config, Earcon, SpeechService, Tone, Utterance, UtteranceQueue};
use orpheus_ui::text::StrBuf;

/// The speech output rate.
pub const RATE: u32 = 16_000;

/// One thing the reader did, at a simulated time (ms).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    Text(u64, String),
    Earcon(u64, Earcon),
    /// Speech was silenced (only logged when something was sounding or
    /// queued).
    Stop(u64),
}

impl Said {
    /// The golden-file line: `   1234 say …`, `   1234 earcon enter`,
    /// `   1234 stop`.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Said::Text(t, s) => format!("{t:7} say {s}"),
            Said::Earcon(t, e) => format!("{t:7} earcon {}", e.name()),
            Said::Stop(t) => format!("{t:7} stop"),
        }
    }
}

enum Playing {
    Idle,
    Speech,
    Tone { tone: Tone, at: u32 },
}

/// See the module documentation.
pub struct SimSpeech {
    queue: UtteranceQueue<1024>,
    speaker: Speaker,
    playing: Playing,
    /// The settings the next utterance starts with.
    cfg: Config,
    applied: Option<(u8, u8, i8)>,
    now_ms: u64,
    /// Fractional frames carried between advances.
    frames_due: u64,
    pub log: Vec<Said>,
    /// The whole output since the start (16 kHz mono).
    pub audio: Vec<i16>,
    /// Keep the audio (off: render, count and hash only).
    pub keep_audio: bool,
    hash: u64,
    rendered: u64,
}

impl Default for SimSpeech {
    fn default() -> Self {
        Self::new()
    }
}

fn voice(cfg: &Config) -> Voice {
    let v = Voice::ALL.get(usize::from(cfg.voice)).copied().unwrap_or(Voice::NEUTRAL);
    v.with_pitch(f32::from(cfg.pitch))
}

impl SimSpeech {
    #[must_use]
    pub fn new() -> SimSpeech {
        let cfg = Config::DEFAULT;
        let tts = TtsConfig {
            rate: RATE,
            channels: 1,
            voice: voice(&cfg),
            speed: Speed::from_quarters(cfg.rate),
            seed: 0x5EED,
        };
        SimSpeech {
            queue: UtteranceQueue::new(),
            speaker: Speaker::new(tts, Options::default()),
            playing: Playing::Idle,
            cfg,
            applied: None,
            now_ms: 0,
            frames_due: 0,
            log: Vec::new(),
            audio: Vec::new(),
            keep_audio: true,
            hash: 0xcbf2_9ce4_8422_2325,
            rendered: 0,
        }
    }

    /// Something sounds or waits.
    #[must_use]
    pub fn busy(&self) -> bool {
        !matches!(self.playing, Playing::Idle) || !self.queue.is_empty()
    }

    /// The texts said, in order (the log without times, earcons and stops).
    #[must_use]
    pub fn texts(&self) -> Vec<&str> {
        self.log
            .iter()
            .filter_map(|s| match s {
                Said::Text(_, t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The last text said.
    #[must_use]
    pub fn last(&self) -> Option<&str> {
        self.texts().last().copied()
    }

    /// FNV-1a (64-bit) of every sample rendered so far.
    #[must_use]
    pub fn audio_hash(&self) -> u64 {
        self.hash
    }

    /// Samples rendered so far (16 kHz).
    #[must_use]
    pub fn rendered(&self) -> u64 {
        self.rendered
    }

    fn put(&mut self, s: i16) {
        for b in s.to_le_bytes() {
            self.hash = (self.hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        self.rendered += 1;
        if self.keep_audio {
            self.audio.push(s);
        }
    }

    fn apply(&mut self) {
        let want = (self.cfg.rate, self.cfg.voice, self.cfg.pitch);
        if self.applied != Some(want) {
            self.speaker.set_speed(Speed::from_quarters(self.cfg.rate.clamp(1, 40)));
            self.speaker.set_voice(voice(&self.cfg));
            self.applied = Some(want);
        }
    }

    /// Start the next queued utterance, if any.
    fn next(&mut self) {
        let mut text = StrBuf::<255>::new();
        match self.queue.pop(&mut text) {
            Some(Utterance::Text) => {
                self.apply();
                let mut rest = text.as_str().as_bytes();
                while !rest.is_empty() {
                    let n = self.speaker.feed(rest);
                    if n == 0 {
                        break;
                    }
                    rest = &rest[n..];
                }
                self.speaker.finish();
                self.playing = Playing::Speech;
            }
            Some(Utterance::Earcon(e)) => self.playing = Playing::Tone { tone: e.tone(), at: 0 },
            None => self.playing = Playing::Idle,
        }
    }

    /// Render `frames` samples.
    fn render(&mut self, mut frames: usize) {
        let mut block = [0i16; 256];
        while frames > 0 {
            if matches!(self.playing, Playing::Idle) {
                self.next();
            }
            match self.playing {
                Playing::Idle => {
                    for _ in 0..frames {
                        self.put(0);
                    }
                    return;
                }
                Playing::Speech => {
                    let n = frames.min(block.len());
                    let got = self.speaker.render(&mut block[..n], |_, _| {});
                    for &s in &block[..got] {
                        self.put(s);
                    }
                    frames -= got;
                    if got < n || self.speaker.is_idle() {
                        self.playing = Playing::Idle;
                    }
                }
                Playing::Tone { tone, at } => {
                    let total = u32::from(tone.ms) * RATE / 1000;
                    let n = u32::try_from(frames).unwrap_or(u32::MAX).min(total - at);
                    for k in at..at + n {
                        let s = tone_sample(&tone, k, total);
                        self.put(s);
                    }
                    frames -= n as usize;
                    self.playing =
                        if at + n >= total { Playing::Idle } else { Playing::Tone { tone, at: at + n } };
                }
            }
        }
    }

    /// Let `ms` of simulated time pass: render that much output (nothing
    /// before the first utterance).
    pub fn advance(&mut self, ms: u64) {
        self.now_ms += ms;
        if self.log.is_empty() {
            return;
        }
        let due = ms * u64::from(RATE) + self.frames_due;
        self.frames_due = due % 1000;
        self.render(usize::try_from(due / 1000).unwrap_or(0));
    }

    /// Render until everything queued was said (at most `max_ms`).
    pub fn drain(&mut self, max_ms: u64) {
        let mut t = 0;
        while self.busy() && t < max_ms {
            self.advance(10);
            t += 10;
        }
    }

    /// The audio as a WAV file (16 kHz, mono, 16 bit).
    #[must_use]
    pub fn wav(&self) -> Vec<u8> {
        wav_bytes(&self.audio)
    }
}

/// A 16 kHz mono 16-bit WAV of `pcm`.
#[must_use]
pub fn wav_bytes(pcm: &[i16]) -> Vec<u8> {
    let data = u32::try_from(pcm.len() * 2).unwrap_or(u32::MAX);
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

/// Sample `k` of `total` of an earcon: a sine glide with 4 ms fades
/// (`orpheus_synth::math::sin_turns`: no libm, the same samples on every
/// host).
fn tone_sample(t: &Tone, k: u32, total: u32) -> i16 {
    let (f0, f1) = (f64::from(t.hz[0]), f64::from(t.hz[1]));
    // Phase of a linear glide in turns: f0·t + (f1 − f0)·t²/(2T).
    let secs = f64::from(k) / f64::from(RATE);
    let dur = (f64::from(total) / f64::from(RATE)).max(1e-6);
    let phase = f0 * secs + (f1 - f0) * secs * secs / (2.0 * dur);
    let fade = 64.0;
    let env = (f64::from(k) / fade).min(f64::from(total - k) / fade).min(1.0);
    #[allow(clippy::cast_possible_truncation)]
    let s = orpheus_synth::math::sin_turns(phase.fract() as f32);
    let v = f64::from(s) * env * f64::from(t.level) / 255.0 * 32767.0;
    #[allow(clippy::cast_possible_truncation)]
    {
        v.round().clamp(-32768.0, 32767.0) as i16
    }
}

impl SpeechService for SimSpeech {
    fn stop(&mut self) {
        if self.busy() {
            self.log.push(Said::Stop(self.now_ms));
        }
        self.queue.clear();
        match self.playing {
            Playing::Speech => self.speaker.stop(Stop::Now),
            Playing::Tone { .. } => self.playing = Playing::Idle,
            Playing::Idle => {}
        }
    }

    fn say(&mut self, text: &str) {
        self.log.push(Said::Text(self.now_ms, text.to_string()));
        self.queue.push_text(text);
    }

    fn earcon(&mut self, e: Earcon) {
        self.log.push(Said::Earcon(self.now_ms, e));
        self.queue.push_earcon(e);
    }

    fn configure(&mut self, cfg: &Config) {
        self.cfg = *cfg;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaks_in_order_and_stops_now() {
        let mut s = SimSpeech::new();
        s.earcon(Earcon::Enter);
        s.say("Settings. Sound, submenu, 1 of 13");
        s.advance(300);
        assert!(s.busy());
        let loud = s.audio.iter().filter(|x| x.unsigned_abs() > 300).count();
        assert!(loud > 1000, "{loud} loud samples in 300 ms");
        s.stop();
        s.advance(20);
        assert!(!s.busy(), "stopped within 20 ms");
        let tail = &s.audio[s.audio.len() - 160..];
        assert!(tail.iter().all(|x| x.unsigned_abs() < 50), "silent after the stop");
        assert!(matches!(s.log.last(), Some(Said::Stop(300))));
    }

    #[test]
    fn faster_speech_is_shorter() {
        let len = |rate: u8| {
            let mut s = SimSpeech::new();
            s.configure(&Config { rate, ..Config::DEFAULT });
            s.say("Albums, submenu, 3 of 12");
            s.drain(20_000);
            s.audio.iter().rposition(|x| x.unsigned_abs() > 100).unwrap_or(0)
        };
        let (slow, normal, fast) = (len(2), len(4), len(16));
        assert!(slow > normal && normal > fast, "{slow} {normal} {fast}");
    }
}
