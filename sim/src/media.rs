//! The simulator's Files and Music back end: the card image mounted
//! through `orpheus-fs` (copy-on-write), a windowed browse listing that
//! behaves like the firmware's (first visit of a big folder in on-disk
//! order, sorted shortly after; rows outside the window are not
//! available until `want` + `Event::Data`), text windows, and a fake player
//! that reads real ID3 tags and `.lrc` files from the card and advances
//! with simulated time. MIDI files (`.mid`, `.kar`; MIDI-1) get their real
//! duration and title from `orpheus-midi`'s scan, and a karaoke file's
//! lyrics become timed lines for Now Playing.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use orpheus_apps::browse::{DirId, EntryKind, FileRef, is_hidden_name, kind_of, letter_of, natural_cmp};
use orpheus_apps::services::{
    ArtSize, Bookmark, BrowseFilter, DateTime, Entry, Levels, LibList, LibListing, LibNow, LibRow,
    LibraryState, Listing, LyricLine, PicError, PicInfo, PicReq, PicSource, PicState, PlayerCmd, PlayerState,
    QueueEntry, SortBy, TrackInfo, data,
};
use orpheus_audio::{Format, SliceSource};
use orpheus_fs::host::{Base, OverlayDisk, block_on};
use orpheus_fs::{CacheSlot, DirEntry, MountOptions, OpenMode, Volume};
use orpheus_ui::event::{DataReady, Event, Source};
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::Gfx;

use crate::library::SimLib;
use crate::{card, pictures};

/// A file of the card image as it was built (pictures for the viewer).
fn read_card_file(base: &Base<'static>, path: &str) -> Option<Vec<u8>> {
    let cache: &mut [CacheSlot] = &mut [CacheSlot::EMPTY; 4];
    let disk = OverlayDisk::over(base.clone());
    let opts = MountOptions { read_only: true, ..MountOptions::default() };
    let mut vol = block_on(Volume::mount(disk, cache, opts)).ok()?;
    let mut f = block_on(vol.open(path, OpenMode::Read)).ok()?;
    let mut buf = vec![0u8; usize::try_from(f.size()).ok()?];
    let n = block_on(vol.read_exact_or_eof(&mut f, &mut buf)).ok()?;
    buf.truncate(n);
    Some(buf)
}

/// Rows the browse window holds (the firmware keeps about two pages).
pub const WINDOW: u32 = 32;
/// Folders larger than this are listed in on-disk order on the first
/// visit while their index is "built" (04 §6.2).
pub const SORT_IN_RAM: usize = 128;
/// Simulated latencies.
const LOAD_MS: u64 = 40;
const SORT_MS: u64 = 400;
/// Silence between two tracks with Settings › Playback › Gapless off (the
/// firmware plays out its PCM ring, stops the DAC and opens the next file
/// fresh: about this long on the player).
pub const GAP_MS: u32 = 200;
/// Automatic bookmarks (05 §7.1): files longer than this keep their place,
/// noted this often while playing; the last three are kept.
const LONG_MS: u32 = 20 * 60 * 1000;
const MARK_EVERY_MS: u64 = 30_000;
const MARKS: usize = 3;

#[derive(Clone, Debug)]
pub struct HostEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: DateTime,
    pub file: FileRef,
    pub child: DirId,
    pub hidden: bool,
}

#[derive(Clone, Debug)]
pub struct QItem {
    pub file: FileRef,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u32,
    pub size: u64,
    pub lyrics: Vec<(Option<u32>, String)>,
    pub art: Option<u32>,
}

fn fnv(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

fn conv_time(t: orpheus_fs::DateTime) -> DateTime {
    DateTime {
        year: t.year,
        month: t.month,
        day: t.day,
        hour: t.hour,
        minute: t.minute,
        second: t.second,
        weekday: 0,
    }
}

/// Deterministic album art: a two-colour diagonal gradient with rings.
#[must_use]
pub fn make_art(seed: u32, size: usize) -> Vec<u16> {
    let hue = |k: u32| -> (f32, f32, f32) {
        let h = f32::from(u16::try_from(k % 360).unwrap_or(0)) / 60.0;
        let x = 1.0 - ((h % 2.0) - 1.0).abs();
        match h as u32 {
            0 => (1.0, x, 0.0),
            1 => (x, 1.0, 0.0),
            2 => (0.0, 1.0, x),
            3 => (0.0, x, 1.0),
            4 => (x, 0.0, 1.0),
            _ => (1.0, 0.0, x),
        }
    };
    let (a, b) = (hue(seed), hue(seed / 7 + 140));
    let n = size as f32;
    let mut out = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let t = (x + y) as f32 / (2.0 * n);
            let (dx, dy) = (x as f32 - n * 0.62, y as f32 - n * 0.38);
            let r = (dx * dx + dy * dy).sqrt() / n;
            let ring = if ((r * 9.0) as u32) % 2 == 0 { 1.0 } else { 0.82 };
            let mix = |p: f32, q: f32| ((p * (1.0 - t) + q * t) * ring * 0.85 + 0.08).clamp(0.0, 1.0);
            let (rr, gg, bb) = (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2));
            out.push(orpheus_ui::color::rgb((rr * 255.0) as u8, (gg * 255.0) as u8, (bb * 255.0) as u8));
        }
    }
    out
}

/// Cache slots of dropped [`Media`] (a volume borrows its cache for
/// `'static`): reused by the next one, so a simulator per test or per fuzz
/// input does not leak a cache each (VER-5 LeakSanitizer finding).
static CACHES: Mutex<Vec<&'static mut [CacheSlot]>> = Mutex::new(Vec::new());

/// Real audio files instead of tagged stand-ins (the browser build, WEB-2):
/// a track's length in milliseconds from its bytes and path, `None` if it
/// does not decode.
pub type DurationProbe = fn(&[u8], &str) -> Option<u32>;

/// What the audio output follows (the browser build decodes it for real).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowAudio {
    pub path: String,
    /// Place in the queue.
    pub index: usize,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub playing: bool,
}

pub struct Media {
    /// `None` only while dropping.
    vol: Option<Volume<'static, OverlayDisk<'static>>>,
    /// The image the volume overlays (pictures are read from a second mount).
    base: Base<'static>,
    /// Set: durations and sizes are the files' own (see [`DurationProbe`]).
    pub duration_probe: Option<DurationProbe>,
    /// Where `Resume` starts (a resume point restored from storage).
    resume_at: Option<u32>,
    dir_paths: HashMap<u32, String>,
    file_paths: HashMap<FileRef, String>,
    listing: Listing,
    filter: BrowseFilter,
    rows: Vec<HostEntry>,
    sorted_later: Option<(Vec<HostEntry>, u64)>,
    indexed: HashSet<u32>,
    win0: u32,
    pending_win: Option<(u32, u64)>,
    text: Vec<u8>,
    text_file: Option<FileRef>,
    text_win: u32,
    text_pending: Option<(u32, u64)>,
    positions: HashMap<FileRef, u32>,
    // Player.
    queue: Vec<QItem>,
    pos: usize,
    position: u32,
    playing: bool,
    a: Option<u32>,
    b: Option<u32>,
    bookmarks: Vec<(String, u32)>,
    resume: Option<QItem>,
    pub library: LibraryState,
    /// The library, once built (`set:library=ready`).
    lib: Option<SimLib>,
    /// Every folder has been read (paths of the library's tracks).
    learnt_all: bool,
    art_small: Vec<u16>,
    art_full: Vec<u16>,
    art_for: Option<u32>,
    pub log: Vec<String>,
    queue_pending: Option<u64>,
    events: Vec<Event>,
    /// Settings › Playback › Gapless and Auto-bookmark long files
    /// (`set_options`).
    gapless: bool,
    auto_bm: bool,
    /// Silence left before the next track's time runs (Gapless off).
    gap_left: u32,
    /// Settings › Sound › Fade on pause (`set_options`).
    fade: bool,
    /// The firmware's `status` (VER-13): the last pause's fade (ms) and
    /// whether the last track change at a track's end was gapless.
    pub last_fade: Option<u32>,
    pub last_change: Option<bool>,
    /// Automatic bookmarks: (path, ms), the most recently left last.
    marks: Vec<(String, u32)>,
    /// Playing time, for the automatic bookmarks' 30 s.
    played_ms: u64,
}

impl Drop for Media {
    fn drop(&mut self) {
        if let Some(v) = self.vol.take() {
            let (_, cache) = v.into_parts();
            if let Ok(mut p) = CACHES.lock() {
                p.push(cache);
            }
        }
    }
}

impl Default for Media {
    fn default() -> Self {
        Self::new()
    }
}

impl Media {
    #[must_use]
    pub fn new() -> Media {
        Self::with_card(Base::Borrowed(card::image())).expect("mount the card image")
    }

    /// Mount another card image (copy-on-write); `None` if it does not mount.
    #[must_use]
    pub fn with_card(base: Base<'static>) -> Option<Media> {
        let cache: &'static mut [CacheSlot] = CACHES
            .lock()
            .ok()
            .and_then(|mut p| p.pop())
            .unwrap_or_else(|| Box::leak(vec![CacheSlot::EMPTY; 16].into_boxed_slice()));
        let disk = OverlayDisk::over(base.clone());
        // A card that does not mount keeps its cache (16 slots, once).
        let vol = block_on(Volume::mount(disk, cache, MountOptions::default())).ok()?;
        Some(Self::from_volume(vol, base))
    }

    /// A fresh player state over a mounted card.
    fn from_volume(vol: Volume<'static, OverlayDisk<'static>>, base: Base<'static>) -> Media {
        let mut dir_paths = HashMap::new();
        dir_paths.insert(0, String::new());
        Media {
            vol: Some(vol),
            base,
            duration_probe: None,
            resume_at: None,
            dir_paths,
            file_paths: HashMap::new(),
            listing: Listing::default(),
            filter: BrowseFilter::default(),
            rows: Vec::new(),
            sorted_later: None,
            indexed: HashSet::new(),
            win0: 0,
            pending_win: None,
            text: Vec::new(),
            text_file: None,
            text_win: 0,
            text_pending: None,
            positions: HashMap::new(),
            queue: Vec::new(),
            pos: 0,
            position: 0,
            playing: false,
            a: None,
            b: None,
            bookmarks: Vec::new(),
            resume: None,
            library: LibraryState::Building { files: 1287 },
            lib: None,
            learnt_all: false,
            art_small: Vec::new(),
            art_full: Vec::new(),
            art_for: None,
            log: Vec::new(),
            queue_pending: None,
            events: Vec::new(),
            gapless: true,
            auto_bm: true,
            gap_left: 0,
            fade: true,
            last_fade: None,
            last_change: None,
            marks: Vec::new(),
            played_ms: 0,
        }
    }

    /// Settings › Storage › Format card (VER-13): the card task's work as
    /// the firmware does it (`orpheus_fs::format` on the card's region,
    /// then a fresh mount and player state); `false` if the format failed
    /// (the card then reads as unformatted: no volume until a new image).
    pub fn format_card(&mut self) -> bool {
        let Some(v) = self.vol.take() else { return false };
        let (mut disk, cache) = v.into_parts();
        let mut buf = [[0u8; 512]; 32];
        let ok = block_on(orpheus_fs::format::format(&mut disk, None, 0x4F52_5048, &mut buf)).is_ok();
        match block_on(Volume::mount(disk, cache, MountOptions::default())) {
            Ok(vol) if ok => {
                *self = Self::from_volume(vol, self.base.clone());
                true
            }
            _ => false,
        }
    }

    /// Settings › Storage › Clear caches: [`orpheus_apps::services::CACHE_DIRS`]
    /// removed under the firmware's write prefix; `false` on a card error.
    pub fn clear_caches(&mut self) -> bool {
        let Some(v) = self.vol.as_mut() else { return false };
        v.set_write_prefix(Some("/.orpheus"));
        let mut buf = [0u8; 256];
        let mut e = orpheus_fs::DirEntry::new();
        let mut ok = true;
        for d in orpheus_apps::services::CACHE_DIRS {
            ok &= block_on(v.remove_tree(d, &mut buf, &mut e)).is_ok();
        }
        v.set_write_prefix(None);
        ok &= block_on(v.flush()).is_ok();
        ok
    }

    /// `set:cardbad=1`: write `/Damaged.bin` (three clusters) and free its
    /// first cluster's link in every FAT (the simulated card is FAT16), so
    /// Check card finds one problem.
    pub fn damage_a_file(&mut self) {
        let Some(v) = self.vol.as_mut() else { return };
        let Ok(mut f) = block_on(v.open("/Damaged.bin", orpheus_fs::OpenMode::CreateNew)) else { return };
        let _ = block_on(v.write(&mut f, &[0x5A; 6000]));
        let first = f.first_cluster();
        let _ = block_on(v.close(f));
        let _ = block_on(v.flush());
        let d = v.device();
        let mut bs = [[0u8; 512]];
        let _ = block_on(orpheus_fs::BlockDevice::read(d, 0, &mut bs));
        let reserved = usize::from(u16::from_le_bytes([bs[0][14], bs[0][15]]));
        let fat_len = usize::from(u16::from_le_bytes([bs[0][22], bs[0][23]]));
        for k in 0..usize::from(bs[0][16]) {
            let at = (reserved + k * fat_len) * 512 + first as usize * 2;
            d.patch(at, 0);
            d.patch(at + 1, 0);
        }
        let n = u32::try_from(fat_len * usize::from(bs[0][16])).unwrap_or(0);
        v.invalidate_cached(u32::try_from(reserved).unwrap_or(0), n);
    }

    /// The developer command `ls PATH` as the firmware reads it: the names,
    /// a `/` after a folder's (`None`: no such directory).
    pub fn ls(&mut self, path: &str) -> Option<Vec<String>> {
        let v = self.vol.as_mut()?;
        let mut cur = block_on(v.open_dir(path)).ok()?;
        let mut e = orpheus_fs::DirEntry::new();
        let mut out = Vec::new();
        while let Ok(true) = block_on(v.read_dir(&mut cur, &mut e)) {
            out.push(format!("{}{}", e.name(), if e.is_dir() { "/" } else { "" }));
        }
        Some(out)
    }

    /// Settings › Storage › Check card: the read-only check.
    pub fn check_card(&mut self) -> Option<orpheus_fs::CheckReport> {
        let v = self.vol.as_mut()?;
        let mut stack = [v.root(); orpheus_apps::services::CHECK_DEPTH];
        let mut e = orpheus_fs::DirEntry::new();
        block_on(v.check(&mut stack, &mut e, &mut |_| false)).ok()
    }

    /// Settings › Playback › Gapless and Auto-bookmark long files, as the
    /// firmware's player follows them, and Settings › Sound › Fade on
    /// pause (VER-13).
    pub fn set_options(&mut self, gapless: bool, auto_bookmark: bool, fade: bool) {
        self.gapless = gapless;
        self.auto_bm = auto_bookmark;
        self.fade = fade;
    }

    /// The automatic bookmarks (path, ms), the most recently left last.
    #[must_use]
    pub fn auto_marks(&self) -> &[(String, u32)] {
        &self.marks
    }

    /// The current track is left: a long file notes its place (or forgets
    /// it at its start or end), as `orpheus_player` does.
    fn leave(&mut self) {
        let Some(q) = self.current().filter(|q| self.auto_bm && q.duration > LONG_MS) else { return };
        let path = q.path.clone();
        let end = q.duration.saturating_sub(10_000);
        self.marks.retain(|m| m.0 != path);
        if self.position > 0 && self.position < end {
            if self.marks.len() == MARKS {
                self.marks.remove(0);
            }
            self.marks.push((path, self.position));
        }
    }

    /// A track was reached at its start: a long file continues at its
    /// automatic bookmark.
    fn arrive(&mut self) {
        if self.position != 0 || !self.auto_bm {
            return;
        }
        if let Some(q) = self.current().filter(|q| q.duration > LONG_MS)
            && let Some(at) = self.marks.iter().find(|m| m.0 == q.path).map(|m| m.1)
        {
            self.position = at;
        }
    }

    /// Start-up with a resume point and Resume at start-up on (05 §6.1):
    /// its track loaded, paused at its place.
    pub fn resume_paused(&mut self) {
        if let Some(q) = self.resume.take() {
            self.queue = vec![q];
            self.pos = 0;
            self.playing = false;
            self.position = self.resume_at.take().unwrap_or(0);
            self.refresh_art();
        }
    }

    /// The mounted volume (the browser build reads its size and free
    /// space; the recorder writes its files through it).
    pub fn volume(&mut self) -> &mut Volume<'static, OverlayDisk<'static>> {
        self.vol.as_mut().expect("mounted")
    }

    /// What is loaded for the audio output: the current queue entry.
    #[must_use]
    pub fn now_audio(&self) -> Option<NowAudio> {
        let q = self.current()?;
        Some(NowAudio {
            path: q.path.clone(),
            index: self.pos,
            position_ms: self.position,
            duration_ms: q.duration,
            playing: self.playing,
        })
    }

    /// The resume point to keep: the current track and position, else a
    /// restored one not yet used.
    #[must_use]
    pub fn resume_point(&self) -> Option<(String, u32)> {
        if let Some(q) = self.current() {
            return Some((q.path.clone(), self.position));
        }
        self.resume.as_ref().map(|q| (q.path.clone(), self.resume_at.unwrap_or(0)))
    }

    /// A resume point restored from storage: `Resume` starts at `ms`.
    pub fn set_resume_at(&mut self, path: &str, ms: u32) {
        self.set_resume(path);
        self.resume_at = Some(ms);
    }

    /// Forget the current listing (files were added): the next browse
    /// reads the folder again.
    pub fn refresh(&mut self) {
        self.rows.clear();
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn data(&mut self, tag: u16) {
        self.events.push(Event::Data(DataReady { tag, first: 0, count: 0 }));
    }

    fn path_of_dir(&self, dir: DirId) -> String {
        self.dir_paths.get(&dir.0).cloned().unwrap_or_default()
    }

    /// Read a directory through orpheus-fs (host memory: the simulator may
    /// hold it; the service interface only ever shows a window).
    fn read_dir(&mut self, dir: DirId) -> Vec<HostEntry> {
        let base = self.path_of_dir(dir);
        let path = if base.is_empty() { "/".to_string() } else { base.clone() };
        let Ok(mut cur) = block_on(self.vol.as_mut().expect("mounted").open_dir(&path)) else {
            return Vec::new();
        };
        let mut e = DirEntry::new();
        let mut out = Vec::new();
        while let Ok(true) = block_on(self.vol.as_mut().expect("mounted").read_dir(&mut cur, &mut e)) {
            let name = e.name().to_string();
            let is_dir = e.is_dir();
            let loc = e.locator();
            let file = FileRef { dir: loc.dir, index: loc.index };
            let full = format!("{base}/{name}");
            let child = if is_dir { DirId(e.first_cluster()) } else { DirId::ROOT };
            if is_dir {
                self.dir_paths.insert(child.0, full.clone());
            }
            self.file_paths.insert(file, full);
            out.push(HostEntry {
                kind: kind_of(&name, is_dir),
                hidden: is_hidden_name(&name) || e.attributes() & 0x06 != 0,
                name,
                size: e.size(),
                modified: conv_time(e.modified()),
                file,
                child,
            });
        }
        out
    }

    fn sort(rows: &mut [HostEntry], by: SortBy) {
        rows.sort_by(|x, y| {
            let dirs = (y.kind == EntryKind::Dir).cmp(&(x.kind == EntryKind::Dir));
            dirs.then_with(|| match by {
                SortBy::Name => std::cmp::Ordering::Equal,
                SortBy::Date => {
                    let k = |e: &HostEntry| {
                        (
                            e.modified.year,
                            e.modified.month,
                            e.modified.day,
                            e.modified.hour,
                            e.modified.minute,
                        )
                    };
                    k(y).cmp(&k(x))
                }
                SortBy::Size => y.size.cmp(&x.size),
            })
            .then_with(|| natural_cmp(&x.name, &y.name))
        });
    }

    pub fn browse(&mut self, dir: DirId, filter: BrowseFilter) -> Listing {
        if self.listing.dir == dir && self.filter == filter && !self.rows.is_empty() {
            return self.listing;
        }
        let mut rows: Vec<HostEntry> = self
            .read_dir(dir)
            .into_iter()
            .filter(|e| filter.hidden || !e.hidden)
            .filter(|e| filter.keeps(e.kind))
            .collect();
        let big = rows.len() > SORT_IN_RAM && !self.indexed.contains(&dir.0);
        let mut sorted = rows.clone();
        Self::sort(&mut sorted, filter.sort);
        if big {
            self.sorted_later = Some((sorted, SORT_MS));
            self.indexed.insert(dir.0);
        } else {
            rows = sorted;
            self.sorted_later = None;
        }
        let count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        self.rows = rows;
        self.filter = filter;
        self.listing = Listing { dir, count, sorted: !big, complete: true };
        self.win0 = 0;
        self.pending_win = None;
        self.listing
    }

    pub fn listing(&self) -> Listing {
        self.listing
    }

    pub fn entry(&self, i: u32) -> Option<Entry<'_>> {
        if i < self.win0 || i >= self.win0 + WINDOW {
            return None;
        }
        let e = self.rows.get(usize::try_from(i).ok()?)?;
        Some(Entry {
            name: &e.name,
            kind: e.kind,
            size: e.size,
            modified: e.modified,
            file: e.file,
            child: e.child,
        })
    }

    pub fn want(&mut self, first: u32, count: u32) {
        if first >= self.win0 && first + count <= self.win0 + WINDOW {
            return;
        }
        let start = first.saturating_sub(WINDOW / 4);
        self.pending_win = Some((start, LOAD_MS));
    }

    pub fn position(&self, f: FileRef) -> Option<u32> {
        self.rows.iter().position(|e| e.file == f).and_then(|p| u32::try_from(p).ok())
    }

    pub fn letter(&self, i: u32) -> Option<char> {
        self.entry(i).and_then(|e| letter_of(e.name))
    }

    pub fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        // The firmware answers this from the directory index; here the
        // whole sorted listing is at hand.
        let at = |i: usize| self.rows.get(i).and_then(|e| letter_of(&e.name));
        let from = usize::try_from(from).ok()?;
        let cur = at(from)?;
        if forward {
            let i = (from + 1..self.rows.len()).find(|&i| at(i).is_some_and(|l| l != cur))?;
            Some((u32::try_from(i).ok()?, at(i)?))
        } else {
            let mut i = (0..from).rev().find(|&i| at(i).is_some_and(|l| l != cur))?;
            let l = at(i)?;
            while i > 0 && at(i - 1) == Some(l) {
                i -= 1;
            }
            Some((u32::try_from(i).ok()?, l))
        }
    }

    pub fn dir_name(&self, dir: DirId) -> &str {
        self.dir_paths.get(&dir.0).map_or("", |p| p.rsplit('/').next().unwrap_or(""))
    }

    pub fn delete(&mut self, f: FileRef) -> bool {
        let Some(path) = self.file_paths.get(&f).cloned() else { return false };
        let ok = block_on(self.vol.as_mut().expect("mounted").remove(&path)).is_ok();
        if ok {
            self.log.push(format!("deleted {path}"));
            let (dir, filter) = (self.listing.dir, self.filter);
            self.rows.clear();
            self.browse(dir, filter);
        }
        ok
    }

    /// A whole file of the card (empty if it cannot be read).
    pub fn read_file(&mut self, path: &str) -> Vec<u8> {
        let Ok(mut f) = block_on(self.vol.as_mut().expect("mounted").open(path, OpenMode::Read)) else {
            return Vec::new();
        };
        let mut buf = vec![0u8; usize::try_from(f.size()).unwrap_or(0)];
        let n =
            block_on(self.vol.as_mut().expect("mounted").read_exact_or_eof(&mut f, &mut buf)).unwrap_or(0);
        buf.truncate(n);
        buf
    }

    // ------------------------------------------------------------ text

    pub fn text_open(&mut self, f: FileRef) -> Option<u32> {
        let path = self.file_paths.get(&f)?.clone();
        self.text = self.read_file(&path);
        self.text_file = Some(f);
        self.text_win = u32::MAX;
        u32::try_from(self.text.len()).ok()
    }

    pub fn text_window(&self) -> (u32, &[u8]) {
        if self.text_win == u32::MAX {
            return (0, &[]);
        }
        let s = usize::try_from(self.text_win).unwrap_or(0);
        let e = (s + 1536).min(self.text.len());
        (self.text_win, self.text.get(s..e).unwrap_or(&[]))
    }

    pub fn want_text(&mut self, offset: u32) {
        let (ws, win) = self.text_window();
        let have = self.text_win != u32::MAX
            && offset >= ws
            && (offset as usize + 600 <= ws as usize + win.len()
                || ws as usize + win.len() == self.text.len());
        if !have {
            self.text_pending = Some((offset.saturating_sub(512) / 512 * 512, LOAD_MS));
        }
    }

    pub fn text_position(&self, f: FileRef) -> u32 {
        self.positions.get(&f).copied().unwrap_or(0)
    }

    pub fn set_text_position(&mut self, f: FileRef, offset: u32) {
        self.positions.insert(f, offset);
    }

    /// Write the on-device suite's books (`card::SUITE_BOOKS`, as
    /// `cargo xtask dev suite --setup` puts them on the player) to the card
    /// (once): `set:bookstest=on`.
    pub fn add_suite_books(&mut self) {
        let vol = self.volume();
        if block_on(vol.exists(crate::card::SUITE_BOOKS)).unwrap_or(false) {
            return;
        }
        for d in ["/.orpheus", "/.orpheus/test", "/.orpheus/test/suite", crate::card::SUITE_BOOKS] {
            let _ = block_on(vol.create_dir(d));
        }
        let epub = crate::card::epub();
        for (path, data) in [
            (crate::card::SUITE_TXT, crate::card::SUITE_BOOK.as_bytes()),
            (crate::card::SUITE_EPUB, &epub[..]),
        ] {
            if let Ok(mut f) = block_on(vol.open(path, OpenMode::CreateNew)) {
                let _ = block_on(vol.write(&mut f, data));
                let _ = block_on(vol.close(f));
            }
        }
        let _ = block_on(vol.flush());
    }

    // ------------------------------------------------------------ player

    /// Write the MIDI demo songs to `/Music/MIDI` (once) and return the
    /// folder (`set:queue=midi`). The built card has no MIDI files, so the
    /// other journeys and goldens do not see them.
    pub fn add_midi_demo(&mut self) -> &'static str {
        const DIR: &str = "/Music/MIDI";
        type Song = fn(&mut [u8]) -> Option<usize>;
        let songs: [(&str, Song); 3] = [
            ("Etude.mid", orpheus_midi::demo::etude),
            ("Hill Song.kar", orpheus_midi::demo::karaoke),
            ("Sound tour.mid", orpheus_midi::demo::tour),
        ];
        let vol = self.volume();
        if !block_on(vol.exists(DIR)).unwrap_or(false) {
            let _ = block_on(vol.create_dir(DIR));
            for (name, song) in songs {
                let mut buf = vec![0u8; 64 * 1024];
                let n = song(&mut buf).unwrap_or(0);
                let path = format!("{DIR}/{name}");
                if let Ok(mut f) = block_on(vol.open(&path, OpenMode::CreateNew)) {
                    let _ = block_on(vol.write(&mut f, &buf[..n]));
                    let _ = block_on(vol.close(f));
                }
            }
            self.refresh();
        }
        DIR
    }

    /// Duration, title and karaoke lines of a MIDI file (MIDI-1).
    fn load_midi(item: &mut QItem, data: &[u8], kar: bool) {
        let text = |b: &[u8]| match std::str::from_utf8(b) {
            Ok(s) => s.to_string(),
            Err(_) => b.iter().map(|&c| char::from(c)).collect(), // Latin-1
        };
        let mut lines: Vec<(Option<u32>, String)> = Vec::new();
        let mut cur: Option<(u32, String)> = None;
        let r = orpheus_midi::summary(&mut orpheus_midi::SliceSource::new(data), 8000, kar, |l| {
            let ms = u32::try_from(l.frame * 1000 / 8000).unwrap_or(u32::MAX);
            if l.before != orpheus_midi::kar::Break::None
                && let Some((t, s)) = cur.take()
                && !s.trim().is_empty()
            {
                lines.push((Some(t), s.trim_end().to_string()));
            }
            let (_, s) = cur.get_or_insert_with(|| (ms, String::new()));
            s.push_str(&text(l.text()));
            if l.after != orpheus_midi::kar::Break::None
                && let Some((t, s)) = cur.take()
            {
                lines.push((Some(t), s.trim_end().to_string()));
            }
        });
        let Ok((info, _)) = r else {
            item.duration = 0;
            return;
        };
        if let Some((t, s)) = cur.take()
            && !s.trim().is_empty()
        {
            lines.push((Some(t), s.trim_end().to_string()));
        }
        item.duration = u32::try_from(info.duration_ms()).unwrap_or(u32::MAX);
        if !info.title().is_empty() {
            item.title = text(info.title());
        }
        item.lyrics = lines;
    }

    fn load_item(&mut self, file: FileRef, path: &str) -> QItem {
        let data = self.read_file(path);
        let stem = path.rsplit('/').next().unwrap_or(path);
        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
        // The podcasts are long (automatic bookmarks, over 20 min).
        let long = if path.contains("/Podcasts/") { 75 * 60_000 } else { 0 };
        let mut item = QItem {
            file,
            path: path.to_string(),
            title: stem.to_string(),
            artist: String::new(),
            album: String::new(),
            duration: long + 150_000 + (fnv(path) % 150) * 1000 + fnv(stem) % 1000,
            size: data.len() as u64 * 9000 + 3_000_000,
            lyrics: Vec::new(),
            art: None,
        };
        if let Some(kar) = orpheus_midi::extension(path) {
            item.size = data.len() as u64;
            Self::load_midi(&mut item, &data, kar);
            return item;
        }
        if let Some(probe) = self.duration_probe {
            item.size = data.len() as u64;
            item.duration = probe(&data, path).unwrap_or(0);
        }
        let tags = orpheus_library::FileFormat::of(path).tag_format().unwrap_or(Format::Mpeg);
        if let Ok(t) =
            orpheus_meta::read_all(&mut SliceSource::new(&data), tags, orpheus_meta::Opts::default())
        {
            if !t.title.is_empty() {
                item.title = t.title.as_str().to_string();
            }
            item.artist = t.artist.as_str().to_string();
            item.album = t.album.as_str().to_string();
        }
        if !item.album.is_empty() {
            item.art = Some(fnv(&item.album));
        }
        let lrc_path = format!("{}.lrc", path.rsplit_once('.').map_or(path, |(s, _)| s));
        let lrc = self.read_file(&lrc_path);
        let mut r = orpheus_meta::lrc::LrcReader::new(orpheus_meta::CodePage::default());
        for line in lrc.split(|&b| b == b'\n') {
            if let Some(l) = r.parse_line(line) {
                for t in &l.times[..usize::from(l.n)] {
                    item.lyrics.push((Some(*t), l.text.as_str().to_string()));
                }
            }
        }
        item.lyrics.sort_by_key(|l| l.0);
        item
    }

    fn audio_in(&mut self, dir: DirId) -> Vec<(FileRef, String)> {
        let mut rows = self.read_dir(dir);
        Self::sort(&mut rows, SortBy::Name);
        let base = self.path_of_dir(dir);
        rows.into_iter()
            .filter(|e| e.kind == EntryKind::Audio)
            .map(|e| (e.file, format!("{base}/{}", e.name)))
            .collect()
    }

    fn walk_audio(&mut self, dir: DirId, out: &mut Vec<(FileRef, String)>, depth: u8) {
        out.extend(self.audio_in(dir));
        if depth == 0 {
            return;
        }
        let mut rows = self.read_dir(dir);
        Self::sort(&mut rows, SortBy::Name);
        for e in rows.into_iter().filter(|e| e.kind == EntryKind::Dir && !e.hidden) {
            self.walk_audio(e.child, out, depth - 1);
        }
    }

    fn shuffle<T>(v: &mut [T], seed: u32) {
        let mut s = seed | 1;
        for i in (1..v.len()).rev() {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            v.swap(i, s as usize % (i + 1));
        }
    }

    fn set_queue(&mut self, files: Vec<(FileRef, String)>, start: usize) {
        self.leave();
        self.queue = files.iter().map(|(f, p)| self.load_item(*f, p)).collect();
        self.pos = start.min(self.queue.len().saturating_sub(1));
        self.position = 0;
        self.gap_left = 0;
        self.playing = !self.queue.is_empty();
        self.a = None;
        self.b = None;
        self.arrive();
        self.refresh_art();
    }

    fn refresh_art(&mut self) {
        let seed = self.queue.get(self.pos).and_then(|q| q.art);
        if seed != self.art_for {
            self.art_for = seed;
            if let Some(s) = seed {
                self.art_small = make_art(s, 52);
                self.art_full = make_art(s, 128);
            }
        }
    }

    fn current(&self) -> Option<&QItem> {
        self.queue.get(self.pos)
    }

    pub fn command(&mut self, c: PlayerCmd, repeat: i16) {
        self.log.push(format!("{c:?}"));
        match c {
            PlayerCmd::PlayPause => {
                if self.queue.is_empty() {
                    if self.resume.is_some() {
                        self.command(PlayerCmd::Resume, repeat);
                    }
                } else {
                    if self.playing {
                        self.leave();
                        // The firmware fades out over 60 ms first.
                        self.last_fade = Some(if self.fade { 60 } else { 0 });
                    }
                    self.playing = !self.playing;
                }
            }
            PlayerCmd::Stop => {
                self.leave();
                self.playing = false;
                self.position = 0;
                self.resume = None;
            }
            PlayerCmd::Next => self.next(repeat, true),
            PlayerCmd::Prev => {
                self.leave();
                self.pos = self.pos.saturating_sub(1);
                self.position = 0;
                self.arrive();
            }
            PlayerCmd::SeekTo(ms) => self.position = ms.min(self.current().map_or(0, |q| q.duration)),
            PlayerCmd::SeekBy(d) => {
                let dur = self.current().map_or(0, |q| q.duration);
                self.position =
                    u32::try_from((i64::from(self.position) + i64::from(d)).clamp(0, i64::from(dur)))
                        .unwrap_or(0);
            }
            PlayerCmd::Jump(n) => {
                if (n as usize) < self.queue.len() {
                    self.leave();
                    self.pos = n as usize;
                    self.position = 0;
                    self.playing = true;
                    self.arrive();
                }
            }
            PlayerCmd::Remove(n) => {
                if (n as usize) < self.queue.len() && n as usize != self.pos {
                    self.queue.remove(n as usize);
                    if (n as usize) < self.pos {
                        self.pos -= 1;
                    }
                }
            }
            PlayerCmd::MoveUp(n) => {
                let n = n as usize;
                if n > self.pos + 1 && n < self.queue.len() {
                    self.queue.swap(n, n - 1);
                }
            }
            PlayerCmd::MoveDown(n) => {
                let n = n as usize;
                if n > self.pos && n + 1 < self.queue.len() {
                    self.queue.swap(n, n + 1);
                }
            }
            PlayerCmd::PlayDir { dir, start, shuffle } => {
                let mut files = self.audio_in(dir);
                let mut at = start.and_then(|s| files.iter().position(|(f, _)| *f == s)).unwrap_or(0);
                if shuffle {
                    Self::shuffle(&mut files, 0x5EED);
                    at = 0;
                }
                self.set_queue(files, at);
            }
            PlayerCmd::ShuffleAll if self.lib.is_some() => {
                let locs = self
                    .lib
                    .as_ref()
                    .map(|l| l.sample(self.vol.as_mut().expect("mounted"), 0xC0FFEE, 256))
                    .unwrap_or_default();
                let mut files = self.paths_of(&locs);
                Self::shuffle(&mut files, 0xC0FFEE);
                self.set_queue(files, 0);
            }
            PlayerCmd::ShuffleAll => {
                let mut files = Vec::new();
                let music =
                    self.dir_paths.iter().find(|(_, p)| p.as_str() == "/Music").map(|(k, _)| DirId(*k));
                let root = music.unwrap_or_else(|| {
                    let _ = self.read_dir(DirId::ROOT);
                    self.dir_paths
                        .iter()
                        .find(|(_, p)| p.as_str() == "/Music")
                        .map_or(DirId::ROOT, |(k, _)| DirId(*k))
                });
                self.walk_audio(root, &mut files, 3);
                // The 5000 empty files of the big folder are not music.
                let sizes: Vec<bool> = files.iter().map(|(_, p)| !p.contains("/Big folder/")).collect();
                let mut k = sizes.into_iter();
                files.retain(|_| k.next().unwrap_or(true));
                Self::shuffle(&mut files, 0xC0FFEE);
                self.set_queue(files, 0);
            }
            PlayerCmd::PlayLib { list, start, shuffle } => {
                let Some(lib) = self.lib.as_ref() else { return };
                let (locs, at) = lib.queue(self.vol.as_mut().expect("mounted"), list, start, 256);
                let mut files = self.paths_of(&locs);
                let mut at = at.min(files.len().saturating_sub(1));
                if shuffle {
                    Self::shuffle(&mut files, 0x5EED);
                    at = 0;
                }
                self.set_queue(files, at);
            }
            PlayerCmd::PlayNext(f) | PlayerCmd::AddToQueue(f) => {
                if let Some(p) = self.file_paths.get(&f).cloned() {
                    let item = self.load_item(f, &p);
                    let at =
                        if matches!(c, PlayerCmd::PlayNext(_)) { self.pos + 1 } else { self.queue.len() };
                    self.queue.insert(at.min(self.queue.len()), item);
                }
            }
            PlayerCmd::SetA => self.a = Some(self.position),
            PlayerCmd::SetB => self.b = Some(self.position),
            PlayerCmd::ClearAb => {
                self.a = None;
                self.b = None;
            }
            PlayerCmd::Bookmark => {
                if let Some(q) = self.current() {
                    let t = q.title.clone();
                    self.bookmarks.push((t, self.position));
                }
            }
            PlayerCmd::Favourite => {}
            PlayerCmd::PlayBookmark(i) => {
                if let Some((t, at)) = self.bookmarks.get(i as usize).cloned()
                    && let Some(p) = self.queue.iter().position(|q| q.title == t)
                {
                    self.pos = p;
                    self.position = at;
                    self.playing = true;
                }
            }
            PlayerCmd::Resume => {
                if let Some(q) = self.resume.take() {
                    self.queue = vec![q];
                    self.pos = 0;
                    self.playing = true;
                    if let Some(at) = self.resume_at.take() {
                        self.position = at;
                    }
                }
            }
        }
        self.refresh_art();
        self.queue_pending = Some(LOAD_MS);
    }

    fn next(&mut self, repeat: i16, user: bool) {
        if user {
            self.leave();
        } else if let Some(q) = self.current() {
            // Played to its end: no place to keep.
            let path = q.path.clone();
            if self.auto_bm {
                self.marks.retain(|m| m.0 != path);
            }
        }
        self.gap_left = 0;
        if repeat == 1 && !user {
            self.position = 0;
            if !self.gapless {
                self.gap_left = GAP_MS;
            }
            return;
        }
        if self.pos + 1 < self.queue.len() || repeat == 2 {
            self.pos = if self.pos + 1 < self.queue.len() { self.pos + 1 } else { 0 };
            if !user {
                self.last_change = Some(self.gapless);
                if !self.gapless {
                    self.gap_left = GAP_MS;
                }
            }
        } else {
            self.playing = false;
        }
        self.position = 0;
        self.arrive();
        self.refresh_art();
    }

    /// Simulated time passes.
    pub fn advance(&mut self, ms: u64, repeat: i16) {
        if self.playing
            && let Some(q) = self.current()
        {
            let dur = q.duration;
            // Gapless off: the silence between two tracks first.
            let gap = self.gap_left.min(u32::try_from(ms).unwrap_or(u32::MAX));
            self.gap_left -= gap;
            let ms = ms - u64::from(gap);
            let before = self.played_ms;
            self.played_ms += ms;
            self.position = self.position.saturating_add(u32::try_from(ms).unwrap_or(0));
            if before / MARK_EVERY_MS != self.played_ms / MARK_EVERY_MS && self.position < dur {
                self.leave();
            }
            if let (Some(a), Some(b)) = (self.a, self.b)
                && b > a
                && self.position >= b
            {
                self.position = a;
            }
            if self.position >= dur {
                self.next(repeat, false);
            }
        }
        let tick = |p: &mut Option<(u32, u64)>| -> Option<u32> {
            let (v, left) = (*p)?;
            if left <= ms {
                *p = None;
                Some(v)
            } else {
                *p = Some((v, left - ms));
                None
            }
        };
        if let Some(w) = tick(&mut self.pending_win) {
            self.win0 = w;
            self.data(data::BROWSE);
        }
        if let Some(t) = tick(&mut self.text_pending) {
            self.text_win = t;
            self.data(data::TEXT);
        }
        if let Some((rows, left)) = self.sorted_later.take() {
            if left <= ms {
                self.rows = rows;
                self.listing.sorted = true;
                self.data(data::BROWSE);
            } else {
                self.sorted_later = Some((rows, left - ms));
            }
        }
        if let Some(left) = self.queue_pending.take() {
            if left <= ms {
                self.data(data::QUEUE);
            } else {
                self.queue_pending = Some(left - ms);
            }
        }
    }

    pub fn state(&self) -> PlayerState {
        let Some(q) = self.current() else { return PlayerState::default() };
        PlayerState {
            source: if self.playing { Source::Playing } else { Source::Paused },
            position_ms: self.position,
            duration_ms: q.duration,
            queue_pos: u32::try_from(self.pos).unwrap_or(0),
            queue_len: u32::try_from(self.queue.len()).unwrap_or(0),
            a_ms: self.a,
            b_ms: self.b,
            buffer_pct: 87,
            cpu_pct: 23,
            art: self.art_for.map_or(0, |s| s.max(1)),
        }
    }

    pub fn track(&self) -> Option<TrackInfo<'_>> {
        let q = self.current()?;
        if orpheus_midi::extension(&q.path).is_some() {
            // Synthesised at the DAC rate; no bit rate, no ReplayGain.
            return Some(TrackInfo {
                title: &q.title,
                artist: &q.artist,
                album: &q.album,
                file: q.file,
                path: &q.path,
                size: q.size,
                codec: "MIDI",
                sample_rate: 48_000,
                bits: 16,
                channels: 2,
                bitrate_kbps: 0,
                gain_cdb: None,
            });
        }
        Some(TrackInfo {
            title: &q.title,
            artist: &q.artist,
            album: &q.album,
            file: q.file,
            path: &q.path,
            size: q.size,
            codec: "MP3",
            sample_rate: 44_100,
            bits: 16,
            channels: 2,
            bitrate_kbps: 320,
            gain_cdb: (self.pos == 0).then_some(-620),
        })
    }

    pub fn queue_entry(&self, i: u32) -> Option<QueueEntry<'_>> {
        if self.queue_pending.is_some() {
            return None;
        }
        let q = self.queue.get(i as usize)?;
        Some(QueueEntry { title: &q.title, artist: &q.artist, duration_ms: q.duration })
    }

    pub fn art(&self, size: ArtSize) -> Option<&[u16]> {
        self.art_for?;
        Some(match size {
            ArtSize::Small => &self.art_small,
            ArtSize::Full => &self.art_full,
        })
    }

    /// `PictureService::show_picture`: decoded at once into the screen.
    /// Picture files are read from a second, read-only mount of the card
    /// (the service call cannot borrow the browsing volume mutably).
    pub fn show_picture(&self, g: &mut Gfx<'_>, r: Rect, req: PicReq) -> PicState {
        let (w, h) = (u16::try_from(r.w).unwrap_or(0), u16::try_from(r.h).unwrap_or(0));
        match req.src {
            PicSource::Art(size) => match self.art(size) {
                Some(px) if px.len() == usize::from(w) * usize::from(h) => {
                    g.image(r.x, r.y, r.w, px);
                    let n = u32::from(size.px().unsigned_abs());
                    PicState::Shown(PicInfo { width: n, height: n, format: None, reduced: false })
                }
                _ => PicState::Nothing,
            },
            // The decoder memory is the audio decoder's while a track is
            // loaded (as on the player).
            PicSource::File(_) if self.current().is_some() => PicState::Busy,
            PicSource::File(f) => {
                let Some(data) = self.file_paths.get(&f).and_then(|p| read_card_file(&self.base, p)) else {
                    return PicState::Failed(PicError::Io);
                };
                match pictures::decode(&data, w, h, req.fill, req.turns) {
                    Ok((px, info)) => {
                        g.image(r.x, r.y, r.w, &px);
                        PicState::Shown(info)
                    }
                    Err(e) => PicState::Failed(e),
                }
            }
        }
    }

    pub fn lyrics_count(&self) -> u32 {
        self.current().map_or(0, |q| u32::try_from(q.lyrics.len()).unwrap_or(0))
    }

    pub fn lyric(&self, i: u32) -> Option<LyricLine<'_>> {
        let (t, s) = self.current()?.lyrics.get(i as usize)?;
        Some(LyricLine { time_ms: *t, text: s })
    }

    pub fn levels(&self) -> Levels {
        let t = self.position as f32 / 1000.0;
        let wave = |f: f32, ph: f32| (t * f + ph).sin() * 0.5 + 0.5;
        let mut bands = [0u8; 16];
        for (i, b) in bands.iter_mut().enumerate() {
            let fi = i as f32;
            let base = 1.0 - fi / 20.0;
            *b = ((base * (0.55 + 0.45 * wave(1.3 + fi * 0.37, fi))) * 230.0) as u8;
        }
        let l = (170.0 + 70.0 * wave(2.1, 0.0)) as u8;
        let r = (160.0 + 80.0 * wave(1.7, 1.0)) as u8;
        Levels { peak: [l, r], hold: [l.saturating_add(14), r.saturating_add(10)], bands }
    }

    pub fn resume_title(&self) -> Option<&str> {
        self.resume.as_ref().map(|q| q.title.as_str())
    }

    /// Set a resume point (as after a power cycle) for scripts.
    pub fn set_resume(&mut self, path: &str) {
        let f = self.file_paths.iter().find(|(_, p)| p.as_str() == path).map(|(f, _)| *f);
        let item = self.load_item(f.unwrap_or_default(), path);
        self.resume = Some(item);
        self.queue.clear();
        self.playing = false;
    }

    /// Paths of library tracks (every folder is read once).
    fn paths_of(&mut self, locs: &[orpheus_fs::Locator]) -> Vec<(FileRef, String)> {
        if !self.learnt_all {
            self.learnt_all = true;
            let mut q = vec![DirId::ROOT];
            while let Some(d) = q.pop() {
                for e in self.read_dir(d) {
                    if e.kind == EntryKind::Dir && !e.hidden {
                        q.push(e.child);
                    }
                }
            }
        }
        locs.iter()
            .filter_map(|l| {
                let f = FileRef { dir: l.dir, index: l.index };
                Some((f, self.file_paths.get(&f)?.clone()))
            })
            .collect()
    }

    /// `set:library=…`: `ready` builds the library from the card.
    pub fn set_library(&mut self, st: LibraryState) {
        self.library = st;
        if st == LibraryState::Ready && self.lib.is_none() {
            self.lib = SimLib::build(self.vol.as_mut().expect("mounted"));
            if self.lib.is_none() {
                self.library = LibraryState::Missing;
            }
        }
    }

    pub fn lib_open(&mut self, list: LibList) -> LibListing {
        let l = self.lib.as_mut().map(|lib| lib.open(self.vol.as_mut().expect("mounted"), list));
        l.unwrap_or(LibListing { list, count: 0, complete: true })
    }

    pub fn lib_listing(&self) -> LibListing {
        self.lib
            .as_ref()
            .map_or(LibListing { list: LibList::Artists, count: 0, complete: false }, SimLib::listing)
    }

    pub fn lib_row(&self, i: u32) -> Option<LibRow<'_>> {
        self.lib.as_ref()?.row(i)
    }

    pub fn lib_position(&self, id: u32) -> Option<u32> {
        self.lib.as_ref()?.position(id)
    }

    pub fn lib_letter(&self, i: u32) -> Option<char> {
        self.lib.as_ref()?.letter(i)
    }

    pub fn lib_seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        self.lib.as_ref()?.seek_letter(from, forward)
    }

    pub fn lib_now(&self) -> Option<LibNow> {
        self.lib.as_ref()?.now(self.current()?.file)
    }

    /// Make sure the paths under `dir_path` are known (scripts).
    pub fn learn(&mut self, dir_path: &str) {
        let mut d = DirId::ROOT;
        let mut path = String::new();
        let _ = self.read_dir(d);
        for part in dir_path.split('/').filter(|s| !s.is_empty()) {
            path = format!("{path}/{part}");
            if let Some((k, _)) = self.dir_paths.iter().find(|(_, p)| **p == path) {
                d = DirId(*k);
                let _ = self.read_dir(d);
            }
        }
    }

    pub fn bookmark_count(&self) -> u32 {
        u32::try_from(self.bookmarks.len()).unwrap_or(0)
    }

    pub fn bookmark(&self, i: u32) -> Option<Bookmark<'_>> {
        let (t, p) = self.bookmarks.get(i as usize)?;
        Some(Bookmark { title: t, position_ms: *p })
    }

    #[must_use]
    pub fn active(&self) -> bool {
        !self.queue.is_empty()
    }

    pub fn title_artist(&self) -> Option<(&str, &str, bool, u16)> {
        let q = self.current()?;
        let prog = u16::try_from(u64::from(self.position) * 1000 / u64::from(q.duration.max(1))).unwrap_or(0);
        Some((&q.title, &q.artist, self.playing, prog))
    }

    /// The dir id of a path learnt so far.
    pub fn dir_id(&self, path: &str) -> Option<DirId> {
        self.dir_paths.iter().find(|(_, p)| p.as_str() == path).map(|(k, _)| DirId(*k))
    }

    /// The path of a file listed before (USB Drive).
    #[must_use]
    pub fn file_path(&self, f: FileRef) -> Option<String> {
        self.file_paths.get(&f).cloned()
    }

    pub fn file_ref(&self, path: &str) -> Option<FileRef> {
        self.file_paths.iter().find(|(_, p)| p.as_str() == path).map(|(f, _)| *f)
    }
}
