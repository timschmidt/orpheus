//! The simulator's music library: `orpheus-library` built for real over the
//! card image (through the same `card` adapters as the firmware, in
//! suspended slices), and every list answered by its queries. The host
//! keeps a whole list in memory; the screens still see it through the
//! windowed `LibraryService`.

use std::collections::HashMap;
use std::task::Poll;

use orpheus_apps::services::{FileRef, LibKind, LibList, LibListing, LibNow, LibRow, RECENT_ROWS};
use orpheus_fs::host::{OverlayDisk, block_on};
use orpheus_fs::{DirEntry, Locator, Volume};
use orpheus_index::card::{CardIo, Handles, Mode};
use orpheus_library::card::{CardTags, TagState};
use orpheus_library::{Builder, Config, Item, LIBRARY_DIR, Library, List, Progress, Suspended};

pub type Vol = Volume<'static, OverlayDisk<'static>>;
type Io<'a, 'c> = CardIo<'a, 'static, 'c, OverlayDisk<'static>>;

/// Run `f` in one session on the library's files.
fn with_io<R>(vol: &mut Vol, mode: Mode, f: impl FnOnce(&Io<'_, '_>) -> R) -> R {
    let mut h = Handles::new();
    let mut f = Some(f);
    block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new_in(vol, cx, &mut h, mode, LIBRARY_DIR);
        let r = f.take().map(|f| f(&io));
        let _ = io.close_all();
        r.map_or(Poll::Pending, Poll::Ready)
    }))
}

#[derive(Clone, Debug)]
struct SimRow {
    kind: LibKind,
    id: u32,
    name: String,
    duration_ms: u32,
}

pub struct SimLib {
    lib: Library,
    listing: LibListing,
    rows: Vec<SimRow>,
    letters: Vec<Option<char>>,
    now: HashMap<FileRef, LibNow>,
}

impl SimLib {
    /// Build the library (as the firmware does: steps in sessions that
    /// are suspended in between) and open it.
    #[must_use]
    pub fn build(vol: &mut Vol) -> Option<SimLib> {
        let cfg = Config { serial: vol.serial(), ..Config::default() };
        let mut scratch = vec![0u8; 16 * 1024];
        let mut st: Option<Suspended> = None;
        let lib = loop {
            let mut e = DirEntry::new();
            let mut tags = TagState::new();
            let out = with_io(vol, Mode::Write { alt: false }, |io| {
                let mut b = match st.take() {
                    Some(s) => s.resume(&mut scratch)?,
                    None => Builder::new(&mut scratch, cfg)?,
                };
                for _ in 0..64 {
                    let mut src = CardTags::new(io, &mut tags, &mut e);
                    if let Progress::Done(l) = b.step(&mut io.store(), &mut src, 64).ok()? {
                        return Some(Ok(l));
                    }
                }
                Some(Err(b.suspend(&mut io.store()).ok()?))
            })?;
            match out {
                Ok(l) => break l,
                Err(s) => st = Some(s),
            }
        };
        let mut now = HashMap::new();
        with_io(vol, Mode::Read, |io| {
            for id in 0..lib.tracks() {
                if let Ok(t) = lib.track(&mut io.store(), id) {
                    let f = FileRef { dir: t.r.dir, index: t.r.index };
                    now.insert(f, LibNow { track: id, album: t.album_id, artist: t.artist_id });
                }
            }
        });
        Some(SimLib {
            lib,
            listing: LibListing { list: LibList::Artists, count: 0, complete: false },
            rows: Vec::new(),
            letters: Vec::new(),
            now,
        })
    }

    /// Make `list` current (all its rows, Recently added capped).
    pub fn open(&mut self, vol: &mut Vol, list: LibList) -> LibListing {
        if self.listing.list == list && self.listing.complete {
            return self.listing;
        }
        let lib = self.lib;
        let l = List::from(list);
        let (rows, letters) = with_io(vol, Mode::Read, |io| {
            let mut s = io.store();
            let mut n = lib.len(&mut s, l).unwrap_or(0);
            if list == LibList::Recent {
                n = n.min(RECENT_ROWS);
            }
            let mut rows = Vec::new();
            let mut letters = Vec::new();
            let mut buf = [Item::default(); 8];
            while (rows.len() as u32) < n {
                let k = lib.rows(&mut s, l, rows.len() as u32, &mut buf).unwrap_or(0);
                if k == 0 {
                    break;
                }
                for it in &buf[..k.min((n as usize) - rows.len())] {
                    let mut b = [0u8; 128];
                    let name = lib.text(&mut s, it.name, &mut b).unwrap_or("?").to_string();
                    letters.push(lib.letter(&mut s, l, rows.len() as u32).ok().flatten());
                    rows.push(SimRow { kind: it.kind.into(), id: it.id, name, duration_ms: it.duration_ms });
                }
            }
            (rows, letters)
        });
        self.listing = LibListing { list, count: rows.len() as u32, complete: true };
        self.rows = rows;
        self.letters = letters;
        self.listing
    }

    #[must_use]
    pub fn listing(&self) -> LibListing {
        self.listing
    }

    #[must_use]
    pub fn row(&self, i: u32) -> Option<LibRow<'_>> {
        let r = self.rows.get(i as usize)?;
        Some(LibRow { kind: r.kind, id: r.id, name: &r.name, duration_ms: r.duration_ms })
    }

    #[must_use]
    pub fn position(&self, id: u32) -> Option<u32> {
        self.rows.iter().position(|r| r.id == id).map(|p| p as u32)
    }

    #[must_use]
    pub fn letter(&self, i: u32) -> Option<char> {
        self.letters.get(i as usize).copied().flatten()
    }

    /// Letter jump over the loaded letters (`ListSource::seek_letter`).
    #[must_use]
    pub fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        let at = |i: usize| self.letters.get(i).copied().flatten();
        let cur = at(from as usize)?;
        let from = from as usize;
        if forward {
            let i = (from + 1..self.letters.len()).find(|&i| at(i).is_some_and(|c| c != cur))?;
            return Some((i as u32, at(i)?));
        }
        let mut i = (0..from).rev().find(|&i| at(i).is_some_and(|c| c != cur))?;
        let l = at(i)?;
        while i > 0 && at(i - 1) == Some(l) {
            i -= 1;
        }
        Some((i as u32, l))
    }

    /// The locators of a track list's queue: up to `max` tracks around
    /// row `start`; and the queue position of `start`.
    pub fn queue(&self, vol: &mut Vol, list: LibList, start: Option<u32>, max: u32) -> (Vec<Locator>, usize) {
        let (lib, l) = (self.lib, List::from(list));
        with_io(vol, Mode::Read, |io| {
            let mut s = io.store();
            let n = lib.len(&mut s, l).unwrap_or(0);
            let at = start.unwrap_or(0).min(n.saturating_sub(1));
            let first = at.saturating_sub(max / 2).min(n.saturating_sub(max));
            let mut v = Vec::new();
            let _ = lib.queue(&mut s, l, first, max, |t| {
                v.push(t.locator());
                true
            });
            (v, (at - first) as usize)
        })
    }

    /// Shuffle all: a uniform sample of `n` tracks.
    pub fn sample(&self, vol: &mut Vol, seed: u64, n: u32) -> Vec<Locator> {
        let lib = self.lib;
        with_io(vol, Mode::Read, |io| {
            let mut v = Vec::new();
            let _ = lib.sample(&mut io.store(), seed, n, |t| {
                v.push(t.locator());
                true
            });
            v
        })
    }

    /// The current track's place in the library.
    #[must_use]
    pub fn now(&self, f: FileRef) -> Option<LibNow> {
        self.now.get(&f).copied()
    }
}
