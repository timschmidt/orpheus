//! The step player: walks a [`Project`] and drives an
//! [`orpheus_synth::Engine`] with every note on its exact sample.
//!
//! # Timing
//!
//! Time is counted in *units*: a pair of nominal 1/16 steps is 57,600
//! units. A step at local index `i` of its pattern lasts `576 · swing`
//! units when `i` is even and `576 · (100 − swing)` when odd (swing 50:
//! 28,800 each); a micro-timing delay of `m`/24 step is `1,200 · m`; the
//! `j`-th of `r` ratchet hits lies `j · duration / r` into the step; a
//! synth note's gate is a number of quarter steps (7,200 units each) — so
//! every one of these is a whole number of units (a multiple of 48).
//! Units become frames with
//!
//! ```text
//! frame = anchor + ⌊units · rate · 5 · m / (tempo · 96 · d)⌋      (tempo in 1/100 BPM)
//! ```
//!
//! where `m/d` is the step length ([`StepLen`]: 1 for 1/16 steps, 2 for
//! 1/8, ½ for 1/32) and `units` counts from the anchor: the start of the
//! play, re-set to the start of a step when the tempo or the step length
//! changes. Every step, hit and note end lands on the floor of its exact
//! time however long the song plays; nothing accumulates rounding.
//!
//! # Edits while playing
//!
//! A step is *latched* (its tracks read, its hits computed) on the sample
//! where it starts. Its hits all lie in or after it (micro-timing only
//! delays), so everything the project says at that moment is what plays:
//! an edit of the project between two render calls takes effect at the
//! first step that starts after it — the tempo, swing, a step, a lock,
//! mute, solo, a drum track's level or note, a pattern's length (a step
//! past a shortened length ends the pattern), the pattern chosen to play
//! next (pattern mode: [`Project::current`] at the end of the pattern),
//! the song, the scale (chords) and the fill key ([`Sequencer::set_fill`]).
//! Kits, synth patches, the synth tracks' levels and the sends are
//! installed between render calls (a new patch cuts the part's sounding
//! notes within 256 samples, as the engine does).
//!
//! # Synth notes
//!
//! A synth step plays its note (plus the pattern's transpose) or its chord
//! (built in the project's scale) on the track's engine part for its gate;
//! the note-off is computed with the note-on and kept in a small table of
//! the track's sounding notes ([`HELD`]): an off is always on the first
//! sample of a render piece, before that sample's note-ons, so a gate of
//! exactly one step re-attacks the next note and a longer one overlaps it
//! (a mono legato patch glides). A note played again while it sounds is
//! retriggered and gets the new note's end; a full table ends its oldest
//! note first. [`Sequencer::stop`] ends every sounding note at the next
//! render.
//!
//! # Locks
//!
//! A drum hit's locked tune, decay and tone become the engine part's drum
//! offsets ([`orpheus_synth::Param::DrumTune`] …: the locked value against
//! the track's kit entry) just before its note-on; a synth step's locked
//! cutoff, resonance and glide become the part's parameters before its
//! note-on, and the patch's own values come back at the track's next
//! unlocked note. Only changes are sent.
//!
//! # Rendering
//!
//! [`Sequencer::render_i16`] / [`Sequencer::render_f32`] fill an
//! interleaved stereo block: the block is cut at every step start (to
//! latch it) and at every note end, and each piece goes to the engine with
//! its events at their offsets (more than [`EVENTS`] events in a piece are
//! rendered in turns, the engine applies the events of an empty render at
//! once). The engine's output does not depend on block sizes, nor does the
//! step clock, so a project renders the same samples in any block size —
//! on the screen's clock or with the screen off, where nothing but these
//! calls run (the firmware's synth task calls `render_i16` into the DAC
//! ring; the UI only edits the project and reads
//! [`Sequencer::position_at`]).

use orpheus_synth::{Engine, Event, EventKind, Param, Patch};

use crate::clock::{self, ClockEvent, Realtime};

use crate::model::{
    ACCENT, ALL_TRACKS, LockParam, PATTERNS, PROB_LEVELS, PlayMode, Project, SYNTH_TRACKS, TRACKS,
};

/// Units of a pair of nominal steps.
pub const UNITS_PER_PAIR: u64 = 57_600;
/// Units of a nominal (unswung) step.
pub const UNITS_PER_STEP: u64 = UNITS_PER_PAIR / 2;
/// Units of one micro-timing step (1/24 of a nominal step).
pub const UNITS_PER_MICRO: u64 = UNITS_PER_STEP / 24;
/// Units of a quarter step (synth gates).
pub const UNITS_PER_QUARTER: u64 = UNITS_PER_STEP / 4;
/// Engine events handed over per render call (a piece with more is
/// rendered in turns).
pub const EVENTS: usize = 64;
/// Hits computed but not yet played (a step makes at most 8 × 4 drum hits
/// and 2 × 4 synth notes; a step's late hits can still be waiting when the
/// next two have been latched).
pub const PENDING: usize = 128;
/// Sounding notes remembered per synth track (for their note-offs).
pub const HELD: usize = 8;
/// Notes played from the keys (audition, live record) waiting for the next
/// render.
pub const LIVE: usize = 8;
/// Latched steps remembered for [`Sequencer::position_at`].
pub const HISTORY: usize = 16;
/// The engine part kit A plays on (one part, so the kit's choke groups
/// work across tracks: a closed hat cuts the open one).
pub const PART: u8 = 0;
/// The engine part of kit B.
pub const PART_KIT_B: u8 = 1;
/// The engine parts of the synth tracks.
pub const PART_SYNTH: [u8; SYNTH_TRACKS] = [2, 3];
/// The engine part of the metronome.
pub const PART_CLICK: u8 = 4;
/// The engine's kit slot kit A is installed in (kit B: slot 1).
pub const KIT_SLOT: u8 = 0;
/// The track number of the metronome's clicks in [`Hit`]s.
pub const CLICK: u8 = ALL_TRACKS as u8;
/// The note of the metronome (accented: the first beat of a bar) and of
/// its other beats (General MIDI high and low wood block).
pub const CLICK_NOTES: [u8; 2] = [76, 77];
/// "Not locked" in [`Hit::locks`].
pub const NO_LOCK: u8 = 0xFF;

/// What a step is in note values.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StepLen {
    /// A sixteenth note (four steps a beat).
    #[default]
    Sixteenth,
    /// An eighth note.
    Eighth,
    /// A thirty-second note.
    ThirtySecond,
}

impl StepLen {
    /// The frames of a unit relative to a 1/16 step, as `(m, d)`.
    #[must_use]
    pub const fn ratio(self) -> (u64, u64) {
        match self {
            StepLen::Sixteenth => (1, 1),
            StepLen::Eighth => (2, 1),
            StepLen::ThirtySecond => (1, 2),
        }
    }

    /// Steps in a beat (a quarter note).
    #[must_use]
    pub const fn beat_steps(self) -> u32 {
        match self {
            StepLen::Sixteenth => 4,
            StepLen::Eighth => 2,
            StepLen::ThirtySecond => 8,
        }
    }

    /// Steps in a quarter note's worth of MIDI ticks: ticks per step at
    /// `ppq` ticks a quarter.
    #[must_use]
    pub const fn ticks_per_step(self, ppq: u64) -> u64 {
        ppq / self.beat_steps() as u64
    }
}

/// How the player plays apart from the project (the Sequencer's settings).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Options {
    /// A click on every beat while playing.
    pub metronome: bool,
    /// Bars of clicks before the play starts (0–2).
    pub count_in: u8,
    /// What a step is.
    pub step_len: StepLen,
}

/// Length of the step at local index `local` with `swing` per cent, units.
#[must_use]
pub const fn step_units(swing: u8, local: usize) -> u64 {
    if local % 2 == 0 { 576 * swing as u64 } else { 576 * (100 - swing as u64) }
}

/// `units` from the anchor as frames at `rate` and `tempo` (1/100 BPM),
/// rounded down (1/16 steps).
#[must_use]
pub fn units_to_frames(units: u64, rate: u32, tempo: u16) -> u64 {
    scaled_frames(units, rate, tempo, (1, 1))
}

/// The same with the step length's ratio `(m, d)`.
#[must_use]
pub fn scaled_frames(units: u64, rate: u32, tempo: u16, (m, d): (u64, u64)) -> u64 {
    let n = u128::from(units) * u128::from(rate) * 5 * u128::from(m);
    let den = u128::from(tempo.max(1)) * 96 * u128::from(d);
    u64::try_from(n / den).unwrap_or(u64::MAX)
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

/// A lock value in engine units: cutoff in Hz (20 Hz–20 kHz, logarithmic
/// over 0–127), resonance 0–1, glide in seconds; a drum lock's raw value.
#[must_use]
pub fn lock_value(param: LockParam, v: u8) -> f32 {
    match param {
        // 20 · 1000^(v/127) = 20 · 2^(v · log2(1000)/127).
        LockParam::Cutoff => 20.0 * orpheus_synth::math::exp2(f32::from(v.min(127)) * (9.965_784 / 127.0)),
        LockParam::Resonance | LockParam::Glide => f32::from(v.min(100)) / 100.0,
        _ => f32::from(v),
    }
}

/// The cutoff lock value nearest to `hz`.
#[must_use]
pub fn cutoff_code(hz: f32) -> u8 {
    let x = orpheus_synth::math::log2(hz.max(20.0) / 20.0) * (127.0 / 9.965_784);
    (x + 0.5).clamp(0.0, 127.0) as u8
}

/// One note waiting for its sample (a note-on, or in a dry run's output
/// also a synth note-off: velocity 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Hit {
    /// Frame on the player's clock.
    pub frame: u64,
    /// Synth notes: frames until the note-off (0 for drums and clicks).
    pub len: u32,
    /// Note (a drum track's kit note).
    pub note: u8,
    /// Velocity, 1–127 (0: a note-off, dry runs only).
    pub velocity: u8,
    /// Track, 0–9, or [`CLICK`].
    pub track: u8,
    /// The step's locked values of the track's three lockable parameters
    /// ([`LockParam::of_track`]), [`NO_LOCK`] where unlocked.
    pub locks: [u8; 3],
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    note: u8,
    off: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Live {
    track: u8,
    note: u8,
    velocity: u8,
}

/// The step player (transport, step clock, hit queue, the synth tracks'
/// sounding notes). About 4.3 KiB.
#[derive(Clone, Debug)]
pub struct Sequencer {
    rate: u32,
    playing: bool,
    starting: bool,
    release: bool,
    fill: bool,
    opts: Options,
    clock: u64,
    // The next step to latch.
    pattern: u8,
    step: u8,
    entry: u8,
    repeat: u8,
    // Step clock.
    tempo: u16,
    ratio: (u64, u64),
    anchor: u64,
    units: u64,
    next: u64,
    count: u64,
    count_in: u32,
    count_in_total: u32,
    // Hits computed, sorted by frame (stable: track order, then ratchet).
    pending: [Hit; PENDING],
    n: usize,
    dropped: u32,
    held: [[Held; HELD]; SYNTH_TRACKS],
    n_held: [u8; SYNTH_TRACKS],
    live: [Live; LIVE],
    n_live: u8,
    history: [Position; HISTORY],
    latched: u64,
    // What the engine has: installed revisions and sent values.
    kit_rev: Option<u32>,
    sound_rev: Option<u32>,
    tempo_sent: u16,
    mix_sent: [[u8; 3]; 4],
    synth_sent: [[u8; 3]; SYNTH_TRACKS],
    drum_sent: [[f32; 3]; 2],
    hits: u64,
    mclk: clock::State,
}

/// Where a piece's notes go: the engine (as events) or a dry run's
/// callback.
trait Sink {
    fn off(&mut self, frame: u64, track: u8, part: u8, note: u8);
    fn on(&mut self, h: &Hit, part: u8);
    fn param(&mut self, frame: u64, part: u8, param: Param, value: f32);
    /// The piece ends at `frame`.
    fn end(&mut self, frame: u64);
    /// Whether parameters are wanted (not in a dry run).
    fn params(&self) -> bool;
}

struct Dry<F: FnMut(Hit)>(F);

impl<F: FnMut(Hit)> Sink for Dry<F> {
    fn off(&mut self, frame: u64, track: u8, _part: u8, note: u8) {
        (self.0)(Hit { frame, len: 0, note, velocity: 0, track, locks: [NO_LOCK; 3] });
    }
    fn on(&mut self, h: &Hit, _part: u8) {
        (self.0)(*h);
    }
    fn param(&mut self, _: u64, _: u8, _: Param, _: f32) {}
    fn end(&mut self, _: u64) {}
    fn params(&self) -> bool {
        false
    }
}

/// Events for the engine, rendered piece by piece through `render`
/// (engine, from, to — block-relative frames —, events).
struct Wet<'a, R: FnMut(&mut Engine, usize, usize, &[Event])> {
    eng: &'a mut Engine,
    render: R,
    base: u64,
    pos: usize,
    ev: [Event; EVENTS],
    n: usize,
}

impl<R: FnMut(&mut Engine, usize, usize, &[Event])> Wet<'_, R> {
    fn push(&mut self, frame: u64, kind: EventKind) {
        let at = (frame - self.base) as usize;
        if self.n == EVENTS {
            self.flush(at);
        }
        self.ev[self.n] = Event { offset: (at - self.pos) as u32, kind };
        self.n += 1;
    }

    fn flush(&mut self, at: usize) {
        (self.render)(self.eng, self.pos, at, &self.ev[..self.n]);
        self.pos = at;
        self.n = 0;
    }
}

impl<R: FnMut(&mut Engine, usize, usize, &[Event])> Sink for Wet<'_, R> {
    fn off(&mut self, frame: u64, _track: u8, part: u8, note: u8) {
        self.push(frame, EventKind::NoteOff { part, note });
    }
    fn on(&mut self, h: &Hit, part: u8) {
        self.push(h.frame, EventKind::NoteOn { part, note: h.note, velocity: h.velocity });
    }
    fn param(&mut self, frame: u64, part: u8, param: Param, value: f32) {
        self.push(frame, EventKind::Param { part, param, value });
    }
    fn end(&mut self, frame: u64) {
        let at = (frame - self.base) as usize;
        self.flush(at);
    }
    fn params(&self) -> bool {
        true
    }
}

/// The metronome's sound (a short clave).
#[must_use]
pub fn click_patch() -> Patch {
    Patch::drum(orpheus_synth::DrumParams::new(orpheus_synth::DrumKind::Clave))
}

impl Sequencer {
    /// A stopped player at `rate` Hz (the engine's rate).
    #[must_use]
    pub const fn new(rate: u32) -> Self {
        const NO_HIT: Hit = Hit { frame: 0, len: 0, note: 0, velocity: 0, track: 0, locks: [NO_LOCK; 3] };
        Sequencer {
            rate,
            playing: false,
            starting: false,
            release: false,
            fill: false,
            opts: Options { metronome: false, count_in: 0, step_len: StepLen::Sixteenth },
            clock: 0,
            pattern: 0,
            step: 0,
            entry: 0,
            repeat: 0,
            tempo: crate::model::TEMPO_DEFAULT,
            ratio: (1, 1),
            anchor: 0,
            units: 0,
            next: 0,
            count: 0,
            count_in: 0,
            count_in_total: 0,
            pending: [NO_HIT; PENDING],
            n: 0,
            dropped: 0,
            held: [[Held { note: 0, off: 0 }; HELD]; SYNTH_TRACKS],
            n_held: [0; SYNTH_TRACKS],
            live: [Live { track: 0, note: 0, velocity: 0 }; LIVE],
            n_live: 0,
            history: [Position { pattern: 0, step: 0, entry: 0, repeat: 0, frame: 0 }; HISTORY],
            latched: 0,
            kit_rev: None,
            sound_rev: None,
            tempo_sent: 0,
            mix_sent: [[0xFF; 3]; 4],
            synth_sent: [[NO_LOCK; 3]; SYNTH_TRACKS],
            drum_sent: [[0.0, 1.0, 0.0]; 2],
            hits: 0,
            mclk: clock::State::OFF,
        }
    }

    /// Sample rate.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    /// The options (metronome, count-in, step length).
    #[must_use]
    pub const fn options(&self) -> Options {
        self.opts
    }

    /// Set the options: the metronome from the next step, the count-in at
    /// the next play, the step length from the next step.
    pub fn set_options(&mut self, o: Options) {
        self.opts = Options { count_in: o.count_in.min(2), ..o };
    }

    /// Start playing at the first frame of the next render call: pattern
    /// mode from the start of [`Project::current`], song mode from the
    /// song's first entry; after the count-in, if one is set.
    pub fn play(&mut self) {
        self.playing = true;
        self.starting = true;
        self.n = 0;
    }

    /// Stop: no more hits; the synth tracks' sounding notes end at the
    /// next render (drums ring out).
    pub fn stop(&mut self) {
        self.playing = false;
        self.starting = false;
        self.n = 0;
        self.count_in = 0;
        self.release = true;
        self.mclk.halt(self.clock);
    }

    /// Turn MIDI clock out on or off ([`crate::clock`]): on from the next
    /// play; off sends Stop on the next frame if the clock is running.
    pub fn set_midi_clock(&mut self, on: bool) {
        self.mclk.on = on;
        if !on {
            self.mclk.halt(self.clock);
        }
    }

    /// Whether MIDI clock out is on.
    #[must_use]
    pub const fn midi_clock(&self) -> bool {
        self.mclk.on
    }

    /// Hand the MIDI clock's messages since the last call to `f`, in frame
    /// order (call it after every render).
    pub fn take_clock(&mut self, f: impl FnMut(ClockEvent)) {
        self.mclk.take(f);
    }

    /// MIDI clock messages dropped because nobody took them
    /// ([`clock::QUEUE`]).
    #[must_use]
    pub const fn clock_dropped(&self) -> u32 {
        self.mclk.dropped
    }

    /// Playing (or about to start, or counting in).
    #[must_use]
    pub const fn playing(&self) -> bool {
        self.playing
    }

    /// Beats of the count-in left, the one sounding included ("4 3 2 1";
    /// 0: not counting in).
    #[must_use]
    pub fn counting_in(&self) -> u32 {
        let beat = self.opts.step_len.beat_steps();
        if self.starting && self.playing {
            return u32::from(self.opts.count_in) * 4;
        }
        if !self.playing || self.count_in_total == 0 || self.latched > 0 {
            return 0;
        }
        let done = self.count_in_total - self.count_in;
        self.count_in_total / beat - done.saturating_sub(1) / beat
    }

    /// Hold (`true`) or let go of the fill key: while held, the steps come
    /// from the project's fill pattern (at the same step, within its
    /// length), from the next step.
    pub fn set_fill(&mut self, on: bool) {
        self.fill = on;
    }

    /// Whether the fill key is held.
    #[must_use]
    pub const fn fill(&self) -> bool {
        self.fill
    }

    /// Play `note` on track `track` (0–9) now (at the next render call):
    /// the keys' audition and live recording. A synth note sounds for a
    /// step. Ignored when [`LIVE`] notes already wait.
    pub fn live(&mut self, track: u8, note: u8, velocity: u8) {
        if usize::from(self.n_live) < LIVE && usize::from(track) < ALL_TRACKS {
            self.live[usize::from(self.n_live)] =
                Live { track, note: note.min(127), velocity: velocity.clamp(1, 127) };
            self.n_live += 1;
        }
    }

    /// Whether something sounds or waits (notes from the keys, synth notes
    /// held): the caller keeps rendering.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.playing || self.n_live > 0 || self.n_held.iter().any(|n| *n > 0) || self.release
    }

    /// Frames rendered through the player since it was made.
    #[must_use]
    pub const fn clock(&self) -> u64 {
        self.clock
    }

    /// Steps latched since the play started (the count-in not counted).
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.count
    }

    /// Note-ons handed to the engine since the player was made.
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

    /// The step nearest to `frame` (a key pressed then, live recording:
    /// the step it falls in, or the next one from the step's middle on),
    /// as (pattern, step); `None` when not playing, counting in or too
    /// far back.
    #[must_use]
    pub fn quantize(&self, frame: u64) -> Option<(u8, u8)> {
        if !self.playing || self.starting || self.count_in > 0 {
            return None;
        }
        let n = self.latched.min(HISTORY as u64) as usize;
        let mut later = (self.next, self.pattern, self.step);
        for k in 0..n {
            let p = self.history[((self.latched - 1 - k as u64) % HISTORY as u64) as usize];
            if p.frame <= frame {
                return Some(if 2 * (frame - p.frame) < later.0 - p.frame {
                    (p.pattern, p.step)
                } else {
                    (later.1, later.2)
                });
            }
            later = (p.frame, p.pattern, p.step);
        }
        None
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

    /// Advance `frames` without an engine: the notes that would play, in
    /// order, go to `hit` — note-ons, and the synth tracks' note-offs as
    /// velocity 0 (tests, the MIDI export, a dry run).
    pub fn run_dry(&mut self, p: &Project, frames: usize, hit: impl FnMut(Hit)) {
        let mut sink = Dry(hit);
        self.drive(p, frames, &mut sink);
        self.clock += frames as u64;
    }

    fn run(
        &mut self,
        p: &Project,
        eng: &mut Engine,
        frames: usize,
        render: impl FnMut(&mut Engine, usize, usize, &[Event]),
    ) {
        self.install(p, eng);
        let base = self.clock;
        let mut sink = Wet { eng, render, base, pos: 0, ev: [Event::note_on(0, PART, 0, 0); EVENTS], n: 0 };
        self.drive(p, frames, &mut sink);
        self.clock += frames as u64;
    }

    /// Kits, patches, levels, sends and the tempo into the engine.
    fn install(&mut self, p: &Project, eng: &mut Engine) {
        if self.kit_rev != Some(p.kit_rev()) {
            eng.set_kit(KIT_SLOT, p.kit_of(0));
            eng.set_kit(KIT_SLOT + 1, p.kit_of(1));
            if self.kit_rev.is_none() {
                eng.set_patch(PART, &Patch::kit(KIT_SLOT));
                eng.set_patch(PART_KIT_B, &Patch::kit(KIT_SLOT + 1));
                eng.set_patch(PART_CLICK, &click_patch());
            }
            self.kit_rev = Some(p.kit_rev());
        }
        if self.sound_rev != Some(p.sound_rev()) {
            for (st, part) in PART_SYNTH.iter().enumerate() {
                if eng.patch(*part).is_none_or(|x| !crate::model::patch_bits_eq(x, &p.synth(st).patch)) {
                    eng.set_patch(*part, &p.synth(st).patch);
                    self.synth_sent[st] = [NO_LOCK; 3];
                }
            }
            self.sound_rev = Some(p.sound_rev());
        }
        if self.tempo_sent != p.tempo() {
            eng.apply(EventKind::Tempo(f32::from(p.tempo()) / 100.0));
            self.tempo_sent = p.tempo();
        }
        let ds = p.drum_sends();
        for part in 0..4 {
            let want = match part {
                0 | 1 => [0xFE, ds[0], ds[1]],
                _ => {
                    let s = p.synth(part - 2);
                    [s.level, s.delay, s.reverb]
                }
            };
            for (k, param) in [Param::Volume, Param::DelaySend, Param::ReverbSend].into_iter().enumerate() {
                if want[k] != 0xFE && want[k] != self.mix_sent[part][k] {
                    eng.apply(EventKind::Param {
                        part: part as u8,
                        param,
                        value: f32::from(want[k]) / 100.0,
                    });
                    self.mix_sent[part][k] = want[k];
                }
            }
        }
    }

    fn to_frames(&self, units: u64) -> u64 {
        self.anchor + scaled_frames(units, self.rate, self.tempo, self.ratio)
    }

    fn drive<S: Sink>(&mut self, p: &Project, frames: usize, sink: &mut S) {
        let mut pos = 0usize;
        let stop = self.clock + frames as u64;
        while pos < frames {
            let now = self.clock + pos as u64;
            if self.starting {
                self.begin(p, now);
            }
            while self.playing && self.next <= now {
                self.latch(p);
            }
            // Note-offs first (a stop ends every synth note).
            for (st, part) in PART_SYNTH.into_iter().enumerate() {
                let mut k = 0;
                while k < usize::from(self.n_held[st]) {
                    let h = self.held[st][k];
                    if self.release || h.off <= now {
                        sink.off(now, (TRACKS + st) as u8, part, h.note);
                        self.unhold(st, k);
                    } else {
                        k += 1;
                    }
                }
            }
            self.release = false;
            let mut end = stop;
            if self.playing {
                end = end.min(self.next);
            }
            for (st, held) in self.held.iter().enumerate() {
                for h in &held[..usize::from(self.n_held[st])] {
                    end = end.min(h.off);
                }
            }
            // Notes from the keys.
            for i in 0..usize::from(self.n_live) {
                let l = self.live[i];
                let len =
                    scaled_frames(UNITS_PER_STEP, self.rate, p.tempo(), self.opts.step_len.ratio()).max(1);
                let h = Hit {
                    frame: now,
                    len: len.min(u64::from(u32::MAX)) as u32,
                    note: l.note,
                    velocity: l.velocity,
                    track: l.track,
                    locks: [NO_LOCK; 3],
                };
                self.emit(p, &h, sink, &mut end);
            }
            self.n_live = 0;
            // The hits of this piece.
            let mut k = 0;
            while k < self.n && self.pending[k].frame < end {
                let h = self.pending[k];
                self.emit(p, &h, sink, &mut end);
                k += 1;
            }
            self.pending.copy_within(k..self.n, 0);
            self.n -= k;
            if self.mclk.running {
                let t = clock::tick_units(self.ratio);
                loop {
                    let f = self.to_frames(self.mclk.next);
                    if f >= end {
                        break;
                    }
                    self.mclk.push(f, Realtime::Clock);
                    self.mclk.next += t;
                }
            }
            sink.end(end);
            pos = (end - self.clock) as usize;
        }
    }

    /// Hand one note-on to the sink, with its locks; a synth note goes
    /// into the track's sounding notes and may end the piece earlier (at
    /// its own end).
    fn emit<S: Sink>(&mut self, p: &Project, h: &Hit, sink: &mut S, end: &mut u64) {
        let t = usize::from(h.track);
        self.hits += 1;
        if t == usize::from(CLICK) {
            sink.on(h, PART_CLICK);
            return;
        }
        if t < TRACKS {
            let part = if p.track(t).kit == 0 { PART } else { PART_KIT_B };
            if sink.params() {
                let want = drum_offsets(p, t, h.locks);
                let sent = &mut self.drum_sent[usize::from(part)];
                for (k, param) in [Param::DrumTune, Param::DrumDecay, Param::DrumTone].into_iter().enumerate()
                {
                    if want[k].to_bits() != sent[k].to_bits() {
                        sink.param(h.frame, part, param, want[k]);
                        sent[k] = want[k];
                    }
                }
            }
            sink.on(h, part);
            return;
        }
        let st = t - TRACKS;
        let part = PART_SYNTH[st];
        if sink.params() {
            let base = &p.synth(st).patch;
            for (k, param) in LockParam::SYNTH.into_iter().enumerate() {
                if h.locks[k] != self.synth_sent[st][k] {
                    let value = if h.locks[k] == NO_LOCK {
                        match param {
                            LockParam::Cutoff => base.filter.cutoff,
                            LockParam::Resonance => base.filter.resonance,
                            _ => base.glide,
                        }
                    } else {
                        lock_value(param, h.locks[k])
                    };
                    let ep = match param {
                        LockParam::Cutoff => Param::Cutoff,
                        LockParam::Resonance => Param::Resonance,
                        _ => Param::Glide,
                    };
                    sink.param(h.frame, part, ep, value);
                    self.synth_sent[st][k] = h.locks[k];
                }
            }
        }
        let off = h.frame + u64::from(h.len.max(1));
        let n = usize::from(self.n_held[st]);
        if let Some(i) = self.held[st][..n].iter().position(|x| x.note == h.note) {
            self.held[st][i].off = off;
        } else {
            if n == HELD {
                // The note that would end first ends now.
                let (i, _) = self.held[st]
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, x)| x.off)
                    .unwrap_or((0, &self.held[st][0]));
                sink.off(h.frame, h.track, part, self.held[st][i].note);
                self.unhold(st, i);
            }
            let n = usize::from(self.n_held[st]);
            self.held[st][n] = Held { note: h.note, off };
            self.n_held[st] += 1;
        }
        sink.on(h, part);
        *end = (*end).min(off);
    }

    fn unhold(&mut self, st: usize, i: usize) {
        let n = usize::from(self.n_held[st]);
        self.held[st].copy_within(i + 1..n, i);
        self.n_held[st] -= 1;
    }

    fn begin(&mut self, p: &Project, now: u64) {
        self.starting = false;
        self.mclk.halt(now);
        self.n = 0;
        self.step = 0;
        self.entry = 0;
        self.repeat = 0;
        self.pattern = match p.song.entries().first() {
            Some(e) if p.mode() == PlayMode::Song => e.pattern,
            _ => p.current() as u8,
        };
        self.tempo = p.tempo();
        self.ratio = self.opts.step_len.ratio();
        self.anchor = now;
        self.units = 0;
        self.next = now;
        self.count = 0;
        self.count_in_total = u32::from(self.opts.count_in) * 4 * self.opts.step_len.beat_steps();
        self.count_in = self.count_in_total;
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

    fn click(&mut self, frame: u64, accent: bool) {
        self.push(Hit {
            frame,
            len: 0,
            note: CLICK_NOTES[usize::from(!accent)],
            velocity: if accent { 127 } else { 90 },
            track: CLICK,
            locks: [NO_LOCK; 3],
        });
    }

    /// Latch the step starting at `self.next`.
    fn latch(&mut self, p: &Project) {
        if p.tempo() != self.tempo || self.opts.step_len.ratio() != self.ratio {
            // The new tempo or step length from this step on.
            let ratio = self.opts.step_len.ratio();
            self.mclk.reanchor(self.units, self.ratio, ratio);
            self.anchor = self.next;
            self.units = 0;
            self.tempo = p.tempo();
            self.ratio = self.opts.step_len.ratio();
        }
        let beat = u64::from(self.opts.step_len.beat_steps());
        if self.count_in > 0 {
            let k = u64::from(self.count_in_total - self.count_in);
            if k % beat == 0 {
                let f = self.next;
                self.click(f, k % (4 * beat) == 0);
            }
            self.units += step_units(p.swing(), k as usize);
            self.next = self.to_frames(self.units);
            self.count_in -= 1;
            return;
        }
        if self.count == 0 && self.mclk.on && !self.mclk.running {
            // The first step: Start, and the first Clock on the same frame.
            self.mclk.running = true;
            self.mclk.next = self.units;
            self.mclk.push(self.next, Realtime::Start);
        }
        if usize::from(self.step) >= p.pattern(usize::from(self.pattern)).len() {
            // Shortened under the cursor: the pattern ends here.
            self.end_of_pattern(p);
        }
        let local = usize::from(self.step);
        let (src, src_step) = match p.fill() {
            Some(f) if self.fill => (f, local % p.pattern(f).len()),
            _ => (usize::from(self.pattern), local),
        };
        let pat = p.pattern(src);
        let dur = step_units(p.swing(), local);
        if self.opts.metronome && self.count % beat == 0 {
            let f = self.next;
            self.click(f, self.count % (4 * beat) == 0);
        }
        for t in 0..TRACKS {
            let s = pat.step(t, src_step);
            if !s.on() || !p.audible(t) || !chance(p.seed, self.count, t, s.prob()) {
                continue;
            }
            let tr = p.track(t);
            let vel = hit_velocity(s.velocity(), s.accent(), tr.level);
            if vel == 0 {
                continue;
            }
            let mut locks = [NO_LOCK; 3];
            for (l, param) in locks.iter_mut().zip(LockParam::DRUM) {
                *l = pat.locks.get(src_step, t, param).unwrap_or(NO_LOCK);
            }
            let r = u64::from(s.ratchet());
            for j in 0..r {
                let u = self.units + u64::from(s.micro()) * UNITS_PER_MICRO + j * dur / r;
                let frame = self.to_frames(u);
                self.push(Hit { frame, len: 0, note: tr.note, velocity: vel, track: t as u8, locks });
            }
        }
        let (scale, root) = p.scale();
        for st in 0..SYNTH_TRACKS {
            let t = TRACKS + st;
            let s = pat.synth_step(st, src_step);
            if !s.on() || !p.audible(t) || !chance(p.seed, self.count, t, s.common().prob()) {
                continue;
            }
            let vel = hit_velocity(s.common().velocity(), s.common().accent(), 100);
            let mut locks = [NO_LOCK; 3];
            for (l, param) in locks.iter_mut().zip(LockParam::SYNTH) {
                *l = pat.locks.get(src_step, t, param).unwrap_or(NO_LOCK);
            }
            let note = (i16::from(s.pitch()) + i16::from(pat.transpose())).clamp(0, 127) as u8;
            let mut notes = [0u8; 4];
            let k = scale.chord(root, note, s.chord(), &mut notes);
            let k = if s.chord() == crate::model::Chord::Single {
                notes[0] = note;
                1
            } else {
                k
            };
            let r = if k == 1 { u64::from(s.common().ratchet()) } else { 1 };
            let gate = u64::from(s.gate_quarters()) * UNITS_PER_QUARTER;
            let gate = if r > 1 { gate.min(dur / r) } else { gate };
            for j in 0..r {
                let u = self.units + u64::from(s.common().micro()) * UNITS_PER_MICRO + j * dur / r;
                let frame = self.to_frames(u);
                let len = (self.to_frames(u + gate) - frame).clamp(1, u64::from(u32::MAX)) as u32;
                for n in &notes[..k] {
                    self.push(Hit { frame, len, note: *n, velocity: vel, track: t as u8, locks });
                }
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
        self.next = self.to_frames(self.units);
        self.count += 1;
        self.step += 1;
        if usize::from(self.step) >= p.pattern(usize::from(self.pattern)).len() {
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

/// The engine's drum offsets for drum track `t`'s hit with `locks` (its
/// kit entry's tune, decay and tone against the locked values).
fn drum_offsets(p: &Project, t: usize, locks: [u8; 3]) -> [f32; 3] {
    let mut o = [0.0, 1.0, 0.0];
    if locks == [NO_LOCK; 3] {
        return o;
    }
    let Some(e) = p.entry(t) else { return o };
    if locks[0] != NO_LOCK {
        o[0] = (f32::from(locks[0]) - 96.0 - f32::from(e.tune)) / 4.0;
    }
    if locks[1] != NO_LOCK {
        o[1] = f32::from(locks[1]) / f32::from(e.decay.max(1));
    }
    if locks[2] != NO_LOCK {
        o[2] = (f32::from(locks[2]) - f32::from(e.tone)) / 100.0;
    }
    o
}
