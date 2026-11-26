//! The MIDI player: a decoder-like source of PCM for `.mid`, `.midi` and
//! `.kar` files.
//!
//! [`MidiPlayer`] holds the synthesis engine, the parser's track cursors,
//! the tick clock and the channel state; nothing else (≈ 70 KB, measured in
//! the RAM test; no allocation). Life of a file, as for a codec:
//!
//! 1. [`MidiPlayer::open`] with the file's length, then [`MidiPlayer::step`]
//!    until it answers `Ready(true)`, answering each [`Pull::Need`] with
//!    [`MidiPlayer::supply`]. Opening reads the header, finds the tracks
//!    and scans the whole file once (bounded steps, no sound): the duration,
//!    the channels used, the title, where the lyrics are. Then it seeks to 0.
//! 2. [`MidiPlayer::render`] fills an interleaved stereo `i16` block at the
//!    engine's rate and returns the frames written; `End` after the last
//!    frame (the last event + [`TAIL_SECONDS`]).
//! 3. [`MidiPlayer::seek`] to a frame, then `step` until ready: the file is
//!    re-scanned from the start without sound (≤ `budget` events per step)
//!    up to the target, the engine is reset with its clock at the target,
//!    and the whole channel state is installed at once; notes held at the
//!    target are struck again (melodic channels; option `chase`). Starting
//!    is a seek to 0, so a seek leaves the engine exactly as playing from
//!    the start would, except for what sounds across the target (the
//!    re-struck notes start afresh, effect tails are empty) and the noise
//!    seeds.
//! 4. Pause: stop calling `render`; everything waits where it is.
//!
//! The `*_from` methods drive the same calls from a [`MidiSource`].
//!
//! Events land on their exact frame: the block is cut at program changes
//! (a patch is installed between engine calls) and when the event buffer is
//! full; the engine's output does not depend on where blocks are cut.

use orpheus_synth::{Engine, Event, EventKind, Patch};

use crate::gm;
use crate::kar::{self, Break, LyricMode};
use crate::seq::{Action, BEND_SPAN, KIT_SOUND, Sequencer};
use crate::smf::{Division, MidiError, MidiEvent, Msg, Need, Pull, Smf, TEXT_MAX, TextKind, WINDOW};
use crate::source::MidiSource;
use crate::timing::{Clock, DEFAULT_TEMPO, TempoMap};

/// Seconds played after the last event (release and reverb tails).
pub const TAIL_SECONDS: u32 = 2;
/// Engine events buffered within a render block.
pub const EV_MAX: usize = 32;
/// Lyric pieces queued for the UI.
pub const LYRIC_QUEUE: usize = 8;
/// Bytes of the drivers' read buffer (a window or the file head).
const READ_BUF: usize = if WINDOW > crate::smf::HEAD_LEN { WINDOW } else { crate::smf::HEAD_LEN };
/// Events per step used by the `*_from` drivers.
pub const STEP_BUDGET: u32 = 512;

/// Player settings.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Output rate, Hz (the DAC's: 44 100 or 48 000; 8 000–48 000).
    pub rate: u32,
    /// Seed of the engine's noise.
    pub seed: u32,
    /// Reverb and chorus sends follow CC 91 and 93 (off: no effect work).
    pub effects: bool,
    /// Master gain, 0–2.
    pub gain: f32,
    /// Re-strike notes held at a seek target.
    pub chase: bool,
    /// The file is a karaoke file (`.kar`): text events are lyrics.
    pub kar: bool,
}

impl Options {
    /// Defaults at `rate`: effects on, gain 0.5, chase on.
    #[must_use]
    pub const fn new(rate: u32) -> Self {
        Self { rate, seed: 0x4D49_4449, effects: true, gain: 0.5, chase: true, kar: false }
    }
}

/// What the scan found.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SongInfo {
    /// Format (0, 1, 2).
    pub format: u16,
    /// Tracks.
    pub tracks: u8,
    /// Time division.
    pub division: Division,
    /// Output rate the frames count in.
    pub rate: u32,
    /// Frame of the last event (an end of track usually).
    pub end_frame: u64,
    /// Frames the player renders: `end_frame` + [`TAIL_SECONDS`].
    pub duration: u64,
    /// Channels with notes (bit per channel).
    pub channels: u16,
    /// Note-on events.
    pub notes: u32,
    /// Events.
    pub events: u32,
    /// Tempo changes.
    pub tempo_changes: u32,
    /// Where the lyrics are.
    pub lyrics: LyricMode,
    /// Lyric pieces.
    pub lyric_pieces: u32,
    title: [u8; TEXT_MAX],
    title_len: u8,
}

impl SongInfo {
    /// The title (karaoke `@T`, else the first track's name), as stored.
    #[must_use]
    pub fn title(&self) -> &[u8] {
        self.title.get(..usize::from(self.title_len)).unwrap_or_default()
    }

    /// Duration in milliseconds.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.duration * 1000 / u64::from(self.rate.max(1))
    }
}

/// The scan's tallies.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug)]
struct Scan {
    info: SongInfo,
    lyric_events: u32,
    kar_texts: u32,
    kar_tag: bool,
    kar_title: bool,
}

impl Scan {
    fn new(rate: u32, format: u16, tracks: u8, division: Division) -> Self {
        Self {
            info: SongInfo {
                format,
                tracks,
                division,
                rate,
                end_frame: 0,
                duration: 0,
                channels: 0,
                notes: 0,
                events: 0,
                tempo_changes: 0,
                lyrics: LyricMode::None,
                lyric_pieces: 0,
                title: [0; TEXT_MAX],
                title_len: 0,
            },
            lyric_events: 0,
            kar_texts: 0,
            kar_tag: false,
            kar_title: false,
        }
    }

    fn set_title(&mut self, t: &[u8]) {
        let n = t.len().min(TEXT_MAX);
        self.info.title[..n].copy_from_slice(&t[..n]);
        self.info.title_len = n as u8;
    }

    fn add(&mut self, frame: u64, ev: &MidiEvent) {
        let i = &mut self.info;
        i.events += 1;
        i.end_frame = i.end_frame.max(frame);
        match ev.msg {
            Msg::Channel { status, d2, .. } if status & 0xF0 == 0x90 && d2 > 0 => {
                i.notes += 1;
                i.channels |= 1 << (status & 15);
            }
            Msg::Tempo(_) => i.tempo_changes += 1,
            Msg::Text(TextKind::Lyric) => self.lyric_events += 1,
            Msg::Text(TextKind::Text) => {
                let t = ev.text();
                if kar::is_kar_tag(t) {
                    self.kar_tag = true;
                }
                if let Some(title) = kar::kar_title(t) {
                    if !self.kar_title && !title.is_empty() {
                        self.kar_title = true;
                        self.set_title(title);
                    }
                } else if !t.starts_with(b"@") {
                    self.kar_texts += 1;
                }
            }
            Msg::Text(TextKind::TrackName)
                if ev.track == 0 && !self.kar_title && self.info.title_len == 0 =>
            {
                let t = ev.text();
                let t = trim(t);
                if !t.is_empty() {
                    self.set_title(t);
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self, kar_file: bool) -> SongInfo {
        let i = &mut self.info;
        i.lyrics = if (self.kar_tag || kar_file) && self.kar_texts > 0 {
            i.lyric_pieces = self.kar_texts;
            LyricMode::Kar
        } else if self.lyric_events > 0 {
            i.lyric_pieces = self.lyric_events;
            LyricMode::Lyric
        } else {
            LyricMode::None
        };
        i.duration = i.end_frame + u64::from(TAIL_SECONDS) * u64::from(i.rate);
        *i
    }
}

fn trim(mut t: &[u8]) -> &[u8] {
    while let [b' ' | 0, rest @ ..] = t {
        t = rest;
    }
    while let [rest @ .., b' ' | 0 | b'\r' | b'\n'] = t {
        t = rest;
    }
    t
}

/// A lyric piece at its frame.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LyricEvent {
    /// Output frame the syllable is sung at.
    pub frame: u64,
    /// Break before.
    pub before: Break,
    /// Break after.
    pub after: Break,
    text: [u8; TEXT_MAX],
    len: u8,
}

impl LyricEvent {
    /// The syllable.
    #[must_use]
    pub fn text(&self) -> &[u8] {
        self.text.get(..usize::from(self.len)).unwrap_or_default()
    }

    fn of(frame: u64, ev: &MidiEvent, mode: LyricMode) -> Option<Self> {
        let Msg::Text(kind) = ev.msg else { return None };
        let p = kar::piece(kind, ev.text(), mode)?;
        let mut text = [0; TEXT_MAX];
        let n = p.text.len().min(TEXT_MAX);
        for (d, s) in text.iter_mut().zip(p.text) {
            *d = *s;
        }
        Some(Self { frame, before: p.before, after: p.after, text, len: n as u8 })
    }
}

/// Output sample types (`i16` for the DAC, `f32` for checks).
pub trait Sample: Copy {
    /// Render into `out` with `events`.
    fn render(engine: &mut Engine, out: &mut [Self], events: &[Event]);
}

impl Sample for i16 {
    fn render(engine: &mut Engine, out: &mut [i16], events: &[Event]) {
        engine.render_i16(out, events);
    }
}

impl Sample for f32 {
    fn render(engine: &mut Engine, out: &mut [f32], events: &[Event]) {
        engine.render_f32(out, events);
    }
}

#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Phase {
    Idle,
    Open,
    Scan,
    Seek(u64),
    Play,
    Ended,
}

/// The player (see the module documentation).
#[derive(bytemuck::Zeroable, Clone, Debug)]
pub struct MidiPlayer {
    engine: Engine,
    smf: Smf,
    clock: Clock,
    seq: Sequencer,
    opts: Options,
    phase: Phase,
    scan: Scan,
    info: crate::opt::Opt<SongInfo>,
    /// Frames rendered (the position).
    pos: u64,
    /// The next event, pulled but not yet played.
    ahead: crate::opt::Opt<(u64, MidiEvent)>,
    /// The file's events are all played.
    events_done: bool,
    evs: [Event; EV_MAX],
    n_ev: usize,
    lyrics: [LyricEvent; LYRIC_QUEUE],
    ly_head: u8,
    ly_len: u8,
}

impl MidiPlayer {
    /// A player with `opts`; no file.
    ///
    /// About 70 KB: on the player use [`MidiPlayer::init`] on zeroed
    /// memory (an all-zero `MidiPlayer` is a valid value:
    /// `bytemuck::Zeroable`).
    #[must_use]
    pub fn new(opts: Options) -> Self {
        let mut p = <Self as bytemuck::Zeroable>::zeroed();
        p.init(opts);
        p
    }

    /// Make this player what [`MidiPlayer::new`]`(opts)` returns, in place
    /// (no temporary larger than an engine part, ≈ 1 KiB): the firmware
    /// builds it in the decoder arena. `new` is this on a zeroed value.
    pub fn init(&mut self, opts: Options) {
        self.engine.init(opts.rate, opts.seed);
        self.engine.apply(EventKind::Master(opts.gain));
        let rate = self.engine.rate();
        let opts = Options { rate, ..opts };
        self.smf.clear();
        self.clock = Clock::new(rate, Division::Metrical(96));
        self.seq = Sequencer::new(opts.effects);
        self.opts = opts;
        self.phase = Phase::Idle;
        self.scan = Scan::new(rate, 0, 0, Division::Metrical(96));
        self.info.set(None);
        self.pos = 0;
        self.ahead.set(None);
        self.events_done = false;
        self.evs.fill(<Event as bytemuck::Zeroable>::zeroed());
        self.n_ev = 0;
        let blank =
            LyricEvent { frame: 0, before: Break::None, after: Break::None, text: [0; TEXT_MAX], len: 0 };
        self.lyrics.fill(blank);
        self.ly_head = 0;
        self.ly_len = 0;
    }

    /// Start opening a file of `len` bytes (`kar`: a `.kar` file); then
    /// [`MidiPlayer::step`] until ready.
    pub fn open(&mut self, len: u32, kar: bool) {
        self.opts.kar = kar;
        self.smf.begin(len);
        self.info.set(None);
        self.phase = Phase::Open;
        self.pos = 0;
    }

    /// The settings (the rate as the engine took it).
    #[must_use]
    pub fn options(&self) -> &Options {
        &self.opts
    }

    /// What the scan found (after opening).
    #[must_use]
    pub fn info(&self) -> Option<&SongInfo> {
        self.info.as_ref()
    }

    /// Frames played (the position).
    #[must_use]
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Frames in all (0 before the scan).
    #[must_use]
    pub fn duration(&self) -> u64 {
        self.info.get().map_or(0, |i| i.duration)
    }

    /// Current tempo, BPM (at the last event played).
    #[must_use]
    pub fn bpm(&self) -> f32 {
        self.clock.bpm()
    }

    /// The engine (voices, levels).
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Use at most `n` voices from the next note on (the player's load
    /// governor, SYN-3; `Engine::set_voice_limit`).
    pub fn set_voice_limit(&mut self, n: usize) {
        self.engine.set_voice_limit(n);
    }

    /// Enable the send effects of `mask` (`orpheus_synth::load::FX_*`),
    /// the others fade out (the load governor; `Engine::set_fx_enabled`).
    pub fn set_fx_enabled(&mut self, mask: u8) {
        self.engine.set_fx_enabled(mask);
    }

    /// Fade out voices until at most `keep` sound, released ones first
    /// (the load governor; `Engine::shed`); how many it faded.
    pub fn shed(&mut self, keep: usize) -> usize {
        self.engine.shed(keep)
    }

    /// The channel state.
    #[must_use]
    pub fn sequencer(&self) -> &Sequencer {
        &self.seq
    }

    /// Whether `render` can run.
    #[must_use]
    pub fn ready(&self) -> bool {
        matches!(self.phase, Phase::Play | Phase::Ended)
    }

    /// Hand over the bytes of the last `Need`.
    pub fn supply(&mut self, d: &[u8]) {
        self.smf.supply(d);
    }

    /// Mute channel `c` (0–15); its sounding notes are released (the
    /// engine's all-notes-off lifts the pedal too).
    pub fn set_mute(&mut self, c: u8, on: bool) {
        self.seq.set_mute(c, on);
        self.silence_inaudible();
    }

    /// Solo channel `c`; while any channel is soloed only soloed channels
    /// play.
    pub fn set_solo(&mut self, c: u8, on: bool) {
        self.seq.set_solo(c, on);
        self.silence_inaudible();
    }

    fn silence_inaudible(&mut self) {
        for c in 0..16u8 {
            if !self.seq.audible(c) {
                self.engine.apply(EventKind::AllNotesOff { part: c });
            }
        }
    }

    /// The next lyric piece reached by the playback (in order; the UI
    /// shows it when the output reaches its frame).
    pub fn pop_lyric(&mut self) -> Option<LyricEvent> {
        if self.ly_len == 0 {
            return None;
        }
        let e = self.lyrics[usize::from(self.ly_head)];
        self.ly_head = (self.ly_head + 1) % LYRIC_QUEUE as u8;
        self.ly_len -= 1;
        Some(e)
    }

    fn push_lyric(&mut self, e: LyricEvent) {
        if usize::from(self.ly_len) == LYRIC_QUEUE {
            // Full: the oldest goes.
            self.ly_head = (self.ly_head + 1) % LYRIC_QUEUE as u8;
            self.ly_len -= 1;
        }
        let i = (usize::from(self.ly_head) + usize::from(self.ly_len)) % LYRIC_QUEUE;
        self.lyrics[i] = e;
        self.ly_len += 1;
    }

    /// Start a seek to `frame` (clamped to the duration); then
    /// [`MidiPlayer::step`] until ready.
    pub fn seek(&mut self, frame: u64) -> Result<(), MidiError> {
        let Some(info) = self.info.get() else { return Err(MidiError::State) };
        self.smf.rewind();
        self.clock = Clock::new(self.opts.rate, info.division);
        self.seq.reset();
        self.ahead.set(None);
        self.events_done = false;
        self.phase = Phase::Seek(frame.min(info.duration));
        Ok(())
    }

    /// The next event with its frame (the pending one first). The event's
    /// effect on the clock is applied when it is consumed.
    fn timed(&mut self) -> Pull<(u64, MidiEvent)> {
        if let Some(a) = self.ahead.take() {
            return Pull::Ready(a);
        }
        self.parse_next()
    }

    /// [`MidiPlayer::timed`] without a pending event: the parser. Out of
    /// line: most render pieces only look at the pending event (the next
    /// one lies beyond them), and the code a piece runs on the player must
    /// fit its code cache with the render's (SYN-3).
    #[inline(never)]
    fn parse_next(&mut self) -> Pull<(u64, MidiEvent)> {
        if self.events_done {
            return Pull::End;
        }
        match self.smf.next_event() {
            Pull::Ready(ev) => {
                let f = self.clock.frame(ev.tick);
                if f > self.clock.max_frame() {
                    self.events_done = true;
                    return Pull::End;
                }
                Pull::Ready((f, ev))
            }
            Pull::Need(n) => Pull::Need(n),
            Pull::End => {
                self.events_done = true;
                Pull::End
            }
        }
    }

    /// Apply an event's effect on the clock.
    fn clock_event(&mut self, ev: &MidiEvent) {
        match ev.msg {
            Msg::Tempo(us) => self.clock.set_tempo(ev.tick, us),
            Msg::Pattern(_) => self.clock.set_tempo(ev.tick, DEFAULT_TEMPO),
            _ => {}
        }
    }

    /// Advance opening, scanning or seeking by at most `budget` events.
    /// `Ready(true)`: ready to render.
    pub fn step(&mut self, budget: u32) -> Result<Pull<bool>, MidiError> {
        match self.phase {
            Phase::Idle => Err(MidiError::State),
            Phase::Play | Phase::Ended => Ok(Pull::Ready(true)),
            Phase::Open => match self.smf.open()? {
                Pull::Need(n) => Ok(Pull::Need(n)),
                Pull::Ready(h) => {
                    let rate = self.opts.rate;
                    self.clock = Clock::new(rate, h.division);
                    self.scan = Scan::new(rate, h.format, self.smf.tracks() as u8, h.division);
                    self.ahead.set(None);
                    self.events_done = false;
                    self.phase = Phase::Scan;
                    Ok(Pull::Ready(false))
                }
                Pull::End => Err(MidiError::State),
            },
            Phase::Scan => {
                for _ in 0..budget {
                    match self.timed() {
                        Pull::Need(n) => return Ok(Pull::Need(n)),
                        Pull::Ready((f, ev)) => {
                            self.clock_event(&ev);
                            self.scan.add(f, &ev);
                        }
                        Pull::End => {
                            self.info.set(Some(self.scan.finish(self.opts.kar)));
                            self.seek(0)?;
                            return Ok(Pull::Ready(false));
                        }
                    }
                }
                Ok(Pull::Ready(false))
            }
            Phase::Seek(target) => {
                for _ in 0..budget {
                    match self.timed() {
                        Pull::Need(n) => return Ok(Pull::Need(n)),
                        Pull::Ready((f, ev)) if f >= target => {
                            self.ahead.set(Some((f, ev)));
                            self.finish_seek(target);
                            return Ok(Pull::Ready(true));
                        }
                        Pull::Ready((_, ev)) => {
                            self.clock_event(&ev);
                            if let Msg::Channel { status, d1, d2 } = ev.msg {
                                self.seq.handle(status, d1, d2, false, &mut |_| {});
                            }
                        }
                        Pull::End => {
                            self.finish_seek(target);
                            return Ok(Pull::Ready(true));
                        }
                    }
                }
                Ok(Pull::Ready(false))
            }
        }
    }

    fn install(engine: &mut Engine, a: Action) {
        match a {
            Action::Engine(k) => engine.apply(k),
            Action::Sound { part, sound } => {
                let mut p = if sound == KIT_SOUND { Patch::KIT } else { *gm::patch(sound) };
                p.bend_range = BEND_SPAN;
                engine.set_patch(part, &p);
            }
            Action::Kit(k) => engine.set_kit(0, &gm::kit(k)),
        }
    }

    fn finish_seek(&mut self, target: u64) {
        let engine = &mut self.engine;
        engine.reset_at(self.opts.seed, target);
        engine.apply(EventKind::Tempo(self.clock.bpm()));
        self.seq.restore(self.opts.chase, &mut |a| Self::install(engine, a));
        self.pos = target;
        self.n_ev = 0;
        self.ly_head = 0;
        self.ly_len = 0;
        let end = self.info.get().map_or(0, |i| i.duration);
        self.phase = if target >= end { Phase::Ended } else { Phase::Play };
    }

    /// Render frames [`self.pos`, `upto`) into `out` (which starts at frame
    /// `start`) with the buffered events.
    fn flush<T: Sample>(&mut self, out: &mut [T], start: u64, upto: u64) {
        if upto <= self.pos {
            return;
        }
        let a = ((self.pos - start) * 2) as usize;
        let b = ((upto - start) * 2) as usize;
        // (`get`: no panic path, whose location data would be resident on
        // the player; the ranges are in bounds by construction.)
        if let Some(o) = out.get_mut(a..b) {
            T::render(&mut self.engine, o, self.evs.get(..self.n_ev).unwrap_or_default());
        }
        self.n_ev = 0;
        self.pos = upto;
    }

    fn push_event<T: Sample>(&mut self, out: &mut [T], start: u64, f: u64, kind: EventKind) {
        if self.n_ev == EV_MAX {
            self.flush(out, start, f);
            if self.n_ev == EV_MAX {
                // Still full (all at the block's first frame): apply them now.
                for e in self.evs.iter().take(self.n_ev) {
                    self.engine.apply(e.kind);
                }
                self.n_ev = 0;
            }
        }
        // (Room: the buffer was flushed above when full.)
        if let Some(e) = self.evs.get_mut(self.n_ev) {
            *e = Event { offset: (f - self.pos) as u32, kind };
            self.n_ev += 1;
        }
    }

    /// Render the next frames into `out` (interleaved stereo; its length
    /// in frames is `out.len() / 2`). `Ready(n)`: `n` frames written (fewer
    /// than asked when the parser needs bytes first; then `Need` on the next
    /// call, or at the end of the song); `Need`: nothing written, supply
    /// and call again; `End`: the song is over.
    pub fn render(&mut self, out: &mut [i16]) -> Pull<usize> {
        self.render_any(out)
    }

    /// [`MidiPlayer::render`] in `f32` (the engine's samples before the
    /// conversion to 16 bits).
    pub fn render_f32(&mut self, out: &mut [f32]) -> Pull<usize> {
        self.render_any(out)
    }

    fn render_any<T: Sample>(&mut self, out: &mut [T]) -> Pull<usize> {
        match self.phase {
            Phase::Play => {}
            Phase::Ended => return Pull::End,
            _ => {
                return match self.step(STEP_BUDGET) {
                    Ok(Pull::Need(n)) => Pull::Need(n),
                    Ok(_) => Pull::Ready(0),
                    Err(_) => Pull::End,
                };
            }
        }
        let start = self.pos;
        let duration = self.duration();
        let end = (start + (out.len() / 2) as u64).min(duration);
        if start >= end {
            self.phase = Phase::Ended;
            return Pull::End;
        }
        let mut last = start;
        let mut acts = [Action::Kit(0); 8];
        loop {
            let (f, ev) = match self.timed() {
                Pull::Need(n) => {
                    self.flush(out, start, last);
                    return if self.pos > start {
                        Pull::Ready((self.pos - start) as usize)
                    } else {
                        Pull::Need(n)
                    };
                }
                Pull::End => break,
                Pull::Ready(x) => x,
            };
            if f >= end {
                self.ahead.set(Some((f, ev)));
                break;
            }
            let f = f.max(self.pos);
            last = f;
            self.clock_event(&ev);
            let mut n = 0;
            match ev.msg {
                Msg::Channel { status, d1, d2 } => {
                    self.seq.handle(status, d1, d2, true, &mut |a| {
                        if n < acts.len() {
                            acts[n] = a;
                            n += 1;
                        }
                    });
                }
                Msg::Tempo(_) | Msg::Pattern(_) => {
                    acts[0] = Action::Engine(EventKind::Tempo(self.clock.bpm()));
                    n = 1;
                }
                Msg::Text(_) => {
                    let mode = self.info.get().map_or(LyricMode::None, |i| i.lyrics);
                    if let Some(l) = LyricEvent::of(f, &ev, mode) {
                        self.push_lyric(l);
                    }
                }
                _ => {}
            }
            for &a in acts.iter().take(n) {
                match a {
                    Action::Engine(k) => self.push_event(out, start, f, k),
                    _ => {
                        self.flush(out, start, f);
                        // Events at `f` were applied after the flushed block.
                        let pending = self.n_ev;
                        for e in self.evs.iter().take(pending) {
                            self.engine.apply(e.kind);
                        }
                        self.n_ev = 0;
                        Self::install(&mut self.engine, a);
                    }
                }
            }
        }
        self.flush(out, start, end);
        if self.pos >= duration {
            self.phase = Phase::Ended;
        }
        Pull::Ready((self.pos - start) as usize)
    }

    fn fill<S: MidiSource>(&mut self, src: &mut S, n: Need) -> Result<(), MidiError> {
        let mut buf = [0u8; READ_BUF];
        let len = usize::from(n.len).min(buf.len());
        let got = src.read_at(n.offset, &mut buf[..len])?;
        self.supply(&buf[..got]);
        Ok(())
    }

    /// Open the file of `src` and scan it.
    pub fn open_from<S: MidiSource>(&mut self, src: &mut S, kar: bool) -> Result<SongInfo, MidiError> {
        self.open(src.len(), kar);
        self.run(src)?;
        self.info.get().ok_or(MidiError::State)
    }

    /// Seek to `frame` reading from `src`; returns the position.
    pub fn seek_from<S: MidiSource>(&mut self, src: &mut S, frame: u64) -> Result<u64, MidiError> {
        self.seek(frame)?;
        self.run(src)?;
        Ok(self.pos)
    }

    fn run<S: MidiSource>(&mut self, src: &mut S) -> Result<(), MidiError> {
        loop {
            match self.step(STEP_BUDGET)? {
                Pull::Need(n) => self.fill(src, n)?,
                Pull::Ready(true) => return Ok(()),
                Pull::Ready(false) => {}
                Pull::End => return Err(MidiError::State),
            }
        }
    }

    /// Render into `out` reading from `src`; returns the frames written (0
    /// at the end; fewer than asked only at the end).
    pub fn render_from<S: MidiSource>(&mut self, src: &mut S, out: &mut [i16]) -> Result<usize, MidiError> {
        self.render_any_from(src, out)
    }

    /// [`MidiPlayer::render_from`] in `f32`.
    pub fn render_f32_from<S: MidiSource>(
        &mut self,
        src: &mut S,
        out: &mut [f32],
    ) -> Result<usize, MidiError> {
        self.render_any_from(src, out)
    }

    fn render_any_from<S: MidiSource, T: Sample>(
        &mut self,
        src: &mut S,
        out: &mut [T],
    ) -> Result<usize, MidiError> {
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            match self.render_any(&mut out[done * 2..frames * 2]) {
                Pull::Ready(n) => done += n,
                Pull::Need(n) => self.fill(src, n)?,
                Pull::End => break,
            }
        }
        Ok(done)
    }
}

/// What a file holds, without playing it: the scan of [`MidiPlayer`] plus
/// the tempo map, and each lyric piece passed to `lyric` (two passes over
/// the file; no engine).
pub fn summary<S: MidiSource>(
    src: &mut S,
    rate: u32,
    kar_file: bool,
    mut lyric: impl FnMut(&LyricEvent),
) -> Result<(SongInfo, TempoMap), MidiError> {
    let mut smf = Smf::new();
    smf.begin(src.len());
    let mut buf = [0u8; READ_BUF];
    let mut fill = |smf: &mut Smf, src: &mut S, n: Need| -> Result<(), MidiError> {
        let len = usize::from(n.len).min(buf.len());
        let got = src.read_at(n.offset, &mut buf[..len])?;
        smf.supply(&buf[..got]);
        Ok(())
    };
    let h = loop {
        match smf.open()? {
            Pull::Need(n) => fill(&mut smf, src, n)?,
            Pull::Ready(h) => break h,
            Pull::End => return Err(MidiError::State),
        }
    };
    let rate = rate.clamp(8000, 48_000);
    let mut info = None;
    let mut map = TempoMap::new(rate, h.division);
    for pass in 0..2 {
        smf.rewind();
        let mut clock = Clock::new(rate, h.division);
        let mut scan = Scan::new(rate, h.format, smf.tracks() as u8, h.division);
        loop {
            let ev = match smf.next_event() {
                Pull::Need(n) => {
                    fill(&mut smf, src, n)?;
                    continue;
                }
                Pull::End => break,
                Pull::Ready(ev) => ev,
            };
            let f = clock.frame(ev.tick);
            if f > clock.max_frame() {
                break;
            }
            match ev.msg {
                Msg::Tempo(us) => clock.set_tempo(ev.tick, us),
                Msg::Pattern(_) => clock.set_tempo(ev.tick, DEFAULT_TEMPO),
                _ => {}
            }
            if pass == 0 {
                if matches!(ev.msg, Msg::Tempo(_) | Msg::Pattern(_)) {
                    map.push(&clock);
                }
                scan.add(f, &ev);
            } else if let Some(i) = info.as_ref().map(|i: &SongInfo| i.lyrics)
                && let Some(l) = LyricEvent::of(f, &ev, i)
            {
                lyric(&l);
            }
        }
        if pass == 0 {
            let i = scan.finish(kar_file);
            info = Some(i);
            if i.lyrics == LyricMode::None {
                break;
            }
        }
    }
    info.map(|i| (i, map)).ok_or(MidiError::State)
}
