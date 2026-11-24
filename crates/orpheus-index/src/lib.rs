//! orpheus-index: the sorted directory index on the card (04 §6.2, 05 §7.2).
//!
//! A directory is listed in the Files app's order — directories first, then
//! by [`SortBy`] (date and size: newest/largest first), then
//! [`natural_cmp`](orpheus_apps::browse::natural_cmp), then the entry's
//! locator — from a per-directory file under `/.orpheus/index/` of fixed
//! 32-byte rows. Building it is an external merge sort in a borrowed
//! scratch buffer (≥ [`MIN_SCRATCH`] bytes, 4–8 KiB intended) driven by
//! [`Builder::step`] in bounded slices; querying reads windows of rows with
//! no whole-directory state in RAM.
//!
//! `no_std`, no allocator, no `unsafe`, no hardware: the card is reached
//! through two small synchronous traits the firmware implements over
//! `orpheus-fs` — [`Storage`] (files inside `/.orpheus/index/` only) and
//! [`DirSource`] (the directory being indexed); [`NameSource`] reads one
//! entry's name by locator for [`Index::find`]. The [`card`] module is that
//! adapter for any `orpheus-fs` volume (index files sharded under
//! `/.orpheus/index/X/`; the async file system is driven to completion with
//! the caller's task context), host-tested over FAT32 and exFAT images.
//! [`LetterTable`] copies a listing's letter runs into RAM so a UI can
//! answer letter jumps without the card.
//!
//! # Staleness
//!
//! The header stores the [`Signature`] of the directory as the build saw it:
//! entry count, total name bytes, newest mtime and an order-independent
//! 64-bit hash over every entry's name, locator, size, mtime and flags. Any
//! add, remove, rename, resize or touch changes it. Checking it needs one
//! sequential pass over the directory ([`Verifier`], or [`Signature::add`]
//! inside a pass the caller makes anyway, e.g. while it shows the unsorted
//! listing) — no sorting, no writes. The header also carries an opaque
//! caller-supplied *quick stamp* (e.g. the directory's own entry mtime and
//! cluster count) that can be compared with no directory I/O at all; FAT
//! does not reliably update directory timestamps, so it is advisory: use it
//! to show the indexed order at once and confirm with the signature in the
//! background.
//!
//! # File format
//!
//! `[header block, 512 B][rows × 32 B][letter runs × 8 B]` — see
//! [`record`] for the field tables. Rows start sector aligned, so an aligned
//! 32-row window is two sector reads. The header is zero while a build is
//! in progress and is written last (with a CRC); a power cut leaves a file
//! that [`Index::open`] rejects. Letter runs are the maximal runs of equal
//! [`letter_of`](orpheus_apps::browse::letter_of) letters along the order
//! (`first row u32, letter u32`, 0 = no letter), so a letter jump is a
//! binary search.

#![no_std]
#![forbid(unsafe_code)]

mod build;
pub mod card;
pub mod extsort;
pub mod key;
mod letters;
mod query;
pub mod record;

pub use build::{Builder, MAX_K, MIN_SCRATCH, Progress, Suspended, Verifier};
pub use extsort::{ExtSort, MAX_SORT_ITEM, MIN_SORT_SCRATCH};
pub use letters::LetterTable;
pub use orpheus_apps::browse::{EntryKind, is_hidden_name, kind_of, letter_of, natural_cmp};
pub use orpheus_apps::services::{BrowseFilter, SortBy};
pub use orpheus_fs::{Locator, MAX_NAME_UTF8};
pub use query::{Index, Probe};
pub use record::crc32_update;
pub use record::{HEADER, LETTER_RUN, RECORD, Row, Signature, VERSION};

/// Longest name (UTF-8 bytes) an entry may have: 255 UTF-16 units.
pub const MAX_NAME: usize = MAX_NAME_UTF8;

/// Which listing an index holds. Each combination is its own file.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Spec {
    pub sort: SortBy,
    /// Include hidden entries (dot names, `System Volume Information`, FAT
    /// hidden/system attribute).
    pub show_hidden: bool,
    /// Directories, audio and playlists only (Music › Folders).
    pub audio_only: bool,
    /// Directories and pictures only (Pictures).
    pub images_only: bool,
}

impl From<BrowseFilter> for Spec {
    fn from(f: BrowseFilter) -> Spec {
        Spec { sort: f.sort, show_hidden: f.hidden, audio_only: f.audio_only, images_only: f.images_only }
    }
}

impl Spec {
    pub(crate) fn code(self) -> u8 {
        let s = match self.sort {
            SortBy::Name => 0,
            SortBy::Date => 1,
            SortBy::Size => 2,
        };
        s | (u8::from(self.show_hidden) << 2)
            | (u8::from(self.audio_only) << 3)
            | (u8::from(self.images_only) << 4)
    }

    pub(crate) fn from_code(c: u8) -> Option<Spec> {
        let sort = match c & 3 {
            0 => SortBy::Name,
            1 => SortBy::Date,
            2 => SortBy::Size,
            _ => return None,
        };
        (c & !0x1F == 0).then_some(Spec {
            sort,
            show_hidden: c & 4 != 0,
            audio_only: c & 8 != 0,
            images_only: c & 0x10 != 0,
        })
    }

    /// Whether an entry appears in this listing.
    #[must_use]
    pub fn keeps(self, name: &str, e: &SrcEntry) -> bool {
        let hidden = e.hidden || is_hidden_name(name);
        let kind = kind_of(name, e.is_dir);
        (self.show_hidden || !hidden)
            && (!self.audio_only || matches!(kind, EntryKind::Dir | EntryKind::Audio | EntryKind::Playlist))
            && (!self.images_only || matches!(kind, EntryKind::Dir | EntryKind::Image))
    }
}

/// A file name inside `/.orpheus/index/` (ASCII, ≤ 16 bytes). The storage
/// adapter joins it to that directory; the index never names anything else.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileName {
    buf: [u8; 16],
    len: u8,
}

impl FileName {
    /// A file name of at most 16 ASCII bytes (no `/`), `None` otherwise.
    #[must_use]
    pub const fn new(s: &str) -> Option<FileName> {
        let b = s.as_bytes();
        if b.is_empty() || b.len() > 16 {
            return None;
        }
        let mut i = 0;
        while i < b.len() {
            if !b[i].is_ascii() || b[i] == b'/' || b[i] < 0x20 {
                return None;
            }
            i += 1;
        }
        Some(FileName::lit(s))
    }

    const fn lit(s: &str) -> FileName {
        let b = s.as_bytes();
        let mut buf = [0u8; 16];
        let mut i = 0;
        while i < b.len() {
            buf[i] = b[i];
            i += 1;
        }
        FileName { buf, len: b.len() as u8 }
    }

    /// First merge scratch file.
    pub const TEMP_A: FileName = FileName::lit("sort-a.tmp");
    /// Second merge scratch file.
    pub const TEMP_B: FileName = FileName::lit("sort-b.tmp");

    /// The index of `dir` for `spec`: `DDDDDDDD-SF.idx` (directory in hex,
    /// sort letter `n`/`d`/`s`, filter flags in hex).
    #[must_use]
    pub fn index(dir: u32, spec: Spec) -> FileName {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut buf = [0u8; 16];
        for (i, b) in buf[..8].iter_mut().enumerate() {
            *b = HEX[((dir >> (28 - 4 * i)) & 0xF) as usize];
        }
        buf[8] = b'-';
        buf[9] = match spec.sort {
            SortBy::Name => b'n',
            SortBy::Date => b'd',
            SortBy::Size => b's',
        };
        buf[10] = HEX[usize::from(spec.code() >> 2)];
        buf[11..15].copy_from_slice(b".idx");
        FileName { buf, len: 15 }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("")
    }
}

/// Files under `/.orpheus/index/` (the firmware adapts `orpheus-fs`; the host
/// tests use memory). Synchronous: the firmware calls the builder from its
/// own task between audio refills.
pub trait Storage {
    type Error;
    /// File length, `None` if it does not exist.
    fn len(&mut self, file: FileName) -> Result<Option<u32>, Self::Error>;
    /// Read at `offset`; returns the bytes read, fewer than `buf.len()` only
    /// at the end of the file (0 for a missing file).
    fn read_at(&mut self, file: FileName, offset: u32, buf: &mut [u8]) -> Result<usize, Self::Error>;
    /// Write at `offset` (≤ the current length; creates the file at 0).
    fn write_at(&mut self, file: FileName, offset: u32, data: &[u8]) -> Result<(), Self::Error>;
    /// Cut (or create) the file to `len` bytes (≤ the current length).
    fn truncate(&mut self, file: FileName, len: u32) -> Result<(), Self::Error>;
    /// Make everything written so far durable before anything written
    /// later (the builder calls it before the header that completes an
    /// index).
    fn sync(&mut self, _file: FileName) -> Result<(), Self::Error> {
        Ok(())
    }
    /// Delete a file the builder no longer needs (default: empty it). A
    /// missing file is not an error.
    fn remove(&mut self, file: FileName) -> Result<(), Self::Error> {
        self.truncate(file, 0)
    }
}

/// [`SrcEntry::mtime`] from a `orpheus-fs` timestamp: year, month, day,
/// hour and minute packed in that order of significance (seconds dropped,
/// as the simulator's date order compares to the minute).
#[must_use]
pub fn mtime_of(t: &orpheus_fs::DateTime) -> u32 {
    (u32::from(t.year.saturating_sub(1980)) << 20)
        | (u32::from(t.month & 0xF) << 16)
        | (u32::from(t.day & 0x1F) << 11)
        | (u32::from(t.hour & 0x1F) << 6)
        | u32::from(t.minute & 0x3F)
}

/// One directory entry as the source reports it; the name goes into the
/// buffer the caller of [`DirSource::next_entry`] passed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SrcEntry {
    pub loc: Locator,
    pub is_dir: bool,
    /// FAT hidden or system attribute (names are checked separately).
    pub hidden: bool,
    pub size: u64,
    /// Modification time as an ordered integer (larger = newer); entries
    /// with equal values tie and fall back to the name. The firmware uses
    /// [`mtime_of`] (minutes, like the simulator's date order).
    pub mtime: u32,
    /// Bytes of UTF-8 name written to the buffer.
    pub name_len: usize,
}

/// The directory being indexed, read front to back.
pub trait DirSource {
    type Error;
    /// Start again at the first entry.
    fn rewind(&mut self) -> Result<(), Self::Error>;
    /// The next entry (skipping `.`/`..` and free slots), its UTF-8 name in
    /// `name[..entry.name_len]`; `name` holds at least [`MAX_NAME`] bytes.
    fn next_entry(&mut self, name: &mut [u8]) -> Result<Option<SrcEntry>, Self::Error>;
}

/// Reads one entry's name by locator (for [`Index::find`] on key ties).
pub trait NameSource {
    type Error;
    /// Bytes of UTF-8 name written to `name`, `None` if the entry is gone.
    fn name_of(&mut self, loc: Locator, name: &mut [u8]) -> Result<Option<usize>, Self::Error>;
}

/// What went wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error<E> {
    /// The storage or the directory source failed.
    Io(E),
    /// A file of ours is shorter or different than it must be (the index
    /// or a scratch file was changed under us): rebuild.
    Corrupt,
    /// A name is longer than [`MAX_NAME`] or not UTF-8.
    BadName,
    /// The scratch buffer is smaller than [`MIN_SCRATCH`] (or than the
    /// suspended merge needs).
    ScratchTooSmall,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::Io(e)
    }
}
