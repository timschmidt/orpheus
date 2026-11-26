//! The project: patterns of drum and synth steps, tracks, a song, tempo
//! and swing.
//!
//! Everything is fixed-size plain data (no allocation): a [`Project`] holds
//! [`PATTERNS`] patterns of up to [`MAX_STEPS`] steps for [`TRACKS`] drum
//! tracks and [`SYNTH_TRACKS`] synth tracks (SEQ-2: bass and lead, a
//! [`SynthStep`] each, with the pattern's transpose and its parameter
//! locks), the two kits the drum tracks play, the synth tracks' patches,
//! the scale lock, the fill pattern and a song of up to [`SONG_LEN`]
//! entries. A [`Step`] is three bytes, a [`SynthStep`] four, a [`Lock`]
//! four; the whole project is about 35 KiB ([`crate::ram`]).
//!
//! Track numbers: 0–7 the drum tracks, [`TRACKS`] + 0 and + 1 (8 and 9)
//! the bass and the lead ([`ALL_TRACKS`]).
//!
//! Every setter clamps its value to the documented range, so the model
//! never holds a value the player or the file format cannot take; every
//! change bumps [`Project::rev`] (autosave and redraws compare it).

use orpheus_synth::bank::{NAME_LEN, Name};
use orpheus_synth::kit::{self, Kit, KitEntry};
use orpheus_synth::{Patch, presets};

pub use crate::scale::{Chord, Scale};

/// Patterns in a project.
pub const PATTERNS: usize = 16;
/// Most steps in a pattern (1/16 notes: four bars of 4/4).
pub const MAX_STEPS: usize = 64;
/// Steps on one page of the grid.
pub const PAGE: usize = 16;
/// Drum tracks.
pub const TRACKS: usize = 8;
/// Synth tracks (bass, lead).
pub const SYNTH_TRACKS: usize = 2;
/// Every track: drums 0–7, synth 8–9.
pub const ALL_TRACKS: usize = TRACKS + SYNTH_TRACKS;
/// Parameter locks a pattern holds (a lock is one parameter of one
/// track's step).
pub const LOCKS: usize = 24;
/// Kits of a project (each drum track plays kit A or B).
pub const KITS: usize = 2;
/// "No fill pattern".
pub const NO_FILL: u8 = 0xFF;
/// Most transpose of a pattern, semitones (±).
pub const TRANSPOSE_MAX: i8 = 24;
/// Gate lengths of a synth step in quarter steps, by code (0–15).
pub const GATES: [u16; 16] = [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256];
/// The default gate code: one step.
pub const GATE_DEFAULT: u8 = 3;
/// The synth tracks' default notes: C2 (bass) and C4 (lead).
pub const SYNTH_NOTES: [u8; SYNTH_TRACKS] = [36, 60];
/// Entries of the song (pattern chain).
pub const SONG_LEN: usize = 64;
/// Most repeats of one song entry.
pub const MAX_REPEATS: u8 = 16;
/// Tempo range, hundredths of a BPM (40–240 BPM).
pub const TEMPO_MIN: u16 = 4_000;
/// Highest tempo, hundredths of a BPM.
pub const TEMPO_MAX: u16 = 24_000;
/// A new project's tempo (120 BPM).
pub const TEMPO_DEFAULT: u16 = 12_000;
/// Swing range, per cent of a step pair given to its first step (50 =
/// straight, 66.7 ≈ triplet feel, 75 = dotted).
pub const SWING_MIN: u8 = 50;
/// Most swing, per cent.
pub const SWING_MAX: u8 = 75;
/// Most micro-timing delay, 1/24 of a step (half a step).
pub const MICRO_MAX: u8 = 12;
/// Most ratchet hits in a step.
pub const MAX_RATCHET: u8 = 4;
/// Probability levels: a step plays with probability `level/16`.
pub const PROB_LEVELS: u8 = 16;
/// Velocity added by an accent (clamped at 127).
pub const ACCENT: u8 = 32;
/// The default velocity of a new step.
pub const VELOCITY_DEFAULT: u8 = 100;
/// The default track level, per cent.
pub const LEVEL_DEFAULT: u8 = 80;

/// One step of one drum track, three bytes:
///
/// ```text
/// a: bit 7 on, bits 0–6 velocity (1–127; kept while off)
/// b: bits 0–3 probability code k (plays with (16 − k)/16), bits 4–5
///    ratchet − 1, bit 6 accent, bit 7 reserved (0)
/// c: bits 0–4 micro-timing delay in 1/24 step (0–12), bits 5–7 reserved
/// ```
///
/// That is also its form in the project file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Step {
    a: u8,
    b: u8,
    c: u8,
}

impl Default for Step {
    fn default() -> Self {
        Self::OFF
    }
}

impl Step {
    /// An empty step with the default parameters.
    pub const OFF: Step = Step { a: VELOCITY_DEFAULT, b: 0, c: 0 };

    /// A step that plays at `velocity`.
    #[must_use]
    pub const fn hit(velocity: u8) -> Step {
        Step::OFF.with_velocity(velocity).with_on(true)
    }

    /// The step from its three stored bytes, sanitised: reserved bits
    /// cleared, velocity 0 read as 1, the delay clamped.
    #[must_use]
    pub const fn from_bytes(b: [u8; 3]) -> Step {
        let a = if b[0] & 0x7F == 0 { (b[0] & 0x80) | 1 } else { b[0] };
        let mut c = b[2] & 0x1F;
        if c > MICRO_MAX {
            c = MICRO_MAX;
        }
        Step { a, b: b[1] & 0x7F, c }
    }

    /// The three stored bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 3] {
        [self.a, self.b, self.c]
    }

    /// Whether the step plays.
    #[must_use]
    pub const fn on(self) -> bool {
        self.a & 0x80 != 0
    }

    /// Velocity, 1–127.
    #[must_use]
    pub const fn velocity(self) -> u8 {
        self.a & 0x7F
    }

    /// Probability level 1–16: plays with `level/16`.
    #[must_use]
    pub const fn prob(self) -> u8 {
        PROB_LEVELS - (self.b & 0x0F)
    }

    /// Hits in the step, 1–4.
    #[must_use]
    pub const fn ratchet(self) -> u8 {
        ((self.b >> 4) & 3) + 1
    }

    /// Accented.
    #[must_use]
    pub const fn accent(self) -> bool {
        self.b & 0x40 != 0
    }

    /// Delay after the step's start, 1/24 of a nominal step (0–12).
    #[must_use]
    pub const fn micro(self) -> u8 {
        self.c
    }

    /// With `on`.
    #[must_use]
    pub const fn with_on(self, on: bool) -> Step {
        Step { a: (self.a & 0x7F) | if on { 0x80 } else { 0 }, ..self }
    }

    /// With velocity `v` (clamped to 1–127).
    #[must_use]
    pub const fn with_velocity(self, v: u8) -> Step {
        let v = if v == 0 {
            1
        } else if v > 127 {
            127
        } else {
            v
        };
        Step { a: (self.a & 0x80) | v, ..self }
    }

    /// With probability level `p` (clamped to 1–16).
    #[must_use]
    pub const fn with_prob(self, p: u8) -> Step {
        let p = if p == 0 {
            1
        } else if p > PROB_LEVELS {
            PROB_LEVELS
        } else {
            p
        };
        Step { b: (self.b & !0x0F) | (PROB_LEVELS - p), ..self }
    }

    /// With `r` hits (clamped to 1–4).
    #[must_use]
    pub const fn with_ratchet(self, r: u8) -> Step {
        let r = if r == 0 {
            1
        } else if r > MAX_RATCHET {
            MAX_RATCHET
        } else {
            r
        };
        Step { b: (self.b & !0x30) | ((r - 1) << 4), ..self }
    }

    /// With the accent set or cleared.
    #[must_use]
    pub const fn with_accent(self, on: bool) -> Step {
        Step { b: (self.b & !0x40) | if on { 0x40 } else { 0 }, ..self }
    }

    /// With delay `m` (clamped to 0–12).
    #[must_use]
    pub const fn with_micro(self, m: u8) -> Step {
        Step { c: if m > MICRO_MAX { MICRO_MAX } else { m }, ..self }
    }

    /// The probability in per cent (rounded), for the screen.
    #[must_use]
    pub const fn prob_percent(self) -> u8 {
        ((self.prob() as u16 * 100 + 8) / 16) as u8
    }
}

/// One step of a synth track, four bytes:
///
/// ```text
/// a: bit 7 on, bits 0–6 velocity (1–127; kept while off)
/// b: bits 0–3 probability code, bits 4–5 ratchet − 1, bit 6 accent,
///    bit 7 chord bit 0
/// c: bits 0–3 micro-timing delay in 1/24 step (0–12), bits 4–7 gate code
///    (a length of GATES[code] quarter steps)
/// n: bits 0–6 note (0–127, kept while off), bit 7 chord bit 1
/// ```
///
/// That is also its form in the project file. A chord ([`Chord`], built
/// on the note in the project's scale) plays once: its ratchet is
/// ignored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SynthStep {
    a: u8,
    b: u8,
    c: u8,
    n: u8,
}

impl Default for SynthStep {
    fn default() -> Self {
        Self::OFF
    }
}

impl SynthStep {
    /// An empty step on C4 with the default parameters.
    pub const OFF: SynthStep = SynthStep { a: VELOCITY_DEFAULT, b: 0, c: GATE_DEFAULT << 4, n: 60 };

    /// An empty step on `note`.
    #[must_use]
    pub const fn off(note: u8) -> SynthStep {
        SynthStep::OFF.with_note(note)
    }

    /// A step that plays `note` at `velocity` for one step.
    #[must_use]
    pub const fn note(note: u8, velocity: u8) -> SynthStep {
        SynthStep::OFF.with_note(note).with_velocity(velocity).with_on(true)
    }

    /// The step from its four stored bytes, sanitised: velocity 0 read as
    /// 1, the delay clamped.
    #[must_use]
    pub const fn from_bytes(x: [u8; 4]) -> SynthStep {
        let a = if x[0] & 0x7F == 0 { (x[0] & 0x80) | 1 } else { x[0] };
        let mut m = x[2] & 0x0F;
        if m > MICRO_MAX {
            m = MICRO_MAX;
        }
        SynthStep { a, b: x[1], c: (x[2] & 0xF0) | m, n: x[3] }
    }

    /// The four stored bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 4] {
        [self.a, self.b, self.c, self.n]
    }

    /// The parameters it shares with a drum step (on, velocity,
    /// probability, ratchet, accent, delay).
    #[must_use]
    pub const fn common(self) -> Step {
        Step { a: self.a, b: self.b & 0x7F, c: self.c & 0x0F }
    }

    /// With the shared parameters of `s`.
    #[must_use]
    pub const fn with_common(self, s: Step) -> SynthStep {
        SynthStep { a: s.a, b: (s.b & 0x7F) | (self.b & 0x80), c: (self.c & 0xF0) | (s.c & 0x0F), n: self.n }
    }

    /// Whether the step plays.
    #[must_use]
    pub const fn on(self) -> bool {
        self.a & 0x80 != 0
    }

    /// The note, 0–127.
    #[must_use]
    pub const fn pitch(self) -> u8 {
        self.n & 0x7F
    }

    /// The chord.
    #[must_use]
    pub const fn chord(self) -> Chord {
        Chord::from_code((self.b >> 7) | ((self.n >> 6) & 2))
    }

    /// The gate code (0–15).
    #[must_use]
    pub const fn gate(self) -> u8 {
        self.c >> 4
    }

    /// The gate length in quarter steps.
    #[must_use]
    pub const fn gate_quarters(self) -> u16 {
        GATES[(self.c >> 4) as usize]
    }

    /// With `on`.
    #[must_use]
    pub const fn with_on(self, on: bool) -> SynthStep {
        self.with_common(self.common().with_on(on))
    }

    /// With velocity `v` (clamped to 1–127).
    #[must_use]
    pub const fn with_velocity(self, v: u8) -> SynthStep {
        self.with_common(self.common().with_velocity(v))
    }

    /// With note `n` (clamped to 0–127).
    #[must_use]
    pub const fn with_note(self, n: u8) -> SynthStep {
        SynthStep { n: (self.n & 0x80) | if n > 127 { 127 } else { n }, ..self }
    }

    /// With chord `ch`.
    #[must_use]
    pub const fn with_chord(self, ch: Chord) -> SynthStep {
        let c = ch.code();
        SynthStep { b: (self.b & 0x7F) | ((c & 1) << 7), n: (self.n & 0x7F) | ((c & 2) << 6), ..self }
    }

    /// With gate code `g` (clamped to 0–15).
    #[must_use]
    pub const fn with_gate(self, g: u8) -> SynthStep {
        SynthStep { c: (self.c & 0x0F) | ((if g > 15 { 15 } else { g }) << 4), ..self }
    }
}

/// A parameter a step can lock (its own value for that step only).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LockParam {
    /// Drum tracks: tune, quarter semitones + 96 (0–192: ±24 semitones).
    Tune,
    /// Drum tracks: decay scale × 50 (5–200), as a kit entry's.
    Decay,
    /// Drum tracks: tone, per cent.
    Tone,
    /// Synth tracks: filter cutoff, 0–127 on a log scale of 20 Hz–20 kHz.
    Cutoff,
    /// Synth tracks: resonance, per cent.
    Resonance,
    /// Synth tracks: glide, hundredths of a second (0–100).
    Glide,
}

impl LockParam {
    /// The drum tracks' lockable parameters.
    pub const DRUM: [LockParam; 3] = [LockParam::Tune, LockParam::Decay, LockParam::Tone];
    /// The synth tracks' lockable parameters.
    pub const SYNTH: [LockParam; 3] = [LockParam::Cutoff, LockParam::Resonance, LockParam::Glide];

    /// The parameters of track `t` (0–9).
    #[must_use]
    pub const fn of_track(t: usize) -> &'static [LockParam; 3] {
        if t < TRACKS { &LockParam::DRUM } else { &LockParam::SYNTH }
    }

    /// The parameter of a code (the file's).
    #[must_use]
    pub const fn from_code(c: u8) -> Option<LockParam> {
        Some(match c {
            0 => LockParam::Tune,
            1 => LockParam::Decay,
            2 => LockParam::Tone,
            3 => LockParam::Cutoff,
            4 => LockParam::Resonance,
            5 => LockParam::Glide,
            _ => return None,
        })
    }

    /// Its code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// The range of its values.
    #[must_use]
    pub const fn range(self) -> (u8, u8) {
        match self {
            LockParam::Tune => (0, 192),
            LockParam::Decay => (5, 200),
            LockParam::Tone | LockParam::Resonance | LockParam::Glide => (0, 100),
            LockParam::Cutoff => (0, 127),
        }
    }

    /// `v` in range.
    #[must_use]
    pub const fn clamp(self, v: u8) -> u8 {
        let (lo, hi) = self.range();
        if v < lo {
            lo
        } else if v > hi {
            hi
        } else {
            v
        }
    }

    /// Whether it belongs to track `t`'s kind.
    #[must_use]
    pub const fn fits(self, t: usize) -> bool {
        t < ALL_TRACKS && (t < TRACKS) == ((self as u8) < 3)
    }

    /// The value of a drum track's kit entry (`Tune`, `Decay`, `Tone`) in
    /// lock units: what an unlocked step plays.
    #[must_use]
    pub fn of_entry(self, e: &KitEntry) -> u8 {
        match self {
            LockParam::Tune => (i16::from(e.tune).clamp(-96, 96) + 96) as u8,
            LockParam::Decay => self.clamp(e.decay),
            LockParam::Tone => self.clamp(e.tone),
            _ => 0,
        }
    }
}

/// One parameter lock: track `track`'s step `step` plays `param` at
/// `value` (in [`LockParam`]'s units).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Lock {
    /// Step, 0–63.
    pub step: u8,
    /// Track, 0–9.
    pub track: u8,
    /// Which parameter.
    pub param: LockParam,
    /// Its value.
    pub value: u8,
}

/// A pattern's parameter locks: a sparse store of at most [`LOCKS`],
/// sorted by step, track and parameter. A lock more than the store holds
/// is refused ([`LockStore::set`] returns false) and nothing changes.
#[derive(Clone, Copy, Debug)]
pub struct LockStore {
    n: u8,
    items: [Lock; LOCKS],
}

/// Equal when they hold the same locks (the free slots do not count).
impl PartialEq for LockStore {
    fn eq(&self, other: &Self) -> bool {
        self.items() == other.items()
    }
}

impl Eq for LockStore {}

impl LockStore {
    /// No locks.
    pub const EMPTY: LockStore =
        LockStore { n: 0, items: [Lock { step: 0, track: 0, param: LockParam::Tune, value: 0 }; LOCKS] };

    /// The locks, sorted.
    #[must_use]
    pub fn items(&self) -> &[Lock] {
        &self.items[..usize::from(self.n)]
    }

    /// Locks held.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.n as usize
    }

    /// None held.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Whether another lock would be refused.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.n as usize == LOCKS
    }

    fn key(step: u8, track: u8, param: LockParam) -> u32 {
        (u32::from(step) << 16) | (u32::from(track) << 8) | u32::from(param.code())
    }

    fn find(&self, step: u8, track: u8, param: LockParam) -> Result<usize, usize> {
        let k = Self::key(step, track, param);
        self.items().binary_search_by_key(&k, |l| Self::key(l.step, l.track, l.param))
    }

    /// The value locked for `param` of `track` at `step`.
    #[must_use]
    pub fn get(&self, step: usize, track: usize, param: LockParam) -> Option<u8> {
        let (s, t) = (u8::try_from(step).ok()?, u8::try_from(track).ok()?);
        self.find(s, t, param).ok().map(|i| self.items[i].value)
    }

    /// Whether `track`'s step `step` has any lock.
    #[must_use]
    pub fn any(&self, step: usize, track: usize) -> bool {
        self.items().iter().any(|l| usize::from(l.step) == step && usize::from(l.track) == track)
    }

    /// Lock `param` of `track` at `step` to `value` (clamped); false when
    /// the store is full (or the step, track or parameter is impossible).
    pub fn set(&mut self, step: usize, track: usize, param: LockParam, value: u8) -> bool {
        if step >= MAX_STEPS || !param.fits(track) {
            return false;
        }
        let (s, t) = (step as u8, track as u8);
        let value = param.clamp(value);
        match self.find(s, t, param) {
            Ok(i) => {
                self.items[i].value = value;
                true
            }
            Err(_) if self.is_full() => false,
            Err(i) => {
                let n = usize::from(self.n);
                self.items.copy_within(i..n, i + 1);
                self.items[i] = Lock { step: s, track: t, param, value };
                self.n += 1;
                true
            }
        }
    }

    /// Remove the lock of `param` of `track` at `step` (if any).
    pub fn clear(&mut self, step: usize, track: usize, param: LockParam) {
        let (Ok(s), Ok(t)) = (u8::try_from(step), u8::try_from(track)) else { return };
        if let Ok(i) = self.find(s, t, param) {
            let n = usize::from(self.n);
            self.items.copy_within(i + 1..n, i);
            self.n -= 1;
        }
    }

    /// Remove every lock.
    pub fn clear_all(&mut self) {
        self.n = 0;
    }
}

/// A pattern: its length, the steps of every track, its transpose (synth
/// tracks) and its parameter locks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pattern {
    len: u8,
    transpose: i8,
    steps: [[Step; MAX_STEPS]; TRACKS],
    synth: [[SynthStep; MAX_STEPS]; SYNTH_TRACKS],
    /// The parameter locks.
    pub locks: LockStore,
}

const fn synth_rows() -> [[SynthStep; MAX_STEPS]; SYNTH_TRACKS] {
    let mut r = [[SynthStep::OFF; MAX_STEPS]; SYNTH_TRACKS];
    let mut t = 0;
    while t < SYNTH_TRACKS {
        r[t] = [SynthStep::off(SYNTH_NOTES[t]); MAX_STEPS];
        t += 1;
    }
    r
}

impl Pattern {
    /// Sixteen empty steps.
    pub const EMPTY: Pattern = Pattern {
        len: PAGE as u8,
        transpose: 0,
        steps: [[Step::OFF; MAX_STEPS]; TRACKS],
        synth: synth_rows(),
        locks: LockStore::EMPTY,
    };

    /// Steps, 1–64.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Never: a pattern has at least one step (for clippy's `len` rule).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Whether no step is on and the length is the default (not written to
    /// the file).
    #[must_use]
    pub fn is_blank(&self) -> bool {
        // Loops, not a comparison with `Pattern::EMPTY`: that would put a
        // 2 KB image of it in read-only data.
        self.len() == PAGE && self.drums_blank() && self.synth_blank()
    }

    /// Whether the drum tracks hold only empty steps.
    #[must_use]
    pub fn drums_blank(&self) -> bool {
        self.steps.iter().all(|t| t.iter().all(|s| *s == Step::OFF))
    }

    /// Whether the synth tracks hold nothing but empty steps on their
    /// default notes, with no transpose and no locks.
    #[must_use]
    pub fn synth_blank(&self) -> bool {
        self.transpose == 0
            && self.locks.is_empty()
            && self.synth.iter().zip(SYNTH_NOTES).all(|(t, n)| t.iter().all(|s| *s == SynthStep::off(n)))
    }

    /// Empty: sixteen steps, all off, no transpose, no locks (in place).
    pub fn clear(&mut self) {
        self.len = PAGE as u8;
        self.transpose = 0;
        for t in &mut self.steps {
            t.fill(Step::OFF);
        }
        for (t, n) in self.synth.iter_mut().zip(SYNTH_NOTES) {
            t.fill(SynthStep::off(n));
        }
        self.locks.clear_all();
    }

    /// The step of drum track `track` at `step` (an empty step outside).
    #[must_use]
    pub fn step(&self, track: usize, step: usize) -> Step {
        self.steps.get(track).and_then(|t| t.get(step)).copied().unwrap_or(Step::OFF)
    }

    /// The steps of drum track `track` (all 64, also past the length).
    #[must_use]
    pub fn track(&self, track: usize) -> &[Step; MAX_STEPS] {
        &self.steps[track.min(TRACKS - 1)]
    }

    /// The step of synth track `st` (0–1) at `step` (an empty step
    /// outside).
    #[must_use]
    pub fn synth_step(&self, st: usize, step: usize) -> SynthStep {
        self.synth.get(st).and_then(|t| t.get(step)).copied().unwrap_or(SynthStep::OFF)
    }

    /// The steps of synth track `st` (all 64).
    #[must_use]
    pub fn synth_track(&self, st: usize) -> &[SynthStep; MAX_STEPS] {
        &self.synth[st.min(SYNTH_TRACKS - 1)]
    }

    /// Track `t`'s (0–9) step `step` as a drum step (a synth step's shared
    /// parameters).
    #[must_use]
    pub fn any_step(&self, t: usize, step: usize) -> Step {
        if t < TRACKS { self.step(t, step) } else { self.synth_step(t - TRACKS, step).common() }
    }

    /// Transpose of the synth tracks, semitones.
    #[must_use]
    pub const fn transpose(&self) -> i8 {
        self.transpose
    }

    /// Steps that are on in the pattern's length (every track).
    #[must_use]
    pub fn hits(&self) -> usize {
        let n = self.len();
        self.steps.iter().map(|t| t[..n].iter().filter(|s| s.on()).count()).sum::<usize>()
            + self.synth.iter().map(|t| t[..n].iter().filter(|s| s.on()).count()).sum::<usize>()
    }

    /// Steps of each drum track worth storing: the length, or more when a
    /// step past it is not empty (a shortened pattern keeps its steps).
    #[must_use]
    pub fn stored(&self) -> usize {
        let last = self
            .steps
            .iter()
            .filter_map(|t| t.iter().rposition(|s| *s != Step::OFF))
            .max()
            .map_or(0, |i| i + 1);
        last.max(self.len())
    }

    /// The same for the synth tracks.
    #[must_use]
    pub fn synth_stored(&self) -> usize {
        let last = self
            .synth
            .iter()
            .zip(SYNTH_NOTES)
            .filter_map(|(t, n)| t.iter().rposition(|s| *s != SynthStep::off(n)))
            .max()
            .map_or(0, |i| i + 1);
        last.max(self.len())
    }

    pub(crate) fn set_len_raw(&mut self, n: usize) {
        self.len = n.clamp(1, MAX_STEPS) as u8;
    }

    pub(crate) fn set_raw(&mut self, track: usize, step: usize, s: Step) {
        if let Some(x) = self.steps.get_mut(track).and_then(|t| t.get_mut(step)) {
            *x = s;
        }
    }

    pub(crate) fn set_synth_raw(&mut self, st: usize, step: usize, s: SynthStep) {
        if let Some(x) = self.synth.get_mut(st).and_then(|t| t.get_mut(step)) {
            *x = s;
        }
    }

    pub(crate) fn set_transpose_raw(&mut self, t: i8) {
        self.transpose = t.clamp(-TRANSPOSE_MAX, TRANSPOSE_MAX);
    }
}

/// A drum track: which kit note it plays, its level, mute, solo and kit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Track {
    /// The kit note (MIDI note number, 0–127).
    pub note: u8,
    /// Level, per cent (0–100): scales the velocities.
    pub level: u8,
    /// Muted.
    pub mute: bool,
    /// Soloed (while any track is soloed only soloed tracks sound).
    pub solo: bool,
    /// The kit it plays: 0 (A) or 1 (B).
    pub kit: u8,
}

impl Track {
    /// A track on `note` of kit A at the default level.
    #[must_use]
    pub const fn new(note: u8) -> Track {
        Track { note, level: LEVEL_DEFAULT, mute: false, solo: false, kit: 0 }
    }
}

/// The synth tracks' default sounds: the factory presets.
pub const SYNTH_PRESETS: [&str; SYNTH_TRACKS] = ["square-bass", "saw-lead"];

/// A synth track: its sound (a patch of `orpheus-synth`), level, sends,
/// mute and solo.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SynthTrack {
    /// The sound.
    pub patch: Patch,
    /// Its name (a factory preset's, or "custom").
    pub name: Name,
    /// Level, per cent (0–100): the engine part's volume.
    pub level: u8,
    /// Muted.
    pub mute: bool,
    /// Soloed.
    pub solo: bool,
    /// Send to the delay, per cent.
    pub delay: u8,
    /// Send to the reverb, per cent.
    pub reverb: u8,
}

impl SynthTrack {
    /// Synth track `st` as a new project has it.
    #[must_use]
    pub const fn new(st: usize) -> SynthTrack {
        let (patch, name) = if st == 0 {
            (presets::SQUARE_BASS, SYNTH_PRESETS[0])
        } else {
            (presets::SAW_LEAD, SYNTH_PRESETS[1])
        };
        SynthTrack {
            patch,
            name: name16(name),
            level: LEVEL_DEFAULT,
            mute: false,
            solo: false,
            delay: 0,
            reverb: 0,
        }
    }
}

/// A new project's probability seed.
pub const SEED_DEFAULT: u32 = 0x05E0_0001;

/// The default tracks: kick, snare, clap, closed hat, open hat, low tom,
/// high tom, rim (General MIDI notes, so every factory kit has most).
pub const DEFAULT_NOTES: [u8; TRACKS] = [36, 38, 39, 42, 46, 45, 50, 37];

/// One entry of the song: a pattern played `repeats` times.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SongEntry {
    /// Pattern index, 0–15.
    pub pattern: u8,
    /// Times it plays, 1–16.
    pub repeats: u8,
}

/// The song: a chain of patterns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Song {
    len: u8,
    entries: [SongEntry; SONG_LEN],
}

impl Song {
    /// No entries.
    pub const EMPTY: Song = Song { len: 0, entries: [SongEntry { pattern: 0, repeats: 1 }; SONG_LEN] };

    /// The entries.
    #[must_use]
    pub fn entries(&self) -> &[SongEntry] {
        &self.entries[..usize::from(self.len)]
    }

    /// Entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// No entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether another entry would be refused.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.len as usize == SONG_LEN
    }

    /// Append an entry (pattern and repeats clamped); false when full.
    pub fn push(&mut self, pattern: u8, repeats: u8) -> bool {
        self.insert(self.len(), pattern, repeats)
    }

    /// Insert an entry before entry `at` (at the end past it); false when
    /// full.
    pub fn insert(&mut self, at: usize, pattern: u8, repeats: u8) -> bool {
        if self.is_full() {
            return false;
        }
        let n = self.len();
        let at = at.min(n);
        self.entries.copy_within(at..n, at + 1);
        self.entries[at] =
            SongEntry { pattern: pattern.min(PATTERNS as u8 - 1), repeats: repeats.clamp(1, MAX_REPEATS) };
        self.len += 1;
        true
    }

    /// Change entry `i` (pattern and repeats clamped).
    pub fn set(&mut self, i: usize, pattern: u8, repeats: u8) {
        if i < self.len() {
            self.entries[i] = SongEntry {
                pattern: pattern.min(PATTERNS as u8 - 1),
                repeats: repeats.clamp(1, MAX_REPEATS),
            };
        }
    }

    /// Remove entry `i`.
    pub fn remove(&mut self, i: usize) {
        let n = self.len();
        if i < n {
            self.entries.copy_within(i + 1..n, i);
            self.entries[n - 1] = SongEntry { pattern: 0, repeats: 1 };
            self.len -= 1;
        }
    }

    /// Swap entry `i` with the one after it.
    pub fn move_down(&mut self, i: usize) {
        if i + 1 < self.len() {
            self.entries.swap(i, i + 1);
        }
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.len = 0;
        self.entries.fill(SongEntry { pattern: 0, repeats: 1 });
    }
}

/// How the transport walks through the patterns.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PlayMode {
    /// The current pattern loops; a pattern chosen while playing starts at
    /// the end of the current one.
    #[default]
    Pattern,
    /// The song's chain, from its first entry, round again at its end (an
    /// empty song plays the current pattern).
    Song,
}

/// A whole project.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    name: Name,
    tempo: u16,
    swing: u8,
    /// The pattern being edited (and played in pattern mode).
    current: u8,
    mode: PlayMode,
    /// Seed of the probability draws (a saved song renders the same).
    pub seed: u32,
    /// The kits the drum tracks play (A, B), and their names.
    kits: [Kit; KITS],
    kit_names: [Name; KITS],
    tracks: [Track; TRACKS],
    synth: [SynthTrack; SYNTH_TRACKS],
    scale: Scale,
    root: u8,
    fill: u8,
    /// The drum kits' sends to the delay and the reverb, per cent.
    drum_sends: [u8; 2],
    patterns: [Pattern; PATTERNS],
    /// The song.
    pub song: Song,
    rev: u32,
    kit_rev: u32,
    sound_rev: u32,
}

impl Default for Project {
    fn default() -> Self {
        Self::NEW
    }
}

const fn name16(s: &str) -> Name {
    let mut n = [0u8; NAME_LEN];
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && i < NAME_LEN {
        n[i] = b[i];
        i += 1;
    }
    Name(n)
}

const fn default_tracks() -> [Track; TRACKS] {
    let mut t = [Track::new(0); TRACKS];
    let mut i = 0;
    while i < TRACKS {
        t[i] = Track::new(DEFAULT_NOTES[i]);
        i += 1;
    }
    t
}

/// Kit B of a new project: the analog kit (factory kit 1).
pub const KIT_B_DEFAULT: usize = 1;

impl Project {
    /// A new, empty project: 120 BPM, straight, the standard kit (A) and
    /// the analog kit (B), the default tracks, the bass and lead presets,
    /// chromatic, 16 empty patterns of 16 steps.
    pub const NEW: Project = Project {
        name: Project::DEFAULT_NAME,
        tempo: TEMPO_DEFAULT,
        swing: SWING_MIN,
        current: 0,
        mode: PlayMode::Pattern,
        seed: SEED_DEFAULT,
        kits: [Kit::new(kit::STANDARD), Kit::new(kit::FACTORY[KIT_B_DEFAULT].1)],
        kit_names: [name16("standard"), name16(kit::FACTORY[KIT_B_DEFAULT].0)],
        tracks: default_tracks(),
        synth: [SynthTrack::new(0), SynthTrack::new(1)],
        scale: Scale::Chromatic,
        root: 0,
        fill: NO_FILL,
        drum_sends: [0, 0],
        patterns: [Pattern::EMPTY; PATTERNS],
        song: Song::EMPTY,
        rev: 0,
        kit_rev: 0,
        sound_rev: 0,
    };

    /// Back to [`Project::NEW`] in place, keeping the revision counters
    /// moving (so a redraw and the player's kits and sounds follow). Field
    /// by field: `*self = Project::NEW` would copy a 35 KB image of the
    /// constant from read-only data (on the player, flash code pages).
    pub fn reset(&mut self) {
        self.name = Project::DEFAULT_NAME;
        self.tempo = TEMPO_DEFAULT;
        self.swing = SWING_MIN;
        self.current = 0;
        self.mode = PlayMode::Pattern;
        self.seed = SEED_DEFAULT;
        self.kits = [Kit::new(kit::STANDARD), Kit::new(kit::FACTORY[KIT_B_DEFAULT].1)];
        self.kit_names = [name16("standard"), name16(kit::FACTORY[KIT_B_DEFAULT].0)];
        for (t, n) in self.tracks.iter_mut().zip(DEFAULT_NOTES) {
            *t = Track::new(n);
        }
        for (i, s) in self.synth.iter_mut().enumerate() {
            *s = SynthTrack::new(i);
        }
        self.scale = Scale::Chromatic;
        self.root = 0;
        self.fill = NO_FILL;
        self.drum_sends = [0, 0];
        for p in &mut self.patterns {
            p.clear();
        }
        self.song.clear();
        self.rev = self.rev.wrapping_add(1);
        self.kit_rev = self.kit_rev.wrapping_add(1);
        self.sound_rev = self.sound_rev.wrapping_add(1);
    }

    /// The project's name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Rename (cut to 16 bytes at a character boundary).
    pub fn set_name(&mut self, s: &str) {
        self.name = Name::new(s);
        self.touch();
    }

    /// A new project's name.
    pub const DEFAULT_NAME: Name = name16("Beat");

    pub(crate) fn name_raw(&self) -> Name {
        self.name
    }

    pub(crate) fn set_name_raw(&mut self, n: Name) {
        self.name = n;
    }

    /// Changes so far (wraps): compare with a saved value to see edits.
    #[must_use]
    pub const fn rev(&self) -> u32 {
        self.rev
    }

    /// Kit changes so far: the player installs the kits when it moves.
    #[must_use]
    pub const fn kit_rev(&self) -> u32 {
        self.kit_rev
    }

    /// Synth sound changes so far (patches, levels, sends): the player
    /// installs them when it moves.
    #[must_use]
    pub const fn sound_rev(&self) -> u32 {
        self.sound_rev
    }

    /// Zero the revision counters (they are not part of what a project
    /// is: equality after a round trip compares the rest).
    pub fn normalize_revs(&mut self) {
        self.rev = 0;
        self.kit_rev = 0;
        self.sound_rev = 0;
    }

    /// Whether `self` and `other` hold the same project (revision counters
    /// aside).
    #[must_use]
    pub fn same_as(&self, other: &Project) -> bool {
        self.name == other.name
            && self.tempo == other.tempo
            && self.swing == other.swing
            && self.current == other.current
            && self.mode == other.mode
            && self.seed == other.seed
            && self.kits == other.kits
            && self.kit_names == other.kit_names
            && self.tracks == other.tracks
            && self.synth.iter().zip(&other.synth).all(|(a, b)| a.same_as(b))
            && self.scale == other.scale
            && self.root == other.root
            && self.fill == other.fill
            && self.drum_sends == other.drum_sends
            && self.patterns == other.patterns
            && self.song == other.song
    }

    fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    fn touch_sound(&mut self) {
        self.sound_rev = self.sound_rev.wrapping_add(1);
        self.touch();
    }

    /// How the transport plays.
    #[must_use]
    pub const fn mode(&self) -> PlayMode {
        self.mode
    }

    /// Set the play mode (taken at the end of the playing pattern).
    pub fn set_mode(&mut self, m: PlayMode) {
        self.mode = m;
        self.touch();
    }

    /// Tempo, hundredths of a BPM.
    #[must_use]
    pub const fn tempo(&self) -> u16 {
        self.tempo
    }

    /// Set the tempo (hundredths of a BPM, clamped to 40–240 BPM). The
    /// player takes it at the next step.
    pub fn set_tempo(&mut self, t: u16) {
        self.tempo = t.clamp(TEMPO_MIN, TEMPO_MAX);
        self.touch();
    }

    /// Swing, per cent (50–75).
    #[must_use]
    pub const fn swing(&self) -> u8 {
        self.swing
    }

    /// Set the swing (clamped to 50–75 %).
    pub fn set_swing(&mut self, s: u8) {
        self.swing = s.clamp(SWING_MIN, SWING_MAX);
        self.touch();
    }

    /// The pattern being edited.
    #[must_use]
    pub const fn current(&self) -> usize {
        self.current as usize
    }

    /// Choose the pattern to edit (clamped to 0–15).
    pub fn set_current(&mut self, p: usize) {
        self.current = p.min(PATTERNS - 1) as u8;
        self.touch();
    }

    /// Pattern `i` (the last for an index past the end).
    #[must_use]
    pub fn pattern(&self, i: usize) -> &Pattern {
        &self.patterns[i.min(PATTERNS - 1)]
    }

    /// Drum track `t` (the last drum track for a larger index).
    #[must_use]
    pub fn track(&self, t: usize) -> &Track {
        &self.tracks[t.min(TRACKS - 1)]
    }

    /// The drum tracks.
    #[must_use]
    pub fn tracks(&self) -> &[Track; TRACKS] {
        &self.tracks
    }

    /// Synth track `st` (0 bass, 1 lead).
    #[must_use]
    pub fn synth(&self, st: usize) -> &SynthTrack {
        &self.synth[st.min(SYNTH_TRACKS - 1)]
    }

    /// Track `t`'s (0–9) mute, solo and level.
    #[must_use]
    pub fn mix(&self, t: usize) -> (bool, bool, u8) {
        if t < TRACKS {
            let x = &self.tracks[t];
            (x.mute, x.solo, x.level)
        } else {
            let x = self.synth(t - TRACKS);
            (x.mute, x.solo, x.level)
        }
    }

    /// Whether track `t` (0–9) sounds: not muted, and soloed if any track
    /// is.
    #[must_use]
    pub fn audible(&self, t: usize) -> bool {
        let (mute, solo, _) = self.mix(t.min(ALL_TRACKS - 1));
        let any = self.tracks.iter().any(|x| x.solo) || self.synth.iter().any(|x| x.solo);
        !mute && (solo || !any)
    }

    /// Set a drum step.
    pub fn set_step(&mut self, pattern: usize, track: usize, step: usize, s: Step) {
        if pattern < PATTERNS {
            self.patterns[pattern].set_raw(track, step, s);
            self.touch();
        }
    }

    /// Set a synth step of synth track `st` (0–1).
    pub fn set_synth_step(&mut self, pattern: usize, st: usize, step: usize, s: SynthStep) {
        if pattern < PATTERNS {
            self.patterns[pattern].set_synth_raw(st, step, s);
            self.touch();
        }
    }

    /// Set the shared parameters of track `t`'s (0–9) step.
    pub fn set_any_step(&mut self, pattern: usize, t: usize, step: usize, s: Step) {
        if t < TRACKS {
            self.set_step(pattern, t, step, s);
        } else {
            let x = self.pattern(pattern).synth_step(t - TRACKS, step).with_common(s);
            self.set_synth_step(pattern, t - TRACKS, step, x);
        }
    }

    /// Turn track `t`'s (0–9) step on or off (its parameters kept);
    /// returns the new state.
    pub fn toggle(&mut self, pattern: usize, t: usize, step: usize) -> bool {
        let s = self.pattern(pattern).any_step(t, step);
        let on = !s.on();
        self.set_any_step(pattern, t, step, s.with_on(on));
        on
    }

    /// Set a pattern's length (clamped to 1–64). Steps past it are kept
    /// (a longer length brings them back).
    pub fn set_len(&mut self, pattern: usize, n: usize) {
        if pattern < PATTERNS {
            self.patterns[pattern].set_len_raw(n);
            self.touch();
        }
    }

    /// Set a pattern's transpose (synth tracks; clamped to ±24 semitones).
    pub fn set_transpose(&mut self, pattern: usize, t: i8) {
        if pattern < PATTERNS {
            self.patterns[pattern].set_transpose_raw(t);
            self.touch();
        }
    }

    /// Lock `param` of track `t`'s step in `pattern` to `value`; false
    /// (and nothing changed) when the pattern's lock store is full.
    pub fn set_lock(&mut self, pattern: usize, step: usize, t: usize, param: LockParam, value: u8) -> bool {
        let ok = pattern < PATTERNS && self.patterns[pattern].locks.set(step, t, param, value);
        if ok {
            self.touch();
        }
        ok
    }

    /// Remove a lock.
    pub fn clear_lock(&mut self, pattern: usize, step: usize, t: usize, param: LockParam) {
        if pattern < PATTERNS {
            self.patterns[pattern].locks.clear(step, t, param);
            self.touch();
        }
    }

    /// Empty a pattern (its length kept).
    pub fn clear_pattern(&mut self, pattern: usize) {
        if pattern < PATTERNS {
            let len = self.patterns[pattern].len();
            self.patterns[pattern].clear();
            self.patterns[pattern].set_len_raw(len);
            self.touch();
        }
    }

    /// Copy pattern `from` over pattern `to`.
    pub fn copy_pattern(&mut self, from: usize, to: usize) {
        if from < PATTERNS && to < PATTERNS {
            self.patterns[to] = self.patterns[from];
            self.touch();
        }
    }

    /// Mute or unmute track `t` (0–9; at the next step).
    pub fn set_mute(&mut self, t: usize, on: bool) {
        if t < TRACKS {
            self.tracks[t].mute = on;
        } else if t < ALL_TRACKS {
            self.synth[t - TRACKS].mute = on;
        } else {
            return;
        }
        self.touch();
    }

    /// Solo track `t` (0–9) or take its solo away (at the next step).
    pub fn set_solo(&mut self, t: usize, on: bool) {
        if t < TRACKS {
            self.tracks[t].solo = on;
        } else if t < ALL_TRACKS {
            self.synth[t - TRACKS].solo = on;
        } else {
            return;
        }
        self.touch();
    }

    /// Set track `t`'s (0–9) level (clamped to 0–100 %).
    pub fn set_level(&mut self, t: usize, level: u8) {
        if t < TRACKS {
            self.tracks[t].level = level.min(100);
            self.touch();
        } else if t < ALL_TRACKS {
            self.synth[t - TRACKS].level = level.min(100);
            self.touch_sound();
        }
    }

    /// Set the kit note drum track `t` plays (clamped to 0–127).
    pub fn set_note(&mut self, t: usize, note: u8) {
        if let Some(x) = self.tracks.get_mut(t) {
            x.note = note.min(127);
            self.touch();
        }
    }

    /// Make drum track `t` play kit A (0) or B (1).
    pub fn set_track_kit(&mut self, t: usize, k: u8) {
        if let Some(x) = self.tracks.get_mut(t) {
            x.kit = k.min(KITS as u8 - 1);
            self.touch();
        }
    }

    /// Kit A.
    #[must_use]
    pub fn kit(&self) -> &Kit {
        &self.kits[0]
    }

    /// Kit A's name.
    #[must_use]
    pub fn kit_name(&self) -> &str {
        self.kit_names[0].as_str()
    }

    /// Kit `k` (0 A, 1 B).
    #[must_use]
    pub fn kit_of(&self, k: usize) -> &Kit {
        &self.kits[k.min(KITS - 1)]
    }

    /// Kit `k`'s name.
    #[must_use]
    pub fn kit_name_of(&self, k: usize) -> &str {
        self.kit_names[k.min(KITS - 1)].as_str()
    }

    /// The kit entry drum track `t` plays (its note in its kit).
    #[must_use]
    pub fn entry(&self, t: usize) -> Option<&KitEntry> {
        let tr = self.track(t);
        self.kit_of(usize::from(tr.kit)).find(tr.note)
    }

    /// Use `kit` named `name` as kit A; the player installs it at its next
    /// block.
    pub fn set_kit(&mut self, name: &str, kit: &Kit) {
        self.set_kit_of(0, name, kit);
    }

    /// Use `kit` named `name` as kit `k`.
    pub fn set_kit_of(&mut self, k: usize, name: &str, kit: &Kit) {
        let k = k.min(KITS - 1);
        self.kits[k] = *kit;
        self.kit_names[k] = Name::new(name);
        self.kit_rev = self.kit_rev.wrapping_add(1);
        self.touch();
    }

    /// Change drum track `t`'s kit entry (its note in its kit; the sound
    /// screen): `f` edits a copy, which replaces it. Nothing when the kit
    /// has no entry for the note.
    pub fn edit_entry(&mut self, t: usize, f: impl FnOnce(&mut KitEntry)) {
        let tr = *self.track(t);
        let k = usize::from(tr.kit).min(KITS - 1);
        let mut entries = [KitEntry::drum(0, orpheus_synth::DrumKind::Kick); kit::KIT_ENTRIES];
        let src = self.kits[k].entries();
        let n = src.len();
        entries[..n].copy_from_slice(src);
        let Some(e) = entries[..n].iter_mut().find(|e| e.note == tr.note) else { return };
        f(e);
        self.kits[k] = Kit::new(&entries[..n]);
        self.kit_rev = self.kit_rev.wrapping_add(1);
        self.touch();
    }

    /// Use factory kit `i` ([`orpheus_synth::kit::FACTORY`], wrapping) as
    /// kit A.
    pub fn set_factory_kit(&mut self, i: usize) {
        self.set_factory_kit_of(0, i);
    }

    /// Use factory kit `i` as kit `k`.
    pub fn set_factory_kit_of(&mut self, k: usize, i: usize) {
        let (name, entries) = kit::FACTORY[i % kit::FACTORY.len()];
        self.set_kit_of(k, name, &Kit::new(entries));
    }

    /// The factory kit kit A is, by name.
    #[must_use]
    pub fn factory_kit(&self) -> Option<usize> {
        self.factory_kit_of(0)
    }

    /// The factory kit kit `k` is, by name.
    #[must_use]
    pub fn factory_kit_of(&self, k: usize) -> Option<usize> {
        kit::FACTORY.iter().position(|(n, _)| *n == self.kit_name_of(k))
    }

    /// Give synth track `st` the patch `patch` named `name`.
    pub fn set_patch(&mut self, st: usize, name: &str, patch: &Patch) {
        if let Some(s) = self.synth.get_mut(st) {
            s.patch = patch.sanitized();
            s.name = Name::new(name);
            self.touch_sound();
        }
    }

    /// Change synth track `st`'s patch: `f` edits it (then sanitised; the
    /// name becomes "custom" unless `f` keeps a preset's values).
    pub fn edit_patch(&mut self, st: usize, f: impl FnOnce(&mut Patch)) {
        if let Some(s) = self.synth.get_mut(st) {
            let mut p = s.patch;
            f(&mut p);
            s.patch = p.sanitized();
            if presets::MELODIC.iter().all(|(_, x)| *x != s.patch) {
                s.name = name16("custom");
            }
            self.touch_sound();
        }
    }

    /// Give synth track `st` factory preset `i` ([`presets::MELODIC`],
    /// wrapping).
    pub fn set_preset(&mut self, st: usize, i: usize) {
        let (name, patch) = presets::MELODIC[i % presets::MELODIC.len()];
        self.set_patch(st, name, &patch);
    }

    /// The factory preset synth track `st` plays, by name.
    #[must_use]
    pub fn preset(&self, st: usize) -> Option<usize> {
        presets::MELODIC.iter().position(|(n, _)| *n == self.synth(st).name.as_str())
    }

    /// Set synth track `st`'s sends (per cent, clamped).
    pub fn set_synth_sends(&mut self, st: usize, delay: u8, reverb: u8) {
        if let Some(s) = self.synth.get_mut(st) {
            s.delay = delay.min(100);
            s.reverb = reverb.min(100);
            self.touch_sound();
        }
    }

    /// The drum kits' sends (delay, reverb), per cent.
    #[must_use]
    pub const fn drum_sends(&self) -> [u8; 2] {
        self.drum_sends
    }

    /// Set the drum kits' sends (per cent, clamped).
    pub fn set_drum_sends(&mut self, delay: u8, reverb: u8) {
        self.drum_sends = [delay.min(100), reverb.min(100)];
        self.touch_sound();
    }

    /// The scale lock and its root (0 = C … 11 = B).
    #[must_use]
    pub const fn scale(&self) -> (Scale, u8) {
        (self.scale, self.root)
    }

    /// Set the scale lock (the root wraps to 0–11). Steps already entered
    /// keep their notes; chords follow the scale.
    pub fn set_scale(&mut self, s: Scale, root: u8) {
        self.scale = s;
        self.root = root % 12;
        self.touch();
    }

    /// The fill pattern (played while the fill key is held), if any.
    #[must_use]
    pub fn fill(&self) -> Option<usize> {
        (usize::from(self.fill) < PATTERNS).then_some(usize::from(self.fill))
    }

    /// Choose the fill pattern (`None`: no fill).
    pub fn set_fill(&mut self, p: Option<usize>) {
        self.fill = match p {
            Some(i) if i < PATTERNS => i as u8,
            _ => NO_FILL,
        };
        self.touch();
    }

    /// The pattern each step of the song plays, in order, as the player
    /// walks it (`n` patterns from the start); for tests and the screen.
    pub fn song_walk(&self, n: usize, mut out: impl FnMut(usize)) {
        let mut done = 0;
        if self.song.is_empty() {
            return;
        }
        'outer: loop {
            for e in self.song.entries() {
                for _ in 0..e.repeats {
                    if done == n {
                        break 'outer;
                    }
                    out(usize::from(e.pattern));
                    done += 1;
                }
            }
        }
    }

    /// Edit the song (any change; bumps the revision).
    pub fn edit_song(&mut self, f: impl FnOnce(&mut Song)) {
        f(&mut self.song);
        self.touch();
    }

    // ------------------------------------------------- file decoding hooks

    pub(crate) fn set_kit_raw(&mut self, k: usize, name: Name, kit: Kit) {
        let k = k.min(KITS - 1);
        self.kits[k] = kit;
        self.kit_names[k] = name;
        self.kit_rev = self.kit_rev.wrapping_add(1);
    }

    pub(crate) fn pattern_mut(&mut self, i: usize) -> &mut Pattern {
        &mut self.patterns[i.min(PATTERNS - 1)]
    }

    pub(crate) fn track_mut(&mut self, t: usize) -> &mut Track {
        &mut self.tracks[t.min(TRACKS - 1)]
    }

    pub(crate) fn synth_mut(&mut self, st: usize) -> &mut SynthTrack {
        self.sound_rev = self.sound_rev.wrapping_add(1);
        &mut self.synth[st.min(SYNTH_TRACKS - 1)]
    }

    pub(crate) fn set_raw(&mut self, tempo: u16, swing: u8, current: u8) {
        self.tempo = tempo.clamp(TEMPO_MIN, TEMPO_MAX);
        self.swing = swing.clamp(SWING_MIN, SWING_MAX);
        self.current = current.min(PATTERNS as u8 - 1);
    }

    pub(crate) fn set_extra_raw(&mut self, scale: u8, root: u8, fill: u8, sends: [u8; 2]) {
        self.scale = Scale::from_code(scale);
        self.root = root % 12;
        self.fill = if usize::from(fill) < PATTERNS { fill } else { NO_FILL };
        self.drum_sends = [sends[0].min(100), sends[1].min(100)];
    }

    pub(crate) fn fill_raw(&self) -> u8 {
        self.fill
    }
}

impl SynthTrack {
    /// Whether `self` and `other` are the same (patches compared bit for
    /// bit: a round trip keeps every `f32` exactly).
    #[must_use]
    pub fn same_as(&self, other: &SynthTrack) -> bool {
        self.name == other.name
            && self.level == other.level
            && self.mute == other.mute
            && self.solo == other.solo
            && self.delay == other.delay
            && self.reverb == other.reverb
            && patch_bits_eq(&self.patch, &other.patch)
    }
}

/// Whether two patches are equal bit for bit (their bank encodings).
#[must_use]
pub fn patch_bits_eq(a: &Patch, b: &Patch) -> bool {
    let mut x = [0u8; orpheus_synth::bank::PATCH_MAX];
    let mut y = [0u8; orpheus_synth::bank::PATCH_MAX];
    match (orpheus_synth::bank::encode_patch(a, &mut x), orpheus_synth::bank::encode_patch(b, &mut y)) {
        (Ok(n), Ok(m)) => x[..n] == y[..m],
        _ => false,
    }
}

/// A short label for a kit note (two letters, as drum machines print
/// them) and a longer one for the header.
#[must_use]
pub fn note_label(note: u8) -> (&'static str, &'static str) {
    match note {
        35 | 36 => ("BD", "Kick"),
        37 => ("RS", "Rim"),
        38 | 40 => ("SD", "Snare"),
        39 => ("CP", "Clap"),
        41 | 43 => ("LT", "Low tom"),
        45 | 47 => ("MT", "Mid tom"),
        48 | 50 => ("HT", "High tom"),
        42 => ("CH", "Closed hat"),
        44 => ("PH", "Pedal hat"),
        46 => ("OH", "Open hat"),
        49 | 52 | 55 | 57 => ("CY", "Crash"),
        51 | 53 | 59 => ("RD", "Ride"),
        54 => ("TB", "Tambourine"),
        56 | 67 | 68 => ("CB", "Cowbell"),
        60 | 61 => ("BG", "Bongo"),
        62..=64 => ("CG", "Conga"),
        69 | 70 => ("SH", "Shaker"),
        75..=77 => ("CL", "Clave"),
        _ => ("--", "Sound"),
    }
}

/// Labels of the synth tracks: ("BS", "Bass"), ("LD", "Lead").
pub const SYNTH_LABELS: [(&str, &str); SYNTH_TRACKS] = [("BS", "Bass"), ("LD", "Lead")];

/// Track `t`'s (0–9) short and long label.
#[must_use]
pub fn track_label(p: &Project, t: usize) -> (&'static str, &'static str) {
    if t < TRACKS { note_label(p.track(t).note) } else { SYNTH_LABELS[(t - TRACKS).min(SYNTH_TRACKS - 1)] }
}
