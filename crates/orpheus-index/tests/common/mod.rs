//! Host doubles: an in-memory `Storage` with I/O accounting and power-cut
//! injection, a mock directory, random name generators and the reference
//! order (a full in-memory sort with `natural_cmp`, as the simulator sorts).

#![allow(dead_code)]

use std::collections::HashMap;

use orpheus_index::{
    Builder, DirSource, EntryKind, Error, FileName, Index, Locator, MAX_NAME, NameSource, Progress, Row,
    SortBy, Spec, SrcEntry, Storage, kind_of, letter_of, natural_cmp,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IoErr {
    Cut,
    Missing,
}

#[derive(Clone, Default)]
pub struct MemStore {
    pub files: HashMap<String, Vec<u8>>,
    pub reads: u64,
    pub read_bytes: u64,
    pub writes: u64,
    pub write_bytes: u64,
    pub syncs: u64,
    /// Bytes that may still be written before the power fails (the write
    /// that crosses it is torn).
    pub cut_after: Option<u64>,
    pub dead: bool,
    /// Largest size any file reached.
    pub peak: HashMap<String, usize>,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }
    pub fn revive(&mut self) {
        self.dead = false;
        self.cut_after = None;
    }
    pub fn get(&self, f: FileName) -> Option<&Vec<u8>> {
        self.files.get(f.as_str())
    }
    pub fn get_mut(&mut self, f: FileName) -> Option<&mut Vec<u8>> {
        self.files.get_mut(f.as_str())
    }
    pub fn reset_counters(&mut self) {
        self.reads = 0;
        self.read_bytes = 0;
        self.writes = 0;
        self.write_bytes = 0;
        self.syncs = 0;
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
            if (data.len() as u64) > *left {
                data = &data[..*left as usize];
                torn = true;
            }
            *left -= data.len() as u64;
        }
        self.writes += 1;
        self.write_bytes += data.len() as u64;
        let name = file.as_str().to_string();
        let v = self.files.entry(name.clone()).or_default();
        let o = offset as usize;
        assert!(o <= v.len(), "write_at past the end of {name}: {o} > {}", v.len());
        if v.len() < o + data.len() {
            v.resize(o + data.len(), 0);
        }
        v[o..o + data.len()].copy_from_slice(data);
        let l = v.len();
        let p = self.peak.entry(name).or_default();
        *p = (*p).max(l);
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
        self.syncs += 1;
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

#[derive(Clone, Debug)]
pub struct MockEntry {
    pub name: String,
    pub is_dir: bool,
    pub hidden: bool,
    pub size: u64,
    pub mtime: u32,
    pub loc: Locator,
}

#[derive(Clone, Default)]
pub struct MockDir {
    pub entries: Vec<MockEntry>,
    pub pos: usize,
    pub reads: u64,
}

impl MockDir {
    pub fn new(entries: Vec<MockEntry>) -> MockDir {
        MockDir { entries, pos: 0, reads: 0 }
    }
}

impl DirSource for MockDir {
    type Error = IoErr;
    fn rewind(&mut self) -> Result<(), IoErr> {
        self.pos = 0;
        Ok(())
    }
    fn next_entry(&mut self, name: &mut [u8]) -> Result<Option<SrcEntry>, IoErr> {
        assert!(name.len() >= MAX_NAME);
        let Some(e) = self.entries.get(self.pos) else { return Ok(None) };
        self.pos += 1;
        self.reads += 1;
        // An over-long name is reported with its true length (the builder
        // refuses it).
        let n = e.name.len().min(name.len());
        name[..n].copy_from_slice(&e.name.as_bytes()[..n]);
        Ok(Some(SrcEntry {
            loc: e.loc,
            is_dir: e.is_dir,
            hidden: e.hidden,
            size: e.size,
            mtime: e.mtime,
            name_len: e.name.len(),
        }))
    }
}

impl NameSource for MockDir {
    type Error = IoErr;
    fn name_of(&mut self, loc: Locator, name: &mut [u8]) -> Result<Option<usize>, IoErr> {
        self.reads += 1;
        Ok(self.entries.iter().find(|e| e.loc == loc).map(|e| {
            name[..e.name.len()].copy_from_slice(e.name.as_bytes());
            e.name.len()
        }))
    }
}

/// xorshift64*.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
    pub fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

const LATIN: &[&str] = &[
    "track",
    "Track",
    "TRACK",
    "love",
    "Love",
    "night",
    "Émile",
    "élan",
    "zebra",
    "Alpha",
    "beta",
    "The",
    "a",
    "Ångström",
    "über",
    "Zoë",
    "mix",
    "Live",
    "remix",
    "demo",
    "x",
];
const CYRILLIC: &[&str] =
    &["Ёлка", "ёж", "Ежи", "песня", "Песня", "звезда", "Кино", "кино", "Ария", "ЯД", "юность", "Щит"];
const CJK: &[&str] = &["日本", "東京", "音楽", "夜", "愛", "中文歌", "한국어", "ひらがな", "カタカナ"];
const SEPS: &[&str] = &[" ", " - ", "_", ".", "", "(", ")", "-", "  ", "!", "#", "~"];
const EXTS: &[&str] = &[".mp3", ".flac", ".MP3", ".ogg", ".txt", ".jpg", ".m3u", ".cue", "", ".bin", ".opus"];

pub fn random_name(r: &mut Rng) -> String {
    let mut s = String::new();
    if r.chance(4) {
        s.push('.');
    }
    let parts = 1 + r.below(4);
    for p in 0..parts {
        if p > 0 {
            s.push_str(r.pick(SEPS));
        }
        match r.below(10) {
            0..=3 => s.push_str(r.pick(LATIN)),
            4..=5 => s.push_str(r.pick(CYRILLIC)),
            6 => s.push_str(r.pick(CJK)),
            _ => {
                let range = if r.chance(5) { u64::MAX } else { 2000 };
                let n = r.below(range);
                if r.chance(20) {
                    s.push('0');
                }
                s.push_str(&n.to_string());
            }
        }
    }
    s.push_str(r.pick(EXTS));
    if s.is_empty() {
        s.push('x');
    }
    s
}

/// Names that share long prefixes (album folders, numbered tracks).
pub fn prefixed_name(r: &mut Rng, i: usize) -> String {
    let artist = *r.pick(&["Artist Name", "artist name", "Artist Name Two", "Исполнитель", "アーティスト"]);
    let album = *r.pick(&["Greatest Hits", "Greatest Hits Vol 2", "Greatest Hits Vol 10", "Live"]);
    let track = r.below(120);
    let pad = if r.chance(50) { format!("{track:02}") } else { track.to_string() };
    format!("{artist} - {album} - {pad} Song {i}.mp3")
}

pub const DIR: u32 = 0x0000_1234;

pub fn make_dir(names: Vec<String>, r: &mut Rng) -> Vec<MockEntry> {
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let is_dir = r.chance(8);
            let range = if r.chance(10) { 1 << 40 } else { 5000 };
            MockEntry {
                hidden: r.chance(3),
                size: if is_dir { 0 } else { r.below(range) },
                mtime: 0x5A00_0000 + r.below(500) as u32,
                loc: Locator { dir: DIR, index: (i as u32) * 3 + 2 },
                name,
                is_dir,
            }
        })
        .collect()
}

pub fn random_dir(r: &mut Rng, n: usize) -> Vec<MockEntry> {
    let names = (0..n).map(|_| random_name(r)).collect();
    make_dir(names, r)
}

/// The listing as the simulator would produce it.
pub fn oracle(entries: &[MockEntry], spec: Spec) -> Vec<MockEntry> {
    let mut v: Vec<MockEntry> = entries
        .iter()
        .filter(|e| {
            spec.keeps(
                &e.name,
                &SrcEntry {
                    loc: e.loc,
                    is_dir: e.is_dir,
                    hidden: e.hidden,
                    size: e.size,
                    mtime: e.mtime,
                    name_len: e.name.len(),
                },
            )
        })
        .cloned()
        .collect();
    v.sort_by(|x, y| {
        y.is_dir
            .cmp(&x.is_dir)
            .then_with(|| match spec.sort {
                SortBy::Name => std::cmp::Ordering::Equal,
                SortBy::Date => y.mtime.cmp(&x.mtime),
                SortBy::Size => y.size.cmp(&x.size),
            })
            .then_with(|| natural_cmp(&x.name, &y.name))
            .then_with(|| (x.loc.dir, x.loc.index).cmp(&(y.loc.dir, y.loc.index)))
    });
    v
}

pub fn expect_row(e: &MockEntry) -> Row {
    Row {
        loc: e.loc,
        size: e.size,
        mtime: e.mtime,
        letter: letter_of(&e.name),
        kind: kind_of(&e.name, e.is_dir),
        hidden: e.hidden || orpheus_index::is_hidden_name(&e.name),
    }
}

pub struct BuildStats {
    pub steps: u32,
    pub max_pass: u8,
}

/// Run a build to completion; every `suspend_every` steps (if non-zero)
/// suspend and resume with a fresh, garbage-filled scratch of `scratch_len`.
pub fn build_with(
    store: &mut MemStore,
    src: &mut MockDir,
    spec: Spec,
    scratch_len: usize,
    budget: u32,
    suspend_every: u32,
) -> Result<(Index, BuildStats), Error<IoErr>> {
    let mut scratch = vec![0xA5u8; scratch_len];
    let mut b = Builder::new(&mut scratch, DIR, spec, 77).expect("scratch");
    let mut stats = BuildStats { steps: 0, max_pass: 0 };
    loop {
        let p = b.step(store, src, budget)?;
        stats.steps += 1;
        match p {
            Progress::Done(ix) => return Ok((ix, stats)),
            Progress::Merging { pass, .. } => stats.max_pass = stats.max_pass.max(pass),
            _ => {}
        }
        if suspend_every != 0 && stats.steps % suspend_every == 0 {
            let s = b.suspend(store)?;
            // A different buffer each time, with junk in it.
            let fresh = vec![(stats.steps as u8).wrapping_mul(37); scratch_len];
            scratch = fresh;
            b = s.resume(&mut scratch).expect("resume");
        }
        assert!(stats.steps < 10_000_000, "build does not terminate");
    }
}

pub fn build(store: &mut MemStore, src: &mut MockDir, spec: Spec, scratch_len: usize) -> Index {
    build_with(store, src, spec, scratch_len, 64, 0).expect("build").0
}

/// Read every row through 32-row windows and compare to the oracle.
pub fn check_listing(store: &mut MemStore, ix: &Index, entries: &[MockEntry], spec: Spec) {
    let want = oracle(entries, spec);
    assert_eq!(ix.count() as usize, want.len(), "row count");
    assert_eq!(ix.dirs() as usize, want.iter().filter(|e| e.is_dir).count(), "dir count");
    let mut win = [expect_row(&want.first().cloned().unwrap_or_else(dummy)); 32];
    let mut first = 0u32;
    while (first as usize) < want.len() {
        let n = ix.rows(store, first, &mut win).expect("rows");
        assert_eq!(n, (want.len() - first as usize).min(32));
        for (k, row) in win[..n].iter().enumerate() {
            let e = &want[first as usize + k];
            assert_eq!(*row, expect_row(e), "row {} ({:?}) spec {spec:?}", first as usize + k, e.name);
        }
        first += 32;
    }
    for e in &want {
        assert_eq!(kind_of(&e.name, e.is_dir) == EntryKind::Dir, e.is_dir);
    }
}

pub fn dummy() -> MockEntry {
    MockEntry {
        name: "x".into(),
        is_dir: false,
        hidden: false,
        size: 0,
        mtime: 0,
        loc: Locator { dir: 0, index: 0 },
    }
}

/// `ListSource::seek_letter`'s default algorithm over a plain letter list.
pub fn ref_seek_letter(letters: &[Option<char>], from: u32, forward: bool) -> Option<(u32, char)> {
    let n = letters.len() as u32;
    let cur = (*letters.get(from as usize)?)?;
    let mut i = from;
    loop {
        if forward {
            i += 1;
            if i >= n {
                return None;
            }
        } else {
            i = i.checked_sub(1)?;
        }
        if let Some(l) = letters[i as usize]
            && l != cur
        {
            if !forward {
                while i > 0 && letters[i as usize - 1] == Some(l) {
                    i -= 1;
                }
            }
            return Some((i, l));
        }
    }
}

pub const ALL_SPECS: &[Spec] = &[
    Spec { sort: SortBy::Name, show_hidden: false, audio_only: false, images_only: false },
    Spec { sort: SortBy::Name, show_hidden: true, audio_only: false, images_only: false },
    Spec { sort: SortBy::Date, show_hidden: false, audio_only: false, images_only: false },
    Spec { sort: SortBy::Size, show_hidden: true, audio_only: false, images_only: false },
    Spec { sort: SortBy::Name, show_hidden: false, audio_only: true, images_only: false },
    Spec { sort: SortBy::Date, show_hidden: true, audio_only: true, images_only: false },
    Spec { sort: SortBy::Name, show_hidden: false, audio_only: false, images_only: true },
];
