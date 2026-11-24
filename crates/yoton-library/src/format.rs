//! On-card encodings of the library file (all integers little endian
//! unless a sort key says big endian).
//!
//! # File (`lib-a.ydb` / `lib-b.ydb`)
//!
//! `[header 512 B][sections…]`. The header is zero while the file is being
//! built and is written last, after a sync, so a partial file never opens.
//! Two slots alternate: a build writes the slot that is not current, and
//! [`crate::Library::open`] takes the valid slot with the higher
//! generation, so the old library stays usable until the new one is done.
//!
//! Header:
//!
//! | off | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `YLIB` |
//! | 4 | 2 | version |
//! | 6 | 2 | options (bit 0: leading articles ignored) |
//! | 8 | 4 | generation |
//! | 12 | 4 | volume serial |
//! | 16 | 4 | tracks |
//! | 20 | 4 | build stamp (the caller's `now`) |
//! | 24 | 16 | string kinds: start offset of artists, albums, genres, titles in `STR` |
//! | 40 | 4 | end of the string pool (= `STR` length) |
//! | 44 | 4 | section count ([`SECTIONS`]) |
//! | 48 | 12 × n | sections: offset, length, CRC-32 |
//! | 508 | 4 | CRC-32 of bytes 0..508 |
//!
//! Sections ([`Sec`]):
//!
//! * `CAT` catalogue: one variable item per track ([`cat`]), grouped by
//!   folder; the raw tags, the input of the next incremental scan.
//! * `DIRS`: 40-byte rows sorted by folder identity: identity, signature
//!   (entries, name bytes, newest mtime, hash), offset and length of the
//!   folder's items in `CAT`, track count.
//! * `STR` string pool: `[length u8][UTF-8]` entries, per kind in
//!   collation order (so a string reference — the entry's offset — orders
//!   like the strings), deduplicated by folded comparison.
//! * `TRK`: 64-byte track rows ([`trk`]) by (album artist, album, disc,
//!   track, title, locator).
//! * `ARTISTS`, `ALBUMS`, `GENRES`, `GARTISTS`, `GALBUMS`: 32-byte group
//!   rows ([`grp`]). Artists and albums are groups of `TRK`; genres,
//!   genre-artists and genre-albums are groups of the `GENRE` order.
//! * Orders (`u32` per entry): `SONGS` (track rows by title), `GENRE`
//!   (track rows by genre, then `TRK` order), `RECENT` (by first-seen
//!   time, newest first), `BYLOC` (by locator), `ALBYEAR` (album rows by
//!   artist, year, name), `ALBNAME` (album rows by name, artist, year),
//!   `GALBYEAR` (genre-album rows by genre-artist, year, name).
//! * Letter runs (`first row u32, letter u32`) of the Artists, Albums,
//!   Genres and Songs lists.

use yoton_fs::Locator;

pub(crate) const HEADER: usize = 512;
pub(crate) const MAGIC: [u8; 4] = *b"YLIB";
/// Bump when the file layout or an order changes.
pub const VERSION: u16 = 1;
const HDR_CRC: usize = 508;
const SEC_TABLE: usize = 48;

/// Sections of the library file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Sec {
    Cat,
    Dirs,
    Str,
    Trk,
    Songs,
    Genre,
    Recent,
    ByLoc,
    Artists,
    Albums,
    Genres,
    GArtists,
    GAlbums,
    AlbYear,
    AlbName,
    GAlbYear,
    LArtists,
    LAlbums,
    LGenres,
    LSongs,
}

/// Number of sections.
pub const SECTIONS: usize = 20;
const _: () = assert!(SEC_TABLE + 12 * SECTIONS <= HDR_CRC);

/// String kinds (sections of `STR`, in this order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Artist = 0,
    Album = 1,
    Genre = 2,
    Title = 3,
}

pub(crate) fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn wr16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wr32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn rd64(b: &[u8], o: usize) -> u64 {
    u64::from(rd32(b, o)) | (u64::from(rd32(b, o + 4)) << 32)
}

/// Offset, length and CRC of one section.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct SecInfo {
    pub off: u32,
    pub len: u32,
    pub crc: u32,
}

/// The header fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Header {
    pub options: u16,
    pub generation: u32,
    pub serial: u32,
    pub tracks: u32,
    pub stamp: u32,
    pub kinds: [u32; 4],
    pub str_end: u32,
    pub secs: [SecInfo; SECTIONS],
}

impl Header {
    pub fn encode(&self, out: &mut [u8]) {
        out[..HEADER].fill(0);
        out[..4].copy_from_slice(&MAGIC);
        wr16(out, 4, VERSION);
        wr16(out, 6, self.options);
        wr32(out, 8, self.generation);
        wr32(out, 12, self.serial);
        wr32(out, 16, self.tracks);
        wr32(out, 20, self.stamp);
        for (i, k) in self.kinds.iter().enumerate() {
            wr32(out, 24 + 4 * i, *k);
        }
        wr32(out, 40, self.str_end);
        wr32(out, 44, SECTIONS as u32);
        for (i, s) in self.secs.iter().enumerate() {
            let o = SEC_TABLE + 12 * i;
            wr32(out, o, s.off);
            wr32(out, o + 4, s.len);
            wr32(out, o + 8, s.crc);
        }
        let c = yoton_index::crc32_update(0, &out[..HDR_CRC]);
        wr32(out, HDR_CRC, c);
    }

    pub fn decode(b: &[u8]) -> Option<Header> {
        if b.len() < HEADER
            || b[..4] != MAGIC
            || rd16(b, 4) != VERSION
            || rd32(b, 44) != SECTIONS as u32
            || yoton_index::crc32_update(0, &b[..HDR_CRC]) != rd32(b, HDR_CRC)
        {
            return None;
        }
        let mut h = Header {
            options: rd16(b, 6),
            generation: rd32(b, 8),
            serial: rd32(b, 12),
            tracks: rd32(b, 16),
            stamp: rd32(b, 20),
            kinds: [0; 4],
            str_end: rd32(b, 40),
            secs: [SecInfo::default(); SECTIONS],
        };
        for (i, k) in h.kinds.iter_mut().enumerate() {
            *k = rd32(b, 24 + 4 * i);
        }
        for (i, s) in h.secs.iter_mut().enumerate() {
            let o = SEC_TABLE + 12 * i;
            *s = SecInfo { off: rd32(b, o), len: rd32(b, o + 4), crc: rd32(b, o + 8) };
        }
        Some(h)
    }

    /// File length the sections imply (the largest section end).
    pub fn file_len(&self) -> u64 {
        self.secs.iter().map(|s| u64::from(s.off) + u64::from(s.len)).max().unwrap_or(0).max(HEADER as u64)
    }
}

/// A string in the pool: its offset in `STR`; references of one kind
/// order like the strings. [`StrRef::NONE`] for no string.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct StrRef(pub u32);

impl StrRef {
    pub const NONE: StrRef = StrRef(u32::MAX);
    #[must_use]
    pub fn is_none(self) -> bool {
        self == StrRef::NONE
    }
}

/// Catalogue items: `[length u16][fixed][5 × (length u8, UTF-8)]`.
///
/// | off | size | field |
/// |---|---|---|
/// | 0 | 2 | item length |
/// | 2 | 4 | locator `dir` |
/// | 6 | 4 | locator `index` |
/// | 10 | 4 | name hash ([`yoton_player::name_hash`]) |
/// | 14 | 4 | size (low 32 bits) |
/// | 18 | 4 | mtime |
/// | 22 | 4 | first seen |
/// | 26 | 4 | duration ms (0 = unknown) |
/// | 30 | 2 | year |
/// | 32 | 2 | track |
/// | 34 | 2 | disc |
/// | 36 | 1 | format ([`crate::FileFormat`]) |
/// | 37 | 1 | flags ([`crate::flags`]) |
/// | 38 | … | title, artist, album, album artist (as tagged), genre |
pub mod cat {
    pub const FIXED: usize = 38;
    pub const FIELDS: usize = 5;
    /// Largest item.
    pub const MAX: usize = FIXED + FIELDS * 128;
    pub const TITLE: usize = 0;
    pub const ARTIST: usize = 1;
    pub const ALBUM: usize = 2;
    pub const ALBUM_ARTIST: usize = 3;
    pub const GENRE: usize = 4;
}

/// The fixed part of a catalogue item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct CatFixed {
    pub loc: Locator,
    pub name_hash: u32,
    pub size: u32,
    pub mtime: u32,
    pub added: u32,
    pub duration_ms: u32,
    pub year: u16,
    pub track: u16,
    pub disc: u16,
    pub format: u8,
    pub flags: u8,
}

impl CatFixed {
    pub fn encode(&self, out: &mut [u8]) {
        wr32(out, 2, self.loc.dir);
        wr32(out, 6, self.loc.index);
        wr32(out, 10, self.name_hash);
        wr32(out, 14, self.size);
        wr32(out, 18, self.mtime);
        wr32(out, 22, self.added);
        wr32(out, 26, self.duration_ms);
        wr16(out, 30, self.year);
        wr16(out, 32, self.track);
        wr16(out, 34, self.disc);
        out[36] = self.format;
        out[37] = self.flags;
    }
    pub fn decode(b: &[u8]) -> CatFixed {
        CatFixed {
            loc: Locator { dir: rd32(b, 2), index: rd32(b, 6) },
            name_hash: rd32(b, 10),
            size: rd32(b, 14),
            mtime: rd32(b, 18),
            added: rd32(b, 22),
            duration_ms: rd32(b, 26),
            year: rd16(b, 30),
            track: rd16(b, 32),
            disc: rd16(b, 34),
            format: b[36],
            flags: b[37],
        }
    }
}

/// The five strings of a catalogue item (`item` is the whole item).
pub(crate) fn cat_fields(item: &[u8]) -> [&[u8]; cat::FIELDS] {
    let mut out: [&[u8]; cat::FIELDS] = [&[]; cat::FIELDS];
    let mut o = cat::FIXED;
    for f in &mut out {
        let n = usize::from(*item.get(o).unwrap_or(&0));
        let end = (o + 1 + n).min(item.len());
        *f = item.get(o + 1..end).unwrap_or(&[]);
        o = end;
    }
    out
}

/// Bytes per `DIRS` row.
pub(crate) const DIR_ROW: usize = 40;

/// Bytes per `TRK` row.
pub const TRK_ROW: usize = 64;

/// `TRK` row layout.
///
/// | off | size | field |
/// |---|---|---|
/// | 0 | 4 | locator `dir` |
/// | 4 | 4 | locator `index` |
/// | 8 | 4 | name hash |
/// | 12 | 4 | title |
/// | 16 | 4 | artist (as tagged) |
/// | 20 | 4 | album |
/// | 24 | 4 | album artist (the artist when untagged) |
/// | 28 | 4 | genre |
/// | 32 | 4 | album row (`ALBUMS`) |
/// | 36 | 4 | artist row (`ARTISTS`) |
/// | 40 | 4 | duration ms |
/// | 44 | 4 | first seen |
/// | 48 | 2 | year |
/// | 50 | 2 | track |
/// | 52 | 2 | disc |
/// | 54 | 1 | format |
/// | 55 | 1 | flags |
/// | 56 | 4 | size (low 32 bits) |
/// | 60 | 4 | mtime |
pub mod trk {
    pub const TITLE: usize = 12;
    pub const ARTIST: usize = 16;
    pub const ALBUM: usize = 20;
    pub const AARTIST: usize = 24;
    pub const GENRE: usize = 28;
    pub const ALBUM_ROW: usize = 32;
    pub const ARTIST_ROW: usize = 36;
    pub const DURATION: usize = 40;
    pub const ADDED: usize = 44;
    pub const YEAR: usize = 48;
    pub const TRACK: usize = 50;
    pub const DISC: usize = 52;
    pub const FORMAT: usize = 54;
    pub const FLAGS: usize = 55;
    pub const SIZE: usize = 56;
    pub const MTIME: usize = 60;
}

/// Bytes per group row.
pub const GRP_ROW: usize = 32;

/// Group row layout (`ARTISTS`, `ALBUMS`, `GENRES`, `GARTISTS`,
/// `GALBUMS`).
///
/// | off | size | field |
/// |---|---|---|
/// | 0 | 4 | name |
/// | 4 | 4 | second string (albums: album artist; genre-artists: genre) |
/// | 8 | 4 | first track position (in `TRK` or the `GENRE` order) |
/// | 12 | 4 | tracks |
/// | 16 | 4 | first child row (artists: `ALBYEAR` position; genres: `GARTISTS` row; genre-artists: `GALBYEAR` position; genre-albums: the `ALBUMS` row) |
/// | 20 | 4 | child rows |
/// | 24 | 4 | parent row (albums: `ARTISTS`; genre-artists: `GENRES`; genre-albums: `GARTISTS`) |
/// | 28 | 2 | year (albums: earliest non-zero) |
/// | 30 | 1 | flags: bit 0 tracks by several artists |
/// | 31 | 1 | discs (highest disc number, saturated) |
pub mod grp {
    pub const NAME: usize = 0;
    pub const SUB: usize = 4;
    pub const FIRST: usize = 8;
    pub const COUNT: usize = 12;
    pub const CHILD: usize = 16;
    pub const CHILDREN: usize = 20;
    pub const PARENT: usize = 24;
    pub const YEAR: usize = 28;
    pub const FLAGS: usize = 30;
    pub const DISCS: usize = 31;
}

/// A group row being accumulated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Group {
    pub name: u32,
    pub sub: u32,
    pub first: u32,
    pub count: u32,
    pub child: u32,
    pub children: u32,
    pub parent: u32,
    pub year: u16,
    pub flags: u8,
    pub discs: u8,
    /// Track artist of the first track (for the "several artists" flag).
    pub artist: u32,
}

impl Group {
    pub const EMPTY: Group = Group {
        name: u32::MAX,
        sub: u32::MAX,
        first: 0,
        count: 0,
        child: 0,
        children: 0,
        parent: u32::MAX,
        year: 0,
        flags: 0,
        discs: 0,
        artist: u32::MAX,
    };

    pub fn encode(&self) -> [u8; GRP_ROW] {
        let mut b = [0u8; GRP_ROW];
        wr32(&mut b, grp::NAME, self.name);
        wr32(&mut b, grp::SUB, self.sub);
        wr32(&mut b, grp::FIRST, self.first);
        wr32(&mut b, grp::COUNT, self.count);
        wr32(&mut b, grp::CHILD, self.child);
        wr32(&mut b, grp::CHILDREN, self.children);
        wr32(&mut b, grp::PARENT, self.parent);
        wr16(&mut b, grp::YEAR, self.year);
        b[grp::FLAGS] = self.flags;
        b[grp::DISCS] = self.discs;
        b
    }

    /// Account for one track (year, disc, artist).
    pub fn add(&mut self, year: u16, disc: u16, artist: u32) {
        if self.count == 0 {
            self.artist = artist;
        } else if artist != self.artist {
            self.flags |= 1;
        }
        self.count += 1;
        if year != 0 && (self.year == 0 || year < self.year) {
            self.year = year;
        }
        self.discs = self.discs.max(disc.min(255) as u8);
    }
}

/// Bytes per letter run.
pub(crate) const LETTER_RUN: usize = 8;
