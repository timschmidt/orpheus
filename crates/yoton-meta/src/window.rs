//! Tags of a file read through a caller-owned window with a byte budget
//! (the library scanner, AU-12: tag reads are its bottleneck).
//!
//! [`read_budgeted`] drives a [`TagReader`] over a [`ReadAt`] file: the
//! first read fetches the start of the file (where most tags are), later
//! reads fetch only what the parser asks for, rounded up to whole
//! sectors; seeks over pictures and padding cost nothing. Once the budget
//! (bytes or read calls) would be exceeded, or a unit needs more than the
//! window holds, the file is treated as ending there: the parsers finish
//! with what they have. The stream length (duration) is taken from the
//! bytes after the leading tags when the format makes that cheap
//! ([`crate::length::duration_ms`]).

use yoton_audio::probe::id3v2_len;
use yoton_audio::{ByteSource, Format, SeekSource, SourceError, View};

use crate::{MetaStep, Opts, TagReader, Tags};

const SECTOR: u64 = 512;
/// Bytes after the leading tags examined for the stream length.
const LENGTH_HEAD: usize = 2048;
/// Bytes fetched at once when the parser first reads near the end.
const TAIL: usize = 1024;

/// A file that can be read at any offset.
pub trait ReadAt {
    type Error;
    /// Read at `offset`; fewer bytes than `buf.len()` only at the end.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

/// Limits for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Bytes read at most.
    pub bytes: u32,
    /// Read calls at most.
    pub reads: u16,
    /// Bytes of the first read (the start of the file).
    pub first: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { bytes: 16 * 1024, reads: 8, first: 4096 }
    }
}

/// What reading one file cost and found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadInfo {
    pub bytes: u32,
    pub reads: u16,
    /// The budget or the window stopped the parsers early.
    pub truncated: bool,
    /// Playing time, when the format gives it cheaply.
    pub duration_ms: Option<u32>,
}

struct Win<'w> {
    buf: &'w mut [u8],
    /// File offset of `buf[0]`.
    start: u64,
    lo: usize,
    hi: usize,
    len: u64,
    cut: bool,
}

impl ByteSource for Win<'_> {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])> {
        (n <= self.hi - self.lo).then(|| (&self.buf[self.lo..self.lo + n], &[][..]))
    }
    fn available(&self) -> usize {
        self.hi - self.lo
    }
    fn consume(&mut self, n: usize) {
        self.lo += n.min(self.hi - self.lo);
    }
    fn position(&self) -> u64 {
        self.start + self.lo as u64
    }
    fn len(&self) -> Option<u64> {
        Some(self.len)
    }
    fn at_end(&self) -> bool {
        self.cut || self.start + self.hi as u64 >= self.len
    }
}

impl SeekSource for Win<'_> {
    fn seek(&mut self, pos: u64) -> Result<(), SourceError> {
        if pos > self.len {
            return Err(SourceError::OutOfRange);
        }
        if pos >= self.start && pos <= self.start + self.hi as u64 {
            self.lo = (pos - self.start) as usize;
        } else {
            self.start = pos;
            self.lo = 0;
            self.hi = 0;
        }
        Ok(())
    }
}

struct Reader<'f, R: ReadAt> {
    file: &'f mut R,
    budget: Budget,
    info: ReadInfo,
}

impl<R: ReadAt> Reader<'_, R> {
    /// Read `[at, at + n)` into `buf` within the budget; `Ok(None)` if the
    /// budget does not allow it.
    fn read(&mut self, at: u64, buf: &mut [u8]) -> Result<Option<usize>, R::Error> {
        if buf.is_empty() {
            return Ok(Some(0));
        }
        if self.info.reads >= self.budget.reads
            || u64::from(self.info.bytes) + buf.len() as u64 > u64::from(self.budget.bytes)
        {
            self.info.truncated = true;
            return Ok(None);
        }
        let got = self.file.read_at(at, buf)?;
        self.info.reads += 1;
        self.info.bytes += got as u32;
        Ok(Some(got))
    }
}

impl Win<'_> {
    /// Make `n` bytes available at the read position (whole sectors, at
    /// least `min`). `false`: cut the file here.
    fn fill<R: ReadAt>(&mut self, r: &mut Reader<'_, R>, n: usize, min: usize) -> Result<bool, R::Error> {
        if n > self.buf.len() {
            return Ok(false);
        }
        let tail = TAIL.min(self.buf.len()) as u64;
        if self.lo == self.hi && self.start + tail >= self.len && self.start > 0 {
            // Near the end (ID3v1, APE footer, Lyrics3): fetch the whole
            // tail at once so that the footers read before and after
            // this one need no further call.
            let pos = self.start + self.lo as u64;
            let from = (self.len.saturating_sub(tail) / SECTOR * SECTOR).min(pos);
            let k = (self.len - from) as usize;
            if k <= self.buf.len() && k >= n + (pos - from) as usize {
                return match r.read(from, &mut self.buf[..k])? {
                    Some(got) => {
                        self.start = from;
                        self.lo = (pos - from) as usize;
                        self.hi = got;
                        if got < k {
                            self.len = from + got as u64;
                        }
                        Ok(self.hi >= self.lo + n)
                    }
                    None => Ok(false),
                };
            }
        }
        self.buf.copy_within(self.lo..self.hi, 0);
        self.start += self.lo as u64;
        self.hi -= self.lo;
        self.lo = 0;
        let from = self.start + self.hi as u64;
        let want_end = self.start + n.max(min) as u64;
        let end = want_end.div_ceil(SECTOR) * SECTOR;
        let end = end.min(self.start + self.buf.len() as u64).min(self.len);
        if end <= from {
            return Ok(self.hi >= n);
        }
        let k = (end - from) as usize;
        match r.read(from, &mut self.buf[self.hi..self.hi + k])? {
            Some(got) => {
                self.hi += got;
                if got < k {
                    // The file is shorter than it said.
                    self.len = self.start + self.hi as u64;
                }
                Ok(self.hi >= n)
            }
            None => Ok(false),
        }
    }
}

/// Read the tags of a file of `len` bytes into `tags` through `window`
/// (≥ 4.5 KiB reads every unit the parsers may ask for at once; less
/// cuts large picture headers short). Errors of `file` abort the read.
pub fn read_budgeted<R: ReadAt>(
    file: &mut R,
    len: u64,
    format: Format,
    opts: Opts,
    window: &mut [u8],
    budget: Budget,
    tags: &mut Tags,
) -> Result<ReadInfo, R::Error> {
    let mut r = Reader { file, budget, info: ReadInfo::default() };
    let mut w = Win { buf: window, start: 0, lo: 0, hi: 0, len, cut: false };
    let first = (budget.first as usize).min(w.buf.len());
    if !w.fill(&mut r, 1.min(len as usize), first)? && len > 0 {
        w.cut = true;
    }
    // Leading tags and the stream length from the first read, if it holds
    // them.
    let mut lead = 0u64;
    while let Some(off) = usize::try_from(lead).ok().filter(|&o| o + 10 <= w.hi) {
        match id3v2_len(&View::new(&w.buf[off..w.hi], &[])) {
            Some(n) => lead += n,
            None => break,
        }
    }
    let first_hi = w.hi as u64;
    let mut duration = None;
    if lead < first_hi {
        duration = crate::length::duration_ms(format, &w.buf[lead as usize..w.hi], lead, len);
    }
    let mut reader = TagReader::new(format, opts);
    for _ in 0..4096 {
        match reader.step(&mut w, tags) {
            Ok(MetaStep::Done) | Err(_) => break,
            Ok(MetaStep::Continue) => {}
            Ok(MetaStep::NeedData(n)) => {
                if w.cut || !w.fill(&mut r, n, 0)? {
                    w.cut = true;
                }
            }
        }
    }
    // A long leading tag: one more read just after it.
    if duration.is_none()
        && matches!(format, Format::Mpeg | Format::Flac | Format::Wav | Format::Aiff)
        && lead < len
        && first_hi < len
        && first_hi.saturating_sub(lead) < LENGTH_HEAD as u64
    {
        let n = LENGTH_HEAD.min(w.buf.len()).min((len - lead) as usize);
        if let Some(got) = r.read(lead, &mut w.buf[..n])? {
            duration = crate::length::duration_ms(format, &w.buf[..got], lead, len);
        }
    }
    let mut info = r.info;
    info.truncated |= w.cut && !(w.start + w.hi as u64 >= w.len);
    info.duration_ms = duration;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    struct Mem<'a>(&'a [u8]);
    impl ReadAt for Mem<'_> {
        type Error = ();
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, ()> {
            let o = (offset as usize).min(self.0.len());
            let n = buf.len().min(self.0.len() - o);
            buf[..n].copy_from_slice(&self.0[o..o + n]);
            Ok(n)
        }
    }

    fn text_frame(id: &[u8; 4], s: &str) -> Vec<u8> {
        let mut f = Vec::from(*id);
        f.extend_from_slice(&(s.len() as u32 + 1).to_be_bytes());
        f.extend_from_slice(&[0, 0, 3]);
        f.extend_from_slice(s.as_bytes());
        f
    }

    fn mp3(art: usize, art_first: bool) -> Vec<u8> {
        let mut frames = Vec::new();
        let mut apic = Vec::from(*b"APIC");
        apic.extend_from_slice(&(art as u32 + 14).to_be_bytes());
        apic.extend_from_slice(&[0, 0, 0]);
        apic.extend_from_slice(b"image/jpeg\0\x03\0");
        apic.resize(apic.len() + art, 0x55);
        if art_first {
            frames.extend_from_slice(&apic);
        }
        frames.extend(text_frame(b"TIT2", "Title"));
        frames.extend(text_frame(b"TPE1", "Artist"));
        frames.extend(text_frame(b"TALB", "Album"));
        if !art_first {
            frames.extend_from_slice(&apic);
        }
        let size = frames.len() as u32;
        let mut f = Vec::from(*b"ID3\x03\0\0");
        f.extend_from_slice(&[
            (size >> 21) as u8 & 0x7F,
            (size >> 14) as u8 & 0x7F,
            (size >> 7) as u8 & 0x7F,
            size as u8 & 0x7F,
        ]);
        f.extend_from_slice(&frames);
        for _ in 0..1000 {
            f.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
            f.resize(f.len() + 413, 0);
        }
        f
    }

    fn read(f: &[u8], b: Budget, tags: &mut Tags) -> ReadInfo {
        let mut win = [0u8; 4608];
        read_budgeted(&mut Mem(f), f.len() as u64, Format::Mpeg, Opts::default(), &mut win, b, tags).unwrap()
    }

    #[test]
    fn small_and_large_tags() {
        for (art, first) in [(1000, false), (200_000, false), (200_000, true)] {
            let f = mp3(art, first);
            let mut tags = Tags::default();
            let info = read(&f, Budget::default(), &mut tags);
            assert_eq!(tags.title.as_str(), "Title");
            assert_eq!(tags.album.as_str(), "Album");
            assert!(info.bytes <= 16 * 1024, "{info:?}");
            // 1000 frames x 1152 / 44100 (+ the ID3v1 probe bytes).
            let d = info.duration_ms.unwrap();
            assert!((26_000..26_200).contains(&d), "{d}");
        }
        // A tight budget: the first read only.
        let f = mp3(200_000, true);
        let mut tags = Tags::default();
        let info = read(&f, Budget { bytes: 4096, reads: 1, first: 4096 }, &mut tags);
        assert!(info.truncated && info.reads == 1 && tags.title.is_empty(), "{info:?}");
    }
}
