//! The Books app's model and its card work (05 §7.15): the library, the
//! open book and the reader's view ([`Shelf`], the screens' model: the UI
//! side), changed by [`Req`]uests from the screens and brought up to date
//! by [`Desk::step_with`] (the card side), one bounded piece of card work
//! at a time (at most 16 sectors read and 12 written: the firmware runs it
//! as paced chores of its card task, lending it the model for each step;
//! the simulator and the tests keep both in a [`Books`], between frames).
//!
//! What a step does, most urgent first: write the reading state; load or
//! search the library; open a book (state, cache check, conversion);
//! fetch the reader's text window; answer a pending jump (previous page,
//! line up, go to per mille, chapter, contents entry) from the page index;
//! fetch contents rows; lay the book out (the background pass that writes
//! the page index).
//!
//! RAM, all fixed (sizes in `tests/ram.rs` and docs/STATUS.md): the
//! [`Shelf`] (5.5 KB: the 2 KiB text window, the library's row window and
//! sort order, the contents window, the page-index cache, the book's state
//! with its bookmarks) and the [`Desk`] (7.7 KB: the card work of the
//! moment — the search, the conversion or the layout, never two at once —
//! the open book's files, a 1 KiB I/O buffer and a directory entry, which
//! keep the step's future small: on the player it is part of the card
//! task's).

use orpheus_fs::{BlockDevice, DirCursor, DirEntry, File, OpenMode, Volume};

use crate::convert::{Converter, Format, MetaReader};
use crate::epub::{Epub, Step, Store, Table};
use crate::fb2::MetaText;
use crate::files::{
    self, BOOKS_DIR, BookState, Bookmark, CACHE_DIR, INDEX_ENTRY, INDEX_HEADER, IndexHeader, LIB_HEADER,
    LIB_ORDER, LIB_PROGRESS_AT, LIB_RECORD, LIBRARY_PATH, LibHeader, LibRecord, MAX_BOOKS, NORM_HEADER,
    NormHeader, PathBuf, SNIPPET, STATE_BYTES, TEST_LIBRARY_PATH, TOC_ENTRY,
};
use crate::layout::{MAX_PAGE_BYTES, Mark, Metrics, Paginator, Params, Skip};
use crate::norm::{Item, Sink, TocEntry, item_at};

/// The reader's text window (a page and the next page's start).
pub const WINDOW: usize = 2048;
/// Library rows held at once.
pub const LIB_ROWS: usize = 8;
/// Contents rows held at once.
pub const TOC_ROWS: usize = 8;
/// Page-index entries cached (one 512-byte read).
pub const IDX_CACHE: usize = 64;
/// Folder depth the library search goes into.
pub const SCAN_DEPTH: usize = 8;
/// Directory entries one search step reads at most.
const SCAN_ENTRIES: u8 = 16;
/// The books folder (05 §7.15; "Find books in" › Books folder).
pub const BOOKS_FOLDER: &str = "/Books";
/// The on-device suite's books (`cargo xtask dev suite --setup` puts them
/// there; CLAUDE.md rule 5: under `/.orpheus/`), the library of
/// [`Scope::Test`].
pub const TEST_FOLDER: &str = "/.orpheus/test/suite/books";
/// Reading-position changes before the state is written anyway (a power
/// cut loses at most this many page turns).
const SAVE_EVERY: u8 = 16;

/// Where the library looks for books.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Scope {
    /// `/Books` and its folders.
    #[default]
    BooksFolder = 0,
    /// The whole card (except `/.orpheus` and hidden folders).
    WholeCard = 1,
    /// [`TEST_FOLDER`] only: the on-device suite's books (a test hook of
    /// the firmware and the simulator, never a setting).
    Test = 2,
}

impl Scope {
    #[must_use]
    pub fn root(self) -> &'static str {
        match self {
            Scope::BooksFolder => BOOKS_FOLDER,
            Scope::WholeCard => "/",
            Scope::Test => TEST_FOLDER,
        }
    }

    /// The library file of this scope: the suite's books have their own
    /// ([`TEST_LIBRARY_PATH`]), so a test never replaces the owner's.
    #[must_use]
    pub fn library(self) -> &'static str {
        if self == Scope::Test { TEST_LIBRARY_PATH } else { LIBRARY_PATH }
    }
}

/// What the screens ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Req {
    /// The library list (read it, or search the card if there is none or
    /// it was made for another scope).
    Library(Scope),
    /// Search the card again.
    Rescan(Scope),
    /// Library rows from this one.
    Rows(u16),
    /// Open the book of this library row.
    Open(u16),
    /// The reader closes: the position is written.
    Close,
    /// The layout (the reader's settings and font); a new key lays the
    /// book out again.
    Params(Params),
    /// Show from this mark (next page, a line down, a bookmark).
    Show(Mark),
    /// The page before the view's top.
    PrevPage,
    /// One line up.
    LineUp,
    /// Go to this per mille of the text.
    Permille(u16),
    /// Go to contents entry `n`.
    Toc(u32),
    /// Contents rows from this one.
    TocRows(u32),
    /// The next (`true`) or previous heading.
    Chapter(bool),
    /// Bookmark this line (the first shown).
    AddBookmark(Mark),
    /// Delete bookmark `n`.
    DeleteBookmark(u8),
    /// Go to bookmark `n`.
    Bookmark(u8),
    /// The first line shown (the reading position).
    Seen(Mark),
}

/// The library's state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LibPhase {
    /// Not asked for yet.
    #[default]
    Unknown,
    Loading,
    /// Searching the card; [`Library::found`] so far.
    Searching,
    Ready,
}

/// A library row.
#[derive(Clone, Copy, Debug, Default)]
pub struct BookRow {
    pub title: MetaText<64>,
    pub author: MetaText<48>,
    pub format: Format,
    /// Per mille read, or [`PROGRESS_NEW`].
    pub progress: u16,
}

/// The library list.
#[derive(Clone, Copy, Debug)]
pub struct Library {
    pub phase: LibPhase,
    pub scope: Scope,
    /// Books listed.
    pub count: u16,
    /// Books found so far while searching.
    pub found: u16,
    order: [u8; MAX_BOOKS],
    first: u16,
    rows: [BookRow; LIB_ROWS],
    /// Rows of the window loaded so far.
    loaded: u8,
    want: Option<u16>,
    /// The scope asked for differs from the file's: search again.
    rescan: Option<Scope>,
}

impl Default for Library {
    fn default() -> Self {
        Library {
            phase: LibPhase::Unknown,
            scope: Scope::BooksFolder,
            count: 0,
            found: 0,
            order: [0; MAX_BOOKS],
            first: 0,
            rows: [BookRow::default(); LIB_ROWS],
            loaded: 0,
            want: None,
            rescan: None,
        }
    }
}

impl Library {
    /// Row `i`, if it is in the loaded window.
    #[must_use]
    pub fn row(&self, i: u16) -> Option<&BookRow> {
        let k = i.checked_sub(self.first)?;
        (k < u16::from(self.loaded) && i < self.count).then(|| self.rows.get(usize::from(k))).flatten()
    }

    fn record_of(&self, row: u16) -> Option<u16> {
        (row < self.count).then(|| self.order.get(usize::from(row)).map(|&r| u16::from(r))).flatten()
    }
}

/// The open book's state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    #[default]
    Closed,
    /// Reading its record, its state and its cache.
    Opening,
    /// Making the normalised text (first open, or the book changed):
    /// [`Book::permille`] done.
    Converting,
    Ready,
    /// The file could not be read.
    Failed,
}

/// A jump waiting for the page index or the contents.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pending {
    Prev,
    Up,
    /// To the page holding this offset.
    At(u32),
    Chapter(bool),
    Toc(u32),
}

/// The steps of an open after the library record, one file operation
/// each ([`Desk::open_more`]): the book's directory entry, its saved
/// state (not for [`Book::ram_only`] books), the cache's header, then
/// the contents file (cached) or the book itself (to convert).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Opening {
    Stat,
    State,
    Cache,
    Toc,
    Source,
}

/// A binary search over the page index file, one chunk read per step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Search {
    target: u32,
    lo: u32,
    hi: u32,
}

/// The open book.
#[derive(Clone, Copy, Debug)]
pub struct Book {
    pub phase: Phase,
    /// Its library row and record, and its id (cache file names).
    pub row: u16,
    rec: u16,
    pub id: u32,
    pub title: MetaText<64>,
    pub format: Format,
    src: PathBuf,
    src_size: u32,
    src_stamp: u32,
    /// Conversion progress, per mille of the file.
    pub permille: u16,
    /// An EPUB's conversion waits for the scratch ([`Desk::step_with`]).
    pub waiting: bool,
    /// The book's language tag (EPUB), its first bytes.
    pub lang: [u8; 4],
    pub text_len: u32,
    pub toc_count: u32,
    /// Position and bookmarks.
    pub state: BookState,
    /// The view: its top and how it starts there.
    pub top: Mark,
    pub skip: Skip,
    /// Pages laid out so far, the bytes laid out, the index complete.
    pub pages: u32,
    pub laid: u32,
    pub complete: bool,
    /// The last page start found: the page holding an offset is known
    /// once a page starts after it (or the index is complete).
    frontier: u32,
    /// The page (of the index) holding the view's top, once known.
    pub page_no: Option<u32>,
    win: [u8; WINDOW],
    win_base: u32,
    win_len: u16,
    win_ok: bool,
    toc: [TocEntry; TOC_ROWS],
    toc_first: u32,
    toc_n: u8,
    toc_want: Option<u32>,
    pending: Option<Pending>,
    search: Option<Search>,
    /// The state changed since it was written; position changes since.
    dirty: bool,
    moves: u8,
    /// A book of the on-device suite's library ([`Scope::Test`]): its
    /// position and bookmarks are kept in RAM only, never read from or
    /// written to the card, so a test leaves nothing behind and starts the
    /// same whatever earlier runs did (BOOK-1/2b).
    pub ram_only: bool,
    idx_cache: [Mark; IDX_CACHE],
    idx_first: u32,
    idx_n: u8,
}

impl Default for Book {
    fn default() -> Self {
        Book {
            phase: Phase::Closed,
            row: 0,
            rec: 0,
            id: 0,
            title: MetaText::default(),
            format: Format::Txt,
            src: PathBuf::new(),
            src_size: 0,
            src_stamp: 0,
            permille: 0,
            waiting: false,
            lang: [0; 4],
            text_len: 0,
            toc_count: 0,
            state: BookState::default(),
            top: Mark::START,
            skip: Skip::None,
            pages: 0,
            laid: 0,
            complete: false,
            frontier: 0,
            page_no: None,
            win: [0; WINDOW],
            win_base: 0,
            win_len: 0,
            win_ok: false,
            toc: [TocEntry::EMPTY; TOC_ROWS],
            toc_first: 0,
            toc_n: 0,
            toc_want: None,
            pending: None,
            search: None,
            dirty: false,
            moves: 0,
            ram_only: false,
            idx_cache: [Mark::START; IDX_CACHE],
            idx_first: 0,
            idx_n: 0,
        }
    }
}

impl Book {
    /// A fresh book in `phase` for library row `row`, in place (built and
    /// moved, a `Book` was 3.8 KB of the UI task's frame twice over on the
    /// player). The windows' bytes are left: their lengths say none is
    /// held.
    pub fn reset(&mut self, phase: Phase, row: u16) {
        let Book {
            phase: ph,
            row: r,
            rec,
            id,
            title,
            format,
            src,
            src_size,
            src_stamp,
            permille,
            waiting,
            lang,
            text_len,
            toc_count,
            state,
            top,
            skip,
            pages,
            laid,
            complete,
            frontier,
            page_no,
            win: _,
            win_base,
            win_len,
            win_ok,
            toc: _,
            toc_first,
            toc_n,
            toc_want,
            pending,
            search,
            dirty,
            moves,
            ram_only,
            idx_cache: _,
            idx_first,
            idx_n,
        } = self;
        *ph = phase;
        *r = row;
        *rec = 0;
        *id = 0;
        *title = MetaText::default();
        *format = Format::Txt;
        src.clear();
        *src_size = 0;
        *src_stamp = 0;
        *permille = 0;
        *waiting = false;
        *lang = [0; 4];
        *text_len = 0;
        *toc_count = 0;
        state.clear();
        *top = Mark::START;
        *skip = Skip::None;
        *pages = 0;
        *laid = 0;
        *complete = false;
        *frontier = 0;
        *page_no = None;
        *win_base = 0;
        *win_len = 0;
        *win_ok = false;
        *toc_first = 0;
        *toc_n = 0;
        *toc_want = None;
        *pending = None;
        *search = None;
        *dirty = false;
        *moves = 0;
        *ram_only = false;
        *idx_first = 0;
        *idx_n = 0;
    }

    /// The text window if it holds the view's top: its first offset, its
    /// bytes, and whether it reaches the end of the book.
    #[must_use]
    pub fn window(&self) -> Option<(u32, &[u8], bool)> {
        if !self.win_ok || self.top.offset < self.win_base {
            return None;
        }
        let len = u32::from(self.win_len);
        if self.top.offset > self.win_base + len {
            return None;
        }
        let at_end = self.win_base + len >= self.text_len;
        Some((self.win_base, self.win.get(..usize::from(self.win_len)).unwrap_or(&[]), at_end))
    }

    /// Whether the window holds a page from the top on.
    fn window_covers(&self) -> bool {
        let Some((base, w, at_end)) = self.window() else { return false };
        at_end || self.top.offset + MAX_PAGE_BYTES + 256 <= base + w.len() as u32
    }

    /// Contents row `i`, if loaded.
    #[must_use]
    pub fn toc_row(&self, i: u32) -> Option<&TocEntry> {
        let k = i.checked_sub(self.toc_first)?;
        (k < u32::from(self.toc_n)).then(|| self.toc.get(k as usize)).flatten()
    }

    /// Per mille of the text before the view's top.
    #[must_use]
    pub fn read_permille(&self) -> u16 {
        permille(self.top.offset, self.text_len)
    }

    /// A jump is waiting (for the layout to get there, or a read).
    #[must_use]
    pub fn busy(&self) -> bool {
        self.pending.is_some() || !self.win_ok
    }

    fn view(&mut self, top: Mark, skip: Skip) {
        self.top = top;
        self.skip = skip;
        self.page_no = None;
        if !self.window_covers() {
            self.win_ok = false;
        }
    }

    /// The page holding `offset` is known (the layout got past it).
    fn known(&self, offset: u32) -> bool {
        self.complete || self.frontier > offset
    }

    /// The page holding `offset` from the cache, if it can tell (a later
    /// cached entry starts after it, or the cache ends with the last
    /// page).
    fn cached_page(&self, offset: u32) -> Option<(u32, Mark)> {
        let n = usize::from(self.idx_n);
        let c = self.idx_cache.get(..n)?;
        if c.first()?.offset > offset {
            return None;
        }
        let k = c.iter().rposition(|m| m.offset <= offset)?;
        let last_page = self.complete && self.idx_first + n as u32 == self.pages;
        (k + 1 < n || last_page).then_some((self.idx_first + k as u32, c[k]))
    }
}

/// The card work of the moment ends (done, failed or given up): the
/// files it still holds become orphans.
fn drop_work(work: &mut Work, orphans: &mut [Option<File>; 5]) {
    match core::mem::replace(work, Work::Idle) {
        Work::Convert(j) => {
            for f in [j.norm, j.toc].into_iter().chain(j.tables) {
                orphan(orphans, f);
            }
        }
        Work::Layout(j) => orphan(orphans, Some(j.idx)),
        Work::Scan(j) => orphan(orphans, j.lib),
        Work::Idle => {}
    }
}

/// `f` (if any) to the orphans (dropped if they are all taken).
fn orphan(orphans: &mut [Option<File>; 5], f: Option<File>) {
    if let Some(f) = f
        && let Some(slot) = orphans.iter_mut().find(|o| o.is_none())
    {
        *slot = Some(f);
    }
}

fn permille(a: u32, b: u32) -> u16 {
    if b == 0 { 0 } else { u16::try_from(u64::from(a) * 1000 / u64::from(b)).unwrap_or(1000).min(1000) }
}

/// A reading state to write.
#[derive(Clone, Copy, Debug)]
struct SaveRec {
    id: u32,
    rec: u16,
    row: u16,
    text_len: u32,
    state: BookState,
    /// Not written ([`Book::ram_only`]).
    ram_only: bool,
}

impl SaveRec {
    fn of(b: &Book) -> SaveRec {
        SaveRec {
            id: b.id,
            rec: b.rec,
            row: b.row,
            text_len: b.text_len,
            state: b.state,
            ram_only: b.ram_only,
        }
    }
}

/// How a candidate's title and author are read.
#[allow(clippy::large_enum_variant)]
enum Head {
    /// No candidate.
    None,
    /// From its first bytes.
    Plain(MetaReader),
    /// From its package document (BOOK-2; needs the scratch for a
    /// deflated one, else the file name stays the title).
    Epub(Epub),
}

/// The search of the library: a stack of folder cursors and the record
/// being made.
struct ScanJob {
    stack: [Option<(DirCursor, u16)>; SCAN_DEPTH],
    depth: u8,
    path: PathBuf,
    lib: Option<File>,
    count: u16,
    keys: [u32; MAX_BOOKS],
    /// A candidate's head being read: how, its file (`Some` while there is
    /// one) and its record (each set in place: [`begin_meta`]).
    head: Head,
    cand: Option<File>,
    rec: LibRecord,
    started: bool,
}

/// A sink for nothing (the library's EPUB reader writes no text).
struct Discard;

impl Sink for Discard {
    fn put(&mut self, _: &[u8]) {}
}

/// The card as the EPUB job sees it: the book, its scratch tables and its
/// contents file.
struct VolStore<'a, 'v, D: BlockDevice> {
    vol: &'a mut Volume<'v, D>,
    src: &'a mut File,
    tables: Option<&'a mut [Option<File>; 3]>,
    toc: Option<&'a mut File>,
}

impl<D: BlockDevice> Store for VolStore<'_, '_, D> {
    async fn read(&mut self, at: u32, buf: &mut [u8]) -> usize {
        self.src.set_position(u64::from(at));
        self.vol.read_exact_or_eof(self.src, buf).await.unwrap_or(0)
    }

    async fn get(&mut self, t: Table, at: u32, buf: &mut [u8]) -> bool {
        let vol = &mut *self.vol;
        let Some(f) = (match t {
            Table::Toc => self.toc.as_deref_mut(),
            _ => self.tables.as_deref_mut().and_then(|ts| ts.get_mut(t as usize)).and_then(Option::as_mut),
        }) else {
            return false;
        };
        if u64::from(at) + buf.len() as u64 > f.size() {
            return false;
        }
        f.set_position(u64::from(at));
        vol.read_exact_or_eof(f, buf).await.is_ok_and(|n| n == buf.len())
    }

    async fn put(&mut self, t: Table, at: u32, data: &[u8]) -> bool {
        let vol = &mut *self.vol;
        let Some(f) = (match t {
            Table::Toc => self.toc.as_deref_mut(),
            _ => self.tables.as_deref_mut().and_then(|ts| ts.get_mut(t as usize)).and_then(Option::as_mut),
        }) else {
            return false;
        };
        f.set_position(u64::from(at));
        vol.write(f, data).await.is_ok()
    }
}

/// Bytes the converter's output buffer holds.
const OUT: usize = 1024;

struct OutBuf {
    b: [u8; OUT],
    n: usize,
}

impl Sink for OutBuf {
    fn put(&mut self, b: &[u8]) {
        let end = (self.n + b.len()).min(OUT);
        let take = end - self.n;
        self.b[self.n..end].copy_from_slice(&b[..take]);
        self.n = end;
    }
    fn room(&self) -> usize {
        OUT - self.n
    }
}

/// What converts the book.
#[allow(clippy::large_enum_variant)]
enum Parse {
    /// Not started (the cache files are made first).
    None,
    Plain(Converter),
    Epub(Epub),
}

struct ConvJob {
    c: Parse,
    src: File,
    read: u32,
    inbuf: [u8; 512],
    in_n: usize,
    in_at: usize,
    out: OutBuf,
    norm: Option<File>,
    toc: Option<File>,
    /// EPUB: the job's scratch tables ([`files::EPUB_TABLES`]).
    tables: [Option<File>; 3],
    toc_n: u32,
    eof: bool,
}

/// Page-index entries the layout job holds before writing (64 written at
/// a time; a 64-byte piece of text adds at most 64).
const TAIL: usize = 128;

struct LayoutJob {
    pg: Paginator,
    key: u32,
    idx: File,
    norm: File,
    /// Entries written to the file, and the ones held.
    written: u32,
    tail: [Mark; TAIL],
    tail_n: usize,
    buf: [u8; 512],
    started: bool,
}

/// The card work of the moment.
#[allow(clippy::large_enum_variant)]
enum Work {
    Idle,
    Scan(ScanJob),
    Convert(ConvJob),
    /// Laying the open book out (never with a search or a conversion: a
    /// search runs from the library, with the book closed; layout starts
    /// when the conversion is done).
    Layout(LayoutJob),
}

/// The layout job of `w`, while the book is laid out.
fn layout_of(w: &Work) -> Option<&LayoutJob> {
    match w {
        Work::Layout(j) => Some(j),
        _ => None,
    }
}

/// The Books app's model: what the screens read and their requests change
/// (the UI side). Card work is the [`Desk`]'s ([`Desk::step_with`]), which
/// is lent the model for one step at a time: the firmware's UI task and
/// card task never hold it at once (`fw/firmware/src/books.rs`), the
/// simulator and the tests keep both in a [`Books`].
///
/// A request only changes the model; what it asks of the card (open a
/// book, write the reading state, drop the open book's files or its
/// layout) waits in it for the next step.
pub struct Shelf {
    pub lib: Library,
    pub book: Book,
    /// The layout the reader asked for (and the font id it measures with).
    pub params: Option<Params>,
    /// Open asked for this library row.
    open: Option<u16>,
    /// A reading state to write (the open book's, or the one before).
    save: Option<SaveRec>,
    close: bool,
    /// The open book's files and its conversion or layout are given up
    /// (another book opens).
    drop_book: bool,
    /// The layout job is given up (a new layout key).
    drop_layout: bool,
    /// Step count (tests).
    pub steps: u32,
}

impl Default for Shelf {
    fn default() -> Self {
        Shelf::new()
    }
}

/// What the next step of the card work does, for the firmware's chore
/// costs (each kind has its own measured need): writes (the sector cache
/// written back, a job's files' entries, the reading state, a library
/// title; a folder looked up or made), the library search, reads (the
/// library's rows, a book opened, the text window, a jump through the
/// page index, contents rows), a conversion piece, a layout piece.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Write = 0,
    Scan = 1,
    Read = 2,
    Convert = 3,
    Layout = 4,
}

/// Kinds of [`Kind`] and their names (the firmware's `chore:` lines).
pub const KINDS: usize = 5;
pub const KIND_NAMES: [&str; KINDS] = ["bwrite", "bscan", "bread", "bconv", "blay"];

/// Bytes of the card side's I/O buffer ([`Desk`]): the largest read or
/// write one step makes into or from RAM of its own (the layout's last
/// index write, 128 entries; the library header and order; a record; the
/// reading state). In the desk, not in the step's future: on the player
/// that future is part of the card task's (static) future and its poll
/// frames.
const IO: usize = 1024;
const _: () = assert!(
    TAIL * INDEX_ENTRY <= IO
        && LIB_HEADER + LIB_ORDER <= IO
        && LIB_RECORD <= IO
        && STATE_BYTES <= IO
        && files::LIB_NAMES <= IO
        && IDX_CACHE * INDEX_ENTRY <= IO
);

/// The card side of the Books app: the card work of the moment, the open
/// book's files and what the steps need between them (the firmware's card
/// task owns it; [`Desk::step_with`] borrows the [`Shelf`] for a step).
pub struct Desk {
    work: Work,
    /// Reading files of the open book (read only after conversion).
    norm: Option<File>,
    idx: Option<File>,
    tocf: Option<File>,
    path: PathBuf,
    /// Folders of [`files::DIRS`] made so far (one per step: making a
    /// folder zeroes a cluster).
    dirs: u8,
    /// Dirty sectors to write back, one per step.
    flush: bool,
    /// An EPUB's package named its title and author: its library record
    /// to rewrite (a step of its own).
    retitle: Option<(u16, MetaText<64>, MetaText<48>)>,
    /// Files a job wrote and left (given up, or the EPUB job's tables):
    /// their entries are written, one per step, before other work (so no
    /// cluster is lost and a file is not opened again before).
    orphans: [Option<File>; 5],
    /// A folder may be made (a cluster zeroed: 256 sectors written on a
    /// card with 128 KiB clusters). The firmware clears it while music
    /// plays; a missing folder then waits ([`Desk::wants_mkdir`]).
    pub mkdir: bool,
    /// A folder is missing and `mkdir` was off.
    mkdir_wait: bool,
    /// The open in progress: its next step ([`Desk::open_more`]).
    opening: Option<Opening>,
    /// The steps' reads and writes of their own ([`IO`]).
    io: [u8; IO],
    /// A directory entry (a book's size and time when it opens).
    entry: DirEntry,
}

impl Default for Desk {
    fn default() -> Self {
        Desk::new()
    }
}

/// The model and its card work in one (the simulator, the tests): the
/// [`Shelf`]'s fields and requests through `Deref`, the [`Desk`]'s steps.
#[derive(Default)]
pub struct Books {
    pub shelf: Shelf,
    pub desk: Desk,
}

impl core::ops::Deref for Books {
    type Target = Shelf;
    fn deref(&self) -> &Shelf {
        &self.shelf
    }
}

impl core::ops::DerefMut for Books {
    fn deref_mut(&mut self) -> &mut Shelf {
        &mut self.shelf
    }
}

impl Books {
    #[must_use]
    pub fn new() -> Books {
        Books { shelf: Shelf::new(), desk: Desk::new() }
    }

    /// [`Desk::step_with`] without the scratch.
    pub async fn step<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, m: &dyn Metrics) -> bool {
        self.desk.step_with(&mut self.shelf, vol, m, None).await
    }

    /// [`Desk::step_with`].
    pub async fn step_with<D: BlockDevice>(
        &mut self,
        vol: &mut Volume<'_, D>,
        m: &dyn Metrics,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        self.desk.step_with(&mut self.shelf, vol, m, scratch).await
    }

    /// [`Desk::needs_scratch`].
    #[must_use]
    pub fn needs_scratch(&self) -> bool {
        self.desk.needs_scratch(&self.shelf)
    }
}

impl Shelf {
    #[must_use]
    pub fn new() -> Shelf {
        Shelf {
            lib: Library::default(),
            book: Book::default(),
            params: None,
            open: None,
            save: None,
            close: false,
            drop_book: false,
            drop_layout: false,
            steps: 0,
        }
    }

    /// A screen's request. Cheap, and only the model: the card work
    /// happens in [`Desk::step_with`].
    pub fn request(&mut self, r: Req) {
        let b = &mut self.book;
        match r {
            Req::Library(scope) => {
                if self.lib.phase == LibPhase::Unknown {
                    self.lib.phase = LibPhase::Loading;
                    self.lib.scope = scope;
                } else if self.lib.scope != scope && self.lib.phase == LibPhase::Ready {
                    self.lib.rescan = Some(scope);
                }
            }
            Req::Rescan(scope) => {
                // Searching: the search is the card work of the moment.
                if self.lib.phase != LibPhase::Searching {
                    self.lib.rescan = Some(scope);
                }
            }
            Req::Rows(first) => self.lib.want = Some(first),
            Req::Open(row) => {
                if b.dirty && b.phase == Phase::Ready {
                    self.save = Some(SaveRec::of(b));
                }
                // The open book's files go (its conversion or layout is
                // given up) before anything else in the next step. A close
                // asked before (the reader left, its state staged above)
                // is done by that: still pending, its step would mark the
                // book being opened Closed (BOOK-1/2b).
                self.drop_book = true;
                self.close = false;
                self.open = Some(row);
                self.book.reset(Phase::Opening, row);
            }
            Req::Close => {
                if b.dirty && b.phase == Phase::Ready {
                    self.save = Some(SaveRec::of(b));
                    b.dirty = false;
                    b.moves = 0;
                }
                self.close = true;
            }
            Req::Params(p) => {
                if self.params != Some(p) {
                    let relayout = self.params.map(|o| o.key()) != Some(p.key());
                    self.params = Some(p);
                    if relayout && b.phase == Phase::Ready {
                        self.drop_layout = true;
                        b.complete = false;
                        b.pages = 0;
                        b.laid = 0;
                        b.frontier = 0;
                        b.idx_n = 0;
                        b.search = None;
                        b.page_no = None;
                        // The view stays at its top (re-layout keeps the
                        // position).
                        b.skip = Skip::None;
                    }
                }
            }
            Req::Show(m) => {
                b.pending = None;
                b.view(m, Skip::None);
            }
            Req::PrevPage => b.pending = (b.top.offset > 0).then_some(Pending::Prev),
            Req::LineUp => b.pending = (b.top.offset > 0).then_some(Pending::Up),
            Req::Permille(p) => {
                let at = u32::try_from(u64::from(b.text_len) * u64::from(p.min(1000)) / 1000).unwrap_or(0);
                b.pending = Some(Pending::At(at.min(b.text_len.saturating_sub(1))));
            }
            Req::Toc(i) => b.pending = (i < b.toc_count).then_some(Pending::Toc(i)),
            Req::TocRows(first) => b.toc_want = Some(first),
            Req::Chapter(next) => b.pending = (b.toc_count > 0).then_some(Pending::Chapter(next)),
            Req::AddBookmark(m) => {
                let mut snippet = MetaText::<SNIPPET>::default();
                if let Some((base, w, _)) = b.window()
                    && let Some(from) = m.offset.checked_sub(base)
                {
                    let mut i = from as usize;
                    let mut full = false;
                    while let Some((it, n)) = item_at(w, i) {
                        if snippet.len >= SNIPPET as u8 - 4 {
                            full = true;
                            break;
                        }
                        match it {
                            Item::Char(c) => snippet.push(c),
                            Item::Para(_) if i > from as usize => snippet.push(' '),
                            _ => {}
                        }
                        i += n;
                    }
                    // Cut at a word, an ellipsis after it.
                    if full && let Some(sp) = snippet.as_str().rfind(' ').filter(|&k| k > 0) {
                        snippet.len = sp as u8;
                        snippet.push('…');
                    }
                }
                if b.state.add(Bookmark { mark: m, snippet }) {
                    b.dirty = true;
                }
            }
            Req::DeleteBookmark(i) => {
                b.state.remove(usize::from(i));
                b.dirty = true;
            }
            Req::Bookmark(i) => {
                if let Some(bm) = b.state.bookmarks().get(usize::from(i)).copied() {
                    b.pending = None;
                    b.view(bm.mark, Skip::None);
                }
            }
            Req::Seen(m) => {
                if b.state.pos != m {
                    b.state.pos = m;
                    b.dirty = true;
                    b.moves = b.moves.saturating_add(1);
                }
            }
        }
    }

    /// The card went away and came back (the firmware's USB hand-over):
    /// the card side starts afresh ([`Desk::new`]), so the open book is
    /// opened again (at its saved position: write it first,
    /// [`Desk::suspend`]) and the library is read again.
    pub fn card_back(&mut self) {
        self.save = None;
        self.close = false;
        self.drop_book = false;
        self.drop_layout = false;
        if self.lib.phase != LibPhase::Unknown {
            self.lib.phase = LibPhase::Loading;
            self.lib.rescan = None;
        }
        match self.book.phase {
            Phase::Closed => {}
            _ => {
                let row = self.book.row;
                self.open = Some(row);
                self.book.reset(Phase::Opening, row);
            }
        }
    }
}

impl Desk {
    #[must_use]
    pub fn new() -> Desk {
        Desk {
            work: Work::Idle,
            norm: None,
            idx: None,
            tocf: None,
            path: PathBuf::new(),
            dirs: 0,
            flush: false,
            retitle: None,
            orphans: [None, None, None, None, None],
            mkdir: true,
            mkdir_wait: false,
            opening: None,
            io: [0; IO],
            entry: DirEntry::new(),
        }
    }

    /// As new, in place (`suspend`; a `Desk` built and moved was 7.7 KB of
    /// the card task's frame). The buffers' bytes are left.
    fn reset(&mut self) {
        self.work = Work::Idle;
        self.norm = None;
        self.idx = None;
        self.tocf = None;
        self.path.clear();
        self.dirs = 0;
        self.flush = false;
        self.retitle = None;
        self.orphans = [None, None, None, None, None];
        self.mkdir = true;
        self.mkdir_wait = false;
        self.opening = None;
    }

    /// Drop the open book's files (its state is written by the close
    /// step first when it has changed).
    fn close_now(&mut self) {
        self.opening = None;
        self.norm = None;
        self.idx = None;
        self.tocf = None;
        if matches!(self.work, Work::Convert(_) | Work::Layout(_)) {
            drop_work(&mut self.work, &mut self.orphans);
        }
    }

    /// Card work wants the scratch ([`crate::inflate::SCRATCH`] bytes):
    /// an EPUB being converted or its title read. Lend it, unchanged,
    /// to every step while this says so (the decoder arena when no track
    /// is loaded, 05 §7.15); a conversion that loses it in the middle of
    /// an entry starts again, a title read falls back to the file name.
    #[must_use]
    pub fn needs_scratch(&self, sh: &Shelf) -> bool {
        match &self.work {
            Work::Convert(_) => sh.book.format == Format::Epub,
            Work::Scan(j) => j.cand.is_some() && matches!(j.head, Head::Epub(_)),
            _ => false,
        }
    }

    /// A folder the work needs is missing and [`Desk::mkdir`] is off: the
    /// work waits for a step with it on.
    #[must_use]
    pub fn wants_mkdir(&self) -> bool {
        self.mkdir_wait
    }

    /// What the next step does ([`Kind`]; `None`: nothing to do as far as
    /// the model tells, the step would return `false`). Follows
    /// [`Desk::step_with`]'s order.
    #[must_use]
    pub fn next_kind(&self, sh: &Shelf) -> Option<Kind> {
        let b = &sh.book;
        if self.flush
            || self.orphans.iter().any(Option::is_some)
            || (usize::from(self.dirs) < files::DIRS.len() && self.needs_dirs(sh))
            || sh.save.is_some()
            || (b.dirty && b.moves >= SAVE_EVERY && b.phase == Phase::Ready)
            || self.retitle.is_some()
        {
            return Some(Kind::Write);
        }
        if sh.close || sh.drop_book || sh.drop_layout {
            return Some(Kind::Read);
        }
        if sh.lib.rescan.is_some() || matches!(self.work, Work::Scan(_)) {
            return Some(Kind::Scan);
        }
        if sh.lib.phase == LibPhase::Loading
            || (sh.lib.want.is_some() && sh.lib.phase == LibPhase::Ready)
            || sh.open.is_some()
            || self.opening.is_some()
        {
            return Some(Kind::Read);
        }
        if matches!(self.work, Work::Convert(_)) {
            return Some(Kind::Convert);
        }
        if b.phase != Phase::Ready {
            return None;
        }
        if !b.win_ok || b.pending.is_some() || b.page_no.is_none() || b.toc_want.is_some() {
            return Some(Kind::Read);
        }
        (!b.complete && sh.params.is_some()).then_some(Kind::Layout)
    }

    /// The card is about to go away (the firmware's USB hand-over): the
    /// reading state written, the files the jobs left closed, the sector
    /// cache written back (at most `max` steps); then the desk starts
    /// afresh and [`Shelf::card_back`] reopens what was open.
    pub async fn suspend<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>, max: u32) {
        let b = &mut sh.book;
        if b.dirty && b.phase == Phase::Ready {
            sh.save = Some(SaveRec::of(b));
            b.dirty = false;
            b.moves = 0;
        }
        // The open book's work is given up (its files become orphans).
        self.close_now();
        drop_work(&mut self.work, &mut self.orphans);
        self.retitle = None;
        let mkdir = self.mkdir;
        for _ in 0..max {
            if !self.flush && self.orphans.iter().all(Option::is_none) && sh.save.is_none() {
                break;
            }
            if sh.save.is_some() {
                self.save_state(sh, vol).await;
                continue;
            }
            if self.flush {
                self.flush = !matches!(vol.flush_step().await, Ok(true) | Err(_));
                continue;
            }
            if let Some(k) = self.orphans.iter().position(Option::is_some)
                && let Some(mut f) = self.orphans[k].take()
            {
                let _ = vol.write_entry(&mut f).await;
                self.flush = true;
            }
        }
        self.reset();
        self.mkdir = mkdir;
        sh.card_back();
    }

    /// One piece of card work on the model `sh`. True if the model changed
    /// (the screens look again). `scratch` lends the EPUB job its inflate
    /// window and tables ([`Desk::needs_scratch`]): without it an EPUB
    /// waits for it.
    pub async fn step_with<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        m: &dyn Metrics,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        sh.steps = sh.steps.wrapping_add(1);
        // What the requests gave up: the open book's files and work, a
        // layout of other parameters (before anything reads them).
        if core::mem::take(&mut sh.drop_book) {
            self.close_now();
        }
        if core::mem::take(&mut sh.drop_layout) && matches!(self.work, Work::Layout(_)) {
            drop_work(&mut self.work, &mut self.orphans);
        }
        // 0. Write-back of what the last job left in the sector cache.
        if self.flush {
            self.flush = !matches!(vol.flush_step().await, Ok(true) | Err(_));
            return true;
        }
        // Entries of files a job left.
        if let Some(k) = self.orphans.iter().position(Option::is_some) {
            if let Some(mut f) = self.orphans[k].take() {
                let _ = vol.write_entry(&mut f).await;
            }
            self.flush = true;
            return true;
        }
        // Folders a job needs, one per step: looked up, and made only when
        // `mkdir` allows (making one zeroes a cluster).
        if usize::from(self.dirs) < files::DIRS.len() && self.needs_dirs(sh) {
            let d = files::DIRS[usize::from(self.dirs)];
            if vol.open_dir(d).await.is_err() {
                if !self.mkdir {
                    self.mkdir_wait = true;
                    return false;
                }
                let _ = vol.create_dir(d).await;
            }
            self.mkdir_wait = false;
            self.dirs += 1;
            return true;
        }
        // 1. The reading state: when the reader closes, another book
        // opens, or often enough while reading.
        if sh.book.dirty && sh.book.moves >= SAVE_EVERY && sh.book.phase == Phase::Ready {
            sh.save = Some(SaveRec::of(&sh.book));
            sh.book.dirty = false;
            sh.book.moves = 0;
        }
        if sh.save.is_some() {
            self.save_state(sh, vol).await;
            return true;
        }
        if let Some(rec) = stage_names(&mut self.retitle, &mut self.io) {
            if let Ok(mut lf) = vol.open(sh.lib.scope.library(), OpenMode::ReadWrite).await {
                lf.set_position(files::lib_record_at(rec) + files::LIB_NAMES_AT as u64);
                if vol.write(&mut lf, &self.io[..files::LIB_NAMES]).await.is_ok() {
                    let _ = vol.flush_file(&mut lf).await;
                }
            }
            self.flush = true;
            return true;
        }
        if sh.close {
            sh.close = false;
            self.close_now();
            sh.book.phase = Phase::Closed;
            return true;
        }
        // 2. The library.
        if let Some(scope) = sh.lib.rescan.take() {
            sh.lib.scope = scope;
            sh.lib.phase = LibPhase::Searching;
            sh.lib.found = 0;
            sh.lib.count = 0;
            sh.lib.loaded = 0;
            begin_scan(&mut self.work);
            return true;
        }
        if let Work::Scan(_) = self.work {
            return self.scan_step(sh, vol, scratch).await;
        }
        if sh.lib.phase == LibPhase::Loading {
            self.load_library(sh, vol).await;
            return true;
        }
        if let Some(first) = sh.lib.want
            && sh.lib.phase == LibPhase::Ready
        {
            return self.rows_step(sh, vol, first).await;
        }
        // 3. Opening a book.
        if let Some(row) = sh.open.take() {
            self.open_book(sh, vol, row).await;
            return true;
        }
        if let Some(at) = self.opening {
            self.open_more(sh, vol, at).await;
            return true;
        }
        if let Work::Convert(_) = self.work {
            return self.convert_step(sh, vol, scratch).await;
        }
        if sh.book.phase != Phase::Ready {
            return false;
        }
        // 4. The reader: its window, jumps, contents, then the layout.
        if !sh.book.win_ok {
            self.load_window(sh, vol).await;
            return true;
        }
        if sh.book.pending.is_some() && self.resolve(sh, vol).await {
            return true;
        }
        if sh.book.page_no.is_none() && self.find_page_no(sh, vol).await {
            return true;
        }
        if let Some(first) = sh.book.toc_want.take() {
            self.load_toc(sh, vol, first).await;
            return true;
        }
        if !sh.book.complete
            && let Some(p) = sh.params
        {
            return self.layout_step(sh, vol, p, m).await;
        }
        false
    }
    // ----- library ------------------------------------------------------

    async fn load_library<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>) {
        let buf = &mut self.io[..LIB_HEADER + LIB_ORDER];
        let ok = match vol.open(sh.lib.scope.library(), OpenMode::Read).await {
            Ok(mut f) => vol.read_exact_or_eof(&mut f, buf).await.is_ok_and(|n| n == LIB_HEADER + LIB_ORDER),
            Err(_) => false,
        };
        match LibHeader::decode(buf).filter(|h| ok && h.complete) {
            Some(h) if h.scope == sh.lib.scope as u8 => {
                sh.lib.count = h.count;
                let n = usize::from(h.count).min(MAX_BOOKS);
                sh.lib.order[..n].copy_from_slice(&buf[LIB_HEADER..LIB_HEADER + n]);
                sh.lib.phase = LibPhase::Ready;
                sh.lib.loaded = 0;
                sh.lib.want.get_or_insert(0);
            }
            _ => {
                // None yet, damaged, or of the other scope: search.
                sh.lib.rescan = Some(sh.lib.scope);
            }
        }
    }

    async fn rows_step<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        first: u16,
    ) -> bool {
        let lib = &mut sh.lib;
        let first = first.min(lib.count.saturating_sub(1));
        if lib.first != first {
            lib.first = first;
            lib.loaded = 0;
        }
        let end = (u32::from(first) + LIB_ROWS as u32).min(u32::from(lib.count));
        let have = u32::from(first) + u32::from(lib.loaded);
        if have >= end {
            lib.want = None;
            return false;
        }
        // Two records per step.
        let Ok(mut f) = vol.open(lib.scope.library(), OpenMode::Read).await else {
            lib.want = None;
            lib.phase = LibPhase::Unknown;
            return true;
        };
        let rec = &mut self.io[..LIB_RECORD];
        for row in have..end.min(have + 2) {
            let Some(r) = lib.record_of(row as u16) else { break };
            f.set_position(files::lib_record_at(r));
            let ok = vol.read_exact_or_eof(&mut f, rec).await.is_ok_and(|n| n == LIB_RECORD);
            let d = if ok { LibRecord::decode(rec) } else { LibRecord::default() };
            if let Some(slot) = lib.rows.get_mut(usize::from(lib.loaded)) {
                *slot = BookRow { title: d.title, author: d.author, format: d.format, progress: d.progress };
            }
            lib.loaded += 1;
        }
        true
    }

    /// Card work that writes is waiting (its folders are made first).
    fn needs_dirs(&self, sh: &Shelf) -> bool {
        sh.save.is_some()
            || matches!(&self.work, Work::Scan(j) if !j.started)
            || matches!(&self.work, Work::Convert(j) if matches!(j.c, Parse::None))
            || (sh.book.phase == Phase::Ready && !sh.book.complete && self.layout().is_none())
    }

    async fn scan_step<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let scope = sh.lib.scope;
        let Work::Scan(job) = &mut self.work else { return false };
        if !job.started {
            job.started = true;
            let Ok(mut f) = vol.open(scope.library(), OpenMode::Truncate).await else {
                drop_work(&mut self.work, &mut self.orphans);
                sh.lib.phase = LibPhase::Ready;
                return true;
            };
            let _ = vol.write(&mut f, &[0u8; LIB_HEADER + LIB_ORDER]).await;
            job.lib = Some(f);
            match vol.open_dir(scope.root()).await {
                Ok(c) => {
                    job.stack[0] = Some((c, 0));
                    job.depth = 1;
                    job.path.clear();
                    if scope.root() != "/" {
                        job.path.push(scope.root());
                    }
                }
                // No books folder: an empty library.
                Err(_) => job.depth = 0,
            }
            return true;
        }
        if job.cand.is_some() {
            return Self::meta_step(job, vol, &mut sh.lib, &mut self.io, scratch).await;
        }
        if job.depth == 0 || usize::from(job.count) >= MAX_BOOKS {
            Self::finish_scan(job, vol, &mut sh.lib, &mut self.io, scope).await;
            drop_work(&mut self.work, &mut self.orphans);
            self.flush = true;
            return true;
        }
        for _ in 0..SCAN_ENTRIES {
            let d = usize::from(job.depth) - 1;
            let Some((mut cur, plen)) = job.stack[d] else {
                job.depth = 0;
                return true;
            };
            let more = vol.read_dir(&mut cur, &mut self.entry).await.unwrap_or(false);
            job.stack[d] = Some((cur, plen));
            if !more {
                // Back in the parent folder.
                job.depth -= 1;
                job.path.truncate(usize::from(plen));
                if job.depth == 0 {
                    return true;
                }
                continue;
            }
            let name = self.entry.name();
            let attr = self.entry.attributes();
            if name.starts_with('.')
                || attr & 0x06 != 0
                || name.eq_ignore_ascii_case("System Volume Information")
            {
                continue;
            }
            if self.entry.is_dir() {
                if usize::from(job.depth) < SCAN_DEPTH {
                    let here = job.path.len();
                    if let Ok(c) = vol.dir_cursor(&self.entry)
                        && job.path.push("/")
                        && job.path.push(name)
                    {
                        job.stack[usize::from(job.depth)] = Some((c, here as u16));
                        job.depth += 1;
                    } else {
                        job.path.truncate(here);
                    }
                }
                continue;
            }
            let Some(format) = Format::from_name(name) else { continue };
            if let Ok(f) = vol.open_entry(&self.entry)
                && begin_meta(job, &self.entry, format, f)
            {
                return true;
            }
        }
        true
    }

    /// One read of a candidate's head (an EPUB: one step of its package
    /// reader); at the end its record is written. In place (the reader,
    /// the EPUB job and the record stay in the job: not in the step's
    /// future); `io`: the desk's buffer.
    async fn meta_step<D: BlockDevice>(
        job: &mut ScanJob,
        vol: &mut Volume<'_, D>,
        lib: &mut Library,
        io: &mut [u8; IO],
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let (Some(f), head, rec) = (job.cand.as_mut(), &mut job.head, &mut job.rec) else { return false };
        let (title, author) = match head {
            Head::None => (MetaText::default(), MetaText::default()),
            Head::Plain(reader) => {
                let buf = &mut io[..512];
                let n = vol.read(f, buf).await.unwrap_or(0);
                if reader.fed == 0 {
                    new_reader(reader, rec.format, &buf[..n], n < 512);
                }
                reader.feed(&buf[..n]);
                if n == 512 && reader.wants_more() {
                    return true;
                }
                finish_reader(reader, rec.format)
            }
            Head::Epub(e) => {
                let mut st = VolStore { vol: &mut *vol, src: f, tables: None, toc: None };
                match e.step(&mut st, scratch, &mut Discard).await {
                    Step::Busy => return true,
                    Step::Done => (e.metadata().title, e.metadata().author),
                    // Refused, or no scratch now: the file name.
                    _ => (MetaText::default(), MetaText::default()),
                }
            }
        };
        if !title.is_empty() {
            rec.title = title;
        }
        rec.author = author;
        io[..LIB_RECORD].copy_from_slice(&rec.encode());
        let key = rec.sort_key();
        job.cand = None;
        job.head = Head::None;
        if let Some(lf) = job.lib.as_mut() {
            lf.set_position(files::lib_record_at(job.count));
            if vol.write(lf, &io[..LIB_RECORD]).await.is_ok() {
                job.keys[usize::from(job.count)] = key;
                job.count += 1;
                lib.found = job.count;
            }
        }
        true
    }

    async fn finish_scan<D: BlockDevice>(
        job: &mut ScanJob,
        vol: &mut Volume<'_, D>,
        lib: &mut Library,
        io: &mut [u8; IO],
        scope: Scope,
    ) {
        let n = usize::from(job.count);
        // Sorted by title key (insertion sort: at most 200), ties in the
        // order found; in the model's order table (no rows are shown
        // while searching).
        let order = &mut lib.order;
        *order = [0; MAX_BOOKS];
        for (i, o) in order.iter_mut().enumerate().take(n) {
            *o = i as u8;
        }
        for i in 1..n {
            let mut j = i;
            while j > 0 && job.keys[usize::from(order[j - 1])] > job.keys[usize::from(order[j])] {
                order.swap(j - 1, j);
                j -= 1;
            }
        }
        let head = &mut io[..LIB_HEADER + LIB_ORDER];
        head.fill(0);
        head[..LIB_HEADER]
            .copy_from_slice(&LibHeader { count: job.count, scope: scope as u8, complete: true }.encode());
        head[LIB_HEADER..LIB_HEADER + n].copy_from_slice(&order[..n]);
        if let Some(mut f) = job.lib.take() {
            f.set_position(0);
            let _ = vol.write(&mut f, head).await;
            let _ = vol.flush_file(&mut f).await;
        }
        lib.count = job.count;
        lib.found = job.count;
        lib.phase = LibPhase::Ready;
        lib.first = 0;
        lib.loaded = 0;
        lib.want = Some(0);
    }

    // ----- opening ------------------------------------------------------

    /// The first step of an open: the book's library record (path, title,
    /// format). The rest follows in steps of one file operation each
    /// ([`Desk::open_more`]).
    async fn open_book<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>, row: u16) {
        let b = &mut sh.book;
        let fail = |b: &mut Book| b.phase = Phase::Failed;
        let Some(rec_i) = sh.lib.record_of(row) else { return fail(b) };
        let Ok(mut lf) = vol.open(sh.lib.scope.library(), OpenMode::Read).await else { return fail(b) };
        let rec = &mut self.io[..LIB_RECORD];
        lf.set_position(files::lib_record_at(rec_i));
        if !vol.read_exact_or_eof(&mut lf, rec).await.is_ok_and(|n| n == LIB_RECORD) {
            return fail(b);
        }
        // Decoded in a function of its own: the record is neither in the
        // step's future nor in its poll frame.
        take_record(b, rec, rec_i);
        b.ram_only = sh.lib.scope == Scope::Test;
        self.opening = Some(Opening::Stat);
    }

    /// One more step of an open (BOOK-1/2b: the whole open was one step of
    /// five path walks, 49–77 ms and once 256 ms on the owner's card while
    /// music played, against chore budgets of 26–30 ms: two underruns at
    /// every open).
    async fn open_more<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>, at: Opening) {
        let b = &mut sh.book;
        self.opening = None;
        match at {
            // The book as it is now.
            Opening::Stat => {
                if vol.stat(b.src.as_str(), &mut self.entry).await.is_err() {
                    b.phase = Phase::Failed;
                    return;
                }
                b.src_size = u32::try_from(self.entry.size()).unwrap_or(u32::MAX);
                b.src_stamp = files::stamp(self.entry.modified());
                // The suite's books start where a fresh book does.
                self.opening = Some(if b.ram_only { Opening::Cache } else { Opening::State });
            }
            // Position and bookmarks.
            Opening::State => {
                let st = &mut self.io[..STATE_BYTES];
                if let Ok(mut f) = vol.open(self.path.file(BOOKS_DIR, b.id, "pos"), OpenMode::Read).await
                    && vol.read_exact_or_eof(&mut f, st).await.is_ok_and(|n| n == STATE_BYTES)
                    && take_state(b, st)
                {}
                self.opening = Some(Opening::Cache);
            }
            // The cache, if it is of this book as it is.
            Opening::Cache => {
                let mut h = [0u8; NORM_HEADER];
                let cached = match vol.open(self.path.file(CACHE_DIR, b.id, "txt"), OpenMode::Read).await {
                    Ok(mut f) => {
                        let ok = vol.read_exact_or_eof(&mut f, &mut h).await.is_ok_and(|n| n == NORM_HEADER);
                        NormHeader::decode(&h)
                            .filter(|h| {
                                ok && h.complete && h.src_size == b.src_size && h.src_stamp == b.src_stamp
                            })
                            .map(|h| (h, f))
                    }
                    Err(_) => None,
                };
                match cached {
                    Some((h, f)) => {
                        b.lang = h.lang;
                        b.text_len = h.text_len;
                        b.toc_count = h.toc_count;
                        self.norm = Some(f);
                        self.opening = Some(Opening::Toc);
                    }
                    None => self.opening = Some(Opening::Source),
                }
            }
            Opening::Toc => {
                self.tocf = vol.open(self.path.file(CACHE_DIR, b.id, "toc"), OpenMode::Read).await.ok();
                Self::ready(b, b.text_len, b.toc_count);
            }
            // Not cached: converted.
            Opening::Source => {
                let Ok(src) = vol.open(b.src.as_str(), OpenMode::Read).await else {
                    b.phase = Phase::Failed;
                    return;
                };
                b.phase = Phase::Converting;
                b.permille = 0;
                begin_convert(&mut self.work, src);
            }
        }
    }

    /// The text is there: the view at the saved position.
    fn ready(b: &mut Book, text_len: u32, toc_count: u32) {
        b.phase = Phase::Ready;
        b.text_len = text_len;
        b.toc_count = toc_count;
        let pos = if b.state.pos.offset < text_len { b.state.pos } else { Mark::START };
        b.view(pos, Skip::None);
        b.win_ok = false;
        b.complete = false;
        b.pages = 0;
        b.laid = 0;
    }

    async fn convert_step<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let Work::Convert(job) = &mut self.work else { return false };
        let b = &mut sh.book;
        if matches!(job.c, Parse::Epub(_)) {
            return self.epub_step(sh, vol, scratch).await;
        }
        if matches!(job.c, Parse::None) {
            if b.format == Format::Epub {
                begin_epub(&mut job.c, b.src_size);
                job.tables = [None, None, None];
                job.out.n = 0;
            } else {
                // The head: the encoding and (plain text) the line style.
                let n = vol.read(&mut job.src, &mut job.inbuf).await.unwrap_or(0);
                job.in_n = n;
                job.in_at = 0;
                job.read = n as u32;
                job.eof = n < job.inbuf.len();
                begin_plain(&mut job.c, b.format, &job.inbuf[..n], job.eof);
            }
            job.norm = vol.open(self.path.file(CACHE_DIR, b.id, "txt"), OpenMode::Truncate).await.ok();
            job.toc = vol.open(self.path.file(CACHE_DIR, b.id, "toc"), OpenMode::Truncate).await.ok();
            let (Some(norm), Some(_)) = (job.norm.as_mut(), job.toc.as_ref()) else {
                b.phase = Phase::Failed;
                drop_work(&mut self.work, &mut self.orphans);
                return true;
            };
            // The header is written last (complete): zeros until then.
            let _ = vol.write(norm, &[0u8; NORM_HEADER]).await;
            return true;
        }
        let Parse::Plain(c) = &mut job.c else { return false };
        // Feed until the input piece is used or the output must be written.
        while job.in_at < job.in_n && job.out.n < 512 {
            let used = c.feed(&job.inbuf[job.in_at..job.in_n], &mut job.out);
            job.in_at += used;
            if let Some(e) = c.take_toc() {
                if let Some(t) = job.toc.as_mut() {
                    let _ = vol.write(t, &files::encode_toc(&e)).await;
                }
                job.toc_n += 1;
                return true;
            }
            if used == 0 {
                break;
            }
        }
        if job.out.n >= 512 {
            if let Some(nf) = job.norm.as_mut() {
                let _ = vol.write(nf, &job.out.b[..512]).await;
            }
            job.out.b.copy_within(512..job.out.n, 0);
            job.out.n -= 512;
            return true;
        }
        if job.in_at < job.in_n {
            return true;
        }
        if !job.eof {
            let n = vol.read(&mut job.src, &mut job.inbuf).await.unwrap_or(0);
            job.in_n = n;
            job.in_at = 0;
            job.read += n as u32;
            job.eof = n == 0;
            b.permille = permille(job.read, b.src_size.max(1)).min(999);
            return true;
        }
        // The end: the last text, the last heading, the header.
        c.finish(&mut job.out);
        if let Some(e) = c.take_toc() {
            if let Some(t) = job.toc.as_mut() {
                let _ = vol.write(t, &files::encode_toc(&e)).await;
            }
            job.toc_n += 1;
        }
        let text_len = c.offset();
        let enc = c.encoding() as u8;
        let (Some(mut nf), Some(mut tf)) = (job.norm.take(), job.toc.take()) else {
            b.phase = Phase::Failed;
            drop_work(&mut self.work, &mut self.orphans);
            return true;
        };
        let _ = vol.write(&mut nf, &job.out.b[..job.out.n]).await;
        let h = NormHeader {
            format: b.format as u8,
            encoding: enc,
            complete: true,
            src_size: b.src_size,
            src_stamp: b.src_stamp,
            text_len,
            toc_count: job.toc_n,
            lang: [0; 4],
        };
        nf.set_position(0);
        let _ = vol.write(&mut nf, &h.encode()).await;
        let _ = vol.write_entry(&mut nf).await;
        let _ = vol.write_entry(&mut tf).await;
        self.flush = true;
        self.norm = Some(nf);
        self.tocf = Some(tf);
        let toc_n = job.toc_n;
        drop_work(&mut self.work, &mut self.orphans);
        Self::ready(b, text_len, toc_n);
        true
    }

    /// One piece of an EPUB's conversion ([`crate::epub`]): its scratch
    /// tables made (one per step), the text written 512 bytes at a time,
    /// else one step of the job.
    async fn epub_step<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let Work::Convert(job) = &mut self.work else { return false };
        let b = &mut sh.book;
        let Parse::Epub(e) = &mut job.c else { return false };
        if let Some(k) = job.tables.iter().position(Option::is_none) {
            job.tables[k] = vol.open(files::EPUB_TABLES[k], OpenMode::Truncate).await.ok();
            if job.tables[k].is_none() {
                b.phase = Phase::Failed;
                drop_work(&mut self.work, &mut self.orphans);
            }
            return true;
        }
        if job.out.n >= 512 {
            if let Some(nf) = job.norm.as_mut() {
                let _ = vol.write(nf, &job.out.b[..512]).await;
            }
            job.out.b.copy_within(512..job.out.n, 0);
            job.out.n -= 512;
            return true;
        }
        let Some(toc) = job.toc.as_mut() else { return false };
        let mut st = VolStore { vol, src: &mut job.src, tables: Some(&mut job.tables), toc: Some(toc) };
        let r = e.step(&mut st, scratch, &mut job.out).await;
        b.permille = e.permille();
        b.waiting = r == Step::NeedScratch;
        match r {
            Step::Busy => true,
            // Nothing to do until the scratch is lent.
            Step::NeedScratch => false,
            Step::Lost => {
                // Taken away in the middle of an entry: from the start.
                for f in [job.norm.take(), job.toc.take()]
                    .into_iter()
                    .chain(job.tables.iter_mut().map(Option::take))
                {
                    orphan(&mut self.orphans, f);
                }
                job.c = Parse::None;
                job.out.n = 0;
                true
            }
            Step::Failed(_) => {
                b.phase = Phase::Failed;
                drop_work(&mut self.work, &mut self.orphans);
                true
            }
            Step::Done => {
                let (text_len, toc_n, meta) = (e.text_len(), e.toc_n, *e.metadata());
                let (Some(mut nf), Some(mut tf)) = (job.norm.take(), job.toc.take()) else {
                    b.phase = Phase::Failed;
                    drop_work(&mut self.work, &mut self.orphans);
                    return true;
                };
                let _ = vol.write(&mut nf, &job.out.b[..job.out.n]).await;
                let mut lang = [0u8; 4];
                let l = meta.language.as_str().as_bytes();
                lang[..l.len().min(4)].copy_from_slice(&l[..l.len().min(4)]);
                let h = NormHeader {
                    format: Format::Epub as u8,
                    encoding: 0,
                    complete: true,
                    src_size: b.src_size,
                    src_stamp: b.src_stamp,
                    text_len,
                    toc_count: toc_n,
                    lang,
                };
                nf.set_position(0);
                let _ = vol.write(&mut nf, &h.encode()).await;
                let _ = vol.write_entry(&mut nf).await;
                let _ = vol.write_entry(&mut tf).await;
                // The package's title and author in the library (the
                // search may have had only the file name), next step.
                if !meta.title.is_empty() {
                    self.retitle = Some((b.rec, meta.title, meta.author));
                    b.title = meta.title;
                    if let Some(k) = b.row.checked_sub(sh.lib.first)
                        && k < u16::from(sh.lib.loaded)
                        && let Some(row) = sh.lib.rows.get_mut(usize::from(k))
                    {
                        row.title = meta.title;
                        row.author = meta.author;
                    }
                }
                b.lang = lang;
                self.flush = true;
                self.norm = Some(nf);
                self.tocf = Some(tf);
                drop_work(&mut self.work, &mut self.orphans);
                Self::ready(b, text_len, toc_n);
                true
            }
        }
    }

    // ----- reading ------------------------------------------------------

    async fn load_window<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>) {
        let b = &mut sh.book;
        let Some(f) = self.norm.as_mut() else {
            b.phase = Phase::Failed;
            return;
        };
        let at = b.top.offset.min(b.text_len);
        let len = (b.text_len - at).min(WINDOW as u32) as usize;
        f.set_position(u64::from(at) + NORM_HEADER as u64);
        let n = vol.read_exact_or_eof(f, &mut b.win[..len]).await.unwrap_or(0);
        b.win_base = at;
        b.win_len = n as u16;
        b.win_ok = true;
    }

    /// The reading state waiting in the model (`sh.save`) written: its
    /// bytes into the desk's buffer first (the 460-byte record is not kept
    /// in the step's future), the position file, the library's progress.
    async fn save_state<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>) {
        let Some((id, rec, row, progress)) = stage_save(sh, &mut self.io) else { return };
        let data = &self.io[..STATE_BYTES];
        let path = self.path.file(BOOKS_DIR, id, "pos");
        // In place when it exists (one sector, nothing allocated).
        let mut done = false;
        if let Ok(mut f) = vol.open(path, OpenMode::ReadWrite).await
            && vol.rewrite(&mut f, data).await.unwrap_or(false)
        {
            done = vol.write_entry(&mut f).await.is_ok();
        }
        if !done
            && let Ok(mut f) = vol.open(path, OpenMode::Truncate).await
            && vol.write(&mut f, data).await.is_ok()
        {
            let _ = vol.flush_file(&mut f).await;
        }
        // The library's progress for this book.
        // (Only the owner's books get here: the suite's are `ram_only`.)
        if let Ok(mut lf) = vol.open(LIBRARY_PATH, OpenMode::ReadWrite).await {
            lf.set_position(files::lib_record_at(rec) + LIB_PROGRESS_AT as u64);
            if vol.write(&mut lf, &progress.to_le_bytes()).await.is_ok() {
                let _ = vol.flush_file(&mut lf).await;
            }
        }
        self.flush = true;
        if let Some(k) = row.checked_sub(sh.lib.first)
            && k < u16::from(sh.lib.loaded)
            && let Some(row) = sh.lib.rows.get_mut(usize::from(k))
        {
            row.progress = progress;
        }
    }

    async fn load_toc<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>, first: u32) {
        let b = &mut sh.book;
        b.toc_first = first;
        b.toc_n = 0;
        let Some(f) = self.tocf.as_mut() else { return };
        let mut e = [0u8; TOC_ENTRY];
        for i in first..(first + TOC_ROWS as u32).min(b.toc_count) {
            f.set_position(u64::from(i) * TOC_ENTRY as u64);
            if !vol.read_exact_or_eof(f, &mut e).await.is_ok_and(|n| n == TOC_ENTRY) {
                break;
            }
            if let Some(t) = files::decode_toc(&e) {
                b.toc[usize::from(b.toc_n)] = t;
                b.toc_n += 1;
            }
        }
    }

    /// One contents entry from the file.
    async fn toc_entry<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        i: u32,
    ) -> Option<TocEntry> {
        if let Some(e) = sh.book.toc_row(i) {
            return Some(*e);
        }
        let f = self.tocf.as_mut()?;
        let mut e = [0u8; TOC_ENTRY];
        f.set_position(u64::from(i) * TOC_ENTRY as u64);
        if !vol.read_exact_or_eof(f, &mut e).await.is_ok_and(|n| n == TOC_ENTRY) {
            return None;
        }
        files::decode_toc(&e)
    }

    /// The page holding `offset`: from the cache, the layout's held
    /// entries, or one step of a search of the index file (`None`: not
    /// known yet; call again).
    async fn page_of<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        offset: u32,
    ) -> Option<(u32, Mark)> {
        let b = &mut sh.book;
        if !b.known(offset) {
            return None;
        }
        if let Some(hit) = b.cached_page(offset) {
            b.search = None;
            return Some(hit);
        }
        if let Some(job) = layout_of(&self.work)
            && job.tail_n > 0
            && job.tail[0].offset <= offset
        {
            let k = job.tail[..job.tail_n].iter().rposition(|m| m.offset <= offset)?;
            return Some((job.written + k as u32, job.tail[k]));
        }
        let written = match layout_of(&self.work) {
            Some(j) => j.written,
            None => b.pages,
        };
        if written == 0 {
            return None;
        }
        // The last 64-entry chunk whose first page starts at or before
        // `offset` (chunk `lo` always does: page 0 starts at 0), one
        // chunk read per call.
        let chunks = written.div_ceil(IDX_CACHE as u32);
        let mut s = match b.search {
            Some(s) if s.target == offset => s,
            _ => Search { target: offset, lo: 0, hi: chunks },
        };
        let probe = if s.hi - s.lo > 1 { (s.lo + s.hi) / 2 } else { s.lo };
        let f = match &mut self.work {
            Work::Layout(j) => &mut j.idx,
            _ => self.idx.as_mut()?,
        };
        let first = probe * IDX_CACHE as u32;
        let n = (written - first).min(IDX_CACHE as u32) as usize;
        let raw = &mut self.io[..IDX_CACHE * INDEX_ENTRY];
        f.set_position(INDEX_HEADER as u64 + u64::from(first) * INDEX_ENTRY as u64);
        let got = vol.read_exact_or_eof(f, &mut raw[..n * INDEX_ENTRY]).await.unwrap_or(0) / INDEX_ENTRY;
        for (k, c) in raw.chunks_exact(INDEX_ENTRY).take(got).enumerate() {
            b.idx_cache[k] = files::decode_mark(c);
        }
        b.idx_first = first;
        b.idx_n = got as u8;
        if got == 0 {
            b.search = None;
            return None;
        }
        if s.hi - s.lo > 1 {
            if b.idx_cache[0].offset <= offset {
                s.lo = probe;
            } else {
                s.hi = probe;
            }
            b.search = Some(s);
            return None;
        }
        b.search = None;
        let k = b.idx_cache[..got].iter().rposition(|m| m.offset <= offset)?;
        Some((first + k as u32, b.idx_cache[k]))
    }

    async fn find_page_no<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>) -> bool {
        let top = sh.book.top.offset;
        if !sh.book.known(top) {
            return false;
        }
        let was = sh.book.search;
        match self.page_of(sh, vol, top).await {
            Some((k, _)) => {
                sh.book.page_no = Some(k);
                true
            }
            None => sh.book.search != was || sh.book.search.is_some(),
        }
    }

    /// Work on the pending jump. True if it did card work or finished.
    async fn resolve<D: BlockDevice>(&mut self, sh: &mut Shelf, vol: &mut Volume<'_, D>) -> bool {
        let Some(p) = sh.book.pending else { return false };
        let top = sh.book.top;
        match p {
            Pending::Prev | Pending::Up => {
                let was = sh.book.search;
                match self.page_of(sh, vol, top.offset.saturating_sub(1)).await {
                    Some((k, m)) => {
                        sh.book.pending = None;
                        let skip = if p == Pending::Up { Skip::Before(top.offset) } else { Skip::None };
                        sh.book.view(m, skip);
                        sh.book.page_no = Some(k);
                        true
                    }
                    None => sh.book.search != was || sh.book.search.is_some(),
                }
            }
            Pending::At(at) => {
                let was = sh.book.search;
                match self.page_of(sh, vol, at).await {
                    Some((k, m)) => {
                        sh.book.pending = None;
                        sh.book.view(m, Skip::None);
                        sh.book.page_no = Some(k);
                        true
                    }
                    None => sh.book.search != was || sh.book.search.is_some(),
                }
            }
            Pending::Toc(i) => {
                sh.book.pending = None;
                if let Some(e) = self.toc_entry(sh, vol, i).await {
                    sh.book.view(e.mark(), Skip::None);
                }
                true
            }
            Pending::Chapter(next) => {
                // Binary search of the contents by offset (each probe one
                // read; the contents are in reading order).
                let (mut lo, mut hi) = (0u32, sh.book.toc_count);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let Some(e) = self.toc_entry(sh, vol, mid).await else { break };
                    if e.offset <= top.offset {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                // `lo`: the first heading after the top.
                let target = if next {
                    lo
                } else {
                    // The heading of the part we are in, or the one
                    // before if we are at its start.
                    let mut k = lo.checked_sub(1);
                    if let Some(i) = k
                        && let Some(e) = self.toc_entry(sh, vol, i).await
                        && e.offset == top.offset
                    {
                        k = i.checked_sub(1);
                    }
                    k.unwrap_or(u32::MAX)
                };
                sh.book.pending = None;
                if let Some(e) = self.toc_entry(sh, vol, target).await {
                    sh.book.view(e.mark(), Skip::None);
                }
                true
            }
        }
    }

    // ----- layout -------------------------------------------------------

    async fn layout_step<D: BlockDevice>(
        &mut self,
        sh: &mut Shelf,
        vol: &mut Volume<'_, D>,
        p: Params,
        m: &dyn Metrics,
    ) -> bool {
        let b = &mut sh.book;
        let key = p.key();
        if layout_of(&self.work).is_some_and(|j| j.key != key) {
            drop_work(&mut self.work, &mut self.orphans);
        }
        let Work::Layout(job) = &mut self.work else {
            // An index made with these parameters?
            let path = self.path.file(CACHE_DIR, b.id, "idx");
            if let Ok(mut f) = vol.open(path, OpenMode::Read).await {
                let mut h = [0u8; INDEX_HEADER];
                let ok = vol.read_exact_or_eof(&mut f, &mut h).await.is_ok_and(|n| n == INDEX_HEADER);
                if let Some(h) = IndexHeader::decode(&h)
                    && ok
                    && h.complete
                    && h.key == key
                    && h.text_len == b.text_len
                {
                    b.complete = true;
                    b.pages = h.pages;
                    b.laid = b.text_len;
                    b.frontier = b.text_len;
                    b.idx_n = 0;
                    self.idx = Some(f);
                    return true;
                }
            }
            let Ok(idx) = vol.open(path, OpenMode::Truncate).await else {
                b.complete = true;
                return true;
            };
            let Some(norm) = self.norm.clone() else { return false };
            begin_layout(&mut self.work, p, key, idx, norm);
            b.pages = 1;
            b.laid = 0;
            b.frontier = 0;
            b.idx_n = 0;
            return true;
        };
        if !job.started {
            job.started = true;
            let _ = vol
                .write(&mut job.idx, &IndexHeader { complete: false, key, ..IndexHeader::default() }.encode())
                .await;
            return true;
        }
        // Held entries out first (64 at a time).
        if job.tail_n >= 64 {
            let raw = &mut self.io[..64 * INDEX_ENTRY];
            for (k, mk) in job.tail[..64].iter().enumerate() {
                raw[k * INDEX_ENTRY..(k + 1) * INDEX_ENTRY].copy_from_slice(&files::encode_mark(*mk));
            }
            job.idx.set_position(INDEX_HEADER as u64 + u64::from(job.written) * INDEX_ENTRY as u64);
            let _ = vol.write(&mut job.idx, raw).await;
            job.tail.copy_within(64..job.tail_n, 0);
            job.tail_n -= 64;
            job.written += 64;
            return true;
        }
        let at = job.pg.next_byte();
        if at < b.text_len {
            let len = (b.text_len - at).min(512) as usize;
            job.norm.set_position(u64::from(at) + NORM_HEADER as u64);
            let n = vol.read_exact_or_eof(&mut job.norm, &mut job.buf[..len]).await.unwrap_or(0);
            if n == 0 {
                b.complete = true;
                return true;
            }
            // In pieces of 64 bytes: each adds at most 64 entries.
            let mut done = 0;
            while done < n {
                if job.tail_n >= 64 {
                    // Not all taken: the rest next time (pos tells).
                    break;
                }
                let end = (done + 64).min(n);
                let (tail, tail_n) = (&mut job.tail, &mut job.tail_n);
                job.pg.feed(&job.buf[done..end], m, &mut |mk| {
                    if let Some(s) = tail.get_mut(*tail_n) {
                        *s = mk;
                        *tail_n += 1;
                    }
                });
                done = end;
            }
            b.pages = job.pg.pages;
            b.laid = job.pg.pos();
            if job.tail_n > 0 {
                b.frontier = job.tail[job.tail_n - 1].offset;
            }
            return true;
        }
        // The end: the last pages, all entries, the header.
        let (tail, tail_n) = (&mut job.tail, &mut job.tail_n);
        job.pg.finish(m, &mut |mk| {
            if let Some(s) = tail.get_mut(*tail_n) {
                *s = mk;
                *tail_n += 1;
            }
        });
        let raw = &mut self.io[..TAIL * INDEX_ENTRY];
        for (k, mk) in job.tail[..job.tail_n].iter().enumerate() {
            raw[k * INDEX_ENTRY..(k + 1) * INDEX_ENTRY].copy_from_slice(&files::encode_mark(*mk));
        }
        job.idx.set_position(INDEX_HEADER as u64 + u64::from(job.written) * INDEX_ENTRY as u64);
        let _ = vol.write(&mut job.idx, &raw[..job.tail_n * INDEX_ENTRY]).await;
        let pages = job.written + job.tail_n as u32;
        job.idx.set_position(0);
        let h = IndexHeader { complete: true, key, text_len: b.text_len, pages };
        let _ = vol.write(&mut job.idx, &h.encode()).await;
        let _ = vol.write_entry(&mut job.idx).await;
        self.flush = true;
        b.pages = pages;
        b.laid = b.text_len;
        b.complete = true;
        b.idx_n = 0;
        if let Work::Layout(j) = core::mem::replace(&mut self.work, Work::Idle) {
            self.idx = Some(j.idx);
        }
        true
    }

    /// The layout job, while the book is laid out.
    fn layout(&self) -> Option<&LayoutJob> {
        layout_of(&self.work)
    }
}

// ----- building in place ------------------------------------------------
//
// Each job and parser is built in a function of its own (not inlined):
// built inside `Desk::step_with`, the values (the search 4 KB, the
// conversion 4.7 KB, the EPUB job 2.9 KB …) were all temporaries of its
// poll function's frame at once, 10,040 B of the card task's stack on the
// player (`cargo xtask stack`, 2026-11-26). Here each is a leaf's frame
// for the moment it is built.

#[inline(never)]
fn begin_scan(w: &mut Work) {
    // The literal here, not `ScanJob::new()`: written straight into `w`
    // (as `begin_convert`'s is); through the call it was built in a
    // 4,544 B temporary and copied (PG-2, the card task's deepest path).
    *w = Work::Scan(ScanJob {
        stack: [None; SCAN_DEPTH],
        depth: 0,
        path: PathBuf::new(),
        lib: None,
        count: 0,
        keys: [0; MAX_BOOKS],
        head: Head::None,
        cand: None,
        rec: LibRecord::default(),
        started: false,
    });
}

#[inline(never)]
fn begin_convert(w: &mut Work, src: File) {
    *w = Work::Convert(ConvJob {
        c: Parse::None,
        src,
        read: 0,
        inbuf: [0; 512],
        in_n: 0,
        in_at: 0,
        out: OutBuf { b: [0; OUT], n: 0 },
        norm: None,
        toc: None,
        tables: [None, None, None],
        toc_n: 0,
        eof: false,
    });
}

#[inline(never)]
fn begin_layout(w: &mut Work, p: Params, key: u32, idx: File, norm: File) {
    *w = Work::Layout(LayoutJob {
        pg: Paginator::new(p),
        key,
        idx,
        norm,
        written: 0,
        tail: [Mark::START; TAIL],
        tail_n: 1,
        buf: [0; 512],
        started: false,
    });
}

#[inline(never)]
fn begin_epub(c: &mut Parse, size: u32) {
    *c = Parse::Epub(Epub::new(size));
}

#[inline(never)]
fn begin_plain(c: &mut Parse, format: Format, head: &[u8], eof: bool) {
    *c = Parse::Plain(Converter::new(format, head, eof));
}

/// A search candidate's record (path, format, size, time; the file name
/// as its title until the head names one) and its head's reader, in the
/// job. False if its path does not fit a record.
#[inline(never)]
fn begin_meta(job: &mut ScanJob, entry: &DirEntry, format: Format, f: File) -> bool {
    let name = entry.name();
    let rec = &mut job.rec;
    rec.clear();
    let here = job.path.len();
    let fits = job.path.push("/") && job.path.push(name) && rec.set_path(job.path.as_str());
    job.path.truncate(here);
    if !fits {
        return false;
    }
    rec.format = format;
    rec.size = u32::try_from(entry.size()).unwrap_or(u32::MAX);
    rec.stamp = files::stamp(entry.modified());
    // The title if the head does not give one: the name.
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    rec.title.push_str(stem);
    let size = rec.size;
    if format == Format::Epub {
        epub_head(&mut job.head, size);
    } else {
        plain_head(&mut job.head, format);
    }
    job.cand = Some(f);
    true
}

#[inline(never)]
fn epub_head(h: &mut Head, size: u32) {
    *h = Head::Epub(Epub::library(size));
}

#[inline(never)]
fn plain_head(h: &mut Head, format: Format) {
    *h = Head::Plain(MetaReader::new(format, &[], false));
}

#[inline(never)]
fn new_reader(reader: &mut MetaReader, format: Format, head: &[u8], eof: bool) {
    *reader = MetaReader::new(format, head, eof);
}

/// The title and author the head reader found (the reader is spent).
#[inline(never)]
fn finish_reader(reader: &mut MetaReader, format: Format) -> (MetaText<64>, MetaText<48>) {
    let done = core::mem::replace(reader, MetaReader::new(format, &[], false));
    let meta = done.finish();
    (meta.title, meta.author)
}

/// The book's library record `raw` (number `rec_i`) into the open book.
#[inline(never)]
fn take_record(b: &mut Book, raw: &[u8], rec_i: u16) {
    let r = LibRecord::decode(raw);
    b.rec = rec_i;
    b.src.clear();
    b.src.push(r.path());
    b.id = files::book_id(r.path());
    b.title = r.title;
    b.format = r.format;
}

/// The position and bookmarks from their file's bytes, if they decode.
#[inline(never)]
fn take_state(b: &mut Book, raw: &[u8]) -> bool {
    match BookState::decode(raw) {
        Some(s) => {
            b.state = s;
            true
        }
        None => false,
    }
}

/// The reading state waiting in the model, its bytes in `io`: the book's
/// id, record, row and progress.
#[inline(never)]
fn stage_save(sh: &mut Shelf, io: &mut [u8; IO]) -> Option<(u32, u16, u16, u16)> {
    let r = sh.save.as_ref()?;
    if r.ram_only {
        // The suite's book: its state stays in RAM (`Book::ram_only`).
        sh.save = None;
        return None;
    }
    io[..STATE_BYTES].copy_from_slice(&r.state.encode());
    let out = (r.id, r.rec, r.row, permille(r.state.pos.offset, r.text_len));
    sh.save = None;
    Some(out)
}

/// A library title to rewrite: its bytes in `io`, the record.
#[inline(never)]
fn stage_names(retitle: &mut Option<(u16, MetaText<64>, MetaText<48>)>, io: &mut [u8; IO]) -> Option<u16> {
    let (rec, title, author) = retitle.take()?;
    io[..files::LIB_NAMES].copy_from_slice(&files::encode_names(&title, &author));
    Some(rec)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::format;

    use super::*;

    /// `Book::reset` (in place, for the player's stack: 2026-11-26) leaves
    /// the book as `Book { phase, row, ..Book::default() }` did, but for
    /// the windows' bytes, which their lengths say are not held.
    #[test]
    fn book_reset_is_a_fresh_book() {
        let mut b = Book { phase: Phase::Ready, text_len: 9000, ..Book::default() };
        b.title.push_str("Old");
        b.src.push("/Books/old.txt");
        b.top = Mark { offset: 77, ..Mark::START };
        b.win[0] = 1;
        b.win_len = 1;
        b.win_ok = true;
        b.toc_n = 2;
        b.idx_n = 3;
        b.pending = Some(Pending::Prev);
        b.state.add(Bookmark::default());
        b.dirty = true;
        b.reset(Phase::Opening, 5);
        let fresh = Book { phase: Phase::Opening, row: 5, ..Book::default() };
        // The windows' and the path's bytes past their lengths are not
        // part of the comparison.
        assert_eq!(b.src.as_str(), "");
        b.src = fresh.src;
        b.win = fresh.win;
        b.toc = fresh.toc;
        b.idx_cache = fresh.idx_cache;
        assert_eq!(format!("{b:?}"), format!("{fresh:?}"));
        assert!(b.window().is_none() && b.toc_row(0).is_none());
    }
}
