//! The music library in the card task (AU-12, `orpheus-library`, files in
//! `/.orpheus/library/`): opened at mount, built in the background while
//! the card task is idle, and queried for the Music screens, the play
//! queue, Shuffle all and "Go to album / artist".
//!
//! **When a build runs.** A build is *wanted* from a time on (`LibState::
//! due`): 10 s after a mount that finds no library, 5 s after the card
//! comes back from the USB card reader (an update: unchanged folders are
//! copied without a tag read), at once on Settings › Storage › Rebuild
//! library. It runs only in the card task's idle loop (no track loaded,
//! after a waiting directory-index build), in steps of [`BUDGET`] units
//! (a file whose tags are read counts 8) on the last [`SCRATCH`] bytes of
//! the decoder arena, sleeping as long as each step took (≥ 2 ms) so
//! thread mode keeps at least half the CPU. Any request, command or
//! player request suspends it at the next step; playback then takes the
//! arena. Suspended, only `orpheus_library::Suspended` (1,088 B) stays, in
//! [`SLOT`] — the last [`STATE`] bytes of the decoder arena, which no
//! playback uses; the next idle moment resumes it. A card hand-over abandons a
//! build (the host may change anything); the update after it starts
//! again (the builder empties its files when it starts).
//!
//! **Stack.** Nothing large is built in the card task's async functions:
//! the build state lives in [`SLOT`] and is driven where it is
//! (`Suspended::step_in` / `pause_in`: no copy), its scratch is the arena,
//! the steps and queries are `#[inline(never)]` functions
//! called inside `poll_fn` (their frames, and the tag parser's, are on top
//! of the card task's poll frame, not in it).
//!
//! **Lists** share the browse window (`media::BROWSE`): a library list is
//! the listing of the pseudo directory [`list_dir`] (tag `0xF` in the top
//! nibble, never a cluster number of a real card), its rows carry the item
//! kind in `file.dir`, the table row in `file.index` and a track's playing
//! time in `size`; the window arrives with `tag::LIBRARY`.

use core::future::poll_fn;
use core::sync::atomic::{AtomicU8, Ordering};
use core::task::{Context, Poll};

use atj2157_hal::time;
use embassy_futures::select::{select, select3};
use embassy_time::Timer;
use orpheus_apps::browse::EntryKind;
use orpheus_apps::services::{DirId, FileRef, LibKind, LibList, LibNow, LibraryState, Listing, RECENT_ROWS};
use orpheus_fs::{DirEntry, Locator};
use orpheus_index::LetterTable;
use orpheus_index::card::{CardIo, Handles, Mode};
use orpheus_library::card::{CardTags, TagState};
use orpheus_library::{Config, Item, ItemKind, LIBRARY_DIR, Library, List, Progress, Suspended};

use crate::media::{self, Io, QUEUE, Server, Vol, WINDOW};

/// Scratch lent to a build: the end of the idle decoder arena. The scan
/// uses up to 3,456 B of buffers plus a 16 KiB tag window; the sorts the
/// rest for runs.
pub const SCRATCH: usize = 20 * 1024;
/// Work units per step (folder entries; a file whose tags are read: 8;
/// sorted items or 512-byte blocks while indexing).
pub const BUDGET: u32 = 32;
/// After a mount without a library, and after a card-reader session.
const START_DELAY_MS: u64 = 10_000;
const UPDATE_DELAY_MS: u64 = 5_000;

/// The pseudo-directory tag of library lists in the shared listing.
const LIST_TAG: u32 = 0xF000_0000;

/// The Tag code page setting (0x0208: Western, Central, Cyrillic, …), set
/// by the UI; read when a build starts.
pub static CODE_PAGE: AtomicU8 = AtomicU8::new(0);

/// The listing directory of a library list.
#[must_use]
pub const fn list_dir(l: LibList) -> DirId {
    DirId(LIST_TAG | l.code())
}

/// The library list a listing directory is, if it is one.
#[must_use]
pub const fn dir_list(d: DirId) -> Option<LibList> {
    if d.0 & 0xF000_0000 == LIST_TAG { LibList::from_code(d.0 & !LIST_TAG) } else { None }
}

/// A row's kind in `Row::file.dir`.
#[must_use]
pub const fn kind_code(k: ItemKind) -> u32 {
    match k {
        ItemKind::Artist => 0,
        ItemKind::Album => 1,
        ItemKind::Genre => 2,
        ItemKind::GenreArtist => 3,
        ItemKind::GenreAlbum => 4,
        ItemKind::Track => 5,
    }
}

#[must_use]
pub const fn kind_of_code(c: u32) -> LibKind {
    match c {
        0 => LibKind::Artist,
        1 => LibKind::Album,
        2 => LibKind::Genre,
        3 => LibKind::GenreArtist,
        4 => LibKind::GenreAlbum,
        _ => LibKind::Track,
    }
}

fn now_ms() -> u64 {
    time::now_us() / 1000
}

// ------------------------------------------------------------ the state

/// The card task's library state (in `Server`).
pub struct LibState {
    /// The newest complete library on the card.
    pub lib: Option<Library>,
    /// The library list the UI shows (its rows are in the browse window).
    pub list: Option<LibList>,
    /// A build is wanted from this time (ms since boot) on.
    pub due: Option<u64>,
    /// Audio files seen by the build in progress.
    files: u32,
    /// Changes when another library is opened.
    generation: u32,
}

impl LibState {
    pub const fn new() -> LibState {
        LibState { lib: None, list: None, due: None, files: 0, generation: 0 }
    }

    /// A build is started or suspended.
    fn building(&self) -> bool {
        // SAFETY: the card task's state (see `slot`).
        self.due.is_some() || slot_ref().is_some_and(|s| s.is_some())
    }

    /// The state for the UI.
    pub fn publish(&self) {
        let st = match self.lib {
            Some(_) => LibraryState::Ready,
            None if self.building() => LibraryState::Building { files: self.files },
            None => LibraryState::Missing,
        };
        let g = self.generation;
        media::player_update(|p| {
            p.lib_state = st;
            p.lib_gen = g;
        });
    }

    fn opened(&mut self, l: Option<Library>) {
        self.lib = l;
        self.generation = self.generation.wrapping_add(1);
        if l.is_none() {
            self.list = None;
        }
    }
}

impl Default for LibState {
    fn default() -> Self {
        Self::new()
    }
}

/// The build's state, in place (`None`: no build). Between the steps of
/// one [`slot`] the scratch holds the build's buffers; after `pause_in`
/// (end of the slot) nothing but this state is needed.
type Slot = Option<Suspended>;

/// Bytes at the end of the decoder arena that hold the build's [`Slot`]
/// (the arena the card task lends to everything else is shorter by this
/// much). No playback uses that part: the largest decoder need (Opus,
/// 59,488 B) plus the 2 KiB kept free while playing leave 4 KiB of the
/// 64 KiB unused.
pub const STATE: usize = (core::mem::size_of::<Slot>() + 7) & !7;

/// Where the [`Slot`] is ([`init`]); only the card task touches it.
static mut SLOT: *mut Slot = core::ptr::null_mut();

/// At the start of the card task: the build state goes into `mem` (the
/// last [`STATE`] bytes of the decoder arena, 8-byte aligned, never lent
/// to anything else).
pub fn init(mem: &'static mut [u8]) {
    let p = mem.as_mut_ptr().cast::<Slot>();
    if mem.len() >= core::mem::size_of::<Slot>() && p.align_offset(core::mem::align_of::<Slot>()) == 0 {
        // SAFETY: `mem` is large and aligned enough, and from now on used
        // only through `SLOT` (by the card task).
        unsafe {
            p.write(None);
            *(&raw mut SLOT) = p;
        }
    }
}

/// The build state; `None` before [`init`].
fn slot_ref() -> Option<&'static mut Slot> {
    // SAFETY: see `SLOT`; each caller drops the borrow before the next.
    unsafe { (*(&raw const SLOT)).as_mut() }
}

/// One session on the library's files (closed after). Not inlined: the
/// open-file table is in this frame, not in the card task's poll frame.
#[inline(never)]
fn session<R>(vol: &mut Vol, cx: &mut Context<'_>, mode: Mode, f: impl FnOnce(&Io<'_, '_>) -> R) -> R {
    let mut h = Handles::new();
    let io = CardIo::new_in(vol, cx, &mut h, mode, LIBRARY_DIR);
    let r = f(&io);
    let _ = io.close_all();
    r
}

/// Run `f` in one session on the library's files. Keep `R` small (it
/// passes through the card task's frame).
pub async fn with_lib_io<R>(vol: &mut Vol, mode: Mode, f: impl FnOnce(&Io<'_, '_>) -> R) -> R {
    let mut f = Some(f);
    poll_fn(|cx| match f.take() {
        Some(f) => Poll::Ready(session(vol, cx, mode, f)),
        None => Poll::Pending,
    })
    .await
}

/// Open the newest complete library into `st`; whether there is one.
#[inline(never)]
fn open_into(io: &Io<'_, '_>, st: &mut LibState) -> bool {
    st.opened(Library::open(&mut io.store()).ok().flatten());
    st.lib.is_some()
}

/// After a mount: open the library; without one, build it soon.
pub async fn mounted(vol: &mut Vol, srv: &mut Server) {
    let st = &mut srv.lib;
    if !with_lib_io(vol, Mode::Read, |io| open_into(io, st)).await {
        srv.lib.due = Some(now_ms() + START_DELAY_MS);
    }
    srv.lib.publish();
}

/// Before the card goes to the USB host: a build in progress is given up.
pub fn card_leaving(srv: &mut Server) {
    if let Some(s) = slot_ref() {
        *s = None;
    }
    srv.lib.list = None;
    srv.lib.due = None;
}

/// The card is back from the USB host: open the library again and update
/// it soon (the host may have changed the music).
pub async fn card_back(vol: &mut Vol, srv: &mut Server) {
    let st = &mut srv.lib;
    with_lib_io(vol, Mode::Read, |io| open_into(io, st)).await;
    srv.lib.due = Some(now_ms() + UPDATE_DELAY_MS);
    srv.lib.publish();
}

/// Settings › Storage › Rebuild library: an update at the next idle moment.
pub fn rebuild(srv: &mut Server) {
    srv.lib.due = Some(now_ms());
    srv.lib.publish();
}

/// Milliseconds until a wanted build may run (`Some(0)`: now), `None` if
/// none is wanted.
#[must_use]
pub fn wait_ms(srv: &Server) -> Option<u64> {
    let d = srv.lib.due.or_else(|| slot_ref().and_then(|s| s.is_some().then_some(0)))?;
    Some(d.saturating_sub(now_ms()))
}

// ------------------------------------------------------------ building

fn config(vol: &Vol) -> Config {
    use orpheus_meta::text::CodePage;
    let code_page = match CODE_PAGE.load(Ordering::Relaxed) {
        2 => CodePage::Windows1251,
        _ => CodePage::Windows1252,
    };
    let t = atj2157_hal::rtc_clock::now();
    let now = if atj2157_hal::rtc_clock::running() && t.year >= 2026 {
        orpheus_index::mtime_of(&orpheus_fs::DateTime {
            year: t.year,
            month: t.month,
            day: t.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
            centis: 0,
        })
    } else {
        0
    };
    Config { serial: vol.serial(), now, code_page, ..Config::default() }
}

/// Start a build unless one is in progress; false if none can run.
#[inline(never)]
fn begin(cfg: Config) -> bool {
    let Some(s) = slot_ref() else { return false };
    if s.is_none() {
        *s = Suspended::new(SCRATCH, cfg);
    }
    s.is_some()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Going,
    Done,
    Failed,
}

/// One step of the build on `scratch`. On completion the new library is
/// opened in `st`.
#[inline(never)]
fn step(
    io: &Io<'_, '_>,
    scratch: &mut [u8],
    tags: &mut TagState,
    e: &mut DirEntry,
    st: &mut LibState,
) -> Step {
    let Some(s) = slot_ref() else { return Step::Failed };
    let Some(b) = s.as_mut() else { return Step::Failed };
    let mut src = CardTags::new(io, tags, e);
    match b.step_in(scratch, &mut io.store(), &mut src, BUDGET) {
        Ok(Progress::Done(l)) => {
            *s = None;
            st.opened(Some(l));
            st.due = None;
            Step::Done
        }
        Ok(Progress::Scanning { files, .. }) => {
            st.files = files;
            Step::Going
        }
        Ok(Progress::Indexing { .. }) => Step::Going,
        Err(_) => {
            *s = None;
            st.due = None;
            Step::Failed
        }
    }
}

/// Stop the build here: its buffers are written out, the scratch is free
/// again. A build that cannot be paused is given up.
#[inline(never)]
fn pause(io: &Io<'_, '_>, scratch: &mut [u8]) {
    let Some(s) = slot_ref() else { return };
    if let Some(b) = s.as_mut()
        && b.pause_in(scratch, &mut io.store()).is_err()
    {
        *s = None;
    }
}

/// Build in the idle card task until a request arrives or the build ends.
pub async fn slot(vol: &mut Vol, srv: &mut Server, arena: &mut [u8]) {
    let n = arena.len();
    if n < SCRATCH || !begin(config(vol)) {
        if let Some(s) = slot_ref() {
            *s = None;
        }
        srv.lib.due = None;
        srv.lib.publish();
        return;
    }
    let scratch = &mut arena[n - SCRATCH..];
    srv.lib.publish();
    let mut h = Handles::new();
    // In this future, not on the stack.
    let mut e = DirEntry::new();
    let mut tags = TagState::new();
    let mut ended = false;
    let mut last_pub = now_ms();
    while !crate::audio::work_waiting() {
        let t0 = time::now_us();
        let st = &mut srv.lib;
        let r = poll_fn(|cx| {
            crate::audio::mark(70);
            let io = CardIo::new_in(vol, cx, &mut h, Mode::Write { alt: false }, LIBRARY_DIR);
            let r = step(&io, scratch, &mut tags, &mut e, st);
            if r != Step::Going {
                let _ = io.close_all();
            }
            crate::audio::mark(0);
            Poll::Ready(r)
        })
        .await;
        if r != Step::Going {
            ended = true;
            break;
        }
        if now_ms() - last_pub >= 1000 {
            last_pub = now_ms();
            srv.lib.publish();
        }
        let dt = time::now_us() - t0;
        let _ = select(
            Timer::after_micros(dt.clamp(2_000, 100_000)),
            select3(
                crate::audio::CMDS.ready_to_receive(),
                media::REQS.ready_to_receive(),
                media::PLAY.ready_to_receive(),
            ),
        )
        .await;
    }
    if !ended {
        poll_fn(|cx| {
            let io = CardIo::new_in(vol, cx, &mut h, Mode::Write { alt: false }, LIBRARY_DIR);
            pause(&io, scratch);
            let _ = io.close_all();
            Poll::Ready(())
        })
        .await;
    }
    srv.lib.publish();
}

// ------------------------------------------------------------- queries

/// Publish rows from near `first` of `list` into the browse window (and,
/// for a new listing, its count and letter table).
#[inline(never)]
fn publish_list(io: &Io<'_, '_>, lib: &Library, list: LibList, dir: DirId, lgen: u32, first: Option<u32>) {
    let l = List::from(list);
    let mut s = io.store();
    let mut count = lib.len(&mut s, l).unwrap_or(0);
    if list == LibList::Recent {
        count = count.min(RECENT_ROWS);
    }
    let start = first.unwrap_or(0).saturating_sub(WINDOW / 4).min(count.saturating_sub(1));
    let mut items = [Item::default(); 4];
    let mut n = 0u32;
    while n < WINDOW && start + n < count {
        let want = ((WINDOW - n).min(count - start - n) as usize).min(items.len());
        let got = match lib.rows(&mut s, l, start + n, &mut items[..want]) {
            Ok(g) if g > 0 => g,
            _ => break,
        };
        for it in &items[..got] {
            let mut b = [0u8; 128];
            let name = lib.text(&mut s, it.name, &mut b).unwrap_or("?");
            let row = media::Row {
                name: media::Name::new(name),
                kind: if it.kind == ItemKind::Track { EntryKind::Audio } else { EntryKind::Dir },
                size: u64::from(it.duration_ms),
                file: FileRef { dir: kind_code(it.kind), index: it.id },
                ..media::Row::EMPTY
            };
            critical_section::with(|cs| media::BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
            n += 1;
        }
    }
    if first.is_none() {
        load_letters(io, lib, l, count, lgen);
    }
    critical_section::with(|cs| {
        let mut w = media::BROWSE.borrow(cs).borrow_mut();
        w.listing = Listing { dir, count, sorted: true, complete: true };
        w.lgen = lgen;
        w.first = start;
        w.n = n;
    });
    media::ready(media::tag::LIBRARY);
}

/// The letter runs of a list into `media::LETTERS` (none if they do not
/// fit: the UI falls back to the window).
#[inline(never)]
fn load_letters(io: &Io<'_, '_>, lib: &Library, l: List, count: u32, lgen: u32) {
    let mut t = LetterTable::<{ media::LETTER_RUNS }>::EMPTY;
    let mut ok = t.begin(count);
    let mut runs = [(0u32, None); 16];
    let mut j = 0u32;
    let mut s = io.store();
    while ok {
        let k = lib.letter_runs(&mut s, l, j, &mut runs).unwrap_or(0);
        if k == 0 {
            break;
        }
        for &(f, c) in &runs[..k] {
            if f < count {
                ok &= t.push(f, c);
            }
        }
        j += k as u32;
    }
    if !ok {
        t = LetterTable::EMPTY;
    }
    critical_section::with(|cs| *media::LETTERS.borrow(cs).borrow_mut() = (lgen, t));
}

/// Make `list` the listing (`Req::LibList`).
pub async fn open_list(vol: &mut Vol, srv: &mut Server, list: LibList) {
    let dir = list_dir(list);
    let lgen = srv.start_listing(dir);
    srv.lib.list = Some(list);
    let Some(lib) = srv.lib.lib.as_ref() else {
        critical_section::with(|cs| {
            let mut w = media::BROWSE.borrow(cs).borrow_mut();
            w.listing = Listing { dir, count: 0, sorted: true, complete: true };
            w.lgen = lgen;
            w.first = 0;
            w.n = 0;
        });
        media::ready(media::tag::LIBRARY);
        return;
    };
    with_lib_io(vol, Mode::Read, |io| publish_list(io, lib, list, dir, lgen, None)).await;
}

/// The window of the library listing from near `first` (`Req::Want`).
pub async fn window(vol: &mut Vol, srv: &Server, first: u32) {
    let (Some(lib), Some(list)) = (srv.lib.lib.as_ref(), srv.lib.list) else { return };
    let (dir, lgen) = (list_dir(list), srv.listing_gen());
    with_lib_io(vol, Mode::Read, |io| publish_list(io, lib, list, dir, lgen, Some(first))).await;
}

/// The tracks of a list into the play queue: up to [`QUEUE`] around row
/// `start`. Returns (queue length, item of `start`).
#[inline(never)]
fn fill_queue(
    io: &Io<'_, '_>,
    lib: &Library,
    list: LibList,
    start: Option<u32>,
    queue: &mut [Locator; QUEUE],
) -> (u32, Option<u32>) {
    let l = List::from(list);
    let mut s = io.store();
    let mut n = lib.len(&mut s, l).unwrap_or(0);
    if list == LibList::Recent {
        n = n.min(RECENT_ROWS);
    }
    let q = QUEUE as u32;
    let at = start.unwrap_or(0).min(n.saturating_sub(1));
    let first = if n <= q { 0 } else { at.saturating_sub(q / 2).min(n - q) };
    let mut k = 0usize;
    let _ = lib.queue(&mut s, l, first, q.min(n), |t| {
        queue[k] = t.locator();
        k += 1;
        k < QUEUE
    });
    let k = k as u32;
    (k, start.map(|_| at - first).filter(|&i| i < k))
}

/// A list's tracks as the play queue (`PlayReq::PlayLib`); the item to
/// start with.
pub async fn queue(vol: &mut Vol, srv: &mut Server, list: LibList, start: Option<u32>) -> Option<u32> {
    let lib = srv.lib.lib.as_ref()?;
    let q = &mut srv.queue;
    let (n, at) = with_lib_io(vol, Mode::Read, |io| fill_queue(io, lib, list, start, q)).await;
    srv.set_queue_len(vol.serial(), n);
    at
}

#[inline(never)]
fn fill_sample(io: &Io<'_, '_>, lib: &Library, seed: u64, queue: &mut [Locator; QUEUE]) -> u32 {
    let mut k = 0usize;
    let _ = lib.sample(&mut io.store(), seed, QUEUE as u32, |t| {
        queue[k] = t.locator();
        k += 1;
        k < QUEUE
    });
    k as u32
}

/// Shuffle all from the library (a uniform sample of [`QUEUE`] tracks);
/// false without a library (the caller walks the card instead).
pub async fn sample(vol: &mut Vol, srv: &mut Server, seed: u32) -> bool {
    let Some(lib) = srv.lib.lib.as_ref() else { return false };
    let q = &mut srv.queue;
    let n = with_lib_io(vol, Mode::Read, |io| fill_sample(io, lib, u64::from(seed), q)).await;
    if n == 0 {
        return false;
    }
    srv.set_queue_len(vol.serial(), n);
    true
}

/// Publish where the track at `loc` is in the library.
#[inline(never)]
fn find(io: &Io<'_, '_>, lib: &Library, loc: Locator) {
    let mut s = io.store();
    let mut now = None;
    if let Ok(Some(id)) = lib.find(&mut s, loc)
        && let Ok(t) = lib.track(&mut s, id)
    {
        now = Some(LibNow { track: id, album: t.album_id, artist: t.artist_id });
    }
    media::player_update(|p| p.lib_now = now);
}

/// The track that opens, in the library ("Go to album / artist").
pub async fn find_now(vol: &mut Vol, srv: &Server, loc: Locator) {
    match srv.lib.lib.as_ref() {
        Some(lib) => with_lib_io(vol, Mode::Read, |io| find(io, lib, loc)).await,
        None => media::player_update(|p| p.lib_now = None),
    }
}
