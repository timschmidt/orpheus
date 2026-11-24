//! Card-backed services for the UI (WP FS-2 / AU-5 firmware part): folder
//! listings, the text viewer window and a folder play queue.
//!
//! The task that owns the mounted volume (`audio`) serves [`Req`]s, also
//! between ring refills while a track plays. Results go into the shared
//! windows below; the UI copies a window into `Fw` when the matching
//! [`READY`] tag arrives and turns it into `Event::Data`.
//!
//! Not yet: sorting (folders are listed in on-disk order with
//! `sorted: false` until the directory index of AU-12 exists), tags and art
//! in the track info, shuffle, the queue list, persistence of text
//! positions.

use core::cell::RefCell;

use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use yoton_apps::browse::{EntryKind, is_hidden_name, kind_of};
use yoton_apps::services::{BrowseFilter, DateTime, DirId, FileRef, Listing};
use yoton_ui::event::Source;

/// Rows per listing window.
pub const WINDOW: u32 = 32;
/// Bytes kept of a name (cut at a character boundary).
pub const NAME: usize = 80;
/// Bytes per text-viewer window.
pub const TEXT_WIN: usize = 1536;
/// Listing checkpoints (one per `WINDOW` rows): folders up to 2048 rows
/// seek directly, larger ones scan from the last checkpoint.
pub const CHECKPOINTS: usize = 64;
/// Play-queue length (one folder).
pub const QUEUE: usize = 256;

pub mod tag {
    pub use yoton_apps::services::data::{BROWSE, QUEUE, TEXT};
}

#[derive(Clone, Copy, Debug)]
pub enum Req {
    /// List `dir`; `entry` is the folder's own directory entry (None for the
    /// root).
    Browse {
        dir: DirId,
        entry: Option<FileRef>,
        filter: BrowseFilter,
    },
    /// Move the listing window to start at or before `first`.
    Want {
        first: u32,
    },
    /// Text window of `file` from `offset`.
    Text {
        file: FileRef,
        offset: u32,
    },
    /// Queue the audio files of a folder and play from `start`.
    PlayDir {
        entry: Option<FileRef>,
        start: Option<FileRef>,
    },
    /// Relative skip in the queue (Next/Prev) or absolute jump.
    Skip(i32),
    Jump(u32),
    Stop,
    /// Hand the card to the USB task (stops playback first).
    Release,
}

pub static REQS: Channel<CriticalSectionRawMutex, Req, 4> = Channel::new();
/// Data-ready tags for the UI.
pub static READY: Channel<CriticalSectionRawMutex, u16, 4> = Channel::new();

/// Send a request; when the queue is full the oldest kind of request is
/// simply retried by the screen (it asks again on its next draw).
pub fn request(r: Req) {
    let _ = REQS.try_send(r);
}

fn ready(t: u16) {
    let _ = READY.try_send(t);
}

// ------------------------------------------------------------------ names

#[derive(Clone, Copy)]
pub struct Name {
    buf: [u8; NAME],
    len: u8,
}

impl Name {
    pub const EMPTY: Name = Name { buf: [0; NAME], len: 0 };

    #[must_use]
    pub fn new(s: &str) -> Name {
        let mut n = s.len().min(NAME);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        let mut buf = [0; NAME];
        buf[..n].copy_from_slice(&s.as_bytes()[..n]);
        Name { buf, len: n as u8 }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("")
    }
}

// ---------------------------------------------------------------- windows

#[derive(Clone, Copy)]
pub struct Row {
    pub name: Name,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: DateTime,
    pub file: FileRef,
    pub child: DirId,
}

impl Row {
    pub const EMPTY: Row = Row {
        name: Name::EMPTY,
        kind: EntryKind::Other,
        size: 0,
        modified: DateTime { year: 2000, month: 1, day: 1, hour: 0, minute: 0, second: 0, weekday: 5 },
        file: FileRef { dir: 0, index: 0 },
        child: DirId(0),
    };
}

#[derive(Clone, Copy)]
pub struct BrowseWin {
    pub listing: Listing,
    pub first: u32,
    pub n: u32,
    pub rows: [Row; WINDOW as usize],
}

impl BrowseWin {
    pub const EMPTY: BrowseWin = BrowseWin {
        listing: Listing { dir: DirId(u32::MAX), count: 0, sorted: false, complete: false },
        first: 0,
        n: 0,
        rows: [Row::EMPTY; WINDOW as usize],
    };

    #[must_use]
    pub fn row(&self, i: u32) -> Option<&Row> {
        if i < self.first || i >= self.first + self.n {
            return None;
        }
        self.rows.get((i - self.first) as usize)
    }
}

#[derive(Clone, Copy)]
pub struct TextWin {
    pub file: FileRef,
    /// `u32::MAX` while nothing is loaded.
    pub offset: u32,
    pub len: u32,
    pub buf: [u8; TEXT_WIN],
}

impl TextWin {
    pub const EMPTY: TextWin =
        TextWin { file: FileRef { dir: 0, index: 0 }, offset: u32::MAX, len: 0, buf: [0; TEXT_WIN] };
}

#[derive(Clone, Copy)]
pub struct PlayerSnap {
    pub source: Source,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub queue_pos: u32,
    pub queue_len: u32,
    pub title: Name,
    pub file: FileRef,
    pub size: u64,
    pub codec: &'static str,
    pub rate: u32,
    pub channels: u8,
    pub bits: u8,
}

impl PlayerSnap {
    pub const IDLE: PlayerSnap = PlayerSnap {
        source: Source::Idle,
        position_ms: 0,
        duration_ms: 0,
        queue_pos: 0,
        queue_len: 0,
        title: Name::EMPTY,
        file: FileRef { dir: 0, index: 0 },
        size: 0,
        codec: "",
        rate: 0,
        channels: 0,
        bits: 0,
    };
}

pub static BROWSE: Mutex<RefCell<BrowseWin>> = Mutex::new(RefCell::new(BrowseWin::EMPTY));
pub static TEXT: Mutex<RefCell<TextWin>> = Mutex::new(RefCell::new(TextWin::EMPTY));
pub static PLAYER: Mutex<RefCell<PlayerSnap>> = Mutex::new(RefCell::new(PlayerSnap::IDLE));

pub fn player_update(f: impl FnOnce(&mut PlayerSnap)) {
    critical_section::with(|cs| f(&mut PLAYER.borrow(cs).borrow_mut()));
}

// ------------------------------------------------------------- the server

use yoton_bsp::card::Card;
use yoton_fs::{DirCursor, DirEntry, File, Locator, Volume};

pub type Vol = Volume<'static, Card>;

fn loc(f: FileRef) -> Locator {
    Locator { dir: f.dir, index: f.index }
}

fn fref(l: Locator) -> FileRef {
    FileRef { dir: l.dir, index: l.index }
}

fn date(d: yoton_fs::DateTime) -> DateTime {
    DateTime {
        year: d.year,
        month: d.month,
        day: d.day,
        hour: d.hour,
        minute: d.minute,
        second: d.second,
        weekday: 0,
    }
}

/// Server state, owned by the card task.
pub struct Server {
    dir: DirId,
    entry: Option<FileRef>,
    filter: BrowseFilter,
    count: u32,
    checkpoints: [u32; CHECKPOINTS],
    text: [u8; TEXT_WIN],
    pub queue: [Locator; QUEUE],
    pub qlen: u32,
    pub qpos: u32,
    /// Set while a track plays: what to do when it stops early.
    pub pending: Option<Req>,
}

impl Server {
    pub const fn new() -> Self {
        Server {
            dir: DirId(u32::MAX),
            entry: None,
            filter: BrowseFilter {
                sort: yoton_apps::services::SortBy::Name,
                hidden: false,
                audio_only: false,
            },
            count: 0,
            checkpoints: [0; CHECKPOINTS],
            text: [0; TEXT_WIN],
            queue: [Locator { dir: 0, index: 0 }; QUEUE],
            qlen: 0,
            qpos: 0,
            pending: None,
        }
    }

    async fn cursor(vol: &mut Vol, entry: Option<FileRef>) -> Option<DirCursor> {
        match entry {
            None => Some(vol.root()),
            Some(f) => {
                let mut e = DirEntry::new();
                vol.open_locator(loc(f), &mut e).await.ok()?;
                vol.dir_cursor(&e).ok()
            }
        }
    }

    fn matches(filter: BrowseFilter, e: &DirEntry) -> bool {
        let name = e.name();
        if name == "." || name == ".." {
            return false;
        }
        if !filter.hidden && (is_hidden_name(name) || e.attributes() & 0x06 != 0) {
            return false;
        }
        !filter.audio_only
            || matches!(kind_of(name, e.is_dir()), EntryKind::Dir | EntryKind::Audio | EntryKind::Playlist)
    }

    fn row(e: &DirEntry) -> Row {
        let name = e.name();
        let dir = e.is_dir();
        Row {
            name: Name::new(name),
            kind: kind_of(name, dir),
            size: e.size(),
            modified: date(e.modified()),
            file: fref(e.locator()),
            child: DirId(if dir { e.first_cluster() } else { 0 }),
        }
    }

    /// Serve one request. Returns false for requests that belong to the
    /// player loop (they are kept in `pending`).
    pub async fn serve(&mut self, vol: &mut Vol, r: Req) -> bool {
        match r {
            Req::Browse { dir, entry, filter } => {
                self.browse(vol, dir, entry, filter).await;
                true
            }
            Req::Want { first } => {
                self.window(vol, first).await;
                true
            }
            Req::Text { file, offset } => {
                self.text(vol, file, offset).await;
                true
            }
            _ => {
                self.pending = Some(r);
                false
            }
        }
    }

    async fn browse(&mut self, vol: &mut Vol, dir: DirId, entry: Option<FileRef>, filter: BrowseFilter) {
        self.dir = dir;
        self.entry = entry;
        self.filter = filter;
        self.count = 0;
        let listing_of = |count, complete| Listing { dir, count, sorted: false, complete };
        let Some(mut cur) = Self::cursor(vol, entry).await else {
            critical_section::with(|cs| {
                let mut w = BROWSE.borrow(cs).borrow_mut();
                w.listing = listing_of(0, true);
                w.first = 0;
                w.n = 0;
            });
            ready(tag::BROWSE);
            return;
        };
        let mut e = DirEntry::new();
        let mut n = 0u32;
        loop {
            let before = cur.index();
            match vol.read_dir(&mut cur, &mut e).await {
                Ok(true) => {}
                _ => break,
            }
            if !Self::matches(filter, &e) {
                continue;
            }
            if n % WINDOW == 0 && ((n / WINDOW) as usize) < CHECKPOINTS {
                self.checkpoints[(n / WINDOW) as usize] = before;
            }
            if n < WINDOW {
                let row = Self::row(&e);
                critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
            }
            n += 1;
        }
        self.count = n;
        critical_section::with(|cs| {
            let mut w = BROWSE.borrow(cs).borrow_mut();
            w.listing = listing_of(n, true);
            w.first = 0;
            w.n = n.min(WINDOW);
        });
        ready(tag::BROWSE);
    }

    async fn window(&mut self, vol: &mut Vol, first: u32) {
        let start = first.saturating_sub(WINDOW / 4).min(self.count.saturating_sub(1));
        let k = ((start / WINDOW) as usize).min(CHECKPOINTS - 1);
        let Some(mut cur) = Self::cursor(vol, self.entry).await else { return };
        if vol.seek_dir(&mut cur, self.checkpoints[k]).await.is_err() {
            return;
        }
        let mut skip = start - k as u32 * WINDOW;
        let mut e = DirEntry::new();
        let mut n = 0u32;
        while n < WINDOW {
            match vol.read_dir(&mut cur, &mut e).await {
                Ok(true) => {}
                _ => break,
            }
            if !Self::matches(self.filter, &e) {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let row = Self::row(&e);
            critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
            n += 1;
        }
        critical_section::with(|cs| {
            let mut w = BROWSE.borrow(cs).borrow_mut();
            w.first = start;
            w.n = n;
        });
        ready(tag::BROWSE);
    }

    async fn text(&mut self, vol: &mut Vol, file: FileRef, offset: u32) {
        let mut e = DirEntry::new();
        let mut got = 0usize;
        if vol.open_locator(loc(file), &mut e).await.is_ok()
            && let Ok(mut f) = vol.open_entry(&e)
        {
            f.set_position(u64::from(offset));
            while got < TEXT_WIN {
                match vol.read(&mut f, &mut self.text[got..]).await {
                    Ok(0) | Err(_) => break,
                    Ok(k) => got += k,
                }
            }
        }
        let text = &self.text;
        critical_section::with(|cs| {
            let mut w = TEXT.borrow(cs).borrow_mut();
            w.file = file;
            w.offset = offset;
            w.len = got as u32;
            w.buf[..got].copy_from_slice(&text[..got]);
        });
        ready(tag::TEXT);
    }

    /// Fill the queue with the audio files of a folder (on-disk order).
    pub async fn build_queue(&mut self, vol: &mut Vol, entry: Option<FileRef>, start: Option<FileRef>) {
        self.qlen = 0;
        self.qpos = 0;
        let Some(mut cur) = Self::cursor(vol, entry).await else { return };
        let mut e = DirEntry::new();
        while (self.qlen as usize) < QUEUE {
            match vol.read_dir(&mut cur, &mut e).await {
                Ok(true) => {}
                _ => break,
            }
            if e.is_dir() || kind_of(e.name(), false) != EntryKind::Audio || is_hidden_name(e.name()) {
                continue;
            }
            let l = e.locator();
            if start.is_some_and(|s| loc(s) == l) {
                self.qpos = self.qlen;
            }
            self.queue[self.qlen as usize] = l;
            self.qlen += 1;
        }
        ready(tag::QUEUE);
    }

    /// Open queue item `qpos`: its file, name and size.
    pub async fn open_current(&mut self, vol: &mut Vol, e: &mut DirEntry) -> Option<File> {
        let l = *self.queue.get(self.qpos as usize)?;
        vol.open_locator(l, e).await.ok()?;
        vol.open_entry(e).ok()
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
