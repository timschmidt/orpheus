//! Host doubles: in-memory storage with I/O accounting and power cuts, a
//! mock card (folder tree with virtual audio files carrying ID3v2.4,
//! ID3v1 or FLAC tags), a random library generator and the reference
//! model of every list.

#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::HashMap;

use orpheus_fs::Locator;
use orpheus_index::{DirSource, FileName, SrcEntry, Storage};
use orpheus_library::{
    Builder, Config, Error, Item, Library, List, Progress, Suspended, TagSource, TrackRef, sort_name,
};

// ------------------------------------------------------------- storage

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IoErr {
    Cut,
}

#[derive(Clone, Default)]
pub struct MemStore {
    pub files: HashMap<String, Vec<u8>>,
    pub reads: u64,
    pub read_bytes: u64,
    pub writes: u64,
    pub write_bytes: u64,
    pub cut_after: Option<u64>,
    pub dead: bool,
    pub peak_total: usize,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }
    pub fn revive(&mut self) {
        self.dead = false;
        self.cut_after = None;
    }
    pub fn reset_counters(&mut self) {
        self.reads = 0;
        self.read_bytes = 0;
        self.writes = 0;
        self.write_bytes = 0;
    }
    pub fn total(&self) -> usize {
        self.files.values().map(Vec::len).sum()
    }
}

impl Storage for MemStore {
    type Error = IoErr;
    fn len(&mut self, file: FileName) -> Result<Option<u32>, IoErr> {
        if self.dead {
            return Err(IoErr::Cut);
        }
        Ok(self.files.get(file.as_str()).map(|v| v.len() as u32))
    }
    fn read_at(&mut self, file: FileName, offset: u32, buf: &mut [u8]) -> Result<usize, IoErr> {
        if self.dead {
            return Err(IoErr::Cut);
        }
        self.reads += 1;
        let Some(v) = self.files.get(file.as_str()) else { return Ok(0) };
        let o = (offset as usize).min(v.len());
        let n = (v.len() - o).min(buf.len());
        buf[..n].copy_from_slice(&v[o..o + n]);
        self.read_bytes += n as u64;
        Ok(n)
    }
    fn write_at(&mut self, file: FileName, offset: u32, data: &[u8]) -> Result<(), IoErr> {
        if self.dead {
            return Err(IoErr::Cut);
        }
        let mut data = data;
        let mut torn = false;
        if let Some(left) = self.cut_after.as_mut() {
            if data.len() as u64 > *left {
                data = &data[..*left as usize];
                torn = true;
            }
            *left -= data.len() as u64;
        }
        self.writes += 1;
        self.write_bytes += data.len() as u64;
        let v = self.files.entry(file.as_str().to_string()).or_default();
        let o = offset as usize;
        assert!(o <= v.len(), "write_at past the end of {}: {o} > {}", file.as_str(), v.len());
        if v.len() < o + data.len() {
            v.resize(o + data.len(), 0);
        }
        v[o..o + data.len()].copy_from_slice(data);
        let t = self.total();
        self.peak_total = self.peak_total.max(t);
        if torn {
            self.dead = true;
            return Err(IoErr::Cut);
        }
        Ok(())
    }
    fn truncate(&mut self, file: FileName, len: u32) -> Result<(), IoErr> {
        if self.dead {
            return Err(IoErr::Cut);
        }
        let v = self.files.entry(file.as_str().to_string()).or_default();
        assert!(len as usize <= v.len(), "truncate grows");
        v.truncate(len as usize);
        Ok(())
    }
    fn sync(&mut self, _file: FileName) -> Result<(), IoErr> {
        if self.dead {
            return Err(IoErr::Cut);
        }
        Ok(())
    }
    fn remove(&mut self, file: FileName) -> Result<(), IoErr> {
        if self.dead {
            return Err(IoErr::Cut);
        }
        self.files.remove(file.as_str());
        Ok(())
    }
}

// ---------------------------------------------------------------- rng

#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
}

// --------------------------------------------------------- tagged files

/// How a file carries its tags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Carrier {
    /// ID3v2.4 (UTF-8) at the front; `art_first`: a big APIC before the
    /// text frames.
    Id3v2 { art: u32, art_first: bool },
    /// Only ID3v1.1 at the end (ASCII, ≤ 30 chars).
    Id3v1,
    /// FLAC: STREAMINFO, optionally a big PICTURE block, VORBIS_COMMENT.
    Flac { art: u32 },
    /// No tags at all (MP3 frames only).
    None,
    /// An Ogg file: not read.
    Ogg,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub year: u16,
    pub track: u16,
    pub disc: u16,
}

/// A virtual file: byte segments at offsets, zeros elsewhere.
#[derive(Clone, Debug)]
pub struct VFile {
    pub size: u64,
    pub segs: Vec<(u64, Vec<u8>)>,
}

impl VFile {
    fn read(&self, off: u64, buf: &mut [u8]) -> usize {
        if off >= self.size {
            return 0;
        }
        let n = ((self.size - off) as usize).min(buf.len());
        buf[..n].fill(0);
        for (at, d) in &self.segs {
            let (s, e) = (*at, *at + d.len() as u64);
            let (rs, re) = (off.max(s), (off + n as u64).min(e));
            if rs < re {
                buf[(rs - off) as usize..(re - off) as usize]
                    .copy_from_slice(&d[(rs - s) as usize..(re - s) as usize]);
            }
        }
        n
    }
}

fn synchsafe(n: u32) -> [u8; 4] {
    [(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]
}

fn mpeg_frames(n: usize) -> Vec<u8> {
    let mut f = Vec::new();
    for _ in 0..n {
        f.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        f.resize(f.len() + 413, 0);
    }
    f
}

pub const ID3V1_GENRES: &[(&str, u8)] = &[("Blues", 0), ("Rock", 17), ("Jazz", 8), ("Pop", 13)];

pub fn make_file(c: Carrier, t: &Tags, size: u64) -> VFile {
    let mut segs = Vec::new();
    match c {
        Carrier::Id3v2 { art, art_first } => {
            let mut frames = Vec::new();
            let text = |frames: &mut Vec<u8>, id: &[u8; 4], s: &str| {
                if s.is_empty() {
                    return;
                }
                frames.extend_from_slice(id);
                frames.extend_from_slice(&synchsafe(s.len() as u32 + 1));
                frames.extend_from_slice(&[0, 0, 3]);
                frames.extend_from_slice(s.as_bytes());
            };
            let apic = |frames: &mut Vec<u8>| {
                if art == 0 {
                    return;
                }
                frames.extend_from_slice(b"APIC");
                frames.extend_from_slice(&synchsafe(art + 14));
                frames.extend_from_slice(&[0, 0, 0]);
                frames.extend_from_slice(b"image/jpeg\0\x03\0");
                frames.resize(frames.len() + art as usize, 0x55);
            };
            if art_first {
                apic(&mut frames);
            }
            text(&mut frames, b"TIT2", &t.title);
            text(&mut frames, b"TPE1", &t.artist);
            text(&mut frames, b"TALB", &t.album);
            text(&mut frames, b"TPE2", &t.album_artist);
            text(&mut frames, b"TCON", &t.genre);
            if t.year != 0 {
                text(&mut frames, b"TDRC", &t.year.to_string());
            }
            if t.track != 0 {
                text(&mut frames, b"TRCK", &format!("{}/20", t.track));
            }
            if t.disc != 0 {
                text(&mut frames, b"TPOS", &format!("{}/2", t.disc));
            }
            if !art_first {
                apic(&mut frames);
            }
            frames.resize(frames.len() + 256, 0);
            let mut h = Vec::from(*b"ID3\x04\0\0");
            h.extend_from_slice(&synchsafe(frames.len() as u32));
            h.extend_from_slice(&frames);
            h.extend(mpeg_frames(4));
            segs.push((0, h));
        }
        Carrier::Id3v1 | Carrier::None => {
            segs.push((0, mpeg_frames(4)));
            if c == Carrier::Id3v1 {
                let mut v = vec![0u8; 128];
                v[..3].copy_from_slice(b"TAG");
                let put = |v: &mut [u8], s: &str| v[..s.len()].copy_from_slice(s.as_bytes());
                put(&mut v[3..33], &t.title);
                put(&mut v[33..63], &t.artist);
                put(&mut v[63..93], &t.album);
                if t.year != 0 {
                    put(&mut v[93..97], &t.year.to_string());
                }
                v[126] = t.track as u8;
                v[127] = ID3V1_GENRES.iter().find(|g| g.0 == t.genre).map_or(255, |g| g.1);
                segs.push((size - 128, v));
            }
        }
        Carrier::Flac { art } => {
            let mut h = Vec::from(*b"fLaC");
            h.extend_from_slice(&[0, 0, 0, 34]);
            let mut si = [0u8; 34];
            let rate = 44100u32;
            si[10] = (rate >> 12) as u8;
            si[11] = (rate >> 4) as u8;
            si[12] = ((rate & 0xF) << 4) as u8 | 2;
            si[13] = 0xF0;
            si[14..18].copy_from_slice(&((size / 4) as u32).to_be_bytes());
            h.extend_from_slice(&si);
            if art > 0 {
                let mut p = Vec::new();
                p.extend_from_slice(&3u32.to_be_bytes());
                p.extend_from_slice(&10u32.to_be_bytes());
                p.extend_from_slice(b"image/jpeg");
                p.extend_from_slice(&0u32.to_be_bytes());
                p.extend_from_slice(&[0; 16]);
                p.extend_from_slice(&art.to_be_bytes());
                let blen = p.len() as u32 + art;
                h.push(6);
                h.extend_from_slice(&blen.to_be_bytes()[1..]);
                h.extend_from_slice(&p);
                segs.push((0, h));
                h = Vec::new();
            }
            let mut c = Vec::new();
            c.extend_from_slice(&4u32.to_le_bytes());
            c.extend_from_slice(b"test");
            let items: Vec<String> = [
                ("TITLE", t.title.clone()),
                ("ARTIST", t.artist.clone()),
                ("ALBUM", t.album.clone()),
                ("ALBUMARTIST", t.album_artist.clone()),
                ("GENRE", t.genre.clone()),
                ("DATE", if t.year != 0 { t.year.to_string() } else { String::new() }),
                ("TRACKNUMBER", if t.track != 0 { t.track.to_string() } else { String::new() }),
                ("DISCNUMBER", if t.disc != 0 { t.disc.to_string() } else { String::new() }),
            ]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
            c.extend_from_slice(&(items.len() as u32).to_le_bytes());
            for i in &items {
                c.extend_from_slice(&(i.len() as u32).to_le_bytes());
                c.extend_from_slice(i.as_bytes());
            }
            h.push(0x84);
            h.extend_from_slice(&(c.len() as u32).to_be_bytes()[1..]);
            h.extend_from_slice(&c);
            let at = if art > 0 { segs[0].1.len() as u64 + u64::from(art) } else { 0 };
            if art > 0 {
                segs.push((at, h));
            } else {
                segs.push((0, h));
            }
        }
        Carrier::Ogg => segs.push((0, b"OggS".to_vec())),
    }
    VFile { size, segs }
}

// ----------------------------------------------------------- mock card

#[derive(Clone, Debug)]
pub struct MEntry {
    pub name: String,
    pub index: u32,
    pub hidden: bool,
    pub mtime: u32,
    /// Folder: its index in `Card::dirs`.
    pub child: Option<usize>,
    pub file: Option<(Carrier, Tags, u64)>,
}

#[derive(Clone, Debug, Default)]
pub struct MDir {
    pub id: u32,
    pub entries: Vec<MEntry>,
    pub next_index: u32,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub dirs: Vec<MDir>,
    pub next_cluster: u32,
    // Session state.
    pub cur_dir: Option<usize>,
    pub pos: usize,
    pub open: Option<VFile>,
    // Accounting.
    pub file_opens: u64,
    pub file_reads: u64,
    pub file_bytes: u64,
    pub dir_entries: u64,
}

impl Card {
    pub fn new() -> Card {
        let mut c = Card {
            dirs: Vec::new(),
            next_cluster: 2,
            cur_dir: None,
            pos: 0,
            open: None,
            file_opens: 0,
            file_reads: 0,
            file_bytes: 0,
            dir_entries: 0,
        };
        c.new_dir();
        c
    }

    fn new_dir(&mut self) -> usize {
        let id = self.next_cluster;
        self.next_cluster += 1 + (id % 7);
        self.dirs.push(MDir { id, entries: Vec::new(), next_index: 0 });
        self.dirs.len() - 1
    }

    /// Folder `name` in folder `parent` (created if missing).
    pub fn dir(&mut self, parent: usize, name: &str, hidden: bool) -> usize {
        if let Some(e) = self.dirs[parent].entries.iter().find(|e| e.name == name && e.child.is_some()) {
            return e.child.unwrap();
        }
        let d = self.new_dir();
        let p = &mut self.dirs[parent];
        let index = p.next_index;
        // Long names take several slots.
        p.next_index += 1 + (name.len() as u32).div_ceil(13);
        p.entries.push(MEntry { name: name.into(), index, hidden, mtime: 1000, child: Some(d), file: None });
        d
    }

    pub fn add_file(&mut self, dir: usize, name: &str, c: Carrier, t: Tags, size: u64, mtime: u32) {
        let p = &mut self.dirs[dir];
        let index = p.next_index;
        p.next_index += 1 + (name.len() as u32).div_ceil(13);
        p.entries.push(MEntry {
            name: name.into(),
            index,
            hidden: false,
            mtime,
            child: None,
            file: Some((c, t, size)),
        });
    }

    pub fn path_dir(&mut self, path: &[&str]) -> usize {
        let mut d = 0;
        for p in path {
            d = self.dir(d, p, p.starts_with('.'));
        }
        d
    }

    fn find(&self, loc: Locator) -> Option<&MEntry> {
        let d = self.dirs.iter().find(|d| d.id == loc.dir)?;
        d.entries.iter().find(|e| e.index == loc.index)
    }
}

impl DirSource for Card {
    type Error = IoErr;
    fn rewind(&mut self) -> Result<(), IoErr> {
        self.pos = 0;
        Ok(())
    }
    fn next_entry(&mut self, name: &mut [u8]) -> Result<Option<SrcEntry>, IoErr> {
        let Some(d) = self.cur_dir else { return Ok(None) };
        let dir = &self.dirs[d];
        let Some(e) = dir.entries.get(self.pos) else { return Ok(None) };
        self.pos += 1;
        self.dir_entries += 1;
        name[..e.name.len()].copy_from_slice(e.name.as_bytes());
        Ok(Some(SrcEntry {
            loc: Locator { dir: dir.id, index: e.index },
            is_dir: e.child.is_some(),
            hidden: e.hidden,
            size: e.file.as_ref().map_or(0, |f| f.2),
            mtime: e.mtime,
            name_len: e.name.len(),
        }))
    }
}

impl TagSource for Card {
    fn open_dir(&mut self, dir: Option<Locator>, after: Option<Locator>) -> Result<Option<u32>, IoErr> {
        let d = match dir {
            None => Some(0),
            Some(l) => self.find(l).and_then(|e| e.child),
        };
        self.cur_dir = d;
        self.pos = 0;
        let Some(d) = d else { return Ok(None) };
        if let Some(a) = after {
            self.pos = self.dirs[d]
                .entries
                .iter()
                .position(|e| e.index > a.index)
                .unwrap_or(self.dirs[d].entries.len());
        }
        Ok(Some(self.dirs[d].id))
    }
    fn open_file(&mut self, loc: Locator) -> Result<Option<u64>, IoErr> {
        self.file_opens += 1;
        let Some((c, t, size)) = self.find(loc).and_then(|e| e.file.clone()) else {
            self.open = None;
            return Ok(None);
        };
        let f = make_file(c, &t, size);
        self.open = Some(f);
        Ok(Some(size))
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, IoErr> {
        let f = self.open.as_ref().expect("no file open");
        let n = f.read(offset, buf);
        self.file_reads += 1;
        self.file_bytes += n as u64;
        Ok(n)
    }
}

// ------------------------------------------------------- random library

pub const ARTISTS: &[&str] = &[
    "Abba",
    "ABBA",
    "The Beatles",
    "Beatles",
    "Боб Дилан",
    "Кино",
    "кино",
    "Ария",
    "椎名林檎",
    "宇多田ヒカル",
    "Björk",
    "Bjork",
    "Sigur Rós",
    "AC/DC",
    "Ac/Dc",
    "2Pac",
    "10cc",
    "U2",
    "a-ha",
    "Æther",
    "Ζορμπάς",
    "Μίκης Θεοδωράκης",
    "The The",
    "Air",
    "Daft Punk",
    "Ёлка",
    "Елка",
    "Radiohead",
    "Portishead",
    "Massive Attack",
    "東京事変",
    "Nirvana",
    "Muse",
    "Queen",
    "Pink Floyd",
    "Led Zeppelin",
    "Metallica",
    "Ludwig van Beethoven",
    "J. S. Bach",
    "Сплин",
];
pub const ALBUMS: &[&str] = &[
    "Greatest Hits",
    "Live",
    "Gold",
    "Arrival",
    "Abbey Road",
    "Группа крови",
    "加爾基 精液 栗ノ花",
    "Debut",
    "OK Computer",
    "Dummy",
    "Mezzanine",
    "Discovery",
    "Nevermind",
    "Absolution",
    "Symphony No. 9",
    "Symphony No. 10",
    "Volume 2",
    "Volume 10",
    "The Wall",
    "IV",
    "Ride the Lightning",
    "Kid A",
    "Ágætis byrjun",
    "Hunting High and Low",
    "Moon Safari",
    "Unplugged",
    "Best Of",
    "Anthology",
    "Night",
    "Ночь",
];
pub const GENRES: &[&str] = &[
    "Rock",
    "Pop",
    "Jazz",
    "Classical",
    "Electronic",
    "Hip-Hop",
    "Рок",
    "ロック",
    "Trip Hop",
    "Blues",
    "rock",
    "",
];
pub const WORDS: &[&str] = &[
    "Love",
    "Night",
    "Song",
    "Dance",
    "Moon",
    "Sun",
    "Rain",
    "Ночь",
    "Звезда",
    "愛",
    "夜",
    "Ψυχή",
    "Track",
    "Intro",
    "Outro",
    "Interlude",
    "Blue",
    "Red",
    "Hey",
    "One",
    "Two",
    "10",
    "2",
    "Ölüm",
    "Été",
];

pub fn word_title(r: &mut Rng) -> String {
    let n = 1 + r.below(4) as usize;
    (0..n).map(|_| *r.pick(WORDS)).collect::<Vec<_>>().join(" ")
}

/// A card with about `tracks` tracks: `/Music/Artist/Album/NN Title.ext`,
/// compilations, multi-disc albums, loose files, untagged files, ID3v1-only
/// files, FLAC with art before the comments, hidden folders.
pub fn random_card(r: &mut Rng, tracks: usize) -> Card {
    let mut c = Card::new();
    let mut n = 0;
    let hidden = c.path_dir(&[".hidden"]);
    c.add_file(hidden, "secret.mp3", Carrier::None, empty_tags(), 1 << 20, 5);
    let y = c.path_dir(&[".orpheus", "library"]);
    c.add_file(y, "fake.mp3", Carrier::None, empty_tags(), 1 << 20, 5);
    c.add_file(0, "readme.txt", Carrier::None, empty_tags(), 100, 5);
    while n < tracks {
        let kind = r.below(100);
        let artist = r.pick(ARTISTS).to_string();
        let album = r.pick(ALBUMS).to_string();
        let genre = r.pick(GENRES).to_string();
        let year = if r.chance(85) { 1960 + r.below(60) as u16 } else { 0 };
        let count = 1 + r.below(14) as usize;
        let compilation = kind < 10;
        let discs = if kind > 92 { 2 } else { 1 };
        let folder_artist = if compilation { "Various Artists".to_string() } else { artist.clone() };
        let d = if kind > 97 {
            0
        } else {
            let a = c.path_dir(&["Music", &folder_artist.replace('/', "_")]);
            c.dir(a, &format!("{album} ({})", r.below(3)), false)
        };
        let untagged = r.chance(8);
        for i in 0..count {
            let disc = if discs > 1 { 1 + (i % 2) as u16 } else { 0 };
            let track = 1 + i as u16;
            let tartist = if compilation { r.pick(ARTISTS).to_string() } else { artist.clone() };
            let title = word_title(r);
            let carrier = if untagged {
                if r.chance(50) { Carrier::None } else { Carrier::Ogg }
            } else {
                match r.below(100) {
                    0..=59 => Carrier::Id3v2 {
                        art: if r.chance(20) { 30_000 + r.below(300_000) as u32 } else { 0 },
                        art_first: r.chance(50),
                    },
                    60..=69 => Carrier::Id3v1,
                    _ => {
                        Carrier::Flac { art: if r.chance(30) { 50_000 + r.below(200_000) as u32 } else { 0 } }
                    }
                }
            };
            let tags = if carrier == Carrier::Id3v1 {
                let ascii = |s: &str, n: usize| -> String {
                    s.chars().filter(char::is_ascii).take(n).collect::<String>().trim().to_string()
                };
                Tags {
                    title: ascii(&title, 30),
                    artist: ascii(&tartist, 30),
                    album: ascii(&album, 30),
                    album_artist: String::new(),
                    genre: ID3V1_GENRES[r.below(4) as usize].0.to_string(),
                    year,
                    track,
                    disc: 0,
                }
            } else {
                Tags {
                    title: title.clone(),
                    artist: tartist,
                    album: album.clone(),
                    album_artist: if compilation {
                        "Various Artists".into()
                    } else if r.chance(30) {
                        artist.clone()
                    } else {
                        String::new()
                    },
                    genre: genre.clone(),
                    year,
                    track,
                    disc,
                }
            };
            let ext = match carrier {
                Carrier::Flac { .. } => "flac",
                Carrier::Ogg => "ogg",
                _ => "mp3",
            };
            let fname = format!("{:02} {}.{ext}", track, title.replace('/', "_"));
            let size = 2_000_000 + r.below(8_000_000);
            c.add_file(d, &fname, carrier, tags, size, 10_000 + r.below(100_000) as u32);
            n += 1;
        }
        if r.chance(10) {
            c.add_file(d, "cover.jpg", Carrier::None, empty_tags(), 50_000, 7);
        }
    }
    c
}

pub fn empty_tags() -> Tags {
    Tags {
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        album_artist: String::new(),
        genre: String::new(),
        year: 0,
        track: 0,
        disc: 0,
    }
}

// --------------------------------------------------------------- model

/// What the library should hold for one track.
#[derive(Clone, Debug)]
pub struct MTrack {
    pub r: TrackRef,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub aa: String,
    pub genre: String,
    pub year: u16,
    pub track: u16,
    pub disc: u16,
}

fn is_audio(name: &str) -> bool {
    orpheus_apps::browse::kind_of(name, false) == orpheus_apps::browse::EntryKind::Audio
}

/// Every visible audio file with the tags the library must show.
pub fn model(c: &Card, serial: u32, depth_max: u8) -> Vec<MTrack> {
    let mut out = Vec::new();
    let mut q = vec![(0usize, String::new(), 0u8)];
    while let Some((d, folder, depth)) = q.pop() {
        let dir = &c.dirs[d];
        for e in &dir.entries {
            let hidden = e.hidden || e.name.starts_with('.');
            if hidden {
                continue;
            }
            if let Some(ch) = e.child {
                if depth < depth_max {
                    q.push((ch, e.name.clone(), depth + 1));
                }
                continue;
            }
            if !is_audio(&e.name) {
                continue;
            }
            let (carrier, t, _) = e.file.clone().unwrap();
            let stem = e.name.rsplit_once('.').unwrap().0.to_string();
            let lead: u16 = {
                let d: String = e.name.chars().take_while(char::is_ascii_digit).collect();
                d.parse().ok().filter(|n| (1..=999).contains(n) && d.len() <= 3).unwrap_or(0)
            };
            let t = if carrier == Carrier::Ogg || carrier == Carrier::None { empty_tags() } else { t };
            let r = TrackRef::new(serial, Locator { dir: dir.id, index: e.index }, &e.name);
            out.push(MTrack {
                r,
                title: if t.title.is_empty() { stem } else { t.title.clone() },
                artist: t.artist.clone(),
                album: if t.album.is_empty() { folder.clone() } else { t.album.clone() },
                aa: if t.album_artist.is_empty() { t.artist.clone() } else { t.album_artist.clone() },
                genre: t.genre.clone(),
                year: t.year,
                track: if t.track == 0 { lead } else { t.track },
                disc: t.disc,
            });
        }
    }
    out
}

/// The collation key of a library string: unknown last, then the sort
/// key; strings with equal keys are one entry.
pub fn ckey(s: &str, artist_or_album: bool, articles: bool) -> (bool, Vec<u8>) {
    let kind = if artist_or_album { orpheus_library::Kind::Artist } else { orpheus_library::Kind::Title };
    let n = sort_name(s, kind, articles);
    let mut k = vec![0u8; 1024];
    let l = orpheus_index::key::sort_key(n, &mut k);
    k.truncate(l);
    (s.is_empty(), k)
}

/// The spelling shown for a folded-equal group: the smallest raw string.
pub fn spellings(
    strings: impl Iterator<Item = (bool, String)>,
    articles: bool,
) -> HashMap<(bool, Vec<u8>), String> {
    let mut m: HashMap<(bool, Vec<u8>), String> = HashMap::new();
    for (aa, s) in strings {
        let k = ckey(&s, aa, articles);
        let e = m.entry(k).or_insert_with(|| s.clone());
        if s < *e {
            *e = s;
        }
    }
    m
}

pub fn cmp_track(a: &MTrack, b: &MTrack, articles: bool) -> Ordering {
    ckey(&a.aa, true, articles)
        .cmp(&ckey(&b.aa, true, articles))
        .then_with(|| ckey(&a.album, true, articles).cmp(&ckey(&b.album, true, articles)))
        .then(a.disc.cmp(&b.disc))
        .then(a.track.cmp(&b.track))
        .then_with(|| ckey(&a.title, false, articles).cmp(&ckey(&b.title, false, articles)))
        .then((a.r.dir, a.r.index).cmp(&(b.r.dir, b.r.index)))
}

// -------------------------------------------------------------- driving

#[derive(Clone, Copy, Debug, Default)]
pub struct RunStats {
    pub last: Option<Progress>,
    pub steps: u32,
    pub suspends: u32,
}

/// Build to completion; suspend (scratch scribbled over) with probability
/// `suspend_pct` per step.
pub fn build_with(
    store: &mut MemStore,
    card: &mut Card,
    cfg: Config,
    scratch_len: usize,
    budget: u32,
    suspend_pct: u64,
    r: &mut Rng,
) -> Result<(Library, RunStats, orpheus_library::ScanStats), Error<IoErr>> {
    let mut scratch = vec![0u8; scratch_len];
    let mut stats = RunStats::default();
    let mut susp: Option<Suspended> = None;
    loop {
        let mut b = match susp.take() {
            None => Builder::new(&mut scratch, cfg).expect("scratch"),
            Some(s) => {
                scratch.fill(0xA5);
                s.resume(&mut scratch).expect("resume")
            }
        };
        loop {
            stats.steps += 1;
            assert!(stats.steps < 50_000_000, "no progress");
            let p = match b.step(store, card, budget) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("build failed after {:?}: {e:?}", stats.last);
                    return Err(e);
                }
            };
            stats.last = Some(p);
            if let Progress::Done(l) = p {
                let sc = b.stats();
                return Ok((l, stats, sc));
            }
            if suspend_pct > 0 && r.chance(suspend_pct) {
                stats.suspends += 1;
                susp = Some(b.suspend(store)?);
                // The card task may have used the folder cursor meanwhile.
                card.cur_dir = None;
                break;
            }
        }
    }
}

pub fn build(store: &mut MemStore, card: &mut Card, cfg: Config) -> Library {
    build_with(store, card, cfg, 12 * 1024, 64, 0, &mut Rng(1)).unwrap().0
}

pub fn text(l: &Library, store: &mut MemStore, r: orpheus_library::StrRef) -> String {
    let mut b = [0u8; 128];
    l.text(store, r, &mut b).unwrap().to_string()
}

pub fn all_rows(l: &Library, store: &mut MemStore, list: List) -> Vec<Item> {
    let n = l.len(store, list).unwrap() as usize;
    let mut v = vec![Item::default(); n];
    let mut i = 0;
    while i < n {
        let k = (n - i).min(13);
        assert_eq!(l.rows(store, list, i as u32, &mut v[i..i + k]).unwrap(), k);
        i += k;
    }
    v
}

pub fn ref_seek_letter(letters: &[Option<char>], from: u32, forward: bool) -> Option<(u32, char)> {
    let cur = letters.get(from as usize).copied().flatten()?;
    // Runs: first row of each maximal run.
    let mut runs: Vec<(u32, Option<char>)> = Vec::new();
    for (i, l) in letters.iter().enumerate() {
        if runs.last().is_none_or(|r| r.1 != *l) {
            runs.push((i as u32, *l));
        }
    }
    let ri = runs.iter().rposition(|r| r.0 <= from).unwrap();
    let hit = |r: &&(u32, Option<char>)| r.1.is_some_and(|c| c != cur);
    let f = if forward { runs[ri + 1..].iter().find(hit) } else { runs[..ri].iter().rev().find(hit) };
    f.map(|r| (r.0, r.1.unwrap()))
}
