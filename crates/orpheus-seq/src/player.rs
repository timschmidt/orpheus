//! The step player: walks a [`Project`] and drives an
//! [`orpheus_synth::Engine`] with every hit on its exact sample.
//!
//! # Timing
//!
//! Time is counted in *units*: a pair of nominal 1/16 steps is 57,600
//! units. A step at local index `i` of its pattern lasts `576 · swing`
//! units when `i` is even and `576 · (100 − swing)` when odd (swing 50:
//! 28,800 each); a micro-timing delay of `m`/24 step is `1,200 · m`; the
//! `j`-th of `r` ratchet hits lies `j · duration / r` into the step (576
//! is divisible by 12, so every one of these is a whole number). Units
//! become frames with
//!
//! ```text
//! frame = anchor + ⌊units · rate · 5 / (tempo · 96)⌋      (tempo in 1/100 BPM)
//! ```
//!
//! (a pair is `rate · 3000 / tempo` frames), where `units` counts from the
//! anchor: the start of the play, re-set to the start of a step when the
//! tempo changes. Every step and hit lands on the floor of its exact time
//! however long the song plays; nothing accumulates rounding.
//!
//! # Edits while playing
//!
//! A step is *latched* (its tracks read, its hits computed) on the sample
//! where it starts. Its hits all lie in or after it (micro-timing only
//! delays), so everything the project says at that moment is what plays:
//! an edit of the project between two render calls takes effect at the
//! first step that starts after it — the tempo, swing, a step, mute, solo,
//! a track's level or note, a pattern's length (a step past a shortened
//! length ends the pattern), the pattern chosen to play next (pattern
//! mode: [`Project::current`] at the end of the pattern) and the song.
//! A kit change is installed between render calls.
//!
//! # Rendering
//!
//! [`Sequencer::render_i16`] / [`Sequencer::render_f32`] fill an
//! interleaved stereo block: the block is cut at every step start (to
//! latch it) and when [`EVENTS`] hits are due, and each piece goes to the
//! engine with its hits as note-on events at their offsets. The engine's
//! output does not depend on block sizes, nor does the step clock, so a
//! project renders the same samples in any block size — on the screen's
//! clock or with the screen off, where nothing but these calls run (the
//! firmware's synth task calls `render_i16` into the DAC ring; the UI
//! only edits the project and reads [`Sequencer::position_at`]).

use orpheus_synth::{Engine, Event, Patch};

use crate::model::{ACCENT, PATTERNS, PROB_LEVELS, PlayMode, Project, TRACKS};

/// Units of a pair of nominal steps.
pub const UNITS_PER_PAIR: u64 = 57_600;
/// Units of a nominal (unswung) step.
pub const UNITS_PER_STEP: u64 = UNITS_PER_PAIR / 2;
/// Units of one micro-timing step (1/24 of a nominal step).
pub const UNITS_PER_MICRO: u64 = UNITS_PER_STEP / 24;
/// Most note-on events handed to the engine per render call (a block is
/// cut when more are due).
pub const EVENTS: usize = 64;
/// Hits computed but not yet played (a step makes at most 8 × 4; a step's
/// late hits can still be waiting when the next two have been latched).
pub const PENDING: usize = 96;
/// Latched steps remembered for [`Sequencer::position_at`].
pub const HISTORY: usize = 16;
/// The engine part the drum tracks play on (one part, so the kit's choke
/// groups work across tracks: a closed hat cuts the open one).
pub const PART: u8 = 0;
/// The engine's kit slot the project's kit is installed in.
pub const KIT_SLOT: u8 = 0;

/// Length of the step at local index `local` with `swing` per cent, units.
#[must_use]
pub const fn step_units(swing: u8, local: usize) -> u64 {
    if local % 2 == 0 { 576 * swing as u64 } else { 576 * (100 - swing as u64) }
}

/// `units` from the anchor as frames at `rate` and `tempo` (1/100 BPM),
/// rounded down.
#[must_use]
pub fn units_to_frames(units: u64, rate: u32, tempo: u16) -> u64 {
    let n = u128::from(units) * u128::from(rate) * 5;
    let d = u128::from(tempo.max(1)) * 96;
    u64::try_from(n / d).unwrap_or(u64::MAX)
}

/// The velocity a step plays with: its own, raised by the accent, scaled
/// by the track's level (0: silent).
#[must_use]
pub fn hit_velocity(velocity: u8, accent: bool, level: u8) -> u8 {
    let v = u32::from(if accent { velocity.saturating_add(ACCENT).min(127) } else { velocity.min(127) });
    if level == 0 {
        return 0;
    }
    ((v * u32::from(level.min(100)) + 50) / 100).clamp(1, 127) as u8
}

/// Whether the probability draw of step number `count` (since the start
/// of the play) on track `track` passes at level `prob` (1–16): a pure
/// function of the seed, so a saved song renders the same.
#[must_use]
pub fn chance(seed: u32, count: u64, track: usize, prob: u8) -> bool {
    if prob >= PROB_LEVELS {
        return true;
    }
    let mut z = (u64::from(seed) << 32) ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (track as u64 + 1);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z & 0xF) as u8) < prob
}

/// One hit waiting for its sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Hit {
    /// Frame on the player's clock.
    pub frame: u64,
    /// Kit note.
    pub note: u8,
    /// Velocity, 1–127.
    pub velocity: u8,
    /// Track.
    pub track: u8,
}

/// Where the player is: a latched step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Position {
    /// Pattern.
    pub pattern: u8,
    /// Step in the pattern.
    pub step: u8,
    /// Song entry (song mode).
    pub entry: u8,
    /// Repeat of the entry (song mode), from 0.
    pub repeat: u8,
    /// The step's first frame on the player's clock.
    pub frame: u64,
}

/// The step player (transport, step clock, hit queue). About 2.2 KiB.
#[derive(Clone, Debug)]
pub struct Sequencer {
    rate: u32,
    playing: bool,
    starting: bool,
    clock: u64,
    // The next step to latch.
    pattern: u8,
    step: u8,
    entry: u8,
    repeat: u8,
    // Step clock.
    tempo: u16,
    anchor: u64,
    units: u64,
    next: u64,
    count: u64,
    // Hits computed, sorted by frame (stable: track order, then ratchet).
    pending: [Hit; PENDING],
    n: usize,
    dropped: u32,
    history: [Position; HISTORY],
    latched: u64,
    kit_rev: Option<u32>,
    hits: u64,
}

impl Sequencer {
    /// A stopped player at `rate` Hz (the engine's rate).
    #[must_use]
    pub const fn new(rate: u32) -> Self {
        Sequencer {
            rate,
            playing: false,
            starting: false,
            clock: 0,
            pattern: 0,
            step: 0,
            entry: 0,
            repeat: 0,
            tempo: crate::model::TEMPO_DEFAULT,
            anchor: 0,
            units: 0,
            next: 0,
            count: 0,
            pending: [Hit { frame: 0, note: 0, velocity: 0, track: 0 }; PENDING],
            n: 0,
            dropped: 0,
            history: [Position { pattern: 0, step: 0, entry: 0, repeat: 0, frame: 0 }; HISTORY],
            latched: 0,
            kit_rev: None,
            hits: 0,
        }
    }

    /// Sample rate.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    /// Start playing at the first frame of the next render call: pattern
    /// mode from the start of [`Project::current`], song mode from the
    /// song's first entry.
    pub fn play(&mut self) {
        self.playing = true;
        self.starting = true;
        self.n = 0;
    }

    /// Stop: no more hits (sounding drums ring out).
    pub fn stop(&mut self) {
        self.playing = false;
        self.starting = false;
        self.n = 0;
    }

    /// Playing (or about to start).
    #[must_use]
    pub const fn playing(&self) -> bool {
        self.playing
    }

    /// Frames rendered through the player since it was made.
    #[must_use]
    pub const fn clock(&self) -> u64 {
        self.clock
    }

    /// Steps latched since the play started.
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.count
    }

    /// Hits handed to the engine since the player was made.
    #[must_use]
    pub const fn hits(&self) -> u64 {
        self.hits
    }

    /// Hits dropped because the queue was full (never in a normal play:
    /// only a jump to a much faster tempo while late hits wait).
    #[must_use]
    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    /// The frame the next step starts on (while playing).
    #[must_use]
    pub const fn next_step_frame(&self) -> u64 {
        self.next
    }

    /// The step playing at the end of what was rendered.
    #[must_use]
    pub fn position(&self) -> Option<Position> {
        self.position_at(self.clock.saturating_sub(1))
    }

    /// The step playing at `frame` of the player's clock (the DAC's frame,
    /// i.e. [`Sequencer::clock`] less the output latency), if it is still
    /// remembered: the playhead and the audio come from one clock.
    #[must_use]
    pub fn position_at(&self, frame: u64) -> Option<Position> {
        if !self.playing || self.starting {
            return None;
        }
        let n = self.latched.min(HISTORY as u64) as usize;
        (0..n)
            .map(|k| self.history[((self.latched - 1 - k as u64) % HISTORY as u64) as usize])
            .find(|p| p.frame <= frame)
    }

    /// Render interleaved stereo `i16` (an odd last sample is set to 0).
    pub fn render_i16(&mut self, p: &Project, eng: &mut Engine, out: &mut [i16]) {
        let frames = out.len() / 2;
        if out.len() % 2 == 1 {
            out[out.len() - 1] = 0;
        }
        self.run(p, eng, frames, |eng, a, b, ev| eng.render_i16(&mut out[2 * a..2 * b], ev));
    }

    /// Render interleaved stereo `f32` (an odd last sample is set to 0).
    pub fn render_f32(&mut self, p: &Project, eng: &mut Engine, out: &mut [f32]) {
        let frames = out.len() / 2;
        if out.len() % 2 == 1 {
            out[out.len() - 1] = 0.0;
        }
        self.run(p, eng, frames, |eng, a, b, ev| eng.render_f32(&mut out[2 * a..2 * b], ev));
    }

    /// Advance `frames` without an engine: the hits that would play, in
    /// order, go to `hit` (tests, the MIDI export of SEQ-2, a dry run).
    pub fn run_dry(&mut self, p: &Project, frames: usize, mut hit: impl FnMut(Hit)) {
        let mut pos = 0usize;
        while pos < frames {
            let end = self.segment(p, pos, frames);
            let take = self.due(end);
            for h in &self.pending[..take] {
                hit(*h);
            }
            self.consume(take);
            pos = end;
        }
        self.clock += frames as u64;
    }

    fn run(
        &mut self,
        p: &Project,
        eng: &mut Engine,
        frames: usize,
        mut render: impl FnMut(&mut Engine, usize, usize, &[Event]),
    ) {
        if self.kit_rev != Some(p.kit_rev()) {
            eng.set_kit(KIT_SLOT, p.kit());
            if self.kit_rev.is_none() {
                eng.set_patch(PART, &Patch::kit(KIT_SLOT));
            }
            self.kit_rev = Some(p.kit_rev());
        }
        let mut events = [Event::note_on(0, PART, 0, 0); EVENTS];
        let mut pos = 0usize;
        while pos < frames {
            let end = self.segment(p, pos, frames);
            let take = self.due(end);
            let seg_start = self.clock + pos as u64;
            for (e, h) in events.iter_mut().zip(&self.pending[..take]) {
                *e = Event::note_on((h.frame - seg_start) as u32, PART, h.note, h.velocity);
            }
            render(eng, pos, end, &events[..take]);
            self.consume(take);
            pos = end;
        }
        self.clock += frames as u64;
    }

    /// Latch the steps that start at `pos` and return where the piece
    /// from `pos` ends: the next step's start, the hit after the
    /// [`EVENTS`]-th due one, or `frames`.
    fn segment(&mut self, p: &Project, pos: usize, frames: usize) -> usize {
        let now = self.clock + pos as u64;
        if self.starting {
            self.begin(p, now);
        }
        while self.playing && self.next <= now {
            self.latch(p);
        }
        let mut end = self.clock + frames as u64;
        if self.playing {
            end = end.min(self.next);
        }
        if self.n > EVENTS && self.pending[EVENTS].frame < end {
            end = self.pending[EVENTS].frame.max(now + 1);
        }
        (end - self.clock) as usize
    }

    /// Hits before `end` (block-relative), at most [`EVENTS`].
    fn due(&self, end: usize) -> usize {
        let end = self.clock + end as u64;
        self.pending[..self.n].iter().take(EVENTS).take_while(|h| h.frame < end).count()
    }

    fn consume(&mut self, k: usize) {
        self.pending.copy_within(k..self.n, 0);
        self.n -= k;
        self.hits += k as u64;
    }

    fn begin(&mut self, p: &Project, now: u64) {
        self.starting = false;
        self.n = 0;
        self.step = 0;
        self.entry = 0;
        self.repeat = 0;
        self.pattern = match p.song.entries().first() {
            Some(e) if p.mode() == PlayMode::Song => e.pattern,
            _ => p.current() as u8,
        };
        self.tempo = p.tempo();
        self.anchor = now;
        self.units = 0;
        self.next = now;
        self.count = 0;
        self.latched = 0;
    }

    fn end_of_pattern(&mut self, p: &Project) {
        self.step = 0;
        let song = p.song.entries();
        if p.mode() == PlayMode::Song && !song.is_empty() {
            let e = usize::from(self.entry).min(song.len() - 1);
            self.repeat += 1;
            if self.repeat >= song[e].repeats {
                self.repeat = 0;
                self.entry = ((e + 1) % song.len()) as u8;
            }
            self.pattern = song[usize::from(self.entry)].pattern.min(PATTERNS as u8 - 1);
        } else {
            self.pattern = p.current() as u8;
        }
    }

    /// Latch the step starting at `self.next`.
    fn latch(&mut self, p: &Project) {
        if p.tempo() != self.tempo {
            // The new tempo from this step on.
            self.anchor = self.next;
            self.units = 0;
            self.tempo = p.tempo();
        }
        if usize::from(self.step) >= p.pattern(usize::from(self.pattern)).len() {
            // Shortened under the cursor: the pattern ends here.
            self.end_of_pattern(p);
        }
        let pat = p.pattern(usize::from(self.pattern));
        let local = usize::from(self.step);
        let dur = step_units(p.swing(), local);
        for t in 0..TRACKS {
            let s = pat.step(t, local);
            if !s.on() || !p.audible(t) || !chance(p.seed, self.count, t, s.prob()) {
                continue;
            }
            let tr = p.track(t);
            let vel = hit_velocity(s.velocity(), s.accent(), tr.level);
            if vel == 0 {
                continue;
            }
            let r = u64::from(s.ratchet());
            for j in 0..r {
                let u = self.units + u64::from(s.micro()) * UNITS_PER_MICRO + j * dur / r;
                let frame = self.anchor + units_to_frames(u, self.rate, self.tempo);
                self.push(Hit { frame, note: tr.note, velocity: vel, track: t as u8 });
            }
        }
        self.history[(self.latched % HISTORY as u64) as usize] = Position {
            pattern: self.pattern,
            step: self.step,
            entry: self.entry,
            repeat: self.repeat,
            frame: self.next,
        };
        self.latched += 1;
        self.units += dur;
        self.next = self.anchor + units_to_frames(self.units, self.rate, self.tempo);
        self.count += 1;
        self.step += 1;
        if usize::from(self.step) >= pat.len() {
            self.end_of_pattern(p);
        }
    }

    /// Insert in frame order after every hit at the same frame.
    fn push(&mut self, h: Hit) {
        if self.n == PENDING {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let mut i = self.n;
        while i > 0 && self.pending[i - 1].frame > h.frame {
            i -= 1;
        }
        self.pending.copy_within(i..self.n, i + 1);
        self.pending[i] = h;
        self.n += 1;
    }
}
