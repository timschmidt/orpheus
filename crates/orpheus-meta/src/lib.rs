//! Tags, album art and lyrics locations (04 §4.4, WP AU-4).
//!
//! A separate crate rather than a module of `orpheus-audio`: the library
//! scanner (AU-12) needs tags without any codec, the decode path needs no
//! tag parser, and each can be fuzzed and paged on its own. (04 §4.4 names
//! the parser `orpheus-audio::meta`; this is that parser under its own name.)
//!
//! Formats: ID3v2.2/2.3/2.4 (Latin-1 with a configurable code page,
//! UTF-16, UTF-8; tag- and frame-level unsynchronisation; extended
//! headers; compressed and encrypted frames skipped), ID3v1/1.1, APEv2,
//! FLAC metadata (Vorbis comments, PICTURE), RIFF `LIST/INFO` and `id3 `
//! chunks, AIFF `ID3 ` chunks; LRC lyric files, ID3 `SYLT`/`USLT` lyric
//! frames. MP4 `ilst` and Ogg comment headers come with their containers
//! (AU-10, AU-7).
//!
//! Bounded memory: every text field is at most 127 bytes of UTF-8;
//! pictures and lyrics are recorded as locations (offset, length), never
//! read; the reader peeks at most one header plus 512 bytes at a time and
//! skips the rest (seeking over large blocks). Restartable: [`TagReader::step`]
//! answers `NeedData` without consuming anything it still needs.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod ape;
mod flac;
mod id3;
mod io;
pub mod lrc;
mod riff;
pub mod sylt;
pub mod text;
mod vorbis;

use orpheus_audio::{ByteSource, Format, Gapless, SeekSource, SourceError};
pub use text::{CodePage, Encoding, Text};

/// Bytes of a unit's content examined (text fields, picture and lyric
/// headers); the rest is skipped.
pub const SCRATCH: usize = 512;
/// The reader's work buffer: one unit header plus [`SCRATCH`].
pub(crate) const BUF: usize = SCRATCH + 16;

/// Where some bytes of the file are (a picture, lyrics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Located {
    /// Absolute file offset to start reading at.
    pub offset: u64,
    /// Logical bytes to discard first (non-zero only with `unsync`).
    pub skip: u32,
    /// Logical length.
    pub len: u32,
    /// The bytes are ID3-unsynchronised (`FF 00` stands for `FF`).
    pub unsync: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mime {
    Jpeg,
    Png,
    Other,
}

impl Mime {
    fn from_bytes(m: &[u8]) -> Self {
        let is = |x: &[u8]| m.eq_ignore_ascii_case(x);
        if is(b"image/jpeg") || is(b"image/jpg") || is(b"jpg") || is(b"jpeg") || is(b"JPG") {
            Self::Jpeg
        } else if is(b"image/png") || is(b"png") {
            Self::Png
        } else {
            Self::Other
        }
    }
}

/// An embedded picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    pub at: Located,
    pub mime: Mime,
    /// ID3/FLAC picture type (3 = front cover).
    pub kind: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsKind {
    /// ID3 `USLT` text in this encoding.
    Unsynced(Encoding),
    /// ID3 `SYLT` frame body (parse with [`sylt::SyltReader`]).
    Synced,
    /// UTF-8 text (Vorbis `LYRICS`, APE `Lyrics`); may be LRC.
    Utf8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lyrics {
    pub at: Located,
    pub kind: LyricsKind,
}

/// ReplayGain and EBU R128 values, fixed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Gains {
    /// Hundredths of a dB.
    pub track_gain: Option<i32>,
    pub album_gain: Option<i32>,
    /// Millionths of full scale.
    pub track_peak: Option<u32>,
    pub album_peak: Option<u32>,
    /// R128 gains (Opus-style), hundredths of a dB.
    pub r128_track: Option<i32>,
    pub r128_album: Option<i32>,
}

/// Everything the tag parsers extract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tags {
    pub title: Text,
    pub artist: Text,
    pub album: Text,
    pub album_artist: Text,
    pub genre: Text,
    pub year: u16,
    pub track: u16,
    pub tracks: u16,
    pub disc: u16,
    pub discs: u16,
    pub gains: Gains,
    pub picture: Option<Picture>,
    pub lyrics: Option<Lyrics>,
    pub synced_lyrics: Option<Lyrics>,
    /// Encoder delay/padding from `iTunSMPB`.
    pub gapless: Option<Gapless>,
}

/// How a key/value pair from Vorbis comments, APE items or `TXXX` frames
/// is applied. `fill`: only set fields that are still empty (lower-priority
/// tag formats).
pub(crate) fn apply_key(tags: &mut Tags, key: &[u8], value: &str, fill: bool) {
    let mut k = [0u8; 24];
    if key.len() > k.len() {
        return;
    }
    for (d, s) in k.iter_mut().zip(key) {
        *d = s.to_ascii_uppercase();
    }
    let k = &k[..key.len()];
    let text = || Text::from_chars(value.chars());
    let set = |t: &mut Text| {
        if !fill || t.is_empty() {
            *t = Text::from_chars(value.chars());
        }
    };
    match k {
        b"TITLE" => set(&mut tags.title),
        b"ARTIST" => set(&mut tags.artist),
        b"ALBUM" => set(&mut tags.album),
        b"ALBUMARTIST" | b"ALBUM ARTIST" | b"ALBUM_ARTIST" => set(&mut tags.album_artist),
        b"GENRE" => {
            if !fill || tags.genre.is_empty() {
                tags.genre = text::genre(&text());
            }
        }
        b"DATE" | b"YEAR" => {
            if (!fill || tags.year == 0)
                && let Some(y) = text::leading_u32(value)
            {
                tags.year = y.min(9999) as u16;
            }
        }
        b"TRACKNUMBER" | b"TRACK" => {
            if !fill || tags.track == 0 {
                let (a, b) = text::number_pair(value);
                tags.track = a;
                if b != 0 {
                    tags.tracks = b;
                }
            }
        }
        b"TRACKTOTAL" | b"TOTALTRACKS" => tags.tracks = text::number_pair(value).0,
        b"DISCNUMBER" | b"DISC" => {
            if !fill || tags.disc == 0 {
                let (a, b) = text::number_pair(value);
                tags.disc = a;
                if b != 0 {
                    tags.discs = b;
                }
            }
        }
        b"DISCTOTAL" | b"TOTALDISCS" => tags.discs = text::number_pair(value).0,
        b"REPLAYGAIN_TRACK_GAIN" => tags.gains.track_gain = gain(value).or(tags.gains.track_gain),
        b"REPLAYGAIN_ALBUM_GAIN" => tags.gains.album_gain = gain(value).or(tags.gains.album_gain),
        b"REPLAYGAIN_TRACK_PEAK" => tags.gains.track_peak = peak(value).or(tags.gains.track_peak),
        b"REPLAYGAIN_ALBUM_PEAK" => tags.gains.album_peak = peak(value).or(tags.gains.album_peak),
        b"R128_TRACK_GAIN" => tags.gains.r128_track = r128(value).or(tags.gains.r128_track),
        b"R128_ALBUM_GAIN" => tags.gains.r128_album = r128(value).or(tags.gains.r128_album),
        b"ITUNSMPB" => tags.gapless = itunsmpb(value).or(tags.gapless),
        _ => {}
    }
}

fn gain(v: &str) -> Option<i32> {
    text::decimal(v, 2).map(|g| g.clamp(-100_000, 100_000) as i32)
}

fn peak(v: &str) -> Option<u32> {
    text::decimal(v, 6).filter(|p| *p >= 0).map(|p| p.min(i64::from(u32::MAX)) as u32)
}

/// Q7.8 dB as a decimal integer string → hundredths of a dB.
fn r128(v: &str) -> Option<i32> {
    let n = text::decimal(v, 0)?;
    Some((n.clamp(-32768, 32767) * 100 / 256) as i32)
}

/// `iTunSMPB`: " 00000000 00000210 000003C4 0000000000ABCDEF …" (hex):
/// field 2 is the encoder delay, field 3 the padding.
fn itunsmpb(v: &str) -> Option<Gapless> {
    let mut it = v.split_ascii_whitespace();
    it.next()?;
    let delay = u32::from_str_radix(it.next()?, 16).ok()?;
    let padding = u32::from_str_radix(it.next()?, 16).ok()?;
    Some(Gapless { delay, padding })
}

/// Options for [`TagReader`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// Character set of 8-bit text that is not marked as UTF-8.
    pub code_page: CodePage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaStep {
    /// Call again once `available() >= n` (or at the end).
    NeedData(usize),
    /// The work bound of one call was reached: call again (lets a
    /// cooperative task yield between calls).
    Continue,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaError {
    Source(SourceError),
}

impl From<SourceError> for MetaError {
    fn from(e: SourceError) -> Self {
        Self::Source(e)
    }
}

/// What a sub-parser reports.
pub(crate) enum Progress {
    Need(usize),
    Done,
}

impl From<io::Pending> for Progress {
    fn from(p: io::Pending) -> Self {
        match p {
            io::Pending::Need(n) => Self::Need(n),
            io::Pending::Eof => Self::Done,
        }
    }
}

macro_rules! tryp {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(p) => return Ok(crate::Progress::from(p)),
        }
    };
}
pub(crate) use tryp;

#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Leading ID3v2 tags (any format may have them).
    Leading(Option<id3::Id3>),
    Flac(flac::Flac),
    Riff(riff::Riff),
    /// ID3v1 and APEv2 at the end of the file.
    Trailing(ape::Tail),
    Done,
}

/// The tag reader for one file: create it with the probed format, call
/// [`TagReader::step`] until it answers `Done`. Start at the beginning of
/// the file (before any leading ID3v2 tag).
pub struct TagReader {
    format: Format,
    opts: Opts,
    phase: Phase,
    scratch: [u8; BUF],
}

impl TagReader {
    #[must_use]
    pub fn new(format: Format, opts: Opts) -> Self {
        Self { format, opts, phase: Phase::Leading(None), scratch: [0; BUF] }
    }

    pub fn step(&mut self, src: &mut dyn SeekSource, tags: &mut Tags) -> Result<MetaStep, MetaError> {
        // Bound the work of one call; each pass consumes, seeks forward or
        // changes phase.
        for _ in 0..256 {
            let p = match &mut self.phase {
                Phase::Leading(state) => {
                    if state.is_none() {
                        match io::peek(src, 3) {
                            Ok(v) if v.starts_with_at(0, b"ID3") => {
                                *state = Some(id3::Id3::new(src.position()));
                            }
                            Ok(_) | Err(io::Pending::Eof) => {
                                self.phase = self.after_leading();
                                continue;
                            }
                            Err(io::Pending::Need(n)) => return Ok(MetaStep::NeedData(n)),
                        }
                    }
                    let Some(st) = state.as_mut() else { continue };
                    let before = src.position();
                    match st.step(src, tags, &self.opts, &mut self.scratch, false)? {
                        Progress::Need(n) => return Ok(MetaStep::NeedData(n)),
                        // A tag cut off by the end of the file.
                        Progress::Done if src.position() == before => self.phase = self.after_leading(),
                        // Another tag may follow.
                        Progress::Done => *state = None,
                    }
                    continue;
                }
                Phase::Flac(f) => f.step(src, tags, &self.opts, &mut self.scratch)?,
                Phase::Riff(r) => r.step(src, tags, &self.opts, &mut self.scratch)?,
                Phase::Trailing(t) => t.step(src, tags, &self.opts, &mut self.scratch)?,
                Phase::Done => return Ok(MetaStep::Done),
            };
            match p {
                Progress::Need(n) => return Ok(MetaStep::NeedData(n)),
                Progress::Done => {
                    self.phase = match self.phase {
                        Phase::Flac(_) | Phase::Riff(_) => Phase::Trailing(ape::Tail::new(self.format)),
                        _ => Phase::Done,
                    };
                }
            }
        }
        Ok(MetaStep::Continue)
    }

    fn after_leading(&self) -> Phase {
        match self.format {
            Format::Flac => Phase::Flac(flac::Flac::new()),
            Format::Wav | Format::Aiff => Phase::Riff(riff::Riff::new(self.format == Format::Aiff)),
            _ => Phase::Trailing(ape::Tail::new(self.format)),
        }
    }
}

/// Read all tags of a file held completely in `src` (tests, host tools).
pub fn read_all(src: &mut dyn SeekSource, format: Format, opts: Opts) -> Result<Tags, MetaError> {
    let mut r = TagReader::new(format, opts);
    let mut tags = Tags::default();
    // Each `Continue` stands for at least one unit of a finite file.
    let limit = src.len().unwrap_or(1 << 32);
    for _ in 0..=limit {
        match r.step(src, &mut tags)? {
            MetaStep::Done => return Ok(tags),
            // Everything was available: nothing more can come.
            MetaStep::NeedData(_) => return Ok(tags),
            MetaStep::Continue => {}
        }
    }
    Ok(tags)
}

/// Whether the source is exhausted (helper for parsers).
pub(crate) fn at_eof(src: &dyn ByteSource) -> bool {
    src.at_end() && src.available() == 0
}

#[cfg(test)]
mod tests;
