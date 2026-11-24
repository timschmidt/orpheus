//! Ogg page and packet layer (RFC 3533), shared by the Ogg codecs (Vorbis,
//! Opus, Speex).
//!
//! One page core, two ways to read packets:
//!
//! * Core: [`PageHeader`] parses and checks one page header; [`page_crc`]
//!   is the Ogg CRC-32 (polynomial `0x04C11DB7`, no reflection, initial 0)
//!   of a page in a [`View`]. [`Bisect`] finds, by bisection over a
//!   [`SeekSource`], the last page of a stream whose granule position is at
//!   or before a target. [`Next`] is the answer of both readers.
//! * [`ranges::Reader`] (zero copy, Vorbis): a packet is up to
//!   [`ranges::MAX_RANGES`] byte ranges of the stream ring, one per page it
//!   spans; nothing is copied, so a packet must fit in the ring (an
//!   oversized one — a cover-art comment packet — is skipped as it streams
//!   past). Pages may be larger than the ring: only the current page's
//!   lacing table is kept. It follows the first stream seen; a chained
//!   stream ends it.
//! * [`assembled::PacketReader`] (copying, Opus and Speex): packets are
//!   reassembled into a caller buffer across pages and ring wraps (longer
//!   ones truncated and flagged), with stream selection by serial, chained
//!   stream hand-over and gap detection; after a [`Bisect`] it resumes
//!   exactly at the found page's granule position.
//!
//! Pages up to [`MAX_PEEK`] bytes are CRC-checked before any of their bytes
//! are used. Larger pages (up to 64 KiB, more than the ring can show at
//! once) are taken from their header; the assembled reader checks their CRC
//! as they stream past and reports a mismatch after their packets.
//!
//! `no_std`, no allocation, no `unsafe`, no floating point.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

use yoton_audio::{ByteSource, MAX_PEEK, View};

pub mod assembled;
pub mod ranges;
mod seek;

pub use seek::{Bisect, SeekStep};

/// Fixed part of a page header.
pub const HEADER_MIN: usize = 27;
/// The largest possible page (header with 255 lacing values, 255 × 255
/// body bytes).
pub const MAX_PAGE: usize = 27 + 255 + 255 * 255;
pub const FLAG_CONTINUED: u8 = 1;
pub const FLAG_BOS: u8 = 2;
pub const FLAG_EOS: u8 = 4;
/// A page on which no packet ends carries this granule position (−1).
pub const NO_GRANULE: u64 = u64::MAX;

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC: [u32; 256] = crc_table();

/// Continue the Ogg CRC-32 over `data`.
#[must_use]
pub fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = (crc << 8) ^ CRC[usize::from((crc >> 24) as u8 ^ b)];
    }
    crc
}

fn crc_view(crc: u32, v: View<'_>) -> u32 {
    crc_update(crc_update(crc, v.a), v.b)
}

/// CRC of the page bytes in `v` (from a page start), its CRC field taken
/// as zero.
fn crc_page_bytes(v: View<'_>) -> u32 {
    let crc = crc_update(crc_view(0, v.take(22)), &[0; 4]);
    crc_view(crc, v.skip(26))
}

/// CRC of a whole page at the start of `v`.
#[must_use]
pub fn page_crc(v: &View<'_>, total: usize) -> u32 {
    crc_page_bytes(v.take(total))
}

/// A parsed page header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageHeader {
    pub flags: u8,
    /// Raw granule position ([`NO_GRANULE`] when no packet ends here).
    pub granule: u64,
    pub serial: u32,
    pub seq: u32,
    pub crc: u32,
    pub segments: u8,
    /// Header bytes including the lacing values.
    pub header_len: usize,
    pub body_len: usize,
}

/// Outcome of [`PageHeader::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parse {
    Page(PageHeader),
    /// The header needs this many bytes in the view.
    Need(usize),
    /// No valid page header at the start of the view.
    NotPage,
}

impl PageHeader {
    /// Parse the header at the start of `v` (the body need not be there).
    #[must_use]
    pub fn parse(v: &View<'_>) -> Parse {
        if v.len() < HEADER_MIN {
            return Parse::Need(HEADER_MIN);
        }
        if !v.starts_with_at(0, b"OggS") || v.byte(4) != 0 || v.byte(5) & !7 != 0 {
            return Parse::NotPage;
        }
        let segments = v.byte(26);
        let header_len = HEADER_MIN + usize::from(segments);
        if v.len() < header_len {
            return Parse::Need(header_len);
        }
        let body_len = (0..usize::from(segments)).map(|i| usize::from(v.byte(HEADER_MIN + i))).sum();
        Parse::Page(PageHeader {
            flags: v.byte(5),
            granule: v.u64_le(6).unwrap_or(NO_GRANULE),
            serial: v.u32_le(14).unwrap_or(0),
            seq: v.u32_le(18).unwrap_or(0),
            crc: v.u32_le(22).unwrap_or(0),
            segments,
            header_len,
            body_len,
        })
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.header_len + self.body_len
    }
    #[must_use]
    pub fn continued(&self) -> bool {
        self.flags & FLAG_CONTINUED != 0
    }
    #[must_use]
    pub fn bos(&self) -> bool {
        self.flags & FLAG_BOS != 0
    }
    #[must_use]
    pub fn eos(&self) -> bool {
        self.flags & FLAG_EOS != 0
    }
    /// The granule position, unless no packet ends on this page.
    #[must_use]
    pub fn granule_pos(&self) -> Option<u64> {
        (self.granule != NO_GRANULE).then_some(self.granule)
    }
}

/// Answer of the packet readers ([`ranges::Next`], [`assembled::Next`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next<P> {
    Packet(P),
    /// Call again once `available() >= n` (at most [`MAX_PEEK`]) or the
    /// source is at its end.
    NeedData(usize),
    /// The stream ended (no further complete page of it).
    End,
    /// Damaged data was skipped (at least one byte consumed), or a large
    /// page failed its CRC after its packets were delivered.
    Corrupt,
}

pub(crate) enum Sync {
    Page(PageHeader, bool),
    Need(usize),
    End,
}

/// Find the next page at or after the read position, consuming junk before
/// it. Pages up to `MAX_PEEK` bytes are returned only when complete and
/// with a good CRC; larger ones from their header. The flag tells whether
/// junk was skipped.
pub(crate) fn sync_page(src: &mut dyn ByteSource) -> Sync {
    let mut junk = false;
    loop {
        let v = View::of(src);
        let end = src.at_end();
        match PageHeader::parse(&v) {
            Parse::Need(n) => {
                if v.len() >= 4 && !v.starts_with_at(0, b"OggS") {
                    // Not a page start: fall through to the search below.
                } else if end {
                    src.consume(v.len());
                    return Sync::End;
                } else {
                    return Sync::Need(n.min(MAX_PEEK));
                }
            }
            Parse::Page(h) => {
                let total = h.total();
                if total > MAX_PEEK {
                    return Sync::Page(h, junk);
                }
                if v.len() < total {
                    if end {
                        // A page cut short by the end of the file.
                        src.consume(v.len());
                        return Sync::End;
                    }
                    return Sync::Need(total);
                }
                if page_crc(&v, total) == h.crc {
                    return Sync::Page(h, junk);
                }
            }
            Parse::NotPage => {}
        }
        // Skip at least one byte, up to the next capture pattern.
        junk = true;
        match v.find(1, b"OggS") {
            Some(k) => src.consume(k),
            None => {
                let k = v.len().saturating_sub(3).max(1).min(v.len());
                src.consume(k);
                if src.available() < HEADER_MIN {
                    if src.at_end() {
                        src.consume(src.available());
                        return Sync::End;
                    }
                    return Sync::Need(HEADER_MIN);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
