//! Standard MIDI Files: the header, the track chunks and the time-ordered
//! merge of the tracks, streamed through fixed track cursors.
//!
//! # Streaming with bounded memory
//!
//! [`Smf`] never holds the file. It keeps up to [`MAX_TRACKS`] cursors, each
//! with a [`WINDOW`]-byte window onto its track, and asks for bytes when a
//! window runs dry: every call that may need data returns [`Pull::Need`]
//! with a file offset and a length; the caller reads those bytes (from the
//! card, from memory) and hands them to [`Smf::supply`], then calls again.
//! Nothing is consumed by a call that answers `Need`, so the protocol works
//! from an `async` task as well as from a plain slice
//! ([`crate::source::MidiSource`] is the synchronous driver).
//!
//! # Lengths are never trusted
//!
//! * Chunk lengths are clamped to the file; a chunk running past the end is
//!   cut there and ends the walk. At most [`MAX_TRACKS`] `MTrk` chunks are
//!   accepted (more: [`MidiError::TooManyTracks`]), at most [`MAX_CHUNKS`]
//!   chunks are walked; the header's track count is ignored.
//! * Meta and SysEx lengths are clamped to the track; skipping never reads.
//! * Variable-length numbers longer than four bytes, data bytes with the
//!   top bit set, a data byte with no running status in effect, and the
//!   status bytes `0xF1`–`0xFE` end the track (the rest of the file plays).
//! * A track without an end-of-track event ends where its bytes end; the
//!   cursor then reports a synthetic [`Msg::EndOfTrack`].
//! * Delta times add up in 64 bits, saturating; huge deltas are the timing
//!   layer's business ([`crate::timing`] caps a song's length).
//!
//! # Order
//!
//! Format 0 and 1: events of all tracks in tick order; equal ticks go by
//! track number, then file order. Format 2 (independent patterns): the
//! tracks one after the other, each starting where the previous ended, with
//! a [`Msg::Pattern`] at each start. A format-0 file with several tracks is
//! played like format 1.
//!
//! Each call of [`Smf::next_event`] parses at most one delta time per track and
//! one event: a bounded step whatever the file holds.

/// Track cursors (more `MTrk` chunks are refused).
pub const MAX_TRACKS: usize = 64;
/// Bytes of a cursor's window onto its track.
pub const WINDOW: usize = 64;
/// Bytes of a text meta event that are kept (lyrics, names); the rest is
/// skipped.
pub const TEXT_MAX: usize = 48;
/// Chunks walked at most (tracks and unknown chunks).
pub const MAX_CHUNKS: u32 = MAX_TRACKS as u32 + 256;
/// Bytes read for the file header (an `RMID` wrapper and `MThd`).
pub const HEAD_LEN: usize = 34;

// The window holds a meta event's header (6 bytes at most) and the kept text.
const _: () = assert!(WINDOW >= 6 + TEXT_MAX);

/// Bytes the caller is asked to read: `len` bytes at file offset `offset`
/// (fewer only at the end of the file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Need {
    /// File offset.
    pub offset: u32,
    /// Bytes wanted (≤ [`WINDOW`] or [`HEAD_LEN`]).
    pub len: u16,
}

/// The answer of a streaming call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull<T> {
    /// Done.
    Ready(T),
    /// Read these bytes, [`Smf::supply`] them, call again.
    Need(Need),
    /// Nothing more.
    End,
}

/// Why a file cannot be played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiError {
    /// No `MThd` header (or `RIFF`/`RMID` without one inside).
    NotMidi,
    /// The header is cut short.
    Truncated,
    /// A format other than 0, 1, 2.
    Format(u16),
    /// Division 0 or an SMPTE rate other than 24, 25, 29.97, 30.
    Division,
    /// No track chunk.
    NoTracks,
    /// More track chunks than [`MAX_TRACKS`].
    TooManyTracks,
    /// The source failed.
    Io,
    /// A call out of order (render before open).
    State,
}

impl MidiError {
    /// A message for the user.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NotMidi => "not a MIDI file",
            Self::Truncated => "MIDI header cut short",
            Self::Format(_) => "unknown MIDI file format",
            Self::Division => "invalid MIDI time division",
            Self::NoTracks => "MIDI file has no tracks",
            Self::TooManyTracks => "MIDI file has more than 64 tracks",
            Self::Io => "read error",
            Self::State => "MIDI player not ready",
        }
    }
}

/// Time division of the file.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Division {
    /// Ticks per quarter note (1–32767); tempo events set the quarter.
    Metrical(u16),
    /// SMPTE: frames per second (24, 25, 29 = 29.97 drop frame, 30) and
    /// ticks per frame; tempo events do not change the time.
    Smpte {
        /// Frames per second (29 means 30000/1001).
        fps: u8,
        /// Ticks per frame (1–255).
        tpf: u8,
    },
}

/// The file header.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// 0, 1 or 2 (as stored).
    pub format: u16,
    /// Track count as stored (not trusted).
    pub declared_tracks: u16,
    /// Time division.
    pub division: Division,
    /// File offset of the first chunk after `MThd`.
    pub chunks_at: u32,
}

/// Parse the head of a file (`b` = its first bytes, up to [`HEAD_LEN`]):
/// `MThd`, or an `RMID` (`RIFF`) wrapper whose `data` chunk starts at
/// offset 12 and holds one.
pub fn parse_header(b: &[u8], file_len: u32) -> Result<Header, MidiError> {
    let mut base = 0usize;
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"RMID" {
        if b.len() < 20 || &b[12..16] != b"data" {
            return Err(MidiError::NotMidi);
        }
        base = 20;
    }
    let h = b.get(base..).unwrap_or(&[]);
    if h.len() < 4 || &h[0..4] != b"MThd" {
        return Err(if h.len() < 4 && h == &b"MThd"[..h.len()] {
            MidiError::Truncated
        } else {
            MidiError::NotMidi
        });
    }
    if h.len() < 14 {
        return Err(MidiError::Truncated);
    }
    let len = u32::from_be_bytes([h[4], h[5], h[6], h[7]]);
    if len < 6 {
        return Err(MidiError::Truncated);
    }
    let format = u16::from_be_bytes([h[8], h[9]]);
    if format > 2 {
        return Err(MidiError::Format(format));
    }
    let declared_tracks = u16::from_be_bytes([h[10], h[11]]);
    let d = u16::from_be_bytes([h[12], h[13]]);
    let division = if d & 0x8000 != 0 {
        let fps = (h[12] as i8).unsigned_abs();
        let tpf = h[13];
        if !matches!(fps, 24 | 25 | 29 | 30) || tpf == 0 {
            return Err(MidiError::Division);
        }
        Division::Smpte { fps, tpf }
    } else if d == 0 {
        return Err(MidiError::Division);
    } else {
        Division::Metrical(d)
    };
    let chunks_at = (base as u64 + 8 + u64::from(len)).min(u64::from(file_len)) as u32;
    Ok(Header { format, declared_tracks, division, chunks_at })
}

/// What kind of text a text meta event carries (its type byte).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TextKind {
    /// 0x01: any text (karaoke files put their syllables here).
    Text,
    /// 0x02.
    Copyright,
    /// 0x03: sequence or track name.
    TrackName,
    /// 0x04.
    Instrument,
    /// 0x05: a lyric syllable.
    Lyric,
    /// 0x06.
    Marker,
    /// 0x07.
    Cue,
}

impl TextKind {
    fn of(t: u8) -> Option<Self> {
        Some(match t {
            1 => Self::Text,
            2 => Self::Copyright,
            3 => Self::TrackName,
            4 => Self::Instrument,
            5 => Self::Lyric,
            6 => Self::Marker,
            7 => Self::Cue,
            _ => return None,
        })
    }
}

/// An event of the file.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Msg {
    /// A channel message: `status` (0x80–0xEF, channel in the low nibble)
    /// and its data bytes (`d2` is 0 for program change and channel
    /// pressure).
    Channel {
        /// Status byte.
        status: u8,
        /// First data byte.
        d1: u8,
        /// Second data byte.
        d2: u8,
    },
    /// Set tempo: microseconds per quarter note (never 0).
    Tempo(u32),
    /// Time signature: `num`/2^`den`, MIDI clocks per metronome click,
    /// 32nd notes per quarter.
    TimeSig {
        /// Numerator.
        num: u8,
        /// Denominator as a power of two.
        den: u8,
        /// MIDI clocks per click.
        clocks: u8,
        /// 32nd notes per 24 MIDI clocks.
        n32: u8,
    },
    /// Key signature: sharps (negative: flats), minor.
    KeySig {
        /// −7 to 7.
        sharps: i8,
        /// Minor key.
        minor: bool,
    },
    /// A text event; the first [`TEXT_MAX`] bytes are in
    /// [`MidiEvent::text`].
    Text(TextKind),
    /// End of a track (from the file or synthetic).
    EndOfTrack,
    /// Format 2: pattern `n` starts.
    Pattern(u8),
    /// Something skipped (SysEx, other meta events, short meta events).
    Other,
}

/// An event with its absolute tick and track.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    /// Ticks from the start of the file (format 2: of the song).
    pub tick: u64,
    /// Track index (chunk order).
    pub track: u8,
    /// The event.
    pub msg: Msg,
    text: [u8; TEXT_MAX],
    text_len: u8,
}

impl MidiEvent {
    /// The kept bytes of a text event (empty for other events).
    #[must_use]
    pub fn text(&self) -> &[u8] {
        self.text.get(..usize::from(self.text_len)).unwrap_or_default()
    }

    /// An event without text.
    #[must_use]
    pub const fn new(tick: u64, track: u8, msg: Msg) -> Self {
        Self { tick, track, msg, text: [0; TEXT_MAX], text_len: 0 }
    }
}

#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum St {
    /// The next thing is a delta time.
    Delta,
    /// `tick` is the next event's; the event is unparsed.
    Ready,
    /// Finished.
    Done,
}

/// Result of parsing from a window.
enum P<T> {
    Ok(T),
    /// More bytes are in the file than in the window.
    Need,
    /// The track ends (or is corrupt) here.
    End,
}

/// One track's cursor.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug)]
struct Track {
    /// File offset of the track's first byte.
    start: u32,
    /// File offset one past its last byte.
    end: u32,
    /// File offset of `win[0]`.
    base: u32,
    lo: u8,
    hi: u8,
    /// Bytes asked for by the last `Need` (a short answer cuts the track).
    asked: u8,
    win: [u8; WINDOW],
    tick: u64,
    status: u8,
    st: St,
}

impl Track {
    const EMPTY: Self = Self {
        start: 0,
        end: 0,
        base: 0,
        lo: 0,
        hi: 0,
        asked: 0,
        win: [0; WINDOW],
        tick: 0,
        status: 0,
        st: St::Done,
    };

    fn rewind(&mut self, tick: u64) {
        self.base = self.start;
        self.lo = 0;
        self.hi = 0;
        self.tick = tick;
        self.status = 0;
        self.st = if self.start < self.end { St::Delta } else { St::Done };
    }

    fn have(&self) -> usize {
        usize::from(self.hi - self.lo)
    }

    fn avail(&self) -> &[u8] {
        self.win.get(usize::from(self.lo)..usize::from(self.hi)).unwrap_or_default()
    }

    /// Bytes from the read position to the end of the track.
    fn left(&self) -> u32 {
        self.end.saturating_sub(self.base + u32::from(self.lo))
    }

    /// Whether `n` bytes from the read position can be had: `Ok` when in
    /// the window, `Need` when the window must be refilled first, `End`
    /// when the track has fewer.
    fn check(&self, n: usize) -> P<()> {
        if self.have() >= n {
            P::Ok(())
        } else if self.left() as usize > self.have() {
            P::Need
        } else {
            P::End
        }
    }

    fn consume(&mut self, n: u32) {
        if n <= u32::from(self.hi - self.lo) {
            self.lo += n as u8;
        } else {
            let pos =
                (u64::from(self.base) + u64::from(self.lo) + u64::from(n)).min(u64::from(self.end)) as u32;
            self.base = pos;
            self.lo = 0;
            self.hi = 0;
        }
    }

    /// Compact the window and say what to read next.
    fn need(&mut self) -> Need {
        let (lo, hi) = (usize::from(self.lo), usize::from(self.hi));
        if lo <= hi && hi <= WINDOW {
            self.win.copy_within(lo..hi, 0);
        }
        self.base += u32::from(self.lo);
        self.hi -= self.lo;
        self.lo = 0;
        let at = self.base + u32::from(self.hi);
        let len = (WINDOW - usize::from(self.hi)).min(self.end.saturating_sub(at) as usize);
        self.asked = len as u8;
        Need { offset: at, len: len as u16 }
    }

    fn supply(&mut self, d: &[u8]) {
        let n = d.len().min(usize::from(self.asked));
        let hi = usize::from(self.hi);
        if let (Some(w), Some(d)) = (self.win.get_mut(hi..hi + n), d.get(..n)) {
            w.copy_from_slice(d);
        }
        self.hi += n as u8;
        if n < usize::from(self.asked) {
            // The source had less than the chunk claimed: the track ends here.
            self.end = self.base + u32::from(self.hi);
        }
        self.asked = 0;
    }

    /// A variable-length number at window offset `at`: (value, bytes).
    fn varlen(&self, at: usize) -> P<(u32, usize)> {
        let mut v = 0u32;
        for i in 0..4 {
            match self.check(at + i + 1) {
                P::Ok(()) => {}
                P::Need => return P::Need,
                P::End => return P::End,
            }
            // `check` made the byte available.
            let Some(&b) = self.avail().get(at + i) else { return P::End };
            v = (v << 7) | u32::from(b & 0x7F);
            if b & 0x80 == 0 {
                return P::Ok((v, i + 1));
            }
        }
        P::End
    }

    fn delta(&mut self) -> P<u32> {
        match self.varlen(0) {
            P::Ok((v, n)) => {
                self.consume(n as u32);
                P::Ok(v)
            }
            P::Need => P::Need,
            P::End => P::End,
        }
    }

    /// Parse the event at the read position into `ev`.
    fn event(&mut self, ev: &mut MidiEvent) -> P<()> {
        match self.check(1) {
            P::Ok(()) => {}
            P::Need => return P::Need,
            P::End => return P::End,
        }
        let Some(&b0) = self.avail().first() else { return P::End };
        match b0 {
            0x00..=0xEF => {
                let (status, i) = if b0 >= 0x80 {
                    (b0, 1)
                } else if self.status != 0 {
                    (self.status, 0)
                } else {
                    return P::End;
                };
                let n = if matches!(status & 0xF0, 0xC0 | 0xD0) { 1 } else { 2 };
                match self.check(i + n) {
                    P::Ok(()) => {}
                    P::Need => return P::Need,
                    P::End => return P::End,
                }
                let a = self.avail();
                // (`check` made them available; a missing byte reads as an
                // invalid one and ends the track.)
                let d1 = a.get(i).copied().unwrap_or(0x80);
                let d2 = if n == 2 { a.get(i + 1).copied().unwrap_or(0x80) } else { 0 };
                if d1 & 0x80 != 0 || d2 & 0x80 != 0 {
                    return P::End;
                }
                self.consume((i + n) as u32);
                self.status = status;
                ev.msg = Msg::Channel { status, d1, d2 };
                P::Ok(())
            }
            0xF0 | 0xF7 => {
                let (len, vl) = match self.varlen(1) {
                    P::Ok(x) => x,
                    P::Need => return P::Need,
                    P::End => return P::End,
                };
                self.consume((1 + vl) as u32);
                self.consume(len);
                self.status = 0;
                ev.msg = Msg::Other;
                P::Ok(())
            }
            0xFF => {
                match self.check(2) {
                    P::Ok(()) => {}
                    P::Need => return P::Need,
                    P::End => return P::End,
                }
                let Some(&ty) = self.avail().get(1) else { return P::End };
                let (len, vl) = match self.varlen(2) {
                    P::Ok(x) => x,
                    P::Need => return P::Need,
                    P::End => return P::End,
                };
                let hdr = 2 + vl;
                let kind = TextKind::of(ty);
                let cap = match ty {
                    0x51 => 3,
                    0x58 => 4,
                    0x59 => 2,
                    _ if kind.is_some() => TEXT_MAX,
                    _ => 0,
                };
                let want = hdr + (len as usize).min(cap);
                if let P::Need = self.check(want) {
                    return P::Need;
                }
                // What the track holds of the payload (it may end early).
                let p = (len as usize).min(cap).min(self.have() - hdr);
                let Some(d) = self.avail().get(hdr..hdr + p) else { return P::End };
                ev.msg = match ty {
                    0x2F => Msg::EndOfTrack,
                    0x51 if p == 3 => {
                        let us = u32::from_be_bytes([0, d[0], d[1], d[2]]);
                        if us == 0 { Msg::Other } else { Msg::Tempo(us) }
                    }
                    0x58 if p == 4 => Msg::TimeSig { num: d[0], den: d[1], clocks: d[2], n32: d[3] },
                    0x59 if p == 2 => Msg::KeySig { sharps: d[0] as i8, minor: d[1] != 0 },
                    _ => match kind {
                        Some(k) => {
                            if let Some(t) = ev.text.get_mut(..p) {
                                t.copy_from_slice(d);
                            }
                            ev.text_len = p as u8;
                            Msg::Text(k)
                        }
                        None => Msg::Other,
                    },
                };
                self.consume(hdr as u32);
                self.consume(len);
                self.status = 0;
                P::Ok(())
            }
            _ => P::End,
        }
    }
}

#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Phase {
    /// Nothing opened.
    Idle,
    /// Waiting for the head bytes.
    Head,
    /// Walking chunk headers at `at`.
    Chunks { at: u32, walked: u32 },
    /// Tracks known.
    Open,
}

#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Pend {
    None,
    Head(u16),
    Track(u8),
}

/// The file's structure and its track cursors (≈ 90 bytes per track).
#[derive(bytemuck::Zeroable, Clone, Debug)]
pub struct Smf {
    tracks: [Track; MAX_TRACKS],
    n: u8,
    header: crate::opt::Opt<Header>,
    file_len: u32,
    phase: Phase,
    pend: Pend,
    head: [u8; HEAD_LEN],
    /// Format 2: the playing pattern.
    cur: u8,
    /// Format 2: the next pattern's start was announced.
    started: bool,
}

impl Default for Smf {
    fn default() -> Self {
        Self::new()
    }
}

impl Smf {
    /// No file.
    #[must_use]
    pub fn new() -> Self {
        let mut s = <Self as bytemuck::Zeroable>::zeroed();
        s.clear();
        s
    }

    /// [`Smf::new`] in place (no 6 KB value through the stack).
    pub fn clear(&mut self) {
        self.tracks.fill(Track::EMPTY);
        self.n = 0;
        self.header.set(None);
        self.file_len = 0;
        self.phase = Phase::Idle;
        self.pend = Pend::None;
        self.head.fill(0);
        self.cur = 0;
        self.started = true;
    }

    /// Start opening a file of `file_len` bytes; then call [`Smf::open`]
    /// until it is ready.
    pub fn begin(&mut self, file_len: u32) {
        self.clear();
        self.file_len = file_len;
        self.phase = Phase::Head;
    }

    /// Read the header and find the tracks.
    pub fn open(&mut self) -> Result<Pull<Header>, MidiError> {
        loop {
            match self.phase {
                Phase::Idle => return Err(MidiError::State),
                Phase::Open => {
                    return self.header.get().map(Pull::Ready).ok_or(MidiError::State);
                }
                Phase::Head => {
                    let len = (HEAD_LEN as u32).min(self.file_len) as u16;
                    match self.pend {
                        Pend::Head(got) => {
                            self.pend = Pend::None;
                            let h = parse_header(
                                self.head.get(..usize::from(got)).unwrap_or_default(),
                                self.file_len,
                            )?;
                            self.header.set(Some(h));
                            self.phase = Phase::Chunks { at: h.chunks_at, walked: 0 };
                        }
                        _ => {
                            self.pend = Pend::Head(0);
                            return Ok(Pull::Need(Need { offset: 0, len }));
                        }
                    }
                }
                Phase::Chunks { at, walked } => {
                    if u64::from(at) + 8 > u64::from(self.file_len) || walked >= MAX_CHUNKS {
                        return self.finish_open();
                    }
                    match self.pend {
                        Pend::Head(got) => {
                            self.pend = Pend::None;
                            if got < 8 {
                                return self.finish_open();
                            }
                            let h = &self.head;
                            let len = u32::from_be_bytes([h[4], h[5], h[6], h[7]]);
                            let body = at + 8;
                            let end = (u64::from(body) + u64::from(len)).min(u64::from(self.file_len)) as u32;
                            if &h[0..4] == b"MTrk" {
                                let Some(t) = self.tracks.get_mut(usize::from(self.n)) else {
                                    return Err(MidiError::TooManyTracks);
                                };
                                t.start = body;
                                t.end = end;
                                self.n += 1;
                            }
                            self.phase = Phase::Chunks { at: end, walked: walked + 1 };
                        }
                        _ => {
                            self.pend = Pend::Head(0);
                            return Ok(Pull::Need(Need { offset: at, len: 8 }));
                        }
                    }
                }
            }
        }
    }

    fn finish_open(&mut self) -> Result<Pull<Header>, MidiError> {
        if self.n == 0 {
            return Err(MidiError::NoTracks);
        }
        self.phase = Phase::Open;
        self.rewind();
        self.header.get().map(Pull::Ready).ok_or(MidiError::State)
    }

    /// Hand over the bytes asked for by the last [`Pull::Need`] (fewer at the
    /// end of the file). Bytes nobody asked for are ignored.
    pub fn supply(&mut self, d: &[u8]) {
        match self.pend {
            Pend::Head(_) => {
                let n = d.len().min(HEAD_LEN);
                for (h, b) in self.head.iter_mut().zip(d) {
                    *h = *b;
                }
                self.pend = Pend::Head(n as u16);
            }
            Pend::Track(i) => {
                if let Some(t) = self.tracks.get_mut(usize::from(i)) {
                    t.supply(d);
                }
                self.pend = Pend::None;
            }
            Pend::None => {}
        }
    }

    /// The header, once open.
    #[must_use]
    pub fn header(&self) -> Option<Header> {
        if self.phase == Phase::Open { self.header.get() } else { None }
    }

    /// Track chunks found.
    #[must_use]
    pub fn tracks(&self) -> usize {
        usize::from(self.n)
    }

    /// Byte range of track `i` in the file.
    #[must_use]
    pub fn track_range(&self, i: usize) -> Option<(u32, u32)> {
        self.tracks.iter().take(usize::from(self.n)).nth(i).map(|t| (t.start, t.end))
    }

    fn sequential(&self) -> bool {
        self.header.get().is_some_and(|h| h.format == 2)
    }

    /// Back to the first event.
    pub fn rewind(&mut self) {
        for t in self.tracks.iter_mut().take(usize::from(self.n)) {
            t.rewind(0);
        }
        self.pend = Pend::None;
        self.cur = 0;
        self.started = true;
        if self.sequential() {
            for t in self.tracks.iter_mut().take(usize::from(self.n)).skip(1) {
                t.st = St::Done;
            }
        }
    }

    /// The next event in time order.
    pub fn next_event(&mut self) -> Pull<MidiEvent> {
        if self.phase != Phase::Open {
            return Pull::End;
        }
        let range = if self.sequential() {
            usize::from(self.cur)..usize::from(self.cur) + 1
        } else {
            0..usize::from(self.n)
        };
        // (Indexing through `get`: no panic path, whose location data
        // would be resident on the player.)
        let cur_tick = self.tracks.get(usize::from(self.cur)).map_or(0, |t| t.tick);
        if !self.started {
            self.started = true;
            return Pull::Ready(MidiEvent::new(cur_tick, self.cur, Msg::Pattern(self.cur)));
        }
        // Every live track's next tick.
        for i in range.clone() {
            let Some(t) = self.tracks.get_mut(i) else { break };
            if t.st == St::Delta {
                match t.delta() {
                    P::Ok(d) => {
                        t.tick = t.tick.saturating_add(u64::from(d));
                        t.st = St::Ready;
                    }
                    P::Need => {
                        self.pend = Pend::Track(i as u8);
                        return Pull::Need(t.need());
                    }
                    P::End => t.st = St::Done,
                }
            }
        }
        let mut best: Option<(usize, u64)> = None;
        for (i, t) in self.tracks.iter().enumerate().take(range.end).skip(range.start) {
            if t.st == St::Ready && best.is_none_or(|(_, b)| t.tick < b) {
                best = Some((i, t.tick));
            }
        }
        let Some((i, _)) = best else {
            // Format 2: the next pattern starts where this one ended.
            if self.sequential() && usize::from(self.cur) + 1 < usize::from(self.n) {
                self.cur += 1;
                if let Some(t) = self.tracks.get_mut(usize::from(self.cur)) {
                    t.rewind(cur_tick);
                }
                self.started = false;
                return self.next_event();
            }
            return Pull::End;
        };
        let Some(t) = self.tracks.get_mut(i) else { return Pull::End };
        let mut ev = MidiEvent::new(t.tick, i as u8, Msg::Other);
        match t.event(&mut ev) {
            P::Ok(()) => {
                t.st = if ev.msg == Msg::EndOfTrack { St::Done } else { St::Delta };
                Pull::Ready(ev)
            }
            P::Need => {
                self.pend = Pend::Track(i as u8);
                Pull::Need(t.need())
            }
            P::End => {
                t.st = St::Done;
                ev.msg = Msg::EndOfTrack;
                Pull::Ready(ev)
            }
        }
    }
}
