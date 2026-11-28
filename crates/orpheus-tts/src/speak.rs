//! The streaming speech renderer: [`Token`]s in, audio blocks out.
//!
//! ```text
//! push(Token) ─► unit queue (QUEUE phones/pauses; the word being received
//!                is "pending" until its end is known: the contour of a
//!                following break goes on its last stressed syllable)
//!             ─► per 1 ms tick: prev/cur/next unit → targets, loci and
//!                transitions, source amplitudes, F0 (declination, accents,
//!                contours) → klatt::Frame
//!             ─► klatt::Dsp (16 kHz) ─► Resampler (output rate) ─► i16
//! ```
//!
//! * **Durations** are TTS-1's, exactly: a phone of `ms` milliseconds lasts
//!   `ms` ticks of 16 samples; an utterance of Σms lasts
//!   [`resample::frames_for`]`(16·Σms, rate)` output frames. Inside
//!   brackets (asides) units are 15 % shorter (rounded to whole ms).
//!   Speed changes nothing in the synthesiser except the pitch depth and
//!   the burst/aspiration timings, so the pitch stays put at any speed.
//! * **Look-ahead**: a unit renders once it is final (its word's pitch is
//!   assigned) and its successor is known (or [`Synth::finish`] was
//!   called). A word is final when the next word or break arrives.
//! * **Prosody** ([`Synth::push`]): an F0 baseline that declines through
//!   the sentence (reset after a sentence-ending break, half reset after a
//!   phrase break); an accent on the primary-stressed vowel of each
//!   content word (function words are found by [`is_function_word`] when
//!   the word's text is given, the first accent of a phrase is higher);
//!   the break's contour (fall, wh-question fall, rise, continue, exclaim)
//!   on the last stressed syllable of the phrase-final word and the rest of
//!   that word; asides lower, narrower and faster; every movement scaled
//!   by the voice's range and [`duration::pitch_depth_pct`].
//! * **Events** ([`Event`]) are reported with the output frame (in the
//!   block) where the word starts sounding (±0.5 ms: the resampler's
//!   latency is not counted).
//! * **Stop**: [`Stop::Now`] fades out over 3 ms; [`Stop::AtWord`] lets the
//!   current word finish and fades at the next word or pause.
//! * **Output**: i16 mono or interleaved stereo at 8–48 kHz
//!   ([`Synth::render`]), or `f32` mono ([`Synth::render_f32`]).
//!
//! RAM: one [`Synth`] is [`ram::synth`] bytes (`ram` test; 04 §8 budget
//! 8 KiB with the converter). No allocation.

use orpheus_synth::math::{clampf, sin_turns, soft_clip, to_i16};

use crate::convert::{Converter, Token};
use crate::duration::{self, Speed};
use crate::klatt::{Dsp, FS, Frame, TICK};
use crate::normalise::Options;
use crate::phoneme::{Class, Ph, Stress};
use crate::prosody::{BreakKind, Contour};
use crate::resample::{self, Resampler, db};
use crate::targets::{self, B45, F45, FNP, FNZ_NASAL, Fric, OFF, P56, PB56, Place, Target};
use crate::voice::{Voice, tilt_coef};

/// Units (phones and pauses) the queue holds: more than a word's phones
/// ([`crate::words::MAX_WORD_PHONES`]) plus its break.
pub const QUEUE: usize = 64;

/// What the renderer reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A sentence starts sounding (its first word's event follows).
    Sentence {
        /// The first word's [`crate::Word::pos`].
        pos: u32,
    },
    /// A word starts sounding.
    Word {
        /// [`crate::Word::pos`]: the byte offset of its source chunk.
        pos: u32,
        /// A letter said by name.
        spelled: bool,
    },
    /// Everything pushed was spoken (after [`Synth::finish`]), or a stop
    /// completed.
    End,
}

/// How to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Fade out over 3 ms now.
    Now,
    /// Finish the word being spoken, then fade out.
    AtWord,
}

/// Renderer settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Output rate (Hz, 8000–48000).
    pub rate: u32,
    /// 1 (mono) or 2 (interleaved stereo, both channels equal).
    pub channels: u8,
    /// The voice.
    pub voice: Voice,
    /// The speed (for the pitch depth and the burst timings; the
    /// durations come with the tokens).
    pub speed: Speed,
    /// Noise seed.
    pub seed: u32,
}

impl Default for Config {
    fn default() -> Config {
        Config { rate: 48_000, channels: 2, voice: Voice::NEUTRAL, speed: Speed::NORMAL, seed: 0x5EED }
    }
}

const F_WORD: u8 = 1;
const F_SENT: u8 = 2;
const F_ASIDE: u8 = 4;
const F_RESET: u8 = 8;
const F_PART: u8 = 16;
const F_UNASP: u8 = 32;
const F_SPELLED: u8 = 64;

/// One phone or pause in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Unit {
    /// `None`: a pause.
    ph: Option<Ph>,
    stress: Stress,
    /// Length in ticks (ms).
    ms: u16,
    /// Pitch deviation at the start and the end (‰ of the baseline).
    f0: [i16; 2],
    flags: u8,
    /// Word position (with `F_WORD`).
    pos: u32,
}

const SIL: Unit = Unit { ph: None, stress: Stress::None, ms: 0, f0: [0, 0], flags: 0, pos: 0 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    /// Absolute index of its first unit.
    start: u32,
    function: bool,
    spelled: bool,
    pos: u32,
    /// No phone yet (the next one carries `F_WORD`).
    fresh: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Speaking or waiting for tokens.
    Run,
    /// Fading out after a stop; samples left.
    Fade(u16),
    /// Stop at the next word or pause.
    StopAtWord,
    /// Finished and drained (or stopped): silent until new tokens.
    Idle,
}

/// Samples of the stop fade (3 ms).
const FADE: u16 = 48;

/// Level references (linear) of the three sources, set by measurement:
/// vowels peak near −8 dBFS in the Neutral voice.
const AV_REF: f32 = 0.03;
const AH_REF: f32 = 0.02;
const AF_REF: f32 = 0.11;
/// Aspiration after a voiceless stop (re AH_REF).
const ASP_STOP: f32 = 0.7;
/// Output scale.
const MASTER: f32 = 1.0;

/// The streaming renderer (see the module documentation).
pub struct Synth {
    rate: u32,
    channels: u8,
    voice: Voice,
    speed: Speed,
    seed: u32,
    tilt: f32,
    tilt_dark: f32,
    breath: f32,
    dsp: Dsp,
    rs: Resampler,
    q: [Unit; QUEUE],
    /// Absolute indices: the current unit, the end of the final units, the
    /// end of the queue.
    head: u32,
    fin: u32,
    tail: u32,
    pending: Option<Pending>,
    /// Tick within the current unit.
    tick: u16,
    prev: Unit,
    buf: [f32; TICK],
    buf_pos: usize,
    finished: bool,
    draining: bool,
    state: State,
    /// 16 kHz samples and output frames of this utterance.
    in_count: u64,
    out_count: u64,
    // Prosody (enqueue side).
    aside: bool,
    sentence_start: bool,
    phrase_accents: u8,
    // Prosody (render side).
    clock: f32,
    dev: f32,
    flutter: [f32; 3],
    /// F0 of the last tick if it was voiced.
    pitch: Option<f32>,
    /// The last sounding frame's filters (a pause holds them, so the
    /// resonators ring out without a coefficient jump).
    hold: Frame,
}

impl core::fmt::Debug for Synth {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Synth").field("rate", &self.rate).field("queued", &(self.tail - self.head)).finish()
    }
}

/// Function words (unaccented): articles, pronouns, auxiliaries,
/// prepositions, conjunctions.
const FUNCTION: [&[u8]; 66] = [
    b"a", b"an", b"the", b"of", b"to", b"in", b"on", b"at", b"by", b"for", b"with", b"from", b"and", b"or",
    b"but", b"as", b"is", b"are", b"was", b"were", b"be", b"been", b"am", b"it", b"its", b"he", b"she",
    b"they", b"we", b"you", b"i", b"me", b"him", b"her", b"them", b"us", b"my", b"your", b"his", b"our",
    b"their", b"that", b"than", b"then", b"so", b"if", b"do", b"does", b"did", b"have", b"has", b"had",
    b"will", b"would", b"can", b"could", b"shall", b"should", b"may", b"might", b"must", b"into", b"onto",
    b"there", b"this", b"these",
];

/// Whether `word` (normalised text, any case) is a function word, which
/// gets no pitch accent.
pub fn is_function_word(word: &[u8]) -> bool {
    if word.is_empty() || word.len() > 6 {
        return false;
    }
    let mut low = [0u8; 6];
    for (d, s) in low.iter_mut().zip(word) {
        *d = s.to_ascii_lowercase();
    }
    FUNCTION.contains(&&low[..word.len()])
}

/// Smoothstep on [0, 1].
#[inline]
fn smooth(x: f32) -> f32 {
    let x = clampf(x, 0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], x: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * x, a[1] + (b[1] - a[1]) * x, a[2] + (b[2] - a[2]) * x]
}

fn f3(v: [u16; 3]) -> [f32; 3] {
    [f32::from(v[0]), f32::from(v[1]), f32::from(v[2])]
}

fn lin(d: i8) -> f32 {
    if d == OFF { 0.0 } else { db(f32::from(d)) }
}

/// A consonant with a place (loci apply at its boundaries with vocalic
/// segments).
fn placed(t: &Target) -> bool {
    !matches!(t.place, Place::Vocalic | Place::Glottal)
}

/// Transition time inside a segment of class `own` towards a neighbour of
/// class `other` (ms).
fn tau(own: Class, other: Class) -> f32 {
    use Class::*;
    let own_voc = matches!(own, Vowel | Diphthong | Liquid | Glide);
    let other_voc = matches!(other, Vowel | Diphthong | Liquid | Glide);
    match (own_voc, other_voc) {
        (true, false) => match other {
            Stop | Affricate => 45.0,
            Nasal => 35.0,
            Fricative => 40.0,
            _ => 0.0,
        },
        (true, true) => match (own, other) {
            // A liquid or glide reaches and holds its target; the vowel
            // next to it carries the long transition.
            (Liquid | Glide, Liquid | Glide) => 30.0,
            (Liquid | Glide, _) => 22.0,
            (_, Liquid | Glide) => 60.0,
            _ => 50.0,
        },
        (false, _) => match own {
            Fricative => 20.0,
            Nasal => 12.0,
            _ => 0.0,
        },
    }
}

/// The formant and source description of one unit in its context.
#[derive(Clone, Copy, Debug)]
struct Spec {
    t: Target,
    class: Class,
    start: [f32; 3],
    end: [f32; 3],
    bw: [f32; 3],
}

fn spec(u: &Unit, prev: &Unit, next: &Unit) -> Option<Spec> {
    let ph = u.ph?;
    let mut t = targets::target(ph, u.stress);
    let class = ph.class();
    let (mut start, mut end) = (f3(t.f), f3(t.g));
    if class == Class::Aspirate {
        // /h/ is the voiceless version of the neighbouring vowel.
        let pick = |n: &Unit, at_end: bool| {
            n.ph.filter(|p| targets::vocalic(*p)).map(|p| {
                let tt = targets::target(p, n.stress);
                f3(if at_end { tt.g } else { tt.f })
            })
        };
        let f = pick(next, false).or_else(|| pick(prev, true)).unwrap_or(start);
        start = f;
        end = f;
        t.b = [300, 150, 200];
    }
    Some(Spec { t, class, start, end, bw: f3(t.b) })
}

/// The formants (F1–F3) at the boundary between `l` and `r`.
fn boundary(l: &Spec, r: &Spec) -> [f32; 3] {
    let lp = placed(&l.t);
    let rp = placed(&r.t);
    let mut b = [0.0; 3];
    for (k, v) in b.iter_mut().enumerate() {
        *v = if lp && !rp {
            targets::onset(l.t.place, k, r.start[k], r.start[1])
        } else if rp && !lp {
            targets::onset(r.t.place, k, l.end[k], l.end[1])
        } else {
            0.5 * (l.end[k] + r.start[k])
        };
    }
    b
}

/// A unit's own source amplitudes (AV, AH, AF: linear, before the
/// references) outside its special phases.
fn own_src(s: &Spec, prev: &Unit) -> (f32, f32, f32) {
    match s.class {
        Class::Stop => {
            // Closure: a voice bar for a voiced stop after a voiced sound.
            let voiced_ctx = prev.ph.is_some_and(Ph::is_voiced);
            (if voiced_ctx { lin(s.t.av) } else { 0.0 }, 0.0, 0.0)
        }
        Class::Affricate => (0.0, 0.0, 0.0),
        // A liquid or glide after a stop (/pl br kw/): released at the
        // vowel's level, an abrupt onset (a glide alone rises slowly).
        Class::Liquid | Class::Glide if prev.ph.is_some_and(|p| p.class() == Class::Stop) => {
            (lin(-1), 0.0, 0.0)
        }
        _ => (lin(s.t.av), lin(s.t.ah), lin(s.t.af)),
    }
}

impl Synth {
    /// A renderer with `cfg`.
    pub fn new(cfg: Config) -> Synth {
        let voice = cfg.voice.sanitised();
        let mut s = Synth {
            rate: cfg.rate.clamp(8000, 48_000),
            channels: cfg.channels.clamp(1, 2),
            voice,
            speed: cfg.speed,
            seed: cfg.seed,
            tilt: 0.0,
            tilt_dark: 0.0,
            breath: 0.0,
            dsp: Dsp::new(cfg.seed),
            rs: Resampler::new(cfg.rate),
            q: [SIL; QUEUE],
            head: 0,
            fin: 0,
            tail: 0,
            pending: None,
            tick: 0,
            prev: SIL,
            buf: [0.0; TICK],
            buf_pos: TICK,
            finished: false,
            draining: false,
            state: State::Idle,
            in_count: 0,
            out_count: 0,
            aside: false,
            sentence_start: true,
            phrase_accents: 0,
            clock: 0.0,
            dev: 0.0,
            flutter: [0.0, 0.31, 0.67],
            pitch: None,
            hold: Frame::SILENT,
        };
        s.set_voice(voice);
        s
    }

    /// Changes the voice (takes effect at once).
    pub fn set_voice(&mut self, v: Voice) {
        self.voice = v.sanitised();
        self.tilt = tilt_coef(self.voice.tilt_db);
        self.tilt_dark = tilt_coef(self.voice.tilt_db + 14.0);
        self.breath = db(self.voice.breath_db);
    }

    /// The voice.
    pub fn voice(&self) -> Voice {
        self.voice
    }

    /// Changes the speed (pitch depth, bursts; durations come with the
    /// tokens).
    pub fn set_speed(&mut self, s: Speed) {
        self.speed = s;
    }

    /// The output rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Output channels.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Units (phones and pauses) that are final and not yet rendered: how
    /// far the queue is ahead of the renderer.
    pub fn ahead(&self) -> u32 {
        self.fin.saturating_sub(self.head)
    }

    /// Free queue slots: [`Synth::push`] of a phone or a break needs one.
    pub fn space(&self) -> usize {
        if matches!(self.state, State::Fade(_) | State::StopAtWord) {
            0
        } else {
            QUEUE - (self.tail - self.head) as usize
        }
    }

    /// The F0 (Hz) of the last 1 ms tick rendered, if it was voiced (for
    /// tests and displays).
    pub fn pitch(&self) -> Option<f32> {
        self.pitch
    }

    /// Whether nothing is queued or sounding.
    pub fn is_idle(&self) -> bool {
        self.state == State::Idle
    }

    /// Whether the renderer waits for tokens (it has nothing it may render
    /// yet and [`Synth::finish`] was not called).
    pub fn is_starved(&self) -> bool {
        self.state != State::Idle && !self.finished && !self.ready()
    }

    /// No more tokens follow: the queue is spoken to the end.
    pub fn finish(&mut self) {
        if self.pending.is_some() {
            self.finalize(None);
        }
        self.finished = true;
    }

    /// Stops (see [`Stop`]).
    pub fn stop(&mut self, how: Stop) {
        match (how, self.state) {
            (_, State::Idle | State::Fade(_)) => {}
            (Stop::Now, _) => self.state = State::Fade(FADE),
            (Stop::AtWord, _) => {
                self.finish();
                self.state = State::StopAtWord;
            }
        }
    }

    /// Adds a token; `false` if the queue is full (render first).
    pub fn push(&mut self, t: Token) -> bool {
        self.push_with_text(t, &[])
    }

    /// Adds a token; for a [`Token::Word`], `text` is its normalised text
    /// ([`Converter::word_text`]), used to leave function words unaccented.
    pub fn push_with_text(&mut self, t: Token, text: &[u8]) -> bool {
        if self.space() == 0 {
            if self.fin == self.head && self.pending.is_some() {
                // Nothing renderable: an over-long "word" (fuzzed input).
                self.finalize(None);
            }
            return false;
        }
        if self.state == State::Idle {
            self.begin();
        }
        match t {
            Token::Word(w) => {
                if self.pending.is_some() {
                    self.finalize(None);
                }
                self.pending = Some(Pending {
                    start: self.tail,
                    function: !w.spelled && is_function_word(text),
                    spelled: w.spelled,
                    pos: w.pos,
                    fresh: true,
                });
            }
            Token::Phone(seg) => {
                if seg.ms == 0 {
                    return true;
                }
                let pend = self.pending.get_or_insert(Pending {
                    start: self.tail,
                    function: false,
                    spelled: false,
                    pos: 0,
                    fresh: false,
                });
                let mut flags = 0;
                let mut pos = 0;
                if pend.fresh {
                    pend.fresh = false;
                    flags |= F_WORD;
                    pos = pend.pos;
                    if pend.spelled {
                        flags |= F_SPELLED;
                    }
                    if self.sentence_start {
                        flags |= F_SENT;
                        self.sentence_start = false;
                    }
                }
                if self.aside {
                    flags |= F_ASIDE;
                }
                let ph = seg.phone.ph;
                if matches!(ph, Ph::P | Ph::T | Ph::K) && self.tail > pend.start {
                    let before = self.q[((self.tail - 1) as usize) % QUEUE];
                    if before.ph.is_some_and(|p| matches!(p, Ph::S | Ph::SH)) {
                        flags |= F_UNASP;
                    }
                }
                let ms =
                    if self.aside { ((u32::from(seg.ms) * 85 + 50) / 100).max(1) as u16 } else { seg.ms };
                self.append(Unit { ph: Some(ph), stress: seg.phone.stress, ms, f0: [0, 0], flags, pos });
            }
            Token::Break(p) => {
                if self.pending.is_some() {
                    let c = if p.kind.ends_phrase() { Some((p.contour, p.kind)) } else { None };
                    self.finalize(c);
                }
                let mut flags = 0;
                if p.kind.ends_sentence() {
                    flags |= F_RESET;
                } else if p.kind.ends_phrase() {
                    flags |= F_PART;
                }
                if p.kind.ends_phrase() {
                    self.phrase_accents = 0;
                }
                if p.ms > 0 {
                    let ms =
                        if self.aside { ((u32::from(p.ms) * 85 + 50) / 100).max(1) as u16 } else { p.ms };
                    self.append(Unit { ms, flags, ..SIL });
                    self.fin = self.tail;
                }
                match p.kind {
                    BreakKind::ParenOpen => self.aside = true,
                    BreakKind::ParenClose => self.aside = false,
                    k if k.ends_sentence() => {
                        self.aside = false;
                        self.sentence_start = true;
                    }
                    _ => {}
                }
            }
        }
        true
    }

    /// Starts a new utterance after silence.
    fn begin(&mut self) {
        self.state = State::Run;
        self.finished = false;
        self.draining = false;
        self.in_count = 0;
        self.out_count = 0;
        self.clock = 0.0;
        self.dev = 0.0;
        self.aside = false;
        self.sentence_start = true;
        self.phrase_accents = 0;
        self.prev = SIL;
        self.tick = 0;
        self.buf_pos = TICK;
        self.hold = Frame::SILENT;
    }

    fn append(&mut self, u: Unit) {
        self.q[(self.tail as usize) % QUEUE] = u;
        self.tail += 1;
    }

    fn at(&self, abs: u32) -> Unit {
        self.q[(abs as usize) % QUEUE]
    }

    /// Assigns the pending word's pitch and makes its units final.
    fn finalize(&mut self, contour: Option<(Contour, BreakKind)>) {
        let Some(p) = self.pending.take() else { return };
        let (s, e) = (p.start, self.tail);
        self.fin = e;
        if s >= e {
            return;
        }
        let mut first_primary = None;
        let mut last_primary = None;
        let mut last_secondary = None;
        let mut last_vowel = None;
        for i in s..e {
            let u = self.at(i);
            if u.ph.is_some_and(Ph::is_vowel) {
                last_vowel = Some(i);
                match u.stress {
                    Stress::Primary => {
                        first_primary.get_or_insert(i);
                        last_primary = Some(i);
                    }
                    Stress::Secondary => last_secondary = Some(i),
                    _ => {}
                }
            }
        }
        let set = |q: &mut [Unit; QUEUE], i: u32, a: i32, b: i32| {
            q[(i as usize) % QUEUE].f0 = [a.clamp(-900, 900) as i16, b.clamp(-900, 900) as i16];
        };
        // The accent of a content word.
        if !p.function {
            let mut a = 200;
            if self.phrase_accents == 0 {
                a = a * 5 / 4;
            }
            if p.spelled {
                a = a * 3 / 5;
            }
            if let Some(v) = first_primary.or(last_secondary).or(last_vowel) {
                set(&mut self.q, v, a, a * 55 / 100);
                let tail = e - v - 1;
                for (k, i) in (v + 1..e).enumerate() {
                    let k = k as i32;
                    let n = tail as i32;
                    let x0 = a * 35 / 100 * (n - k) / n;
                    let x1 = a * 35 / 100 * (n - k - 1) / n;
                    set(&mut self.q, i, x0, x1);
                }
                self.phrase_accents = self.phrase_accents.saturating_add(1);
            }
            for i in s..e {
                let u = self.at(i);
                if u.stress == Stress::Secondary && Some(i) != first_primary {
                    set(&mut self.q, i, 70, 40);
                }
            }
        }
        // The phrase-final contour on the last stressed syllable.
        let Some((c, kind)) = contour else { return };
        let n = last_primary.or(last_secondary).or(last_vowel).unwrap_or(e - 1);
        // The movement ends on the word's last voiced sound (a fall on a
        // final voiceless consonant would not be heard).
        let lv = (n..e).rev().find(|&i| self.at(i).ph.is_some_and(Ph::is_voiced)).unwrap_or(n);
        let has_tail = n < lv;
        let (v0, v1, end) = match c {
            Contour::Level => return,
            Contour::Fall => {
                let peak = if kind == BreakKind::Question {
                    320
                } else if p.function {
                    60
                } else {
                    190
                };
                (peak, if has_tail { peak * 3 / 10 } else { -170 }, -170)
            }
            Contour::Rise => (-60, if has_tail { 120 } else { 480 }, 520),
            Contour::Continue => {
                let start = i32::from(self.at(n).f0[0]).max(90);
                (start, if has_tail { 110 } else { 230 }, 230)
            }
            Contour::Exclaim => (460, if has_tail { 150 } else { -150 }, -150),
        };
        set(&mut self.q, n, v0, v1);
        let m = (lv - n) as i32;
        for (k, i) in (n + 1..=lv).enumerate() {
            let k = k as i32;
            let a = v1 + (end - v1) * k / m;
            let b = v1 + (end - v1) * (k + 1) / m;
            set(&mut self.q, i, a, b);
        }
        for i in lv + 1..e {
            set(&mut self.q, i, end, end);
        }
    }

    /// Whether the current unit may render.
    fn ready(&self) -> bool {
        self.head < self.fin && (self.head + 1 < self.tail || self.finished)
    }

    /// Renders into `out` (i16, interleaved when stereo); returns the
    /// frames written. Fewer than requested: the renderer needs tokens
    /// ([`Synth::is_starved`]) or is idle. `ev(frame, event)` reports
    /// events at their frame in this block.
    pub fn render(&mut self, out: &mut [i16], mut ev: impl FnMut(usize, Event)) -> usize {
        let ch = usize::from(self.channels);
        let frames = out.len() / ch;
        let gain = self.voice.gain * MASTER;
        self.run(frames, &mut ev, |k, y| {
            let v = to_i16(soft_clip(y * gain));
            out[k * ch] = v;
            if ch == 2 {
                out[k * ch + 1] = v;
            }
        })
    }

    /// Renders mono `f32` (soft-clipped, within ±1) into `out`.
    pub fn render_f32(&mut self, out: &mut [f32], mut ev: impl FnMut(usize, Event)) -> usize {
        let gain = self.voice.gain * MASTER;
        self.run(out.len(), &mut ev, |k, y| out[k] = soft_clip(y * gain))
    }

    fn run(
        &mut self,
        frames: usize,
        ev: &mut impl FnMut(usize, Event),
        mut put: impl FnMut(usize, f32),
    ) -> usize {
        let mut k = 0;
        while k < frames {
            if self.state == State::Idle {
                break;
            }
            if self.rs.needs_input() {
                if self.buf_pos >= TICK && !self.next_tick(k, ev) {
                    break;
                }
                self.rs.push(self.buf[self.buf_pos]);
                self.buf_pos += 1;
                continue;
            }
            if self.draining && self.out_count >= resample::frames_for(self.in_count, self.rate) {
                self.end(k, ev);
                break;
            }
            put(k, self.rs.pull());
            self.out_count += 1;
            k += 1;
        }
        k
    }

    /// Back to idle after the end or a stop.
    fn end(&mut self, k: usize, ev: &mut impl FnMut(usize, Event)) {
        self.state = State::Idle;
        self.head = self.tail;
        self.fin = self.tail;
        self.pending = None;
        self.finished = false;
        self.draining = false;
        self.dsp.reset(self.seed);
        self.rs.reset();
        ev(k, Event::End);
    }

    /// Fills `buf` with the next tick; `false` if there is nothing to
    /// render (starved, idle, or draining: then `buf` is zeros).
    fn next_tick(&mut self, k: usize, ev: &mut impl FnMut(usize, Event)) -> bool {
        if let State::Fade(0) = self.state {
            self.end(k, ev);
            return false;
        }
        if self.draining {
            self.buf = [0.0; TICK];
            self.buf_pos = 0;
            return true;
        }
        if !self.ready() {
            if matches!(self.state, State::Fade(_)) {
                self.end(k, ev);
                return false;
            }
            if self.finished && self.head >= self.tail {
                self.draining = true;
                self.buf = [0.0; TICK];
                self.buf_pos = 0;
                return true;
            }
            return false;
        }
        let cur = self.at(self.head);
        if self.tick == 0 {
            if self.state == State::StopAtWord && (cur.flags & F_WORD != 0 || cur.ph.is_none()) {
                self.state = State::Fade(FADE);
            }
            if cur.flags & F_RESET != 0 {
                self.clock = 0.0;
            } else if cur.flags & F_PART != 0 {
                self.clock *= 0.5;
            }
            if cur.flags & F_WORD != 0 && self.state == State::Run {
                if cur.flags & F_SENT != 0 {
                    ev(k, Event::Sentence { pos: cur.pos });
                }
                ev(k, Event::Word { pos: cur.pos, spelled: cur.flags & F_SPELLED != 0 });
            }
        }
        let next = if self.head + 1 < self.tail { self.at(self.head + 1) } else { SIL };
        let fr = self.frame(&cur, &next);
        self.pitch = if fr.av > 0.0 { Some(fr.f0) } else { None };
        self.dsp.set_frame(&fr);
        self.dsp.render(&mut self.buf);
        if let State::Fade(left) = self.state {
            let mut l = left;
            for s in &mut self.buf {
                *s *= f32::from(l) / f32::from(FADE);
                l = l.saturating_sub(1);
            }
            self.state = State::Fade(l);
        }
        self.buf_pos = 0;
        self.in_count += TICK as u64;
        self.tick += 1;
        if self.tick >= cur.ms {
            self.tick = 0;
            self.prev = cur;
            self.head += 1;
        }
        true
    }

    /// The synthesiser's parameters for the current tick.
    fn frame(&mut self, cur: &Unit, next: &Unit) -> Frame {
        let prev = self.prev;
        let d = f32::from(cur.ms.max(1));
        let t = f32::from(self.tick) + 0.5;
        let s_speed = self.speed.as_f32();
        let obstruent_scale = if s_speed > 1.0 { 2.0 / (1.0 + s_speed) } else { 1.0 };
        let mut fr = Frame::SILENT;
        let fs = self.voice.formants;

        // ---- Pitch.
        let u = t / d;
        let target = (f32::from(cur.f0[0]) + (f32::from(cur.f0[1]) - f32::from(cur.f0[0])) * u) * 0.001;
        let tau_ms = clampf(22.0 - 2.0 * (s_speed - 1.0), 8.0, 22.0);
        self.dev += (target - self.dev) * (1.0 / tau_ms);
        let depth = f32::from(duration::pitch_depth_pct(self.speed)) * 0.01;
        let aside = cur.flags & F_ASIDE != 0;
        let (base_k, range_k) = if aside { (0.9, 0.6) } else { (1.0, 1.0) };
        let decl = clampf(1.10 - 0.09 * self.clock, 0.86, 1.10);
        self.clock += 0.001 * s_speed;
        // Flutter (Klatt & Klatt 1990): three slow sines, ±≈1 %.
        let rates = [12.7f32, 7.1, 4.7];
        let mut fl = 0.0;
        for (ph, r) in self.flutter.iter_mut().zip(rates) {
            *ph += r * 0.001;
            if *ph >= 1.0 {
                *ph -= 1.0;
            }
            fl += sin_turns(*ph);
        }
        let f0 = self.voice.f0
            * decl
            * base_k
            * (1.0 + self.voice.range * range_k * depth * self.dev)
            * (1.0 + 0.0035 * fl);
        fr.f0 = clampf(f0, 50.0, 500.0);
        fr.oq = self.voice.oq;
        fr.tilt = self.tilt;

        let Some(cs) = spec(cur, &prev, next) else {
            // A pause: the filters hold, the sources are off.
            let h = &self.hold;
            (fr.f, fr.b, fr.fnp, fr.fnz, fr.pf, fr.pb, fr.pa) = (h.f, h.b, h.fnp, h.fnz, h.pf, h.pb, h.pa);
            return fr;
        };
        let pre = if prev.ph.is_some() { spec(&prev, &SIL, cur) } else { None };
        let nxt = if next.ph.is_some() { spec(next, cur, &SIL) } else { None };

        // ---- Formants F1–F3 and bandwidths.
        let b_in = pre.as_ref().map_or(cs.start, |p| boundary(p, &cs));
        let b_out = nxt.as_ref().map_or(cs.end, |n| boundary(&cs, n));
        let ti = pre.as_ref().map_or(0.0, |p| tau(cs.class, p.class)).min(d * 0.5);
        let to = nxt.as_ref().map_or(0.0, |n| tau(cs.class, n.class)).min(d * 0.5);
        let glide = if cs.class == Class::Diphthong { smooth((u - 0.2) * (1.0 / 0.7)) } else { 0.0 };
        let own = lerp3(cs.start, cs.end, glide);
        let mut f = own;
        if t < ti {
            f = lerp3(b_in, own, t / ti);
        } else if t > d - to {
            f = lerp3(own, b_out, (t - (d - to)) / to);
        }
        let mut bw = cs.bw;
        let bw_edge = |n: &Option<Spec>| n.as_ref().map_or(cs.bw, |n| lerp3(cs.bw, n.bw, 0.5));
        if t < ti {
            bw = lerp3(bw_edge(&pre), cs.bw, t / ti);
        } else if t > d - to {
            bw = lerp3(cs.bw, bw_edge(&nxt), (t - (d - to)) / to);
        }
        // Nasal zero: 450 Hz in a nasal, cancelled on the pole otherwise;
        // vowels next to a nasal are nasalised over their transition.
        let zn = |s: &Spec| if s.t.nasal { f32::from(FNZ_NASAL) } else { f32::from(FNP) };
        let own_z = zn(&cs);
        let mut fnz = own_z;
        if t < ti {
            let e = pre.as_ref().map_or(own_z, |p| 0.5 * (zn(p) + own_z));
            fnz = e + (own_z - e) * (t / ti);
        } else if t > d - to {
            let e = nxt.as_ref().map_or(own_z, |n| 0.5 * (zn(n) + own_z));
            fnz = own_z + (e - own_z) * ((t - (d - to)) / to);
        }
        if cs.t.nasal && t >= ti && t <= d - to {
            fnz = own_z;
        }

        // ---- Sources.
        let (av0, ah0, af0) = own_src(&cs, &prev);
        let src_of = |n: &Option<Spec>, p: &Unit| n.as_ref().map_or((0.0, 0.0, 0.0), |n| own_src(n, p));
        let (pav, pah, paf) = src_of(&pre, &SIL);
        let (nav, nah, naf) = src_of(&nxt, cur);
        let r_in = if pre.is_some() { 3.0f32.min(d * 0.5) } else { 6.0f32.min(d / 3.0) };
        let r_out = if nxt.is_some() { 3.0f32.min(d * 0.5) } else { 8.0f32.min(d / 3.0) };
        let edge = |own: f32, other: f32, live: bool| if live { 0.5 * (own + other) } else { 0.0 };
        let mix = |own: f32, before: f32, after: f32| {
            if t < r_in {
                let e = edge(own, before, pre.is_some());
                e + (own - e) * (t / r_in)
            } else if t > d - r_out {
                let e = edge(own, after, nxt.is_some());
                own + (e - own) * ((t - (d - r_out)) / r_out)
            } else {
                own
            }
        };
        let mut av = mix(av0, pav, nav);
        let mut ah = mix(ah0, pah, nah);
        let mut af = mix(af0, paf, naf);
        let mut fric = cs.t.fric;
        if cs.t.af == OFF {
            if t < r_in {
                fric = pre.as_ref().map_or(Fric::None, |p| p.t.fric);
            } else if t > d - r_out {
                fric = nxt.as_ref().map_or(Fric::None, |n| n.t.fric);
            }
        }
        let mut tilt = if matches!(cs.class, Class::Fricative | Class::Stop | Class::Affricate) {
            self.tilt_dark
        } else {
            self.tilt
        };

        // Stops and affricates: closure, then the burst (or the
        // affricate's frication) at the release.
        let released = nxt.as_ref().is_none_or(|n| !matches!(n.class, Class::Stop | Class::Affricate));
        match cs.class {
            Class::Stop => {
                let ph = cur.ph.unwrap_or(Ph::T);
                let burst = if released {
                    clampf(f32::from(targets::burst_ms(ph)) * obstruent_scale, 2.0, (d * 0.4).max(2.0))
                } else {
                    0.0
                };
                if t > d - burst {
                    av = 0.0;
                    ah = if ph.is_voiced() { 0.0 } else { 0.25 };
                    af = lin(cs.t.af);
                    fric = cs.t.fric;
                    f = b_out;
                } else {
                    af = 0.0;
                    ah = 0.0;
                    // The closure holds the formants where the last sound
                    // left them (no jump in ringing filters).
                    if pre.is_some() {
                        f = b_in;
                    }
                    // A voiced stop is prevoiced over the last 20 ms of its
                    // closure when nothing voiced comes before it.
                    if ph.is_voiced() && av == 0.0 && t > d - burst - 20.0 && t > d * 0.4 {
                        av = lin(cs.t.av);
                    }
                    if av > 0.0 {
                        // The voice bar: F1 low.
                        f[0] = 200.0;
                    }
                }
            }
            Class::Affricate => {
                let close = d * 0.45;
                if t > close {
                    av = lin(cs.t.av);
                    af = lin(cs.t.af) * if t < close + 3.0 { 1.4 } else { 1.0 };
                    if t > d - r_out {
                        af *= 1.0 - 0.5 * ((t - (d - r_out)) / r_out);
                    }
                    fric = cs.t.fric;
                } else {
                    av = 0.0;
                    af = 0.0;
                    if pre.is_some() {
                        f = b_in;
                    }
                }
                ah = 0.0;
            }
            _ => {}
        }
        // Aspiration after a released voiceless stop or affricate; a short
        // voicing lag after a voiced one.
        if let (Some(pp), true) = (prev.ph, pre.is_some())
            && matches!(pp.class(), Class::Stop | Class::Affricate)
            && cs.class != Class::Stop
        {
            let base = f32::from(targets::vot_ms(pp));
            // The syllable's stress: the vowel's own, or the next one's
            // after a liquid or glide (/pr/ in "pray").
            let st =
                if cs.class == Class::Liquid || cs.class == Class::Glide { next.stress } else { cur.stress };
            let stressed = st == Stress::Primary || st == Stress::Secondary;
            let mut vot = base * obstruent_scale * if stressed { 1.0 } else { 0.6 };
            if prev.flags & F_UNASP != 0 {
                vot = vot.min(12.0);
            }
            let vot = vot.min(d * 0.45);
            if t < vot && cur.ph.is_some_and(targets::vocalic) {
                if pp.is_voiced() {
                    av *= t / vot;
                } else {
                    av = 0.0;
                    ah = ASP_STOP * (1.0 - 0.4 * (t / vot));
                    af = 0.0;
                    // The open glottis widens the lower bandwidths.
                    bw = [bw[0] * 3.0, bw[1] * 1.6, bw[2] * 1.3];
                }
            }
        }
        if cs.class == Class::Aspirate {
            // /h/: B1 widened (the open glottis), the vowel's formants.
            tilt = self.tilt;
        }

        // ---- Assemble.
        fr.av = av * AV_REF;
        fr.ah = (ah + av * self.breath) * AH_REF;
        fr.af = af * AF_REF;
        fr.tilt = tilt;
        for k in 0..3 {
            fr.f[k] = f[k] * fs;
            fr.b[k] = bw[k];
        }
        for k in 0..3 {
            fr.f[3 + k] = f32::from(F45[k]) * fs;
            fr.b[3 + k] = f32::from(B45[k]);
        }
        // Keep the formants ordered and below Nyquist.
        fr.f[0] = clampf(fr.f[0], 150.0, 1300.0);
        fr.f[1] = clampf(fr.f[1], fr.f[0] + 150.0, 3200.0 * fs);
        fr.f[2] = clampf(fr.f[2], fr.f[1] + 150.0, 3600.0 * fs);
        fr.f[3] = fr.f[3].max(fr.f[2] + 200.0);
        fr.f[4] = clampf(fr.f[4].max(fr.f[3] + 200.0), 0.0, 6800.0);
        fr.f[5] = clampf(fr.f[5].max(fr.f[4] + 300.0), 0.0, 7400.0);
        fr.fnp = f32::from(FNP);
        fr.fnz = fnz;
        fr.pf = [
            fr.f[1],
            fr.f[2],
            fr.f[3],
            clampf(f32::from(P56[0]) * fs, 0.0, 6800.0),
            clampf(f32::from(P56[1]) * fs, 0.0, 7200.0),
        ];
        fr.pb = [200.0, 250.0, 320.0, f32::from(PB56[0]), f32::from(PB56[1])];
        for (a, d) in fr.pa.iter_mut().zip(targets::spectrum(fric)) {
            *a = lin(d);
        }
        if fric == Fric::None {
            fr.af = 0.0;
        }
        self.hold = fr;
        fr
    }
}

/// Final units [`Speaker`] keeps the renderer ahead by (about two words).
pub const AHEAD: u32 = 12;

/// Text in, audio out: a [`Converter`] feeding a [`Synth`].
pub struct Speaker {
    conv: Converter,
    synth: Synth,
    /// Text was fed and the end has not been reported yet.
    active: bool,
}

impl core::fmt::Debug for Speaker {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Speaker").field("synth", &self.synth).field("active", &self.active).finish()
    }
}

impl Speaker {
    /// A speaker with the built-in lexicon.
    pub fn new(cfg: Config, opts: Options) -> Speaker {
        Speaker { conv: Converter::new(opts, cfg.speed), synth: Synth::new(cfg), active: false }
    }

    /// The converter.
    pub fn converter(&mut self) -> &mut Converter {
        &mut self.conv
    }

    /// The renderer.
    pub fn synth(&mut self) -> &mut Synth {
        &mut self.synth
    }

    /// Adds text (see [`Converter::feed`]); after the end of an utterance
    /// the next text starts a new one.
    pub fn feed(&mut self, text: &[u8]) -> usize {
        if !self.active {
            self.conv.reset();
        }
        let n = self.conv.feed(text);
        if n > 0 {
            self.active = true;
        }
        n
    }

    /// Free input space (bytes).
    pub fn space(&self) -> usize {
        if self.active { self.conv.space() } else { crate::convert::INPUT }
    }

    /// No more text follows.
    pub fn finish(&mut self) {
        if self.active {
            self.conv.finish();
        }
    }

    /// Changes the speed (durations from the next word on).
    pub fn set_speed(&mut self, s: Speed) {
        self.conv.set_speed(s);
        self.synth.set_speed(s);
    }

    /// Changes the voice.
    pub fn set_voice(&mut self, v: Voice) {
        self.synth.set_voice(v);
    }

    /// Stops; the text not yet spoken is dropped.
    pub fn stop(&mut self, how: Stop) {
        self.synth.stop(how);
        self.conv.reset();
        self.conv.finish();
        if self.synth.is_idle() {
            self.active = false;
        }
    }

    /// Whether everything fed was spoken (or stopped).
    pub fn is_idle(&self) -> bool {
        !self.active
    }

    /// Converts text until the renderer has [`AHEAD`] final units in hand
    /// (or its queue is full): a word or two at a time as it speaks, not
    /// the queue's whole 64 units at the start of an utterance (TTS-3b,
    /// 2026-11-28: on the player over music that first conversion took up
    /// to 0.5 s and the speech ring ran dry; the output is the same).
    fn pump(&mut self) -> bool {
        let mut any = false;
        while self.synth.space() > 0 && self.synth.ahead() < AHEAD {
            match self.conv.next_token() {
                Some(t) => {
                    let text = if matches!(t, Token::Word(_)) { self.conv.word_text() } else { &[] };
                    self.synth.push_with_text(t, text);
                    any = true;
                }
                None => {
                    if self.conv.is_done() && !self.synth.is_idle() {
                        self.synth.finish();
                    }
                    break;
                }
            }
        }
        any
    }

    /// Renders like [`Synth::render`], pulling tokens from the converter
    /// as needed. Returns fewer frames when more text is needed or when
    /// everything was spoken ([`Event::End`]).
    pub fn render(&mut self, out: &mut [i16], mut ev: impl FnMut(usize, Event)) -> usize {
        let ch = usize::from(self.synth.channels());
        let frames = out.len() / ch;
        let mut done = 0;
        while self.active && done < frames {
            let pushed = self.pump();
            let mut ended = false;
            let n = self.synth.render(&mut out[done * ch..frames * ch], |k, e| {
                ended |= e == Event::End;
                ev(done + k, e);
            });
            done += n;
            if ended {
                self.active = false;
            }
            if n == 0 && !pushed {
                break;
            }
        }
        done
    }

    /// Renders mono `f32` like [`Synth::render_f32`].
    pub fn render_f32(&mut self, out: &mut [f32], mut ev: impl FnMut(usize, Event)) -> usize {
        let mut done = 0;
        while self.active && done < out.len() {
            let pushed = self.pump();
            let mut ended = false;
            let n = self.synth.render_f32(&mut out[done..], |k, e| {
                ended |= e == Event::End;
                ev(done + k, e);
            });
            done += n;
            if ended {
                self.active = false;
            }
            if n == 0 && !pushed {
                break;
            }
        }
        done
    }
}

/// RAM of the speech engine's parts (bytes, this build).
pub mod ram {
    /// The renderer (queue, synthesiser, resampler).
    pub fn synth() -> usize {
        core::mem::size_of::<super::Synth>()
    }

    /// The signal path alone.
    pub fn dsp() -> usize {
        core::mem::size_of::<crate::klatt::Dsp>()
    }

    /// The resampler.
    pub fn resampler() -> usize {
        core::mem::size_of::<crate::resample::Resampler>()
    }

    /// The text-to-audio speaker (converter + renderer).
    pub fn speaker() -> usize {
        core::mem::size_of::<super::Speaker>()
    }
}

/// Internal rate (re-exported for the tests and examples).
pub const INTERNAL_RATE: u32 = FS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{Pause, Seg, Word};
    use crate::phoneme::Phone;

    /// TTS-3b, 2026-11-28, on the player: the first render of an utterance
    /// converted text until the renderer's whole queue (64 units, ≈ 10
    /// words) was full; over music, with the converter's code and the
    /// lexicon paged in, that one step took up to 0.5 s and the 128 ms
    /// speech ring ran dry (`late=` gaps). The converter now stays about
    /// two words ahead; the speech is the same samples.
    #[test]
    fn regress_first_render_converted_ten_words() {
        extern crate std;
        use std::{vec, vec::Vec};
        let text = b"The quick brown fox jumps over the lazy dog while the band plays on and on.";
        let cfg = Config { rate: 16_000, channels: 1, ..Config::default() };
        let mut sp = Speaker::new(cfg, Options::default());
        sp.feed(text);
        sp.finish();
        let mut block = [0i16; 16];
        assert_eq!(sp.render(&mut block, |_, _| {}), 16);
        let queued = QUEUE - sp.synth().space();
        assert!(queued < 32, "{queued} units queued after the first render");
        // The same samples as with the whole queue converted at once.
        let mut all = Vec::new();
        let mut more = vec![0i16; 4096];
        loop {
            let n = sp.render(&mut more, |_, _| {});
            all.extend_from_slice(&more[..n]);
            if n < more.len() {
                break;
            }
        }
        let mut whole = Synth::new(cfg);
        let mut conv = Converter::new(Options::default(), Speed::NORMAL);
        conv.convert(text, |w, t| {
            let txt: &[u8] = if let Token::Word(_) = t { w.word_text() } else { &[] };
            assert!(whole.push_with_text(t, txt));
        });
        whole.finish();
        let mut ref_all = vec![0i16; 16];
        assert_eq!(whole.render(&mut ref_all, |_, _| {}), 16);
        assert_eq!(ref_all[..], block[..]);
        let mut rest = Vec::new();
        loop {
            let n = whole.render(&mut more, |_, _| {});
            rest.extend_from_slice(&more[..n]);
            if n < more.len() {
                break;
            }
        }
        assert_eq!(rest, all);
    }

    #[test]
    fn function_words() {
        assert!(is_function_word(b"The") && is_function_word(b"of") && is_function_word(b"would"));
        assert!(!is_function_word(b"music") && !is_function_word(b"") && !is_function_word(b"theremin"));
    }

    fn word(s: &mut Synth, phones: &[(Ph, Stress)], text: &[u8]) {
        assert!(s.push_with_text(Token::Word(Word { pos: 0, phrase_final: false, spelled: false }), text));
        for &(ph, st) in phones {
            assert!(s.push(Token::Phone(Seg { phone: Phone::new(ph, st), ms: 80 })));
        }
    }

    #[test]
    fn accents_and_contours_are_assigned_per_word() {
        let mut s = Synth::new(Config::default());
        word(&mut s, &[(Ph::DH, Stress::None), (Ph::AH, Stress::Unstressed)], b"the");
        word(
            &mut s,
            &[(Ph::M, Stress::None), (Ph::Y, Stress::None), (Ph::UW, Stress::Primary), (Ph::Z, Stress::None)],
            b"muse",
        );
        // Nothing is final while the word may still get a contour.
        assert_eq!(s.fin, 2);
        assert!(!s.ready() || s.head < s.fin);
        s.push(Token::Break(Pause { kind: BreakKind::Question, contour: Contour::Rise, ms: 100 }));
        assert_eq!(s.fin, s.tail);
        let f0 = |i: u32| s.at(i).f0;
        // The function word is flat; the stressed vowel of the last word
        // starts low and rises, the voiced tail goes on rising.
        assert_eq!(f0(0), [0, 0]);
        assert_eq!(f0(1), [0, 0]);
        assert!(f0(4)[0] < 0 && f0(4)[1] > 100, "{:?}", f0(4));
        assert!(f0(5)[1] >= 500, "{:?}", f0(5));
    }

    #[test]
    fn a_full_queue_refuses_and_recovers() {
        let mut s = Synth::new(Config { rate: 16_000, channels: 1, ..Config::default() });
        assert!(s.push(Token::Word(Word { pos: 0, phrase_final: false, spelled: false })));
        let seg = Token::Phone(Seg { phone: Phone::new(Ph::AA, Stress::Primary), ms: 5 });
        let mut pushed = 0;
        while s.push(seg) {
            pushed += 1;
        }
        assert_eq!(pushed, QUEUE);
        // The over-long "word" was made final so rendering can free space.
        let mut buf = [0i16; 1000];
        assert!(s.render(&mut buf, |_, _| {}) > 0);
        assert!(s.push(seg));
    }
}
