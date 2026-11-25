//! Reading the library: windows of rows of each list, strings, letters,
//! positions, play queues and samples. Nothing here holds more than a
//! 512-byte stack buffer and the caller's window.

use orpheus_fs::Locator;
use orpheus_index::{Error, FileName, Storage};
use orpheus_player::TrackRef;

use crate::build::SLOTS;
use crate::format::{
    GRP_ROW, HEADER, Header, Kind, LETTER_RUN, SECTIONS, Sec, StrRef, TRK_ROW, grp, rd16, rd32, trk,
};

/// A library search by locator in progress ([`Library::find_begin`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindStep {
    loc: Locator,
    hint: u32,
    /// 0: try `hint + 1`, 1: try `hint`, 2: binary search in `lo..hi`.
    phase: u8,
    lo: u32,
    hi: u32,
}

/// A Shuffle-all sample in progress ([`Library::sample_begin`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SampleStep {
    s: u64,
    i: u32,
    picked: u32,
    want: u32,
    stopped: bool,
}

impl SampleStep {
    /// Tracks picked so far.
    #[must_use]
    pub fn picked(&self) -> u32 {
        self.picked
    }

    /// Rows of the track table looked at so far.
    #[must_use]
    pub fn rows_seen(&self) -> u32 {
        self.i
    }

    /// splitmix64.
    fn next(&mut self) -> u64 {
        self.s = self.s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// An open, complete library (about 230 bytes; no file handle is kept).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Library {
    file: FileName,
    slot: u8,
    options: u16,
    generation: u32,
    serial: u32,
    tracks: u32,
    stamp: u32,
    kinds: [u32; 4],
    str_end: u32,
    secs: [(u32, u32); SECTIONS],
}

/// A list the Music screens show (05 §7.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum List {
    /// Album artists (the track artist where no album artist is tagged).
    Artists,
    /// Albums of an artist (by year, then name).
    ArtistAlbums(u32),
    /// "All songs" of an artist (album order).
    ArtistTracks(u32),
    /// All albums by name.
    Albums,
    /// Tracks of an album (disc, track, title).
    AlbumTracks(u32),
    /// All tracks by title.
    Songs,
    Genres,
    /// Artists of a genre.
    GenreArtists(u32),
    /// Albums of a genre-artist (by year, then name).
    GenreArtistAlbums(u32),
    /// Tracks of a genre-album.
    GenreAlbumTracks(u32),
    /// All tracks of a genre (artist, album order).
    GenreTracks(u32),
    /// All tracks of a genre-artist.
    GenreArtistTracks(u32),
    /// Tracks by first-seen time, newest first.
    Recent,
}

/// What a row is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Artist,
    Album,
    Genre,
    /// An artist within a genre.
    GenreArtist,
    /// An album within a genre-artist.
    GenreAlbum,
    Track,
}

/// One row of a list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Item {
    pub kind: ItemKind,
    /// Row of the item's table: open it with [`List::open`].
    pub id: u32,
    /// Artist, album, genre name or track title.
    pub name: StrRef,
    /// Albums: the album artist; tracks: the artist; genre-artists: the
    /// genre.
    pub sub: StrRef,
    /// Tracks (1 for a track).
    pub tracks: u32,
    /// Albums of an artist, artists of a genre, albums of a genre-artist.
    pub children: u32,
    pub year: u16,
    pub track: u16,
    pub disc: u16,
    /// Tracks: playing time in ms (0 = unknown).
    pub duration_ms: u32,
    /// Albums: tracks by several artists. Tracks: [`crate::flags`].
    pub flags: u8,
}

impl Item {
    const NONE: Item = Item {
        kind: ItemKind::Track,
        id: 0,
        name: StrRef::NONE,
        sub: StrRef::NONE,
        tracks: 0,
        children: 0,
        year: 0,
        track: 0,
        disc: 0,
        duration_ms: 0,
        flags: 0,
    };
}

impl Default for Item {
    fn default() -> Self {
        Item::NONE
    }
}

/// Everything about one track.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Track {
    /// Row in the track table.
    pub id: u32,
    pub r: TrackRef,
    pub title: StrRef,
    pub artist: StrRef,
    pub album: StrRef,
    pub album_artist: StrRef,
    pub genre: StrRef,
    /// Its album and artist rows (for "Go to album / artist").
    pub album_id: u32,
    pub artist_id: u32,
    pub duration_ms: u32,
    /// First seen ([`crate::Config::now`] encoding).
    pub added: u32,
    pub year: u16,
    pub track: u16,
    pub disc: u16,
    pub format: crate::FileFormat,
    pub flags: u8,
}

impl List {
    /// The list that opens from `item` of this list (`None` for tracks).
    #[must_use]
    pub fn open(self, item: &Item) -> Option<List> {
        Some(match item.kind {
            ItemKind::Artist => List::ArtistAlbums(item.id),
            ItemKind::Album => List::AlbumTracks(item.id),
            ItemKind::Genre => List::GenreArtists(item.id),
            ItemKind::GenreArtist => List::GenreArtistAlbums(item.id),
            ItemKind::GenreAlbum => List::GenreAlbumTracks(item.id),
            ItemKind::Track => return None,
        })
    }

    /// The tracks of `item` (Menu → Play / Shuffle / Add to queue on a
    /// container); for a track, `None` (queue the list itself).
    #[must_use]
    pub fn tracks_of(item: &Item) -> Option<List> {
        Some(match item.kind {
            ItemKind::Artist => List::ArtistTracks(item.id),
            ItemKind::Album => List::AlbumTracks(item.id),
            ItemKind::Genre => List::GenreTracks(item.id),
            ItemKind::GenreArtist => List::GenreArtistTracks(item.id),
            ItemKind::GenreAlbum => List::GenreAlbumTracks(item.id),
            ItemKind::Track => return None,
        })
    }

    /// Whether the rows are tracks (a queue can be made of it).
    #[must_use]
    pub fn is_tracks(self) -> bool {
        matches!(
            self,
            List::ArtistTracks(_)
                | List::AlbumTracks(_)
                | List::Songs
                | List::GenreAlbumTracks(_)
                | List::GenreTracks(_)
                | List::GenreArtistTracks(_)
                | List::Recent
        )
    }

    fn letters(self) -> Option<Sec> {
        match self {
            List::Artists => Some(Sec::LArtists),
            List::Albums => Some(Sec::LAlbums),
            List::Genres => Some(Sec::LGenres),
            List::Songs => Some(Sec::LSongs),
            _ => None,
        }
    }
}

/// Where a list's rows are: rows `first..first + count` of `perm` (or of
/// `table` directly), each naming a row of `table`.
#[derive(Clone, Copy)]
struct Layout {
    table: Sec,
    perm: Option<Sec>,
    first: u32,
    count: u32,
    kind: ItemKind,
}

const CHUNK: usize = 64;

/// Byte offset of row `i` of `size`-byte rows (64 bits: rows and counts
/// come from the file).
fn at(i: u32, size: usize) -> u64 {
    u64::from(i) * size as u64
}

impl Library {
    pub(crate) fn from_parts(file: FileName, slot: u8, h: &Header) -> Library {
        let mut secs = [(0, 0); SECTIONS];
        for (d, s) in secs.iter_mut().zip(h.secs.iter()) {
            *d = (s.off, s.len);
        }
        Library {
            file,
            slot,
            options: h.options,
            generation: h.generation,
            serial: h.serial,
            tracks: h.tracks,
            stamp: h.stamp,
            kinds: h.kinds,
            str_end: h.str_end,
            secs,
        }
    }

    /// Open the newest complete library. `None` if there is none (never
    /// built, or every slot incomplete or damaged): build it.
    pub fn open<S: Storage>(store: &mut S) -> Result<Option<Library>, Error<S::Error>> {
        let mut best: Option<Library> = None;
        let mut b = [0u8; HEADER];
        for (slot, &file) in SLOTS.iter().enumerate() {
            let Some(len) = store.len(file)? else { continue };
            if (len as usize) < HEADER || store.read_at(file, 0, &mut b)? != HEADER {
                continue;
            }
            let Some(h) = Header::decode(&b) else { continue };
            if u64::from(len) != h.file_len() {
                continue;
            }
            let l = Library::from_parts(file, slot as u8, &h);
            if best.is_none_or(|x| l.generation.wrapping_sub(x.generation) as i32 > 0) {
                best = Some(l);
            }
        }
        Ok(best)
    }

    pub(crate) fn slot(&self) -> u8 {
        self.slot
    }
    pub(crate) fn file(&self) -> FileName {
        self.file
    }
    /// Offset and length of a section.
    pub(crate) fn section(&self, s: Sec) -> (u32, u32) {
        self.secs[s as usize]
    }
    /// Build generation (grows by one per build).
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.generation
    }
    /// The build's `now` ([`crate::Config::now`]).
    #[must_use]
    pub fn stamp(&self) -> u32 {
        self.stamp
    }
    /// Volume serial the library was built on.
    #[must_use]
    pub fn serial(&self) -> u32 {
        self.serial
    }
    /// Built with leading articles ignored.
    #[must_use]
    pub fn ignores_articles(&self) -> bool {
        self.options & 1 != 0
    }
    #[must_use]
    pub fn tracks(&self) -> u32 {
        self.tracks
    }
    #[must_use]
    pub fn artists(&self) -> u32 {
        self.secs[Sec::Artists as usize].1 / GRP_ROW as u32
    }
    #[must_use]
    pub fn albums(&self) -> u32 {
        self.secs[Sec::Albums as usize].1 / GRP_ROW as u32
    }
    #[must_use]
    pub fn genres(&self) -> u32 {
        self.secs[Sec::Genres as usize].1 / GRP_ROW as u32
    }
    /// Folders with tracks.
    #[must_use]
    pub fn folders(&self) -> u32 {
        self.secs[Sec::Dirs as usize].1 / crate::format::DIR_ROW as u32
    }
    /// Bytes on the card.
    #[must_use]
    pub fn file_len(&self) -> u64 {
        // As `Header::file_len`: the header alone is a file too (VER-4 F-LIB-2).
        self.secs.iter().map(|&(o, l)| u64::from(o) + u64::from(l)).max().unwrap_or(0).max(HEADER as u64)
    }
    /// The range of string references of a kind.
    #[must_use]
    pub fn strings(&self, k: Kind) -> core::ops::Range<u32> {
        let i = k as usize;
        self.kinds[i]..if i + 1 < 4 { self.kinds[i + 1] } else { self.str_end }
    }

    fn read<S: Storage>(
        &self,
        store: &mut S,
        s: Sec,
        off: u64,
        buf: &mut [u8],
    ) -> Result<(), Error<S::Error>> {
        // Offsets are computed in 64 bits from rows and counts in the file:
        // a damaged file makes them anything (VER-4 finding F-LIB-1). Inside
        // the section, `o + off` fits: `open` checked that every section
        // ends inside the file.
        let (o, l) = self.secs[s as usize];
        if off + buf.len() as u64 > u64::from(l) {
            return Err(Error::Corrupt);
        }
        if store.read_at(self.file, o + off as u32, buf)? != buf.len() {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    fn u32_at<S: Storage>(&self, store: &mut S, s: Sec, i: u32) -> Result<u32, Error<S::Error>> {
        let mut b = [0u8; 4];
        self.read(store, s, at(i, 4), &mut b)?;
        Ok(rd32(&b, 0))
    }

    fn group<S: Storage>(&self, store: &mut S, s: Sec, i: u32) -> Result<[u8; GRP_ROW], Error<S::Error>> {
        let mut b = [0u8; GRP_ROW];
        self.read(store, s, at(i, GRP_ROW), &mut b)?;
        Ok(b)
    }

    fn rows_in(&self, s: Sec, size: usize) -> u32 {
        self.secs[s as usize].1 / size as u32
    }

    fn layout<S: Storage>(&self, store: &mut S, list: List) -> Result<Layout, Error<S::Error>> {
        let all =
            |table: Sec, perm: Option<Sec>, kind, n: u32| Layout { table, perm, first: 0, count: n, kind };
        let sub = |store: &mut S, s: Sec, i: u32, children: bool| -> Result<(u32, u32), Error<S::Error>> {
            if i >= self.rows_in(s, GRP_ROW) {
                return Err(Error::Corrupt);
            }
            let g = self.group(store, s, i)?;
            Ok(if children {
                (rd32(&g, grp::CHILD), rd32(&g, grp::CHILDREN))
            } else {
                (rd32(&g, grp::FIRST), rd32(&g, grp::COUNT))
            })
        };
        let range =
            |table, perm, kind, (first, count): (u32, u32)| Layout { table, perm, first, count, kind };
        Ok(match list {
            List::Artists => all(Sec::Artists, None, ItemKind::Artist, self.artists()),
            List::Albums => all(Sec::Albums, Some(Sec::AlbName), ItemKind::Album, self.albums()),
            List::Genres => all(Sec::Genres, None, ItemKind::Genre, self.genres()),
            List::Songs => all(Sec::Trk, Some(Sec::Songs), ItemKind::Track, self.tracks),
            List::Recent => all(Sec::Trk, Some(Sec::Recent), ItemKind::Track, self.tracks),
            List::ArtistAlbums(a) => {
                range(Sec::Albums, Some(Sec::AlbYear), ItemKind::Album, sub(store, Sec::Artists, a, true)?)
            }
            List::ArtistTracks(a) => {
                range(Sec::Trk, None, ItemKind::Track, sub(store, Sec::Artists, a, false)?)
            }
            List::AlbumTracks(a) => {
                range(Sec::Trk, None, ItemKind::Track, sub(store, Sec::Albums, a, false)?)
            }
            List::GenreArtists(g) => {
                range(Sec::GArtists, None, ItemKind::GenreArtist, sub(store, Sec::Genres, g, true)?)
            }
            List::GenreArtistAlbums(g) => range(
                Sec::GAlbums,
                Some(Sec::GAlbYear),
                ItemKind::GenreAlbum,
                sub(store, Sec::GArtists, g, true)?,
            ),
            List::GenreTracks(g) => {
                range(Sec::Trk, Some(Sec::Genre), ItemKind::Track, sub(store, Sec::Genres, g, false)?)
            }
            List::GenreArtistTracks(g) => {
                range(Sec::Trk, Some(Sec::Genre), ItemKind::Track, sub(store, Sec::GArtists, g, false)?)
            }
            List::GenreAlbumTracks(g) => {
                range(Sec::Trk, Some(Sec::Genre), ItemKind::Track, sub(store, Sec::GAlbums, g, false)?)
            }
        })
    }

    /// Rows in `list`.
    pub fn len<S: Storage>(&self, store: &mut S, list: List) -> Result<u32, Error<S::Error>> {
        Ok(self.layout(store, list)?.count)
    }

    /// Table rows of list rows `[first, first + ids.len())` (clipped);
    /// returns how many.
    fn ids<S: Storage>(
        &self,
        store: &mut S,
        l: &Layout,
        first: u32,
        ids: &mut [u32],
    ) -> Result<usize, Error<S::Error>> {
        let n = (l.count.saturating_sub(first) as usize).min(ids.len());
        match l.perm {
            None => {
                for (k, d) in ids[..n].iter_mut().enumerate() {
                    *d = l.first.checked_add(first + k as u32).ok_or(Error::Corrupt)?;
                }
            }
            Some(p) => {
                let mut b = [0u8; 4 * CHUNK];
                let mut done = 0;
                while done < n {
                    let m = (n - done).min(CHUNK);
                    self.read(store, p, at(l.first, 4) + at(first + done as u32, 4), &mut b[..4 * m])?;
                    for (k, d) in ids[done..done + m].iter_mut().enumerate() {
                        *d = rd32(&b, 4 * k);
                    }
                    done += m;
                }
            }
        }
        Ok(n)
    }

    fn item_of(kind: ItemKind, id: u32, b: &[u8]) -> Item {
        if kind == ItemKind::Track {
            return Item {
                kind,
                id,
                name: StrRef(rd32(b, trk::TITLE)),
                sub: StrRef(rd32(b, trk::ARTIST)),
                tracks: 1,
                children: 0,
                year: rd16(b, trk::YEAR),
                track: rd16(b, trk::TRACK),
                disc: rd16(b, trk::DISC),
                duration_ms: rd32(b, trk::DURATION),
                flags: b[trk::FLAGS],
            };
        }
        let sub = match kind {
            ItemKind::Album | ItemKind::GenreAlbum | ItemKind::GenreArtist => StrRef(rd32(b, grp::SUB)),
            _ => StrRef::NONE,
        };
        Item {
            kind,
            id,
            name: StrRef(rd32(b, grp::NAME)),
            sub,
            tracks: rd32(b, grp::COUNT),
            children: if matches!(kind, ItemKind::Album | ItemKind::GenreAlbum) {
                0
            } else {
                rd32(b, grp::CHILDREN)
            },
            year: rd16(b, grp::YEAR),
            track: 0,
            disc: u16::from(b[grp::DISCS]),
            duration_ms: 0,
            flags: b[grp::FLAGS],
        }
    }

    /// Read rows `[first, first + out.len())` of `list` (clipped) into
    /// `out`; returns how many. Contiguous rows are read together;
    /// reordered lists cost one small read per row.
    pub fn rows<S: Storage>(
        &self,
        store: &mut S,
        list: List,
        first: u32,
        out: &mut [Item],
    ) -> Result<usize, Error<S::Error>> {
        let l = self.layout(store, list)?;
        let size = if l.kind == ItemKind::Track { TRK_ROW } else { GRP_ROW };
        let mut done = 0;
        let mut ids = [0u32; 8];
        let mut b = [0u8; 8 * TRK_ROW];
        loop {
            let n = self.ids(store, &l, first + done as u32, &mut ids[..(out.len() - done).min(8)])?;
            if n == 0 {
                break;
            }
            let contiguous = ids[..n].windows(2).all(|w| w[0].checked_add(1) == Some(w[1]));
            if contiguous {
                self.read(store, l.table, at(ids[0], size), &mut b[..n * size])?;
            } else {
                for (k, &id) in ids[..n].iter().enumerate() {
                    self.read(store, l.table, at(id, size), &mut b[k * size..(k + 1) * size])?;
                }
            }
            for k in 0..n {
                out[done + k] = Self::item_of(l.kind, ids[k], &b[k * size..(k + 1) * size]);
            }
            done += n;
        }
        Ok(done)
    }

    /// Row `i` of `list`.
    pub fn row<S: Storage>(
        &self,
        store: &mut S,
        list: List,
        i: u32,
    ) -> Result<Option<Item>, Error<S::Error>> {
        let mut r = [Item::NONE];
        Ok((self.rows(store, list, i, &mut r)? == 1).then_some(r[0]))
    }

    /// Item for table row `id` of the kind `list` shows (e.g. the album
    /// of a track, for "Go to album").
    pub fn item<S: Storage>(&self, store: &mut S, kind: ItemKind, id: u32) -> Result<Item, Error<S::Error>> {
        let (table, size) = match kind {
            ItemKind::Artist => (Sec::Artists, GRP_ROW),
            ItemKind::Album => (Sec::Albums, GRP_ROW),
            ItemKind::Genre => (Sec::Genres, GRP_ROW),
            ItemKind::GenreArtist => (Sec::GArtists, GRP_ROW),
            ItemKind::GenreAlbum => (Sec::GAlbums, GRP_ROW),
            ItemKind::Track => (Sec::Trk, TRK_ROW),
        };
        let mut b = [0u8; TRK_ROW];
        self.read(store, table, at(id, size), &mut b[..size])?;
        Ok(Self::item_of(kind, id, &b[..size]))
    }

    /// The text of a string (`""` for an unknown artist/album/genre).
    pub fn text<'b, S: Storage>(
        &self,
        store: &mut S,
        r: StrRef,
        buf: &'b mut [u8; 128],
    ) -> Result<&'b str, Error<S::Error>> {
        if r.is_none() || r.0 >= self.str_end {
            return Ok("");
        }
        let (o, l) = self.secs[Sec::Str as usize];
        let n = (l.checked_sub(r.0).ok_or(Error::Corrupt)? as usize).min(128);
        if store.read_at(self.file, o + r.0, &mut buf[..n])? != n || n == 0 {
            return Err(Error::Corrupt);
        }
        let len = usize::from(buf[0]);
        if 1 + len > n {
            return Err(Error::Corrupt);
        }
        buf.copy_within(1..1 + len, 0);
        core::str::from_utf8(&buf[..len]).map_err(|_| Error::Corrupt)
    }

    /// Everything about track row `id`.
    pub fn track<S: Storage>(&self, store: &mut S, id: u32) -> Result<Track, Error<S::Error>> {
        let mut b = [0u8; TRK_ROW];
        self.read(store, Sec::Trk, at(id, TRK_ROW), &mut b)?;
        Ok(self.track_of(id, &b))
    }

    fn track_ref(&self, b: &[u8]) -> TrackRef {
        TrackRef { serial: self.serial, dir: rd32(b, 0), index: rd32(b, 4), name_hash: rd32(b, 8) }
    }

    fn track_of(&self, id: u32, b: &[u8]) -> Track {
        Track {
            id,
            r: self.track_ref(b),
            title: StrRef(rd32(b, trk::TITLE)),
            artist: StrRef(rd32(b, trk::ARTIST)),
            album: StrRef(rd32(b, trk::ALBUM)),
            album_artist: StrRef(rd32(b, trk::AARTIST)),
            genre: StrRef(rd32(b, trk::GENRE)),
            album_id: rd32(b, trk::ALBUM_ROW),
            artist_id: rd32(b, trk::ARTIST_ROW),
            duration_ms: rd32(b, trk::DURATION),
            added: rd32(b, trk::ADDED),
            year: rd16(b, trk::YEAR),
            track: rd16(b, trk::TRACK),
            disc: rd16(b, trk::DISC),
            format: crate::FileFormat::from_code(b[trk::FORMAT]),
            flags: b[trk::FLAGS],
        }
    }

    /// The track row of the file at `loc` (binary search of the locator
    /// order: ~2 × log₂(tracks) small reads).
    pub fn find<S: Storage>(&self, store: &mut S, loc: Locator) -> Result<Option<u32>, Error<S::Error>> {
        let (mut lo, mut hi) = (0u32, self.tracks);
        let mut b = [0u8; 8];
        let key = (loc.dir, loc.index);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let id = self.u32_at(store, Sec::ByLoc, mid)?;
            self.read(store, Sec::Trk, at(id, TRK_ROW), &mut b)?;
            match (rd32(&b, 0), rd32(&b, 4)).cmp(&key) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => return Ok(Some(id)),
            }
        }
        Ok(None)
    }

    /// [`Library::find`], trying `hint + 1` and `hint` first: tracks are in
    /// album order, so the next track of an album is usually the row after
    /// the current one (one read instead of a binary search).
    pub fn find_near<S: Storage>(
        &self,
        store: &mut S,
        loc: Locator,
        hint: Option<u32>,
    ) -> Result<Option<u32>, Error<S::Error>> {
        let mut st = self.find_begin(loc, hint);
        loop {
            if let Some(r) = self.find_step(store, &mut st)? {
                return Ok(r);
            }
        }
    }

    /// A [`Library::find_near`] done one probe at a time
    /// ([`Library::find_step`]): the firmware runs it between decoded
    /// blocks while the DAC plays, so no single step holds the card long.
    #[must_use]
    pub fn find_begin(&self, loc: Locator, hint: Option<u32>) -> FindStep {
        FindStep {
            loc,
            hint: hint.unwrap_or(u32::MAX),
            phase: if hint.is_some() { 0 } else { 2 },
            lo: 0,
            hi: self.tracks,
        }
    }

    /// One probe of the search (at most two small reads): `Some(result)`
    /// when it is over.
    pub fn find_step<S: Storage>(
        &self,
        store: &mut S,
        st: &mut FindStep,
    ) -> Result<Option<Option<u32>>, Error<S::Error>> {
        let mut b = [0u8; 8];
        let key = (st.loc.dir, st.loc.index);
        if st.phase < 2 {
            let id = if st.phase == 0 { st.hint.wrapping_add(1) } else { st.hint };
            st.phase += 1;
            if id < self.tracks {
                self.read(store, Sec::Trk, u64::from(id) * TRK_ROW as u64, &mut b)?;
                if (rd32(&b, 0), rd32(&b, 4)) == key {
                    return Ok(Some(Some(id)));
                }
            }
            return Ok(None);
        }
        if st.lo >= st.hi {
            return Ok(Some(None));
        }
        let mid = st.lo + (st.hi - st.lo) / 2;
        let id = self.u32_at(store, Sec::ByLoc, mid)?;
        self.read(store, Sec::Trk, u64::from(id) * TRK_ROW as u64, &mut b)?;
        match (rd32(&b, 0), rd32(&b, 4)).cmp(&key) {
            core::cmp::Ordering::Less => st.lo = mid + 1,
            core::cmp::Ordering::Greater => st.hi = mid,
            core::cmp::Ordering::Equal => return Ok(Some(Some(id))),
        }
        Ok(None)
    }

    /// Where table row `id` is in `list` (a track row for track lists, a
    /// group row otherwise): arithmetic for direct ranges, a sequential
    /// scan of the order otherwise.
    pub fn position_of<S: Storage>(
        &self,
        store: &mut S,
        list: List,
        id: u32,
    ) -> Result<Option<u32>, Error<S::Error>> {
        let l = self.layout(store, list)?;
        let Some(p) = l.perm else {
            return Ok((id >= l.first && id - l.first < l.count).then(|| id - l.first));
        };
        let mut b = [0u8; 4 * CHUNK];
        let mut i = 0u32;
        while i < l.count {
            let m = ((l.count - i) as usize).min(CHUNK);
            self.read(store, p, at(l.first, 4) + at(i, 4), &mut b[..4 * m])?;
            if let Some(k) = (0..m).position(|k| rd32(&b, 4 * k) == id) {
                return Ok(Some(i + k as u32));
            }
            i += m as u32;
        }
        Ok(None)
    }

    /// Hand the tracks of rows `[first, first + max)` of track list `list`
    /// to `sink` in list order (stops early when `sink` answers `false`);
    /// returns how many were handed out. "Play this album/artist/genre",
    /// and OK on a track of the list (the list becomes the queue).
    pub fn queue<S: Storage>(
        &self,
        store: &mut S,
        list: List,
        first: u32,
        max: u32,
        mut sink: impl FnMut(TrackRef) -> bool,
    ) -> Result<u32, Error<S::Error>> {
        let l = self.layout(store, list)?;
        if l.kind != ItemKind::Track {
            return Err(Error::Corrupt);
        }
        let end = first.saturating_add(max).min(l.count);
        let mut i = first;
        let mut ids = [0u32; 8];
        let mut b = [0u8; 8 * TRK_ROW];
        while i < end {
            let n = self.ids(store, &l, i, &mut ids[..((end - i) as usize).min(8)])?;
            let contiguous = ids[..n].windows(2).all(|w| w[0].checked_add(1) == Some(w[1]));
            if contiguous {
                self.read(store, Sec::Trk, at(ids[0], TRK_ROW), &mut b[..n * TRK_ROW])?;
            } else {
                for (k, &id) in ids[..n].iter().enumerate() {
                    self.read(store, Sec::Trk, at(id, TRK_ROW), &mut b[k * TRK_ROW..k * TRK_ROW + 12])?;
                }
            }
            for k in 0..n {
                i += 1;
                if !sink(self.track_ref(&b[k * TRK_ROW..])) {
                    return Ok(i - first);
                }
            }
        }
        Ok(i - first)
    }

    /// A uniform random sample of `n` distinct tracks (all if there are
    /// fewer) for Shuffle all, handed to `sink` in library order — play
    /// it shuffled. One sequential pass over the track table (selection
    /// sampling); returns how many. In one go: the firmware uses
    /// [`Self::sample_begin`] and [`Self::sample_step`] instead, so a large
    /// library cannot hold the CPU long enough for the watchdog.
    pub fn sample<S: Storage>(
        &self,
        store: &mut S,
        seed: u64,
        n: u32,
        mut sink: impl FnMut(TrackRef) -> bool,
    ) -> Result<u32, Error<S::Error>> {
        let mut st = self.sample_begin(seed, n);
        while !self.sample_step(store, &mut st, u32::MAX, &mut sink)? {}
        Ok(st.picked)
    }

    /// Start a [`Self::sample`] to be done in steps.
    #[must_use]
    pub fn sample_begin(&self, seed: u64, n: u32) -> SampleStep {
        SampleStep {
            s: seed ^ 0x9E37_79B9_7F4A_7C15,
            i: 0,
            picked: 0,
            want: n.min(self.tracks),
            stopped: false,
        }
    }

    /// Look at up to `max_rows` more rows of the track table (rounded up
    /// to whole reads of 8); true when the sample is complete (or `sink`
    /// asked to stop). The same picks as [`Self::sample`] in one go,
    /// however the rows are split into steps.
    pub fn sample_step<S: Storage>(
        &self,
        store: &mut S,
        st: &mut SampleStep,
        max_rows: u32,
        mut sink: impl FnMut(TrackRef) -> bool,
    ) -> Result<bool, Error<S::Error>> {
        let total = self.tracks;
        let mut b = [0u8; 8 * TRK_ROW];
        let mut looked = 0u32;
        while !st.stopped && st.i < total && st.picked < st.want && looked < max_rows {
            let m = ((total - st.i) as usize).min(8);
            self.read(store, Sec::Trk, at(st.i, TRK_ROW), &mut b[..m * TRK_ROW])?;
            for k in 0..m {
                let left = u64::from(total - st.i);
                // Pick with probability (want - picked) / left.
                if ((u128::from(st.next()) * u128::from(left)) >> 64) < u128::from(st.want - st.picked) {
                    st.picked += 1;
                    if !sink(self.track_ref(&b[k * TRK_ROW..])) {
                        st.stopped = true;
                        return Ok(true);
                    }
                }
                st.i += 1;
                if st.picked == st.want {
                    break;
                }
            }
            looked = looked.saturating_add(m as u32);
        }
        Ok(st.stopped || st.i >= total || st.picked >= st.want)
    }

    fn letter_run<S: Storage>(&self, store: &mut S, s: Sec, j: u32) -> Result<(u32, u32), Error<S::Error>> {
        let mut b = [0u8; LETTER_RUN];
        self.read(store, s, at(j, LETTER_RUN), &mut b)?;
        Ok((rd32(&b, 0), rd32(&b, 4)))
    }

    fn run_of<S: Storage>(&self, store: &mut S, s: Sec, i: u32) -> Result<Option<u32>, Error<S::Error>> {
        let runs = self.rows_in(s, LETTER_RUN);
        if runs == 0 {
            return Ok(None);
        }
        let (mut lo, mut hi) = (0u32, runs);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.letter_run(store, s, mid)?.0 <= i {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Ok(Some(lo))
    }

    /// Letter-jump letter of row `i` (Artists, Albums, Genres, Songs).
    pub fn letter<S: Storage>(
        &self,
        store: &mut S,
        list: List,
        i: u32,
    ) -> Result<Option<char>, Error<S::Error>> {
        let Some(s) = list.letters() else { return Ok(None) };
        if i >= self.len(store, list)? {
            return Ok(None);
        }
        let Some(j) = self.run_of(store, s, i)? else { return Ok(None) };
        Ok(char::from_u32(self.letter_run(store, s, j)?.1).filter(|&c| c != '\0'))
    }

    /// Letter jump with the semantics of `ListSource::seek_letter` (and
    /// `orpheus_index::Index::seek_letter`): the first row after `from`
    /// (`forward`), or the first row of the group before it, whose letter
    /// differs from `from`'s; rows without a letter are passed over.
    pub fn seek_letter<S: Storage>(
        &self,
        store: &mut S,
        list: List,
        from: u32,
        forward: bool,
    ) -> Result<Option<(u32, char)>, Error<S::Error>> {
        let Some(s) = list.letters() else { return Ok(None) };
        let len = self.len(store, list)?;
        if from >= len {
            return Ok(None);
        }
        let Some(r) = self.run_of(store, s, from)? else { return Ok(None) };
        let cur = self.letter_run(store, s, r)?.1;
        if cur == 0 {
            return Ok(None);
        }
        let total = self.rows_in(s, LETTER_RUN);
        // A run that starts outside the list (a damaged file) is an error,
        // never a row the UI would move to (VER-4 finding F-IDX-1).
        let hit = |(row, l): (u32, u32)| -> Result<Option<(u32, char)>, Error<S::Error>> {
            match char::from_u32(l) {
                Some(c) if l != 0 && l != cur => {
                    if row >= len {
                        return Err(Error::Corrupt);
                    }
                    Ok(Some((row, c)))
                }
                _ => Ok(None),
            }
        };
        if forward {
            for j in r + 1..total {
                if let Some(x) = hit(self.letter_run(store, s, j)?)? {
                    return Ok(Some(x));
                }
            }
        } else {
            for j in (0..r).rev() {
                if let Some(x) = hit(self.letter_run(store, s, j)?)? {
                    return Ok(Some(x));
                }
            }
        }
        Ok(None)
    }

    /// Letter runs `[first, first + out.len())` of a lettered list as
    /// `(first row, letter)`; returns how many (to keep a small table in
    /// RAM).
    pub fn letter_runs<S: Storage>(
        &self,
        store: &mut S,
        list: List,
        first: u32,
        out: &mut [(u32, Option<char>)],
    ) -> Result<usize, Error<S::Error>> {
        let Some(s) = list.letters() else { return Ok(0) };
        let n = (self.rows_in(s, LETTER_RUN).saturating_sub(first) as usize).min(out.len());
        for (k, o) in out[..n].iter_mut().enumerate() {
            let (row, l) = self.letter_run(store, s, first + k as u32)?;
            *o = (row, char::from_u32(l).filter(|&c| c != '\0'));
        }
        Ok(n)
    }

    /// Check every section's CRC against the header (reads the whole file
    /// through `buf`).
    pub fn verify<S: Storage>(&self, store: &mut S, buf: &mut [u8]) -> Result<bool, Error<S::Error>> {
        let mut hb = [0u8; HEADER];
        if buf.is_empty() || store.read_at(self.file, 0, &mut hb)? != HEADER {
            return Ok(false);
        }
        let Some(h) = Header::decode(&hb) else { return Ok(false) };
        for s in h.secs {
            let mut crc = 0;
            let mut off = 0;
            while off < s.len {
                let n = ((s.len - off) as usize).min(buf.len());
                if store.read_at(self.file, s.off + off, &mut buf[..n])? != n {
                    return Ok(false);
                }
                crc = orpheus_index::crc32_update(crc, &buf[..n]);
                off += n as u32;
            }
            if crc != s.crc {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
