//! orpheus-library: the music library index on the card (04 §6.4, 05 §7.1).
//!
//! A [`Builder`] walks every visible folder of the card (breadth first,
//! hidden entries and `/.orpheus` skipped, depth ≤ [`Config::max_depth`]),
//! reads the tags of each audio file through [`TagSource`] with a bounded
//! per-file read budget ([`orpheus_meta::window`]), and writes the library
//! file under `/.orpheus/library/` through [`orpheus_index::Storage`]: a
//! catalogue of the raw tags, a deduplicated string pool in collation
//! order, fixed-size track rows sorted by (album artist, album, disc,
//! track, title), group tables (artists, albums, genres, genre-artists,
//! genre-albums), orders (songs by title, genre tree, recently added, by
//! locator, albums by name and by year) and letter-run tables. Every sort
//! is the bounded-RAM external merge sort of [`orpheus_index::ExtSort`] in a
//! lent scratch buffer (≥ [`MIN_SCRATCH`], 12–16 KiB intended);
//! [`Builder::step`] does bounded work, [`Builder::suspend`] hands the
//! scratch back.
//!
//! Incremental: the next build compares each folder's [`Signature`]
//! (entry count, name bytes, newest mtime, order-independent hash) with
//! the one stored for it; an unchanged folder's catalogue items are copied
//! without reading a single tag, and in a changed folder each file whose
//! locator, name hash, size and mtime are unchanged keeps its item. Only
//! new and changed files are read. The tables after the catalogue are
//! rebuilt every time (sorts of small fixed rows).
//!
//! Power-cut safe: two slots (`lib-a.ydb`, `lib-b.ydb`); a build writes the
//! slot that is not current and its header (zero until then) last, after a
//! sync. A half-written slot never opens; the previous library stays usable
//! during the whole build.
//!
//! Queries ([`Library`]) read windows of rows of each [`List`] with a few
//! hundred bytes of stack, answer counts, letters and letter jumps
//! (`seek_letter`), `position_of`, and produce play queues of
//! [`TrackRef`]s for a list (album, artist, genre, all songs) and a uniform
//! random sample for Shuffle all.
//!
//! `no_std`, no allocator, no `unsafe`, no hardware.
//!
//! # Firmware integration
//!
//! * [`Storage`] over `/.orpheus/library/` ([`LIBRARY_DIR`]; the file names
//!   are at most 10 bytes, no sharding needed: `orpheus_index::card::
//!   CardIo::new_in`) and [`TagSource`] over the volume ([`card::CardTags`]):
//!   `open_dir` = `open_locator` + the entry's cursor (root: the root
//!   cursor; `after`: `seek_dir` to the entry and read past it),
//!   `open_file` = `open_entry` of the entry just listed (else
//!   `open_locator` first), `read_at` = seek + read.
//! * The firmware keeps the build state in place instead of moving a
//!   [`Builder`] around: [`Suspended::new`], then [`Suspended::step_in`]
//!   with the lent scratch, [`Suspended::pause_in`] before the scratch goes
//!   elsewhere (no 1 KiB copies on the stack).
//! * Scheduling as the directory index: [`Builder::step`] in the idle loop
//!   on the lent scratch (8 KiB minimum, 12–16 KiB for whole tag windows),
//!   [`Builder::suspend`] as soon as anything else needs the card or the
//!   arena; [`Suspended`] (≈ 1.1 KiB) is the only permanent state. The
//!   card's open files and folder cursor may be reused between steps
//!   (`open_dir(.., after)` repositions). Start a build on card insertion
//!   when no [`Library`] opens, and as an update when the user asks
//!   (Settings › Storage › Rebuild library) or after USB mass storage.
//! * Music screens: Artists → [`List::Artists`], OK → [`List::open`]
//!   (artist albums → album tracks); Albums → [`List::Albums`]; Songs →
//!   [`List::Songs`]; Genres → [`List::Genres`] → genre artists → their
//!   albums → tracks; Recently added → [`List::Recent`] (cap the rows the
//!   UI shows). Play/Shuffle on a container: [`List::tracks_of`] then
//!   [`Library::queue`] into `/.orpheus/queue.bin`; OK on a track: queue the
//!   list and start at its row. Shuffle all: [`Library::sample`] (in
//!   library order; play it shuffled). Go to album/artist from Now
//!   Playing: [`Library::find`] → [`Track::album_id`] / `artist_id`.
//!   Names: [`Library::text`] (`""` = unknown artist/album/genre).

#![no_std]
#![forbid(unsafe_code)]

mod build;
pub mod card;
mod collate;
pub mod format;
mod io;
mod query;

pub use build::{Builder, Config, MIN_SCRATCH, Progress, ScanStats, Suspended};
pub use collate::{sort_name, strip_article};
pub use format::{GRP_ROW, Kind, StrRef, TRK_ROW, VERSION};
pub use orpheus_index::{DirSource, Error, FileName, Signature, SrcEntry, Storage};
pub use orpheus_meta::window::Budget;
pub use orpheus_player::TrackRef;
pub use query::{FindStep, Item, ItemKind, Library, List, Track};

use orpheus_fs::Locator;

/// The folder under which the library's files live (the firmware's
/// [`Storage`] adapter joins [`FileName`]s to it).
pub const LIBRARY_DIR: &str = "/.orpheus/library";

/// The card's music side: folders listed front to back ([`DirSource`]),
/// opened by their entry's locator, and files read at any offset. The
/// firmware implements it over `orpheus-fs`; the calls are synchronous (the
/// builder runs between audio refills).
pub trait TagSource: DirSource {
    /// List folder `dir` (`None`: the root; else the folder whose entry
    /// is at that locator). With `after`, continue after that entry (the
    /// last one handed out before a suspension); else from the first
    /// entry. Returns the folder's identity — the `dir` field of its
    /// entries' locators (its first cluster) — or `None` if it is gone.
    fn open_dir(&mut self, dir: Option<Locator>, after: Option<Locator>) -> Result<Option<u32>, Self::Error>;
    /// Open the file at `loc` for [`TagSource::read_at`]; its length, or
    /// `None` if it is gone (or a folder).
    fn open_file(&mut self, loc: Locator) -> Result<Option<u64>, Self::Error>;
    /// Read the open file at `offset`; fewer bytes only at its end.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

/// Container/codec of a track, from its extension (`TRK` byte 54).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FileFormat {
    Other = 0,
    Mp3 = 1,
    Mp2 = 2,
    Flac = 3,
    Wav = 4,
    Aiff = 5,
    Ogg = 6,
    Opus = 7,
    Mp4 = 8,
    WavPack = 9,
    Ape = 10,
    Wma = 11,
    Au = 12,
    Speex = 13,
}

impl FileFormat {
    /// From a file name's extension.
    #[must_use]
    pub fn of(name: &str) -> FileFormat {
        let Some((_, ext)) = name.rsplit_once('.') else { return FileFormat::Other };
        let is = |e: &str| ext.eq_ignore_ascii_case(e);
        if is("mp3") {
            FileFormat::Mp3
        } else if is("mp2") {
            FileFormat::Mp2
        } else if is("flac") {
            FileFormat::Flac
        } else if is("wav") {
            FileFormat::Wav
        } else if is("aif") || is("aiff") || is("aifc") {
            FileFormat::Aiff
        } else if is("ogg") || is("oga") {
            FileFormat::Ogg
        } else if is("opus") {
            FileFormat::Opus
        } else if is("m4a") || is("mp4") || is("aac") || is("alac") {
            FileFormat::Mp4
        } else if is("wv") {
            FileFormat::WavPack
        } else if is("ape") {
            FileFormat::Ape
        } else if is("wma") {
            FileFormat::Wma
        } else if is("au") || is("snd") {
            FileFormat::Au
        } else if is("spx") {
            FileFormat::Speex
        } else {
            FileFormat::Other
        }
    }

    #[must_use]
    pub fn from_code(c: u8) -> FileFormat {
        match c {
            1 => FileFormat::Mp3,
            2 => FileFormat::Mp2,
            3 => FileFormat::Flac,
            4 => FileFormat::Wav,
            5 => FileFormat::Aiff,
            6 => FileFormat::Ogg,
            7 => FileFormat::Opus,
            8 => FileFormat::Mp4,
            9 => FileFormat::WavPack,
            10 => FileFormat::Ape,
            11 => FileFormat::Wma,
            12 => FileFormat::Au,
            13 => FileFormat::Speex,
            _ => FileFormat::Other,
        }
    }

    /// The tag parser's format, `None` where `orpheus-meta` reads nothing
    /// useful (Ogg and MP4 carry their tags inside the container): such
    /// files are listed under their file and folder names without a read.
    #[must_use]
    pub fn tag_format(self) -> Option<orpheus_audio::Format> {
        use orpheus_audio::Format as F;
        Some(match self {
            FileFormat::Mp3 | FileFormat::Mp2 => F::Mpeg,
            FileFormat::Flac => F::Flac,
            FileFormat::Wav => F::Wav,
            FileFormat::Aiff => F::Aiff,
            FileFormat::Ape => F::Ape,
            FileFormat::WavPack => F::WavPack,
            _ => return None,
        })
    }
}

/// The UI's names of the lists ([`orpheus_apps::services::LibList`]).
impl From<orpheus_apps::services::LibList> for List {
    fn from(l: orpheus_apps::services::LibList) -> List {
        use orpheus_apps::services::LibList as U;
        match l {
            U::Artists => List::Artists,
            U::ArtistAlbums(i) => List::ArtistAlbums(i),
            U::ArtistTracks(i) => List::ArtistTracks(i),
            U::Albums => List::Albums,
            U::AlbumTracks(i) => List::AlbumTracks(i),
            U::Songs => List::Songs,
            U::Genres => List::Genres,
            U::GenreArtists(i) => List::GenreArtists(i),
            U::GenreArtistAlbums(i) => List::GenreArtistAlbums(i),
            U::GenreAlbumTracks(i) => List::GenreAlbumTracks(i),
            U::GenreTracks(i) => List::GenreTracks(i),
            U::GenreArtistTracks(i) => List::GenreArtistTracks(i),
            U::Recent => List::Recent,
        }
    }
}

impl From<ItemKind> for orpheus_apps::services::LibKind {
    fn from(k: ItemKind) -> Self {
        use orpheus_apps::services::LibKind as U;
        match k {
            ItemKind::Artist => U::Artist,
            ItemKind::Album => U::Album,
            ItemKind::Genre => U::Genre,
            ItemKind::GenreArtist => U::GenreArtist,
            ItemKind::GenreAlbum => U::GenreAlbum,
            ItemKind::Track => U::Track,
        }
    }
}

/// Track flags (`TRK` byte 55, catalogue byte 37).
pub mod flags {
    /// No title tag: the title is the file name without its extension.
    pub const TITLE_FROM_NAME: u8 = 1;
    /// No album tag: the album is the folder name.
    pub const ALBUM_FROM_FOLDER: u8 = 2;
    /// The read budget stopped the tag parser early.
    pub const TRUNCATED: u8 = 4;
    /// No track tag: the number leading the file name.
    pub const TRACK_FROM_NAME: u8 = 8;
}
