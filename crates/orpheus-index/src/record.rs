//! On-card encodings: the 32-byte row record, the header block, CRC-32.

use core::cmp::Ordering;

use orpheus_apps::browse::{EntryKind, natural_cmp};
use orpheus_apps::services::SortBy;
use orpheus_fs::Locator;

use crate::Spec;
use crate::key::{KeyPrefix, PREFIX, prefix_cmp};

/// Bytes per row record.
pub const RECORD: usize = 32;
/// Bytes per letter-run entry (`first row u32, letter u32`).
pub const LETTER_RUN: usize = 8;
/// The header block; records start at this offset (sector aligned, so a
/// 32-row window is exactly two 512-byte sectors).
pub const HEADER: usize = 512;
/// Header bytes that carry fields (the rest of the block is zero).
pub(crate) const HEADER_USED: usize = 68;
pub(crate) const MAGIC: [u8; 4] = *b"YIDX";
/// Bump when the file layout or the order changes.
pub const VERSION: u16 = 1;

const F_DIR: u8 = 1;
const F_HIDDEN: u8 = 2;

pub(crate) fn kind_code(k: EntryKind) -> u8 {
    match k {
        EntryKind::Dir => 0,
        EntryKind::Audio => 1,
        EntryKind::Playlist => 2,
        EntryKind::Image => 3,
        EntryKind::Text => 4,
        EntryKind::DiskImage => 5,
        EntryKind::Firmware => 6,
        EntryKind::OtpImport => 7,
        EntryKind::Other => 8,
    }
}

pub(crate) fn kind_from(c: u8) -> EntryKind {
    match c {
        0 => EntryKind::Dir,
        1 => EntryKind::Audio,
        2 => EntryKind::Playlist,
        3 => EntryKind::Image,
        4 => EntryKind::Text,
        5 => EntryKind::DiskImage,
        6 => EntryKind::Firmware,
        7 => EntryKind::OtpImport,
        _ => EntryKind::Other,
    }
}

pub(crate) fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn rd64(b: &[u8], o: usize) -> u64 {
    u64::from(rd32(b, o)) | (u64::from(rd32(b, o + 4)) << 32)
}
pub(crate) fn wr16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wr32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wr64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// One row of a listing as the index returns it (32 bytes of RAM; a
/// 32-row window is 1 KiB). The name is not stored: read it through the
/// locator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Row {
    pub loc: Locator,
    /// File size in bytes (48 bits kept).
    pub size: u64,
    /// The source's ordered timestamp ([`crate::SrcEntry::mtime`]).
    pub mtime: u32,
    /// Letter-jump letter ([`orpheus_apps::browse::letter_of`]).
    pub letter: Option<char>,
    pub kind: EntryKind,
    /// Hidden by name or attribute (only present when the spec shows them).
    pub hidden: bool,
}

impl Row {
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }
}

/// Record layout (little endian):
///
/// | off | size | field |
/// |---|---|---|
/// | 0 | 4 | locator `dir` |
/// | 4 | 4 | locator `index` |
/// | 8 | 6 | size (48 bits) |
/// | 14 | 4 | mtime |
/// | 18 | 3 | letter (Unicode scalar, 0 = none) |
/// | 21 | 1 | kind |
/// | 22 | 1 | flags: bit 0 directory, bit 1 hidden |
/// | 23 | 1 | sort-key length (saturated at 255) |
/// | 24 | 8 | sort-key prefix |
pub(crate) fn encode(row: &Row, key: &KeyPrefix, out: &mut [u8]) {
    wr32(out, 0, row.loc.dir);
    wr32(out, 4, row.loc.index);
    let s = row.size.min((1 << 48) - 1).to_le_bytes();
    out[8..14].copy_from_slice(&s[..6]);
    wr32(out, 14, row.mtime);
    let l = row.letter.map_or(0, u32::from).to_le_bytes();
    out[18..21].copy_from_slice(&l[..3]);
    out[21] = kind_code(row.kind);
    out[22] = if row.kind == EntryKind::Dir { F_DIR } else { 0 } | if row.hidden { F_HIDDEN } else { 0 };
    out[23] = key.len;
    out[24..32].copy_from_slice(&key.bytes);
}

/// A record in a byte slice (at least [`RECORD`] bytes).
#[derive(Clone, Copy)]
pub(crate) struct Rec<'a>(pub &'a [u8]);

impl Rec<'_> {
    pub fn loc(&self) -> Locator {
        Locator { dir: rd32(self.0, 0), index: rd32(self.0, 4) }
    }
    pub fn size(&self) -> u64 {
        let mut s = [0u8; 8];
        s[..6].copy_from_slice(&self.0[8..14]);
        u64::from_le_bytes(s)
    }
    pub fn mtime(&self) -> u32 {
        rd32(self.0, 14)
    }
    pub fn letter_code(&self) -> u32 {
        u32::from_le_bytes([self.0[18], self.0[19], self.0[20], 0])
    }
    pub fn is_dir(&self) -> bool {
        self.0[22] & F_DIR != 0
    }
    pub fn key(&self) -> KeyPrefix {
        let mut bytes = [0u8; PREFIX];
        bytes.copy_from_slice(&self.0[24..32]);
        KeyPrefix { bytes, len: self.0[23] }
    }
    pub fn row(&self) -> Row {
        Row {
            loc: self.loc(),
            size: self.size(),
            mtime: self.mtime(),
            letter: char::from_u32(self.letter_code()).filter(|&c| c != '\0'),
            kind: kind_from(self.0[21]),
            hidden: self.0[22] & F_HIDDEN != 0,
        }
    }
}

/// Everything of the order that the record alone decides: directories
/// first, then the sort field, then the key prefix. `None`: compare the
/// full names, then the locators.
pub(crate) fn cmp_fixed(sort: SortBy, a: &Rec<'_>, b: &Rec<'_>) -> Option<Ordering> {
    let o = b.is_dir().cmp(&a.is_dir()).then_with(|| match sort {
        SortBy::Name => Ordering::Equal,
        SortBy::Date => b.mtime().cmp(&a.mtime()),
        SortBy::Size => b.size().cmp(&a.size()),
    });
    if o != Ordering::Equal {
        return Some(o);
    }
    prefix_cmp(&a.key(), &b.key())
}

pub(crate) fn loc_cmp(a: Locator, b: Locator) -> Ordering {
    (a.dir, a.index).cmp(&(b.dir, b.index))
}

/// The full order with both names at hand.
pub(crate) fn cmp_full(sort: SortBy, a: &Rec<'_>, an: &[u8], b: &Rec<'_>, bn: &[u8]) -> Ordering {
    match cmp_fixed(sort, a, b) {
        Some(o) => o,
        None => natural_cmp(as_str(an), as_str(bn)).then_with(|| loc_cmp(a.loc(), b.loc())),
    }
}

/// Names are validated when they enter the build.
pub(crate) fn as_str(b: &[u8]) -> &str {
    core::str::from_utf8(b).unwrap_or("")
}

/// CRC-32 (IEEE, reflected), table-free nibble form: 64 bytes of table.
pub(crate) fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    const T: [u32; 16] = [
        0x0000_0000,
        0x1DB7_1064,
        0x3B6E_20C8,
        0x26D9_30AC,
        0x76DC_4190,
        0x6B6B_51F4,
        0x4DB2_6158,
        0x5005_713C,
        0xEDB8_8320,
        0xF00F_9344,
        0xD6D6_A3E8,
        0xCB61_B38C,
        0x9B64_C2B0,
        0x86D3_D2D4,
        0xA00A_E278,
        0xBDBD_F21C,
    ];
    crc = !crc;
    for &b in data {
        crc = (crc >> 4) ^ T[((crc ^ u32::from(b)) & 0xF) as usize];
        crc = (crc >> 4) ^ T[((crc ^ (u32::from(b) >> 4)) & 0xF) as usize];
    }
    !crc
}

/// The directory signature: what the index was built from. Computed over
/// *every* entry the source returned (filtered ones too), independent of
/// the order they come in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Signature {
    pub entries: u32,
    pub name_bytes: u32,
    /// Largest `mtime` seen.
    pub max_mtime: u32,
    /// Wrapping sum of a 64-bit hash of each entry (name, locator, size,
    /// mtime, directory and hidden flags).
    pub hash: u64,
}

impl Signature {
    /// Account for one entry.
    pub fn add(&mut self, e: &crate::SrcEntry, name: &str) {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |b: u8| {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        };
        for &b in name.as_bytes() {
            eat(b);
        }
        eat(0xFF);
        for b in e.loc.dir.to_le_bytes().into_iter().chain(e.loc.index.to_le_bytes()) {
            eat(b);
        }
        for b in e.size.to_le_bytes().into_iter().chain(e.mtime.to_le_bytes()) {
            eat(b);
        }
        eat(u8::from(e.is_dir) | (u8::from(e.hidden) << 1));
        // splitmix64 finaliser so that the sum mixes well.
        let mut z = h;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        self.hash = self.hash.wrapping_add(z);
        self.entries = self.entries.wrapping_add(1);
        self.name_bytes = self.name_bytes.wrapping_add(u32::try_from(name.len()).unwrap_or(u32::MAX));
        self.max_mtime = self.max_mtime.max(e.mtime);
    }
}

/// Header block fields (offsets in bytes):
///
/// | off | size | field |
/// |---|---|---|
/// | 0 | 4 | magic `YIDX` |
/// | 4 | 2 | format version |
/// | 6 | 2 | record size (32) |
/// | 8 | 4 | directory identity (first cluster) |
/// | 12 | 1 | spec: bits 0–1 sort (name, date, size), bit 2 hidden shown, bit 3 audio only |
/// | 13 | 3 | zero |
/// | 16 | 4 | rows |
/// | 20 | 4 | directory rows (rows `0..dirs`) |
/// | 24 | 4 | letter runs |
/// | 28 | 4 | signature: entries |
/// | 32 | 4 | signature: name bytes |
/// | 36 | 4 | signature: newest mtime |
/// | 40 | 8 | signature: hash |
/// | 48 | 8 | quick stamp (caller's, advisory) |
/// | 56 | 4 | CRC-32 of the records |
/// | 60 | 4 | CRC-32 of the letter runs |
/// | 64 | 4 | CRC-32 of bytes 0..64 |
///
/// The block is written as zeros when a build starts and filled in last,
/// so a partial file never has a valid header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Header {
    pub dir: u32,
    pub spec: Spec,
    pub rows: u32,
    pub dirs: u32,
    pub letter_runs: u32,
    pub sig: Signature,
    pub quick: u64,
    pub rec_crc: u32,
    pub run_crc: u32,
}

impl Header {
    pub fn encode(&self, out: &mut [u8]) {
        out[..HEADER_USED].fill(0);
        out[..4].copy_from_slice(&MAGIC);
        wr16(out, 4, VERSION);
        wr16(out, 6, RECORD as u16);
        wr32(out, 8, self.dir);
        out[12] = self.spec.code();
        wr32(out, 16, self.rows);
        wr32(out, 20, self.dirs);
        wr32(out, 24, self.letter_runs);
        wr32(out, 28, self.sig.entries);
        wr32(out, 32, self.sig.name_bytes);
        wr32(out, 36, self.sig.max_mtime);
        wr64(out, 40, self.sig.hash);
        wr64(out, 48, self.quick);
        wr32(out, 56, self.rec_crc);
        wr32(out, 60, self.run_crc);
        let c = crc32_update(0, &out[..64]);
        wr32(out, 64, c);
    }

    pub fn decode(b: &[u8]) -> Option<Header> {
        if b.len() < HEADER_USED
            || b[..4] != MAGIC
            || rd16(b, 4) != VERSION
            || usize::from(rd16(b, 6)) != RECORD
            || crc32_update(0, &b[..64]) != rd32(b, 64)
        {
            return None;
        }
        Some(Header {
            dir: rd32(b, 8),
            spec: Spec::from_code(b[12])?,
            rows: rd32(b, 16),
            dirs: rd32(b, 20),
            letter_runs: rd32(b, 24),
            sig: Signature {
                entries: rd32(b, 28),
                name_bytes: rd32(b, 32),
                max_mtime: rd32(b, 36),
                hash: rd64(b, 40),
            },
            quick: rd64(b, 48),
            rec_crc: rd32(b, 56),
            run_crc: rd32(b, 60),
        })
    }

    /// File length this header implies.
    pub fn file_len(&self) -> u64 {
        HEADER as u64 + u64::from(self.rows) * RECORD as u64 + u64::from(self.letter_runs) * LETTER_RUN as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32_update(0, b"123456789"), 0xCBF4_3926);
        let a = crc32_update(0, b"12345");
        assert_eq!(crc32_update(a, b"6789"), 0xCBF4_3926);
    }

    #[test]
    fn record_round_trip() {
        let row = Row {
            loc: Locator { dir: 0x1234_5678, index: 0x8000_0042 },
            size: 0x0000_ABCD_EF01_2345,
            mtime: 0xDEAD_BEEF,
            letter: Some('Я'),
            kind: EntryKind::Audio,
            hidden: true,
        };
        let key = KeyPrefix::of("Track 10.mp3");
        let mut b = [0u8; RECORD];
        encode(&row, &key, &mut b);
        let r = Rec(&b);
        assert_eq!(r.row(), row);
        assert_eq!(r.key(), key);
        assert!(!r.is_dir());
    }
}
