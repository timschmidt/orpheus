//! Playback service (WP AU-5; docs/plan/04-audio.md §2.2, §6.3, §6.5).
//!
//! A synchronous state machine the firmware's RT audio task drives: feed
//! it [`Event`]s (user commands, "track opened", "reader reached the end of
//! the file", "decoder ended", play position, timer ticks) and carry out
//! the [`Action`]s it returns ("open queue item 17 at frame 0, gapless",
//! "prefetch item 18", "seek", "pause"). It holds no track list: the queue
//! is a length and an identity, items are indices the firmware resolves to
//! [`TrackRef`] locators (from `/.orpheus/queue.bin`), and shuffle is a keyed
//! permutation ([`Shuffle`]), so any queue length costs the same RAM.
//!
//! Gapless hand-over (04 §2.2): when the reader has read the current file
//! to its end the player answers `Prefetch(next)` so the reader continues
//! into the next file; when the decoder reports the end of the stream the
//! player answers `Open { gapless: true }` and the firmware re-opens the
//! decoder in the same arena (inspect → open) while the PCM ring drains —
//! no fade, no silence inserted.
//!
//! Resume (04 §2.2): a [`ResumeRecord`] (track locator, frame, queue
//! position and identity, shuffle seed, repeat) is written through
//! [`PlayerStore`] at pause, stop, track change, every
//! [`Config::save_every_ms`] of playback and at shutdown, alternating
//! between two CRC-checked slots so a power loss mid-write keeps the
//! previous record.
//!
//! `no_std`, no `alloc`, no `unsafe`, no floating point. `Player` is 752
//! bytes.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod image;
pub mod queue;
pub mod shuffle;
pub mod store;

pub use shuffle::Shuffle;
pub use store::{
    Bookmark, Bookmarks, Key, PlayerStore, Repeat, ResumeRecord, TrackRef, load_resume, name_hash,
};

/// User commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Play a new queue of `len` items (identity `id`) from item `start`.
    /// Shuffled, `start` becomes the first of the new order.
    PlayQueue {
        id: u32,
        len: u32,
        start: u32,
    },
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    /// Restart the track, or go to the previous one within the first
    /// [`Config::restart_ms`].
    Prev,
    /// Go to queue item `item`.
    JumpTo(u32),
    /// Seek to an absolute time in the current track.
    SeekMs(u32),
    /// Seek relative to the current position.
    SeekByMs(i32),
    Shuffle(bool),
    Repeat(Repeat),
    /// A–B repeat: set A at the current position, then B; a third press
    /// clears.
    AbPoint,
    AbClear,
    AddBookmark,
    GotoBookmark(u8),
    DeleteBookmark(u8),
    /// Pause after this much playing time; 0 cancels (also a pending
    /// [`Command::SleepAfterTrack`]).
    SleepAfterMs(u32),
    /// Stop at the end of the current track.
    SleepAfterTrack,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Command(Command),
    /// The decoder for the last `Open`/`OpenTrack` is open (and seeked to
    /// its start frame). `frames`: playable length if known.
    Opened {
        track: TrackRef,
        rate: u32,
        frames: Option<u64>,
    },
    /// The last `Open` failed (missing file, unsupported stream).
    OpenFailed,
    /// Frames of the current track played so far (from the PCM sink).
    Position {
        frame: u64,
    },
    /// The reader has read the current file to its end.
    ReaderAtEnd,
    /// The decoder returned `EndOfStream`.
    DecoderEnded,
    /// The decoder gave up (ten corrupt frames in a row).
    DecoderFailed,
    /// The PCM ring has played out after the last track or a stop.
    Drained,
    /// Wall time passed.
    Tick {
        ms: u32,
    },
    /// Power is going away: save now.
    Shutdown,
}

/// What the firmware is to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open queue item `item` and seek to `start_frame`. `gapless`: the
    /// stream ring already holds it (after `Prefetch`), re-open in the same
    /// arena while the PCM ring drains; otherwise fade out, flush and
    /// start fresh. `paused`: open but do not start output.
    Open {
        item: u32,
        start_frame: u64,
        gapless: bool,
        paused: bool,
    },
    /// Open a track outside the queue (bookmark, resume into a different
    /// queue).
    OpenTrack {
        track: TrackRef,
        start_frame: u64,
        paused: bool,
    },
    /// Let the reader continue into queue item `item` after the current
    /// file (gapless read-ahead).
    Prefetch {
        item: u32,
    },
    /// Forget the prefetched file (the next track changed).
    CancelPrefetch,
    Seek {
        frame: u64,
    },
    /// Fade out and hold.
    Pause,
    /// Fade back in.
    Resume,
    /// Fade out, flush, close the decoder.
    Stop,
}

/// Up to four actions per event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Actions {
    a: [Option<Action>; 4],
    n: usize,
}

impl Actions {
    pub fn push(&mut self, x: Action) {
        if self.n < self.a.len() {
            self.a[self.n] = Some(x);
            self.n += 1;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn as_slice(&self) -> [Option<Action>; 4] {
        self.a
    }

    pub fn iter(&self) -> impl Iterator<Item = Action> + '_ {
        self.a[..self.n].iter().flatten().copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// `Prev` within this much of the track start goes to the previous
    /// track; later it restarts the track.
    pub restart_ms: u32,
    /// Resume record interval while playing (04 §2.2: 60 s).
    pub save_every_ms: u32,
    /// Consecutive open/decode failures before playback stops.
    pub max_failures: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { restart_ms: 3000, save_every_ms: 60_000, max_failures: 10 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    /// An `Open` is outstanding.
    Opening,
    Playing,
    Paused,
    /// The queue ended; waiting for the PCM ring to play out.
    Draining,
}

/// The track being played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Current {
    /// Play position in the queue order, and the queue item there.
    pub pos: u32,
    pub item: u32,
    pub track: Option<TrackRef>,
    pub rate: u32,
    pub frames: Option<u64>,
    /// Frames played.
    pub frame: u64,
}

pub struct Player {
    cfg: Config,
    state: State,
    queue_id: u32,
    len: u32,
    shuffle: Option<Shuffle>,
    seed: u32,
    repeat: Repeat,
    cur: Option<Current>,
    /// Play position whose file is being read ahead.
    prefetch: Option<u32>,
    open_paused: bool,
    failures: u32,
    ab: (Option<u64>, Option<u64>),
    sleep_ms: Option<u32>,
    sleep_after_track: bool,
    since_save: u32,
    seq: u32,
    /// Entropy for new shuffle seeds (time played).
    ticks: u32,
    bookmarks: Bookmarks,
    loaded: Option<ResumeRecord>,
}

impl Player {
    /// A stopped player; reads the last resume record and the bookmarks.
    pub fn new(cfg: Config, store: &mut dyn PlayerStore) -> Self {
        let loaded = load_resume(store);
        let mut bm = [0u8; store::BOOKMARKS_LEN];
        let bookmarks = if store.read(Key::Bookmarks, &mut bm) == bm.len() {
            Bookmarks::decode(&bm).unwrap_or_default()
        } else {
            Bookmarks::default()
        };
        Self {
            cfg,
            state: State::Stopped,
            queue_id: 0,
            len: 0,
            shuffle: None,
            seed: loaded.map_or(0, |(r, _)| r.seed),
            repeat: loaded.map_or(Repeat::Off, |(r, _)| r.repeat),
            cur: None,
            prefetch: None,
            open_paused: false,
            failures: 0,
            ab: (None, None),
            sleep_ms: None,
            sleep_after_track: false,
            since_save: 0,
            seq: loaded.map_or(0, |(_, s)| s),
            ticks: 0,
            bookmarks,
            loaded: loaded.map(|(r, _)| r),
        }
    }

    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    #[must_use]
    pub fn current(&self) -> Option<Current> {
        self.cur
    }

    #[must_use]
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    #[must_use]
    pub fn shuffled(&self) -> bool {
        self.shuffle.is_some()
    }

    #[must_use]
    pub fn bookmarks(&self) -> &[Bookmark] {
        self.bookmarks.as_slice()
    }

    /// Queue length and identity (from the last `PlayQueue` or resume).
    #[must_use]
    pub fn len(&self) -> u32 {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn queue_id(&self) -> u32 {
        self.queue_id
    }

    /// A–B repeat points (frames of the current track).
    #[must_use]
    pub fn ab(&self) -> (Option<u64>, Option<u64>) {
        self.ab
    }

    /// Sleep timer: playing time left, and "stop after this track".
    #[must_use]
    pub fn sleep_timer(&self) -> (Option<u32>, bool) {
        (self.sleep_ms, self.sleep_after_track)
    }

    /// The record loaded at start-up, if any.
    #[must_use]
    pub fn saved(&self) -> Option<ResumeRecord> {
        self.loaded
    }

    /// Queue item at play position `pos`.
    #[must_use]
    pub fn item_at(&self, pos: u32) -> u32 {
        self.shuffle.map_or(pos, |s| s.item(pos))
    }

    fn pos_of(&self, item: u32) -> u32 {
        self.shuffle.map_or(item, |s| s.pos(item))
    }

    /// Continue where the saved record says, if its queue is the one now
    /// loaded (`queue_id`, `queue_len`); else open its track directly.
    /// `play`: start playing (else open paused).
    pub fn resume(&mut self, queue_id: u32, queue_len: u32, play: bool, out: &mut Actions) -> bool {
        let Some(r) = self.loaded else { return false };
        self.repeat = r.repeat;
        self.seed = r.seed;
        if r.queue_id == queue_id && r.queue_len == queue_len && r.pos < queue_len {
            self.queue_id = queue_id;
            self.len = queue_len;
            self.shuffle = r.shuffle.then(|| Shuffle::new(queue_len, r.seed));
            self.open(r.pos, r.frame, false, !play, out);
            if let Some(c) = &mut self.cur {
                c.track = r.track;
            }
            true
        } else if let Some(t) = r.track {
            self.state = State::Opening;
            self.open_paused = !play;
            self.cur = Some(Current {
                pos: 0,
                item: u32::MAX,
                track: Some(t),
                rate: 0,
                frames: None,
                frame: r.frame,
            });
            out.push(Action::OpenTrack { track: t, start_frame: r.frame, paused: !play });
            true
        } else {
            false
        }
    }

    fn record(&self) -> ResumeRecord {
        let c = self.cur;
        ResumeRecord {
            track: c.and_then(|c| c.track),
            frame: c.map_or(0, |c| c.frame),
            queue_id: self.queue_id,
            queue_len: self.len,
            pos: c.map_or(0, |c| c.pos),
            item: c.map_or(0, |c| c.item),
            shuffle: self.shuffle.is_some(),
            seed: self.seed,
            repeat: self.repeat,
            playing: matches!(self.state, State::Playing | State::Opening),
        }
    }

    fn save(&mut self, store: &mut dyn PlayerStore) {
        self.seq = self.seq.wrapping_add(1);
        let key = if self.seq & 1 == 1 { Key::ResumeA } else { Key::ResumeB };
        store.write(key, &self.record().encode(self.seq));
        self.since_save = 0;
    }

    fn save_bookmarks(&self, store: &mut dyn PlayerStore) {
        store.write(Key::Bookmarks, &self.bookmarks.encode());
    }

    /// Next play position when the current track ends (`auto`) or on
    /// `Next` (repeat-one does not hold the user back).
    fn next_pos(&self, auto: bool) -> Option<u32> {
        let p = self.cur?.pos;
        if auto && self.repeat == Repeat::One {
            return Some(p);
        }
        if p + 1 < self.len {
            Some(p + 1)
        } else if self.repeat == Repeat::All && self.len > 0 {
            Some(0)
        } else {
            None
        }
    }

    fn prev_pos(&self) -> Option<u32> {
        let p = self.cur?.pos;
        if p > 0 {
            Some(p - 1)
        } else if self.repeat == Repeat::All && self.len > 0 {
            Some(self.len - 1)
        } else {
            None
        }
    }

    fn open(&mut self, pos: u32, frame: u64, gapless: bool, paused: bool, out: &mut Actions) {
        let item = self.item_at(pos);
        self.cur = Some(Current { pos, item, track: None, rate: 0, frames: None, frame });
        self.state = State::Opening;
        self.open_paused = paused;
        self.ab = (None, None);
        out.push(Action::Open { item, start_frame: frame, gapless, paused });
    }

    fn cancel_prefetch(&mut self, out: &mut Actions) {
        if self.prefetch.take().is_some() {
            out.push(Action::CancelPrefetch);
        }
    }

    /// Move on after the current track ended or failed.
    fn advance(&mut self, failed: bool, store: &mut dyn PlayerStore, out: &mut Actions) {
        if failed {
            self.failures += 1;
            if self.failures >= self.cfg.max_failures.min(self.len.max(1)) {
                self.cancel_prefetch(out);
                self.state = State::Stopped;
                out.push(Action::Stop);
                self.save(store);
                return;
            }
        }
        let prefetched = if failed { None } else { self.prefetch.take() };
        if failed {
            self.cancel_prefetch(out);
        }
        // Repeat-one after a failure would retry the same broken file.
        let next = prefetched.or_else(|| if failed { self.next_pos(false) } else { self.next_pos(true) });
        let paused = self.state == State::Paused;
        match next {
            Some(p) if !self.sleep_after_track => self.open(p, 0, prefetched.is_some(), paused, out),
            _ => {
                self.sleep_after_track = false;
                self.state = State::Draining;
                // A later Play (or resume) starts at the track that would
                // have come next, or at the start of the queue.
                let pos = next.unwrap_or(0);
                self.cur = Some(Current {
                    pos,
                    item: self.item_at(pos),
                    track: None,
                    rate: 0,
                    frames: None,
                    frame: 0,
                });
                self.save(store);
            }
        }
    }

    fn seek_to(&mut self, frame: u64, out: &mut Actions) {
        let Some(c) = &mut self.cur else { return };
        if c.rate == 0 {
            return;
        }
        let frame = c.frames.map_or(frame, |n| frame.min(n.saturating_sub(1)));
        c.frame = frame;
        out.push(Action::Seek { frame });
    }

    fn ms_to_frames(&self, ms: u64) -> u64 {
        self.cur.map_or(0, |c| ms * u64::from(c.rate) / 1000)
    }

    /// Handle one event; the actions to carry out (in order) are appended
    /// to `out`.
    pub fn handle(&mut self, ev: Event, store: &mut dyn PlayerStore, out: &mut Actions) {
        match ev {
            Event::Command(c) => self.command(c, store, out),
            Event::Opened { track, rate, frames } => {
                if self.state != State::Opening {
                    return;
                }
                if let Some(c) = &mut self.cur {
                    c.track = Some(track);
                    c.rate = rate;
                    c.frames = frames;
                }
                self.failures = 0;
                self.state = if self.open_paused { State::Paused } else { State::Playing };
                self.save(store);
            }
            Event::OpenFailed => {
                if self.state == State::Opening {
                    self.advance(true, store, out);
                }
            }
            Event::DecoderFailed => {
                if matches!(self.state, State::Playing | State::Paused) {
                    self.advance(true, store, out);
                }
            }
            Event::Position { frame } => {
                let Some(c) = &mut self.cur else { return };
                c.frame = frame;
                if let (Some(a), Some(b)) = self.ab
                    && frame >= b
                {
                    c.frame = a;
                    out.push(Action::Seek { frame: a });
                }
            }
            Event::ReaderAtEnd => {
                if self.prefetch.is_none()
                    && matches!(self.state, State::Playing | State::Paused)
                    && !self.sleep_after_track
                    && let Some(p) = self.next_pos(true)
                {
                    self.prefetch = Some(p);
                    out.push(Action::Prefetch { item: self.item_at(p) });
                }
            }
            Event::DecoderEnded => {
                if matches!(self.state, State::Playing | State::Paused) {
                    self.advance(false, store, out);
                }
            }
            Event::Drained => {
                if self.state == State::Draining {
                    self.state = State::Stopped;
                }
            }
            Event::Tick { ms } => {
                if self.state != State::Playing {
                    return;
                }
                self.ticks = self.ticks.wrapping_add(ms);
                self.since_save = self.since_save.saturating_add(ms);
                if let Some(left) = self.sleep_ms {
                    if left <= ms {
                        self.sleep_ms = None;
                        self.state = State::Paused;
                        out.push(Action::Pause);
                        self.save(store);
                        return;
                    }
                    self.sleep_ms = Some(left - ms);
                }
                if self.since_save >= self.cfg.save_every_ms {
                    self.save(store);
                }
            }
            Event::Shutdown => self.save(store),
        }
    }

    fn command(&mut self, c: Command, store: &mut dyn PlayerStore, out: &mut Actions) {
        match c {
            Command::PlayQueue { id, len, start } => {
                self.cancel_prefetch(out);
                self.queue_id = id;
                self.len = len;
                if len == 0 {
                    self.cur = None;
                    self.state = State::Stopped;
                    out.push(Action::Stop);
                    return;
                }
                let start = start.min(len - 1);
                if self.shuffle.is_some() {
                    self.seed = seed_starting_with(len, self.seed, start);
                }
                self.shuffle = self.shuffle.map(|_| Shuffle::new(len, self.seed));
                self.failures = 0;
                let pos = self.pos_of(start);
                self.open(pos, 0, false, false, out);
            }
            Command::Play => match self.state {
                State::Paused => {
                    self.state = State::Playing;
                    out.push(Action::Resume);
                }
                State::Stopped | State::Draining => {
                    if let Some(c) = self.cur.filter(|c| c.pos < self.len) {
                        self.open(c.pos, c.frame, false, false, out);
                    } else if self.len > 0 {
                        self.open(0, 0, false, false, out);
                    }
                }
                State::Opening => self.open_paused = false,
                State::Playing => {}
            },
            Command::Pause => match self.state {
                State::Playing => {
                    self.state = State::Paused;
                    out.push(Action::Pause);
                    self.save(store);
                }
                State::Opening => self.open_paused = true,
                _ => {}
            },
            Command::Toggle => {
                let c = if matches!(self.state, State::Playing)
                    || (self.state == State::Opening && !self.open_paused)
                {
                    Command::Pause
                } else {
                    Command::Play
                };
                self.command(c, store, out);
            }
            Command::Stop => {
                if self.state != State::Stopped {
                    self.cancel_prefetch(out);
                    self.state = State::Stopped;
                    out.push(Action::Stop);
                    self.save(store);
                }
            }
            Command::Next => {
                if self.cur.is_none() {
                    return;
                }
                self.cancel_prefetch(out);
                let paused = self.state == State::Paused;
                match self.next_pos(false) {
                    Some(p) => self.open(p, 0, false, paused, out),
                    None => {
                        self.state = State::Stopped;
                        out.push(Action::Stop);
                        self.save(store);
                    }
                }
            }
            Command::Prev => {
                let Some(c) = self.cur else { return };
                let restart = c.rate != 0 && c.frame >= self.ms_to_frames(u64::from(self.cfg.restart_ms));
                match self.prev_pos() {
                    Some(p) if !restart => {
                        self.cancel_prefetch(out);
                        let paused = self.state == State::Paused;
                        self.open(p, 0, false, paused, out);
                    }
                    _ => self.seek_to(0, out),
                }
            }
            Command::JumpTo(item) => {
                if item < self.len {
                    self.cancel_prefetch(out);
                    let pos = self.pos_of(item);
                    self.open(pos, 0, false, false, out);
                }
            }
            Command::SeekMs(ms) => {
                let f = self.ms_to_frames(u64::from(ms));
                self.seek_to(f, out);
            }
            Command::SeekByMs(d) => {
                let Some(c) = self.cur else { return };
                let delta = self.ms_to_frames(u64::from(d.unsigned_abs()));
                let f = if d < 0 { c.frame.saturating_sub(delta) } else { c.frame + delta };
                self.seek_to(f, out);
            }
            Command::Shuffle(on) => {
                if on == self.shuffle.is_some() {
                    return;
                }
                self.cancel_prefetch(out);
                if on {
                    // A new order each time it is switched on.
                    self.seed = self.seed.wrapping_mul(0x0100_0193).wrapping_add(self.ticks | 1);
                    self.shuffle = Some(Shuffle::new(self.len, self.seed));
                } else {
                    self.shuffle = None;
                }
                // The current track stays; the order around it changes.
                if let Some(item) = self.cur.map(|c| c.item).filter(|&i| i < self.len) {
                    let pos = self.pos_of(item);
                    if let Some(c) = &mut self.cur {
                        c.pos = pos;
                    }
                }
            }
            Command::Repeat(r) => {
                if r != self.repeat {
                    self.cancel_prefetch(out);
                    self.repeat = r;
                }
            }
            Command::AbPoint => {
                let Some(c) = self.cur else { return };
                self.ab = match self.ab {
                    (None, _) => (Some(c.frame), None),
                    (Some(a), None) if c.frame > a => (Some(a), Some(c.frame)),
                    _ => (None, None),
                };
            }
            Command::AbClear => self.ab = (None, None),
            Command::AddBookmark => {
                if let Some(c) = self.cur
                    && let Some(t) = c.track
                {
                    self.bookmarks.push(Bookmark { track: t, frame: c.frame, rate: c.rate });
                    self.save_bookmarks(store);
                }
            }
            Command::GotoBookmark(i) => {
                let Some(&b) = self.bookmarks.as_slice().get(usize::from(i)) else { return };
                if self.cur.and_then(|c| c.track) == Some(b.track) && self.state != State::Stopped {
                    self.seek_to(b.frame, out);
                } else {
                    self.cancel_prefetch(out);
                    let pos = self.cur.map_or(0, |c| c.pos);
                    self.cur = Some(Current {
                        pos,
                        item: u32::MAX,
                        track: Some(b.track),
                        rate: 0,
                        frames: None,
                        frame: b.frame,
                    });
                    self.state = State::Opening;
                    self.open_paused = false;
                    out.push(Action::OpenTrack { track: b.track, start_frame: b.frame, paused: false });
                }
            }
            Command::DeleteBookmark(i) => {
                if self.bookmarks.remove(usize::from(i)) {
                    self.save_bookmarks(store);
                }
            }
            Command::SleepAfterMs(ms) => {
                // Replaces any timer, "after this track" included.
                self.sleep_ms = (ms > 0).then_some(ms);
                self.sleep_after_track = false;
            }
            Command::SleepAfterTrack => {
                self.sleep_after_track = true;
                self.cancel_prefetch(out);
            }
        }
    }
}

/// A seed near `seed` whose order of `len` items starts with `first`, so a
/// shuffled queue started at an item still plays every item once. Expected
/// `len` tries (a 256-item folder: at most 2,064, a few ms); after the bound
/// (20,000 tries, ≈ 0.2 s at 48 MHz, only for queues over 2,500 items)
/// `seed` itself: the queue then starts at `first` wherever it falls in the
/// order.
fn seed_starting_with(len: u32, seed: u32, first: u32) -> u32 {
    let mut s = seed;
    for _ in 0..len.saturating_mul(8).saturating_add(16).min(20_000) {
        if Shuffle::new(len, s).item(0) == first {
            return s;
        }
        s = s.wrapping_add(0x9E37_79B9);
    }
    seed
}

#[cfg(test)]
mod tests;
