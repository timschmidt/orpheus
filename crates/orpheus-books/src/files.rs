//! The Books app's files on the card (CLAUDE.md rule 5: all under
//! `/.orpheus/`). Little-endian, fixed-size records, so any one is read or
//! rewritten with a single positioned access.
//!
//! * `/.orpheus/cache/books/<id>.txt`: the normalised text
//!   ([`NormHeader`], then the stream of [`crate::norm`]). A cache: made
//!   again when missing, damaged, older than the book or of another
//!   version.
//! * `/.orpheus/cache/books/<id>.idx`: the page index ([`IndexHeader`],
//!   then one [`INDEX_ENTRY`]-byte mark per page) for one set of layout
//!   parameters.
//! * `/.orpheus/cache/books/<id>.toc`: contents entries of [`TOC_ENTRY`]
//!   bytes (their count is in the text's header).
//! * `/.orpheus/books/<id>.pos`: position and bookmarks ([`BookState`]).
//! * `/.orpheus/books/library.bin`: the library ([`LibHeader`], the sorted
//!   order, [`LibRecord`]s).
//!
//! `<id>` is [`book_id`] of the book's path, as eight hex digits.

use crate::convert::Format;
use crate::fb2::MetaText;
use crate::layout::Mark;
use crate::norm::{Kind, TITLE_MAX, TocEntry};

pub const BOOKS_DIR: &str = "/.orpheus/books";
pub const CACHE_DIR: &str = "/.orpheus/cache/books";
pub const LIBRARY_PATH: &str = "/.orpheus/books/library.bin";
/// Folders made, in order, before a file under them is created.
pub const DIRS: [&str; 4] = ["/.orpheus", "/.orpheus/books", "/.orpheus/cache", "/.orpheus/cache/books"];

/// Bumped when the normalised stream changes (caches are made again).
pub const NORM_VERSION: u8 = 1;

fn rd32(b: &[u8], o: usize) -> u32 {
    let mut a = [0u8; 4];
    if let Some(s) = b.get(o..o + 4) {
        a.copy_from_slice(s);
    }
    u32::from_le_bytes(a)
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from(b.get(o).copied().unwrap_or(0)) | (u16::from(b.get(o + 1).copied().unwrap_or(0)) << 8)
}

fn wr32(b: &mut [u8], o: usize, v: u32) {
    if let Some(s) = b.get_mut(o..o + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

fn wr16(b: &mut [u8], o: usize, v: u16) {
    if let Some(s) = b.get_mut(o..o + 2) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

/// A book's id: FNV-1a over its path with ASCII letters folded (FAT names
/// are case-insensitive).
#[must_use]
pub fn book_id(path: &str) -> u32 {
    path.bytes()
        .fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b.to_ascii_lowercase())).wrapping_mul(0x0100_0193))
}

/// A path built in a fixed buffer.
#[derive(Clone, Copy, Debug)]
pub struct PathBuf {
    b: [u8; 256],
    n: u16,
}

impl Default for PathBuf {
    fn default() -> Self {
        PathBuf::new()
    }
}

impl PathBuf {
    #[must_use]
    pub const fn new() -> PathBuf {
        PathBuf { b: [0; 256], n: 0 }
    }

    /// Append `s`; false (and unchanged) if it does not fit.
    pub fn push(&mut self, s: &str) -> bool {
        let at = usize::from(self.n);
        let Some(dst) = self.b.get_mut(at..at + s.len()) else { return false };
        dst.copy_from_slice(s.as_bytes());
        self.n += s.len() as u16;
        true
    }

    pub fn clear(&mut self) {
        self.n = 0;
    }

    pub fn truncate(&mut self, len: usize) {
        self.n = self.n.min(u16::try_from(len).unwrap_or(u16::MAX));
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.n)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.b.get(..usize::from(self.n)).unwrap_or(&[])).unwrap_or("")
    }

    /// `dir/xxxxxxxx.ext` for book `id`.
    pub fn file(&mut self, dir: &str, id: u32, ext: &str) -> &str {
        self.clear();
        let mut hex = [0u8; 8];
        for (i, h) in hex.iter_mut().enumerate() {
            let d = (id >> (28 - 4 * i)) & 0xF;
            *h = b"0123456789abcdef"[d as usize];
        }
        self.push(dir);
        self.push("/");
        self.push(core::str::from_utf8(&hex).unwrap_or("00000000"));
        self.push(".");
        self.push(ext);
        self.as_str()
    }
}

/// Header of the normalised text's cache file.
pub const NORM_HEADER: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct NormHeader {
    pub format: u8,
    pub encoding: u8,
    /// Written last: the conversion ran to the end.
    pub complete: bool,
    /// The book file it was made from (size and modification time).
    pub src_size: u32,
    pub src_stamp: u32,
    pub text_len: u32,
    pub toc_count: u32,
}

impl NormHeader {
    #[must_use]
    pub fn encode(&self) -> [u8; NORM_HEADER] {
        let mut b = [0u8; NORM_HEADER];
        b[..4].copy_from_slice(b"OBN1");
        b[4] = NORM_VERSION;
        b[5] = self.format;
        b[6] = self.encoding;
        b[7] = u8::from(self.complete);
        wr32(&mut b, 8, self.src_size);
        wr32(&mut b, 12, self.src_stamp);
        wr32(&mut b, 16, self.text_len);
        wr32(&mut b, 20, self.toc_count);
        let sum = b[..28].iter().fold(0u32, |a, &x| a.wrapping_mul(31).wrapping_add(u32::from(x)));
        wr32(&mut b, 28, sum);
        b
    }

    /// `None` unless a complete header of this version with a good sum.
    #[must_use]
    pub fn decode(b: &[u8]) -> Option<NormHeader> {
        let b = b.get(..NORM_HEADER)?;
        if &b[..4] != b"OBN1" || b[4] != NORM_VERSION {
            return None;
        }
        let sum = b[..28].iter().fold(0u32, |a, &x| a.wrapping_mul(31).wrapping_add(u32::from(x)));
        if sum != rd32(b, 28) {
            return None;
        }
        Some(NormHeader {
            format: b[5],
            encoding: b[6],
            complete: b[7] == 1,
            src_size: rd32(b, 8),
            src_stamp: rd32(b, 12),
            text_len: rd32(b, 16),
            toc_count: rd32(b, 20),
        })
    }
}

/// Header of a page index.
pub const INDEX_HEADER: usize = 32;
/// Bytes per page in the index: offset (4), state (1), 3 spare.
pub const INDEX_ENTRY: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct IndexHeader {
    pub complete: bool,
    /// `layout::Params::key` it was made with.
    pub key: u32,
    pub text_len: u32,
    pub pages: u32,
}

impl IndexHeader {
    #[must_use]
    pub fn encode(&self) -> [u8; INDEX_HEADER] {
        let mut b = [0u8; INDEX_HEADER];
        b[..4].copy_from_slice(b"OBX1");
        b[4] = NORM_VERSION;
        b[5] = u8::from(self.complete);
        wr32(&mut b, 8, self.key);
        wr32(&mut b, 12, self.text_len);
        wr32(&mut b, 16, self.pages);
        b
    }

    #[must_use]
    pub fn decode(b: &[u8]) -> Option<IndexHeader> {
        let b = b.get(..INDEX_HEADER)?;
        if &b[..4] != b"OBX1" || b[4] != NORM_VERSION {
            return None;
        }
        Some(IndexHeader { complete: b[5] == 1, key: rd32(b, 8), text_len: rd32(b, 12), pages: rd32(b, 16) })
    }
}

#[must_use]
pub fn encode_mark(m: Mark) -> [u8; INDEX_ENTRY] {
    let mut b = [0u8; INDEX_ENTRY];
    wr32(&mut b, 0, m.offset);
    b[4] = m.state();
    b
}

#[must_use]
pub fn decode_mark(b: &[u8]) -> Mark {
    Mark::from_state(rd32(b, 0), b.get(4).copied().unwrap_or(0))
}

/// Bytes per contents entry: offset (4), level (1), length (1), title.
pub const TOC_ENTRY: usize = 6 + TITLE_MAX;

#[must_use]
pub fn encode_toc(e: &TocEntry) -> [u8; TOC_ENTRY] {
    let mut b = [0u8; TOC_ENTRY];
    wr32(&mut b, 0, e.offset);
    b[4] = e.level;
    b[5] = e.len.min(TITLE_MAX as u8);
    b[6..].copy_from_slice(&e.title);
    b
}

#[must_use]
pub fn decode_toc(b: &[u8]) -> Option<TocEntry> {
    let b = b.get(..TOC_ENTRY)?;
    let mut title = [0u8; TITLE_MAX];
    title.copy_from_slice(&b[6..]);
    Some(TocEntry { offset: rd32(b, 0), level: b[4].clamp(1, 3), len: b[5].min(TITLE_MAX as u8), title })
}

impl TocEntry {
    /// Where reading starts for this entry.
    #[must_use]
    pub fn mark(&self) -> Mark {
        Mark::para(self.offset, Kind::heading(self.level))
    }
}

/// Bookmarks kept per book.
pub const MAX_BOOKMARKS: usize = 12;
/// Bytes of text kept with a bookmark (its first words).
pub const SNIPPET: usize = 26;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Bookmark {
    pub mark: Mark,
    pub snippet: MetaText<SNIPPET>,
}

/// A book's reading state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BookState {
    pub pos: Mark,
    pub marks: [Bookmark; MAX_BOOKMARKS],
    pub n: u8,
}

/// Size of a state file.
pub const STATE_BYTES: usize = 16 + MAX_BOOKMARKS * (6 + SNIPPET);

impl BookState {
    #[must_use]
    pub fn bookmarks(&self) -> &[Bookmark] {
        self.marks.get(..usize::from(self.n)).unwrap_or(&[])
    }

    /// Add a bookmark (kept in reading order; one at the same offset is
    /// replaced). False if all are used.
    pub fn add(&mut self, b: Bookmark) -> bool {
        let n = usize::from(self.n);
        if let Some(i) = self.marks[..n].iter().position(|m| m.mark.offset == b.mark.offset) {
            self.marks[i] = b;
            return true;
        }
        if n >= MAX_BOOKMARKS {
            return false;
        }
        let at = self.marks[..n].iter().position(|m| m.mark.offset > b.mark.offset).unwrap_or(n);
        self.marks.copy_within(at..n, at + 1);
        self.marks[at] = b;
        self.n += 1;
        true
    }

    pub fn remove(&mut self, i: usize) {
        let n = usize::from(self.n);
        if i < n {
            self.marks.copy_within(i + 1..n, i);
            self.n -= 1;
        }
    }

    #[must_use]
    pub fn encode(&self) -> [u8; STATE_BYTES] {
        let mut b = [0u8; STATE_BYTES];
        b[..4].copy_from_slice(b"OBP1");
        b[4] = self.n.min(MAX_BOOKMARKS as u8);
        wr32(&mut b, 8, self.pos.offset);
        b[12] = self.pos.state();
        for (i, m) in self.bookmarks().iter().enumerate() {
            let o = 16 + i * (6 + SNIPPET);
            wr32(&mut b, o, m.mark.offset);
            b[o + 4] = m.mark.state();
            b[o + 5] = m.snippet.len.min(SNIPPET as u8);
            b[o + 6..o + 6 + SNIPPET].copy_from_slice(&m.snippet.buf);
        }
        let sum = b.iter().skip(8).fold(0u16, |a, &x| a.wrapping_mul(31).wrapping_add(u16::from(x)));
        wr16(&mut b, 6, sum);
        b
    }

    /// `None` if it is not a good state file.
    #[must_use]
    pub fn decode(b: &[u8]) -> Option<BookState> {
        let b = b.get(..STATE_BYTES)?;
        if &b[..4] != b"OBP1" {
            return None;
        }
        let sum = b.iter().skip(8).fold(0u16, |a, &x| a.wrapping_mul(31).wrapping_add(u16::from(x)));
        if sum != rd16(b, 6) {
            return None;
        }
        let mut s = BookState { n: b[4].min(MAX_BOOKMARKS as u8), ..BookState::default() };
        s.pos = Mark::from_state(rd32(b, 8), b[12]);
        for i in 0..usize::from(s.n) {
            let o = 16 + i * (6 + SNIPPET);
            let mut snippet = MetaText::<SNIPPET>::default();
            snippet.buf.copy_from_slice(&b[o + 6..o + 6 + SNIPPET]);
            snippet.len = b[o + 5].min(SNIPPET as u8);
            s.marks[i] = Bookmark { mark: Mark::from_state(rd32(b, o), b[o + 4]), snippet };
        }
        Some(s)
    }
}

/// Books the library holds at most.
pub const MAX_BOOKS: usize = 200;
/// Header, then the order table (display row → record).
pub const LIB_HEADER: usize = 16;
pub const LIB_ORDER: usize = 256;
/// Bytes per library record.
pub const LIB_RECORD: usize = 384;
/// Longest book path kept (a longer one is not listed).
pub const PATH_MAX: usize = 255;
/// Where a record's progress is (rewritten when a book is closed).
pub const LIB_PROGRESS_AT: usize = 380;

/// Byte offset of record `i` in the library file.
#[must_use]
pub fn lib_record_at(i: u16) -> u64 {
    (LIB_HEADER + LIB_ORDER + usize::from(i) * LIB_RECORD) as u64
}

/// The library file's header.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LibHeader {
    pub count: u16,
    /// Where it was searched (0 the books folder, 1 the whole card).
    pub scope: u8,
    pub complete: bool,
}

impl LibHeader {
    #[must_use]
    pub fn encode(&self) -> [u8; LIB_HEADER] {
        let mut b = [0u8; LIB_HEADER];
        b[..4].copy_from_slice(b"OBL1");
        wr16(&mut b, 4, self.count);
        b[6] = self.scope;
        b[7] = u8::from(self.complete);
        b
    }

    #[must_use]
    pub fn decode(b: &[u8]) -> Option<LibHeader> {
        let b = b.get(..LIB_HEADER)?;
        if &b[..4] != b"OBL1" {
            return None;
        }
        let count = rd16(b, 4);
        (usize::from(count) <= MAX_BOOKS).then_some(LibHeader { count, scope: b[6], complete: b[7] == 1 })
    }
}

/// Reading progress not known (never opened).
pub const PROGRESS_NEW: u16 = u16::MAX;

/// One book of the library.
#[derive(Clone, Copy, Debug)]
pub struct LibRecord {
    pub path: [u8; PATH_MAX],
    pub path_len: u8,
    pub title: MetaText<64>,
    pub author: MetaText<48>,
    pub format: Format,
    pub size: u32,
    pub stamp: u32,
    /// Per mille read, or [`PROGRESS_NEW`].
    pub progress: u16,
}

impl Default for LibRecord {
    fn default() -> Self {
        LibRecord {
            path: [0; PATH_MAX],
            path_len: 0,
            title: MetaText::default(),
            author: MetaText::default(),
            format: Format::Txt,
            size: 0,
            stamp: 0,
            progress: PROGRESS_NEW,
        }
    }
}

impl LibRecord {
    #[must_use]
    pub fn path(&self) -> &str {
        core::str::from_utf8(self.path.get(..usize::from(self.path_len)).unwrap_or(&[])).unwrap_or("")
    }

    pub fn set_path(&mut self, p: &str) -> bool {
        if p.len() > PATH_MAX {
            return false;
        }
        self.path[..p.len()].copy_from_slice(p.as_bytes());
        self.path_len = p.len() as u8;
        true
    }

    #[must_use]
    pub fn encode(&self) -> [u8; LIB_RECORD] {
        let mut b = [0u8; LIB_RECORD];
        b[0] = self.path_len;
        b[1..256].copy_from_slice(&self.path);
        b[256] = self.title.len;
        b[257..321].copy_from_slice(&self.title.buf);
        b[321] = self.author.len;
        b[322..370].copy_from_slice(&self.author.buf);
        b[370] = self.format as u8;
        wr32(&mut b, 372, self.size);
        wr32(&mut b, 376, self.stamp);
        wr16(&mut b, LIB_PROGRESS_AT, self.progress);
        b
    }

    #[must_use]
    pub fn decode(b: &[u8]) -> LibRecord {
        let mut r = LibRecord::default();
        let Some(b) = b.get(..LIB_RECORD) else { return r };
        r.path.copy_from_slice(&b[1..256]);
        r.path_len = b[0].min(PATH_MAX as u8);
        r.title.buf.copy_from_slice(&b[257..321]);
        r.title.len = b[256].min(64);
        r.author.buf.copy_from_slice(&b[322..370]);
        r.author.len = b[321].min(48);
        r.format = Format::from_u8(b[370]);
        r.size = rd32(b, 372);
        r.stamp = rd32(b, 376);
        r.progress = rd16(b, LIB_PROGRESS_AT);
        r
    }

    /// The order the library is listed in: a key from the title's first
    /// characters, folded (ASCII and Cyrillic letters to lower case).
    #[must_use]
    pub fn sort_key(&self) -> u32 {
        let mut k = 0u32;
        let mut n = 0;
        for c in self.title.as_str().chars() {
            if n == 4 {
                break;
            }
            let w = match c {
                'A'..='Z' => u32::from(c) + 32,
                'a'..='z' | '0'..='9' => u32::from(c),
                'А'..='Я' => 0x80 + (u32::from(c) - 0x410),
                'а'..='я' => 0x80 + (u32::from(c) - 0x430),
                'Ё' | 'ё' => 0x85,
                c if c.is_alphanumeric() => 0xF0,
                _ => continue,
            };
            k = (k << 8) | (w & 0xFF);
            n += 1;
        }
        k << (8 * (4 - n))
    }
}

/// A packed modification time (FAT date and time) to compare files.
#[must_use]
pub fn stamp(t: orpheus_fs::DateTime) -> u32 {
    let (d, tm) = t.to_fat();
    (u32::from(d) << 16) | u32::from(tm)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn headers_round_trip() {
        let h = NormHeader {
            format: 2,
            encoding: 3,
            complete: true,
            src_size: 123_456,
            src_stamp: 0xDEAD_BEEF,
            text_len: 99_999,
            toc_count: 42,
        };
        assert_eq!(NormHeader::decode(&h.encode()), Some(h));
        let mut bad = h.encode();
        bad[17] ^= 1;
        assert_eq!(NormHeader::decode(&bad), None, "checksum");
        let i = IndexHeader { complete: true, key: 7, text_len: 9, pages: 11 };
        assert_eq!(IndexHeader::decode(&i.encode()), Some(i));
        let l = LibHeader { count: 12, scope: 1, complete: true };
        assert_eq!(LibHeader::decode(&l.encode()), Some(l));
    }

    #[test]
    fn records_round_trip() {
        let mut r = LibRecord::default();
        assert!(r.set_path("/Books/Война и мир.fb2"));
        r.title.push_str("Война и мир");
        r.author.push_str("Лев Толстой");
        r.format = Format::Fb2;
        r.size = 5_000_000;
        r.stamp = 77;
        r.progress = 314;
        let d = LibRecord::decode(&r.encode());
        assert_eq!((d.path(), d.title.as_str(), d.author.as_str()), (r.path(), "Война и мир", "Лев Толстой"));
        assert_eq!((d.format, d.size, d.stamp, d.progress), (Format::Fb2, 5_000_000, 77, 314));
        assert!(!r.set_path(&"x".repeat(300)));
        let toc = TocEntry { offset: 5, level: 2, len: 3, title: [b'a'; TITLE_MAX] };
        assert_eq!(decode_toc(&encode_toc(&toc)), Some(toc));
    }

    /// Position and bookmarks survive the file (BOOK-1 tests: bookmarks
    /// and position round trip); damage reads as no state.
    #[test]
    fn state_round_trip() {
        let mut s = BookState {
            pos: Mark { offset: 4242, kind: Kind::Quote, style: 2, first: false },
            ..BookState::default()
        };
        for k in [900u32, 100, 500, 100] {
            let mut snippet = MetaText::default();
            snippet.push_str("It was the best of times, it was");
            assert!(s.add(Bookmark { mark: Mark { offset: k, ..Mark::START }, snippet }));
        }
        assert_eq!(s.n, 3, "same offset replaced");
        assert_eq!(
            s.bookmarks().iter().map(|b| b.mark.offset).collect::<std::vec::Vec<u32>>(),
            [100, 500, 900]
        );
        let d = BookState::decode(&s.encode()).unwrap();
        assert_eq!(d, s);
        assert_eq!(d.marks[0].snippet.as_str(), "It was the best of times,");
        s.remove(1);
        assert_eq!(BookState::decode(&s.encode()).unwrap().bookmarks().len(), 2);
        for _ in 0..20 {
            s.add(Bookmark { mark: Mark { offset: s.n.into(), ..Mark::START }, ..Bookmark::default() });
        }
        assert_eq!(usize::from(s.n), MAX_BOOKMARKS);
        let mut bad = s.encode();
        bad[100] ^= 0x40;
        assert!(BookState::decode(&bad).is_none());
        const { assert!(STATE_BYTES <= 512, "one sector") };
    }

    #[test]
    fn ids_and_paths() {
        assert_eq!(book_id("/Books/A.txt"), book_id("/BOOKS/a.TXT"));
        assert_ne!(book_id("/Books/A.txt"), book_id("/Books/B.txt"));
        let mut p = PathBuf::new();
        assert_eq!(p.file(CACHE_DIR, 0x00AB_CDEF, "idx"), "/.orpheus/cache/books/00abcdef.idx");
        let mut r = LibRecord::default();
        r.title.push_str("Zebra");
        let mut a = LibRecord::default();
        a.title.push_str("apple");
        let mut c = LibRecord::default();
        c.title.push_str("Анна");
        assert!(a.sort_key() < r.sort_key() && r.sort_key() < c.sort_key());
    }
}
