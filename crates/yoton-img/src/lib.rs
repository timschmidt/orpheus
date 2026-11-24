//! Streaming picture decoders, the scaler to the RGB565 screen and the
//! thumbnail cache format (docs/plan/04-audio.md §7, WP AU-14).
//!
//! * Compressed bytes come from a [`ByteSource`] (the stream ring on the
//!   device; [`source::ArtSource`] narrows it to a picture embedded in a tag),
//!   so pictures in files and album art inside tags take the same path.
//! * Pixels leave as RGB888 rows through a [`RowSink`]; none of the decoders
//!   holds a full image unless it is small enough to fit the arena (Adam7
//!   PNG, interlaced GIF, progressive JPEG coefficients) and the
//!   [`scale::Scaler`] sink turns the rows into RGB565 in a frame buffer or a
//!   thumbnail buffer (area-weighted down-scaling, bilinear up-scaling,
//!   orientation, letterbox, ordered dithering).
//! * [`thumb`] defines the thumbnail cache files on the card and reads and
//!   writes them through a two-method file trait.
//!
//! Arena high-water marks (from `cargo test`, 104 KiB arena): JPEG baseline
//! 8–25 KiB at 640 × 480 (7.7 KiB of it Huffman/quantisation tables),
//! 71 KiB for 2400 × 1800 at full size, 10 KiB at 1/8; progressive JPEG
//! 37–99 KiB (coefficient storage sized to fit); PNG 37–52 KiB (32 KiB
//! window); GIF 17–48 KiB; BMP and QOI one RGB row (+ 768 B palette).
//! * All memory comes from a caller-provided arena (the mode pool's decoder
//!   arena: up to 104 KiB with no audio, otherwise the codec arena's lendable
//!   remainder). A decoder computes its need once the header is parsed,
//!   chooses a reduced resolution when the full one does not fit (JPEG DCT
//!   scaling 1/2…1/8, Adam7 pass subsets, GIF interlace pass 1) and fails
//!   with [`Error::Arena`] (and the needed size) when even the smallest does
//!   not fit.
//! * Decoders are restartable state machines: [`Decoder::step`] does a
//!   bounded amount of work and answers [`Step::NeedData`] (call again when
//!   that many bytes are buffered), [`Step::Progress`] or [`Step::Done`]. They
//!   never peek more than [`MAX_PEEK`] bytes.
//!
//! `no_std`, no `alloc`, no `unsafe`, no floating point.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod bmp;
pub mod gif;
mod inflate;
pub mod jpeg;
pub mod png;
pub mod qoi;
pub mod scale;
pub mod source;
pub mod thumb;

pub use scale::{Fit, Orient, Placement, Scaler, Target};
pub use yoton_audio::{Arena, ArenaFull, ByteSource, SliceSource, View};

/// One pixel as decoders deliver it (alpha already blended over the
/// background).
pub type Rgb = [u8; 3];

/// The most bytes any decoder asks the source to buffer at once.
pub const MAX_PEEK: usize = 8 * 1024;

/// Largest width or height accepted by any decoder.
pub const MAX_DIM: u32 = 32768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Qoi,
}

/// Recognise a picture by its first bytes (at least 8 should be given).
#[must_use]
pub fn probe(head: &[u8]) -> Option<Format> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if head.starts_with(b"qoif") {
        Some(Format::Qoi)
    } else if head.starts_with(b"BM") {
        Some(Format::Bmp)
    } else {
        None
    }
}

/// What a decoder found in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub format: Format,
    /// Stored size (before EXIF orientation).
    pub width: u32,
    pub height: u32,
    /// EXIF orientation (JPEG); `Orient::NORMAL` elsewhere.
    pub orient: Orient,
    /// Progressive JPEG, Adam7 PNG, interlaced GIF.
    pub interlaced: bool,
    /// The picture has transparency (blended over [`Options::background`]).
    pub alpha: bool,
}

/// What the decoder is about to deliver to the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Pixels per delivered row and number of delivered rows.
    pub width: u32,
    pub height: u32,
    /// The image area the delivered rows cover (stored size). Larger than
    /// `width`/`height` when the decoder reduced the resolution; the two
    /// ratios may differ (GIF interlace pass 1 reduces rows only).
    pub full_width: u32,
    pub full_height: u32,
    /// EXIF orientation of the picture.
    pub orient: Orient,
    /// Rows arrive bottom row first (BMP).
    pub bottom_up: bool,
    /// Less detail than the delivered resolution suggests: progressive JPEG
    /// refinement scans skipped or DC-only streaming, or pixels that are
    /// point samples rather than averages (Adam7 grid, GIF pass 1). The
    /// Pictures app notes this in Details.
    pub approximate: bool,
}

/// Whether the decoder should go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// The sink has every row it needs; the decoder answers `Done` next.
    Stop,
}

/// Where decoded rows go.
pub trait RowSink {
    /// The smallest delivered size that is still useful (decoders reduce
    /// resolution down to this, never below unless memory forces it).
    /// `full_w`/`full_h` are the stored size, `orient` its EXIF orientation.
    fn wanted(&self, full_w: u32, full_h: u32, orient: Orient) -> (u32, u32) {
        let _ = orient;
        (full_w, full_h)
    }

    /// Called once before the first row.
    fn begin(&mut self, frame: &Frame);

    /// Row `y` (0 = first delivered row; with `bottom_up` that is the bottom
    /// one). Rows arrive in order, each exactly once.
    fn row(&mut self, y: u32, px: &[Rgb]) -> Flow;
}

/// Result of one [`Decoder::step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Call again once `available() >= n` (or the source is at its end).
    NeedData(usize),
    /// Some work was done (header parsed, rows delivered): call again.
    Progress,
    /// Every row was delivered (or the sink said stop).
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// Arithmetic-coded, lossless, hierarchical or 12-bit JPEG.
    JpegProcess,
    /// Component count or sampling factors outside what the decoder handles.
    Layout,
    /// A header unit is larger than [`MAX_PEEK`].
    HeaderTooLarge,
    /// Width or height 0 or above [`MAX_DIM`].
    Dimensions,
    /// BMP compression other than none, RLE4/8, bit fields.
    Compression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not a format this crate knows.
    UnknownFormat,
    /// The data contradicts the format.
    Corrupt,
    /// The source ended before the picture did. Rows delivered so far stay
    /// valid.
    Truncated,
    Unsupported(Unsupported),
    /// The arena is too small even at the smallest reduction.
    Arena {
        needed: usize,
        capacity: usize,
    },
}

impl From<ArenaFull> for Error {
    fn from(e: ArenaFull) -> Self {
        Self::Arena { needed: e.needed, capacity: e.capacity }
    }
}

/// Decoder options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// Transparent pixels are blended over this colour.
    pub background: Rgb,
}

/// `fg` over `bg` with alpha `a` (rounded; exact at 0 and 255).
#[inline]
#[must_use]
pub fn blend(fg: Rgb, a: u8, bg: Rgb) -> Rgb {
    match a {
        255 => fg,
        0 => bg,
        _ => {
            let a = u32::from(a);
            let m = |f: u8, b: u8| ((u32::from(f) * a + u32::from(b) * (255 - a) + 127) / 255) as u8;
            [m(fg[0], bg[0]), m(fg[1], bg[1]), m(fg[2], bg[2])]
        }
    }
}

/// Why bytes could not be had.
pub(crate) enum Pending {
    Need(usize),
    Eof,
}

impl From<Pending> for Result<Step, Error> {
    fn from(p: Pending) -> Self {
        match p {
            Pending::Need(n) => Ok(Step::NeedData(n)),
            Pending::Eof => Err(Error::Truncated),
        }
    }
}

/// `n` bytes at the read position, or why not.
pub(crate) fn peek(src: &dyn ByteSource, n: usize) -> Result<View<'_>, Pending> {
    if let Some(v) = View::peek(src, n) {
        return Ok(v);
    }
    if src.at_end() || n > MAX_PEEK { Err(Pending::Eof) } else { Err(Pending::Need(n)) }
}

/// Evaluate a `Result<T, Pending>`; on `Pending` return the matching
/// `Step::NeedData` or `Error::Truncated` from the enclosing `step`.
macro_rules! tryp {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(p) => return <Result<crate::Step, crate::Error>>::from(p),
        }
    };
}
pub(crate) use tryp;

/// Fail with the total need unless `need` more bytes (alignment included)
/// fit the arena: decoders check their whole need before allocating, so
/// the reported size is enough for the whole picture.
pub(crate) fn reserve(arena: &Arena<'_>, need: usize) -> Result<(), Error> {
    if arena.remaining() < need {
        Err(Error::Arena { needed: arena.used() + need, capacity: arena.capacity() })
    } else {
        Ok(())
    }
}

/// Check a width and height.
pub(crate) fn check_dims(w: u32, h: u32) -> Result<(), Error> {
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        Err(Error::Unsupported(Unsupported::Dimensions))
    } else {
        Ok(())
    }
}

enum State<'a> {
    Probe(Option<Arena<'a>>),
    Jpeg(jpeg::Jpeg<'a>),
    Png(png::Png<'a>),
    Gif(gif::Gif<'a>),
    Bmp(bmp::Bmp<'a>),
    Qoi(qoi::Qoi<'a>),
}

/// Any supported picture: probes the format from the first bytes, then
/// forwards to the format's decoder.
pub struct Decoder<'a> {
    state: State<'a>,
    opts: Options,
    capacity: usize,
}

impl<'a> Decoder<'a> {
    /// A decoder taking all its memory from `arena`.
    #[must_use]
    pub fn new(arena: &'a mut [u8], opts: Options) -> Self {
        let capacity = arena.len();
        Self { state: State::Probe(Some(Arena::new(arena))), opts, capacity }
    }

    pub fn step(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        if let State::Probe(arena) = &mut self.state {
            let n = 8.min(src.available().max(if src.at_end() { 0 } else { 8 }));
            let v = tryp!(peek(src, n));
            let mut head = [0u8; 8];
            v.copy_to(0, &mut head[..n]);
            let fmt = probe(&head[..n]).ok_or(Error::UnknownFormat)?;
            let Some(arena) = arena.take() else { return Err(Error::Corrupt) };
            let o = self.opts;
            self.state = match fmt {
                Format::Jpeg => State::Jpeg(jpeg::Jpeg::new(arena, o)),
                Format::Png => State::Png(png::Png::new(arena, o)),
                Format::Gif => State::Gif(gif::Gif::new(arena, o)),
                Format::Bmp => State::Bmp(bmp::Bmp::new(arena, o)),
                Format::Qoi => State::Qoi(qoi::Qoi::new(arena, o)),
            };
        }
        match &mut self.state {
            State::Probe(_) => Err(Error::Corrupt),
            State::Jpeg(d) => d.step(src, sink),
            State::Png(d) => d.step(src, sink),
            State::Gif(d) => d.step(src, sink),
            State::Bmp(d) => d.step(src, sink),
            State::Qoi(d) => d.step(src, sink),
        }
    }

    /// The header, once parsed.
    #[must_use]
    pub fn info(&self) -> Option<Info> {
        match &self.state {
            State::Probe(_) => None,
            State::Jpeg(d) => d.info(),
            State::Png(d) => d.info(),
            State::Gif(d) => d.info(),
            State::Bmp(d) => d.info(),
            State::Qoi(d) => d.info(),
        }
    }

    /// Arena bytes taken so far (the high-water mark).
    #[must_use]
    pub fn arena_used(&self) -> usize {
        match &self.state {
            State::Probe(_) => 0,
            State::Jpeg(d) => d.arena_used(),
            State::Png(d) => d.arena_used(),
            State::Gif(d) => d.arena_used(),
            State::Bmp(d) => d.arena_used(),
            State::Qoi(d) => d.arena_used(),
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Run to completion over a source that already holds everything
    /// (host tools, tests): `NeedData` with the source at its end is
    /// reported as `Truncated`.
    pub fn run(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<(), Error> {
        loop {
            match self.step(src, sink)? {
                Step::Done => return Ok(()),
                Step::Progress => {}
                Step::NeedData(_) => return Err(Error::Truncated),
            }
        }
    }
}
