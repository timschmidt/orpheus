//! The Books app's model and its card work (05 §7.15): the library, the
//! open book and the reader's view, changed by [`Req`]uests from the
//! screens and brought up to date by [`Shelf::step`], one bounded piece of
//! card work at a time (at most a few sector reads and writes: the
//! firmware runs it as a paced chore of its card task, the simulator
//! between frames).
//!
//! What a step does, most urgent first: write the reading state; load or
//! search the library; open a book (state, cache check, conversion);
//! fetch the reader's text window; answer a pending jump (previous page,
//! line up, go to per mille, chapter, contents entry) from the page index;
//! fetch contents rows; lay the book out (the background pass that writes
//! the page index).
//!
//! RAM: [`Shelf`] is a few kilobytes, all fixed (sizes in the tests and
//! docs/STATUS.md): the 2 KiB text window, the library's row window and
//! sort order, the contents window, the page-index cache, the book's
//! state with its bookmarks, and the card work of the moment (the scan's
//! directory entry, the converter, or the paginator, never two at once).

use orpheus_fs::{BlockDevice, DirCursor, DirEntry, File, OpenMode, Volume};

use crate::convert::{Converter, Format, MetaReader};
use crate::epub::{Epub, Step, Store, Table};
use crate::fb2::MetaText;
use crate::files::{
    self, BOOKS_DIR, BookState, Bookmark, CACHE_DIR, INDEX_ENTRY, INDEX_HEADER, IndexHeader, LIB_HEADER,
    LIB_ORDER, LIB_PROGRESS_AT, LIB_RECORD, LIBRARY_PATH, LibHeader, LibRecord, MAX_BOOKS, NORM_HEADER,
    NormHeader, PathBuf, SNIPPET, STATE_BYTES, TOC_ENTRY,
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
}

impl Scope {
    #[must_use]
    pub fn root(self) -> &'static str {
        match self {
            Scope::BooksFolder => BOOKS_FOLDER,
            Scope::WholeCard => "/",
        }
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
    /// An EPUB's conversion waits for the scratch ([`Shelf::step_with`]).
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
            idx_cache: [Mark::START; IDX_CACHE],
            idx_first: 0,
            idx_n: 0,
        }
    }
}

impl Book {
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
}

impl SaveRec {
    fn of(b: &Book) -> SaveRec {
        SaveRec { id: b.id, rec: b.rec, row: b.row, text_len: b.text_len, state: b.state }
    }
}

/// How a candidate's title and author are read.
#[allow(clippy::large_enum_variant)]
enum Head {
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
    entry: DirEntry,
    path: PathBuf,
    lib: Option<File>,
    count: u16,
    keys: [u32; MAX_BOOKS],
    /// A candidate's head being read.
    meta: Option<(Head, File, LibRecord)>,
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

/// The Books app's model and card work.
pub struct Shelf {
    pub lib: Library,
    pub book: Book,
    /// The layout the reader asked for (and the font id it measures with).
    pub params: Option<Params>,
    work: Work,
    /// Reading files of the open book (read only after conversion).
    norm: Option<File>,
    idx: Option<File>,
    tocf: Option<File>,
    path: PathBuf,
    /// Open asked for this library row.
    open: Option<u16>,
    /// A reading state to write (the open book's, or the one before).
    save: Option<SaveRec>,
    /// Folders of [`files::DIRS`] made so far (one per step: making a
    /// folder zeroes a cluster).
    dirs: u8,
    /// Dirty sectors to write back, one per step.
    flush: bool,
    close: bool,
    /// An EPUB's package named its title and author: its library record
    /// to rewrite (a step of its own).
    retitle: Option<(u16, MetaText<64>, MetaText<48>)>,
    /// Files a job wrote and left (given up, or the EPUB job's tables):
    /// their entries are written, one per step, before other work (so no
    /// cluster is lost and a file is not opened again before).
    orphans: [Option<File>; 5],
    /// Step count (tests).
    pub steps: u32,
}

impl Default for Shelf {
    fn default() -> Self {
        Shelf::new()
    }
}

impl Shelf {
    #[must_use]
    pub fn new() -> Shelf {
        Shelf {
            lib: Library::default(),
            book: Book::default(),
            params: None,
            work: Work::Idle,
            norm: None,
            idx: None,
            tocf: None,
            path: PathBuf::new(),
            open: None,
            save: None,
            dirs: 0,
            flush: false,
            close: false,
            retitle: None,
            orphans: [None, None, None, None, None],
            steps: 0,
        }
    }

    /// A screen's request. Cheap: the card work happens in [`Shelf::step`].
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
                if !matches!(self.work, Work::Scan(_)) {
                    self.lib.rescan = Some(scope);
                }
            }
            Req::Rows(first) => self.lib.want = Some(first),
            Req::Open(row) => {
                if b.dirty && b.phase == Phase::Ready {
                    self.save = Some(SaveRec::of(b));
                }
                self.close_now();
                self.open = Some(row);
                self.book = Book { phase: Phase::Opening, row, ..Book::default() };
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
                        if matches!(self.work, Work::Layout(_)) {
                            drop_work(&mut self.work, &mut self.orphans);
                        }
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

    /// Drop the open book's files (its state is written by the close
    /// step first when it has changed).
    fn close_now(&mut self) {
        self.norm = None;
        self.idx = None;
        self.tocf = None;
        if matches!(self.work, Work::Convert(_) | Work::Layout(_)) {
            drop_work(&mut self.work, &mut self.orphans);
        }
    }

    /// One piece of card work. True if the model changed (the screens
    /// look again). Without the scratch: an EPUB waits for it
    /// ([`Shelf::step_with`]).
    pub async fn step<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, m: &dyn Metrics) -> bool {
        self.step_with(vol, m, None).await
    }

    /// Card work wants the scratch ([`crate::inflate::SCRATCH`] bytes):
    /// an EPUB being converted or its title read. Lend it, unchanged,
    /// to every step while this says so (the decoder arena when no track
    /// is loaded, 05 §7.15); a conversion that loses it in the middle of
    /// an entry starts again, a title read falls back to the file name.
    #[must_use]
    pub fn needs_scratch(&self) -> bool {
        match &self.work {
            Work::Convert(_) => self.book.format == Format::Epub,
            Work::Scan(j) => matches!(j.meta, Some((Head::Epub(_), ..))),
            _ => false,
        }
    }

    /// [`Shelf::step`] with the scratch lent (EPUB: the inflate window and
    /// tables).
    pub async fn step_with<D: BlockDevice>(
        &mut self,
        vol: &mut Volume<'_, D>,
        m: &dyn Metrics,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        self.steps = self.steps.wrapping_add(1);
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
        // Folders a job needs, one per step.
        if usize::from(self.dirs) < files::DIRS.len() && self.needs_dirs() {
            let _ = vol.create_dir(files::DIRS[usize::from(self.dirs)]).await;
            self.dirs += 1;
            return true;
        }
        // 1. The reading state: when the reader closes, another book
        // opens, or often enough while reading.
        if self.book.dirty && self.book.moves >= SAVE_EVERY && self.book.phase == Phase::Ready {
            self.save = Some(SaveRec::of(&self.book));
            self.book.dirty = false;
            self.book.moves = 0;
        }
        if let Some(rec) = self.save.take() {
            self.save_state(vol, &rec).await;
            return true;
        }
        if let Some((rec, title, author)) = self.retitle.take() {
            if let Ok(mut lf) = vol.open(LIBRARY_PATH, OpenMode::ReadWrite).await {
                lf.set_position(files::lib_record_at(rec) + files::LIB_NAMES_AT as u64);
                if vol.write(&mut lf, &files::encode_names(&title, &author)).await.is_ok() {
                    let _ = vol.close(lf).await;
                }
            }
            self.flush = true;
            return true;
        }
        if self.close {
            self.close = false;
            self.close_now();
            self.book.phase = Phase::Closed;
            return true;
        }
        // 2. The library.
        if let Some(scope) = self.lib.rescan.take() {
            self.lib.scope = scope;
            self.lib.phase = LibPhase::Searching;
            self.lib.found = 0;
            self.lib.count = 0;
            self.lib.loaded = 0;
            self.work = Work::Scan(ScanJob::new());
            return true;
        }
        if let Work::Scan(_) = self.work {
            return self.scan_step(vol, scratch).await;
        }
        if self.lib.phase == LibPhase::Loading {
            self.load_library(vol).await;
            return true;
        }
        if let Some(first) = self.lib.want
            && self.lib.phase == LibPhase::Ready
        {
            return self.rows_step(vol, first).await;
        }
        // 3. Opening a book.
        if let Some(row) = self.open.take() {
            self.open_book(vol, row).await;
            return true;
        }
        if let Work::Convert(_) = self.work {
            return self.convert_step(vol, scratch).await;
        }
        if self.book.phase != Phase::Ready {
            return false;
        }
        // 4. The reader: its window, jumps, contents, then the layout.
        if !self.book.win_ok {
            self.load_window(vol).await;
            return true;
        }
        if self.book.pending.is_some() && self.resolve(vol).await {
            return true;
        }
        if self.book.page_no.is_none() && self.find_page_no(vol).await {
            return true;
        }
        if let Some(first) = self.book.toc_want.take() {
            self.load_toc(vol, first).await;
            return true;
        }
        if !self.book.complete
            && let Some(p) = self.params
        {
            return self.layout_step(vol, p, m).await;
        }
        false
    }

    // ----- library ------------------------------------------------------

    async fn load_library<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>) {
        let mut buf = [0u8; LIB_HEADER + LIB_ORDER];
        let ok = match vol.open(LIBRARY_PATH, OpenMode::Read).await {
            Ok(mut f) => vol.read_exact_or_eof(&mut f, &mut buf).await.is_ok_and(|n| n == buf.len()),
            Err(_) => false,
        };
        match LibHeader::decode(&buf).filter(|h| ok && h.complete) {
            Some(h) if h.scope == self.lib.scope as u8 => {
                self.lib.count = h.count;
                let n = usize::from(h.count).min(MAX_BOOKS);
                self.lib.order[..n].copy_from_slice(&buf[LIB_HEADER..LIB_HEADER + n]);
                self.lib.phase = LibPhase::Ready;
                self.lib.loaded = 0;
                self.lib.want.get_or_insert(0);
            }
            _ => {
                // None yet, damaged, or of the other scope: search.
                self.lib.rescan = Some(self.lib.scope);
            }
        }
    }

    async fn rows_step<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, first: u16) -> bool {
        let lib = &mut self.lib;
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
        let Ok(mut f) = vol.open(LIBRARY_PATH, OpenMode::Read).await else {
            lib.want = None;
            lib.phase = LibPhase::Unknown;
            return true;
        };
        let mut rec = [0u8; LIB_RECORD];
        for row in have..end.min(have + 2) {
            let Some(r) = lib.record_of(row as u16) else { break };
            f.set_position(files::lib_record_at(r));
            let ok = vol.read_exact_or_eof(&mut f, &mut rec).await.is_ok_and(|n| n == LIB_RECORD);
            let d = if ok { LibRecord::decode(&rec) } else { LibRecord::default() };
            if let Some(slot) = lib.rows.get_mut(usize::from(lib.loaded)) {
                *slot = BookRow { title: d.title, author: d.author, format: d.format, progress: d.progress };
            }
            lib.loaded += 1;
        }
        true
    }

    /// Card work that writes is waiting (its folders are made first).
    fn needs_dirs(&self) -> bool {
        self.save.is_some()
            || matches!(&self.work, Work::Scan(j) if !j.started)
            || matches!(&self.work, Work::Convert(j) if matches!(j.c, Parse::None))
            || (self.book.phase == Phase::Ready && !self.book.complete && self.layout().is_none())
    }

    async fn scan_step<D: BlockDevice>(
        &mut self,
        vol: &mut Volume<'_, D>,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let scope = self.lib.scope;
        let Work::Scan(job) = &mut self.work else { return false };
        if !job.started {
            job.started = true;
            let Ok(mut f) = vol.open(LIBRARY_PATH, OpenMode::Truncate).await else {
                drop_work(&mut self.work, &mut self.orphans);
                self.lib.phase = LibPhase::Ready;
                return true;
            };
            let _ = vol.write(&mut f, &[0u8; LIB_HEADER + LIB_ORDER]).await;
            job.lib = Some(f);
            match vol.open_dir(scope.root()).await {
                Ok(c) => {
                    job.stack[0] = Some((c, 0));
                    job.depth = 1;
                    job.path.clear();
                    if scope == Scope::BooksFolder {
                        job.path.push(BOOKS_FOLDER);
                    }
                }
                // No books folder: an empty library.
                Err(_) => job.depth = 0,
            }
            return true;
        }
        if job.meta.is_some() {
            return Self::meta_step(job, vol, &mut self.lib, scratch).await;
        }
        if job.depth == 0 || usize::from(job.count) >= MAX_BOOKS {
            Self::finish_scan(job, vol, &mut self.lib, scope).await;
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
            let more = vol.read_dir(&mut cur, &mut job.entry).await.unwrap_or(false);
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
            let name = job.entry.name();
            let attr = job.entry.attributes();
            if name.starts_with('.')
                || attr & 0x06 != 0
                || name.eq_ignore_ascii_case("System Volume Information")
            {
                continue;
            }
            if job.entry.is_dir() {
                if usize::from(job.depth) < SCAN_DEPTH {
                    let here = job.path.len();
                    if let Ok(c) = vol.dir_cursor(&job.entry)
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
            let mut rec = LibRecord::default();
            let here = job.path.len();
            let fits = job.path.push("/") && job.path.push(name) && rec.set_path(job.path.as_str());
            job.path.truncate(here);
            if !fits {
                continue;
            }
            rec.format = format;
            rec.size = u32::try_from(job.entry.size()).unwrap_or(u32::MAX);
            rec.stamp = files::stamp(job.entry.modified());
            // The title if the head does not give one: the name.
            let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
            rec.title.push_str(stem);
            if let Ok(f) = vol.open_entry(&job.entry) {
                let head = if format == Format::Epub {
                    Head::Epub(Epub::library(rec.size))
                } else {
                    Head::Plain(MetaReader::new(format, &[], false))
                };
                job.meta = Some((head, f, rec));
                return true;
            }
        }
        true
    }

    /// One read of a candidate's head (an EPUB: one step of its package
    /// reader); at the end its record is written.
    async fn meta_step<D: BlockDevice>(
        job: &mut ScanJob,
        vol: &mut Volume<'_, D>,
        lib: &mut Library,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let Some((head, mut f, mut rec)) = job.meta.take() else { return false };
        let (title, author) = match head {
            Head::Plain(mut reader) => {
                let mut buf = [0u8; 512];
                let n = vol.read(&mut f, &mut buf).await.unwrap_or(0);
                if reader.fed == 0 {
                    reader = MetaReader::new(rec.format, &buf[..n], n < buf.len());
                }
                reader.feed(&buf[..n]);
                if n == buf.len() && reader.wants_more() {
                    job.meta = Some((Head::Plain(reader), f, rec));
                    return true;
                }
                let meta = reader.finish();
                (meta.title, meta.author)
            }
            Head::Epub(mut e) => {
                let mut st = VolStore { vol, src: &mut f, tables: None, toc: None };
                match e.step(&mut st, scratch, &mut Discard).await {
                    Step::Busy => {
                        job.meta = Some((Head::Epub(e), f, rec));
                        return true;
                    }
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
        if let Some(lf) = job.lib.as_mut() {
            lf.set_position(files::lib_record_at(job.count));
            if vol.write(lf, &rec.encode()).await.is_ok() {
                job.keys[usize::from(job.count)] = rec.sort_key();
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
        scope: Scope,
    ) {
        let n = usize::from(job.count);
        // Sorted by title key (insertion sort: at most 200), ties in the
        // order found.
        let mut order = [0u8; MAX_BOOKS];
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
        let mut head = [0u8; LIB_HEADER + LIB_ORDER];
        head[..LIB_HEADER]
            .copy_from_slice(&LibHeader { count: job.count, scope: scope as u8, complete: true }.encode());
        head[LIB_HEADER..LIB_HEADER + n].copy_from_slice(&order[..n]);
        if let Some(mut f) = job.lib.take() {
            f.set_position(0);
            let _ = vol.write(&mut f, &head).await;
            let _ = vol.close(f).await;
        }
        lib.order = order;
        lib.count = job.count;
        lib.found = job.count;
        lib.phase = LibPhase::Ready;
        lib.first = 0;
        lib.loaded = 0;
        lib.want = Some(0);
    }

    // ----- opening ------------------------------------------------------

    async fn open_book<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, row: u16) {
        let b = &mut self.book;
        let fail = |b: &mut Book| b.phase = Phase::Failed;
        let Some(rec_i) = self.lib.record_of(row) else { return fail(b) };
        // The record: path, title, format.
        let Ok(mut lf) = vol.open(LIBRARY_PATH, OpenMode::Read).await else { return fail(b) };
        let mut rec = [0u8; LIB_RECORD];
        lf.set_position(files::lib_record_at(rec_i));
        if !vol.read_exact_or_eof(&mut lf, &mut rec).await.is_ok_and(|n| n == LIB_RECORD) {
            return fail(b);
        }
        let r = LibRecord::decode(&rec);
        b.rec = rec_i;
        b.src.clear();
        b.src.push(r.path());
        b.id = files::book_id(r.path());
        b.title = r.title;
        b.format = r.format;
        // The book as it is now.
        let mut e = DirEntry::new();
        if vol.stat(r.path(), &mut e).await.is_err() {
            return fail(b);
        }
        b.src_size = u32::try_from(e.size()).unwrap_or(u32::MAX);
        b.src_stamp = files::stamp(e.modified());
        // Position and bookmarks.
        let mut st = [0u8; STATE_BYTES];
        if let Ok(mut f) = vol.open(self.path.file(BOOKS_DIR, b.id, "pos"), OpenMode::Read).await
            && vol.read_exact_or_eof(&mut f, &mut st).await.is_ok_and(|n| n == STATE_BYTES)
            && let Some(s) = BookState::decode(&st)
        {
            b.state = s;
        }
        // The cache, if it is of this book as it is.
        let mut h = [0u8; NORM_HEADER];
        let cached = match vol.open(self.path.file(CACHE_DIR, b.id, "txt"), OpenMode::Read).await {
            Ok(mut f) => {
                let ok = vol.read_exact_or_eof(&mut f, &mut h).await.is_ok_and(|n| n == NORM_HEADER);
                NormHeader::decode(&h)
                    .filter(|h| ok && h.complete && h.src_size == b.src_size && h.src_stamp == b.src_stamp)
                    .map(|h| (h, f))
            }
            Err(_) => None,
        };
        match cached {
            Some((h, f)) => {
                b.lang = h.lang;
                self.norm = Some(f);
                self.tocf = vol.open(self.path.file(CACHE_DIR, b.id, "toc"), OpenMode::Read).await.ok();
                Self::ready(b, h.text_len, h.toc_count);
            }
            None => {
                let Ok(src) = vol.open(r.path(), OpenMode::Read).await else { return fail(b) };
                b.phase = Phase::Converting;
                b.permille = 0;
                self.work = Work::Convert(ConvJob {
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
        vol: &mut Volume<'_, D>,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let Work::Convert(job) = &mut self.work else { return false };
        let b = &mut self.book;
        if matches!(job.c, Parse::Epub(_)) {
            return self.epub_step(vol, scratch).await;
        }
        if matches!(job.c, Parse::None) {
            if b.format == Format::Epub {
                job.c = Parse::Epub(Epub::new(b.src_size));
                job.tables = [None, None, None];
                job.out.n = 0;
            } else {
                // The head: the encoding and (plain text) the line style.
                let n = vol.read(&mut job.src, &mut job.inbuf).await.unwrap_or(0);
                job.in_n = n;
                job.in_at = 0;
                job.read = n as u32;
                job.eof = n < job.inbuf.len();
                job.c = Parse::Plain(Converter::new(b.format, &job.inbuf[..n], job.eof));
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
        vol: &mut Volume<'_, D>,
        scratch: Option<&mut [u8]>,
    ) -> bool {
        let Work::Convert(job) = &mut self.work else { return false };
        let b = &mut self.book;
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
                    if let Some(k) = b.row.checked_sub(self.lib.first)
                        && k < u16::from(self.lib.loaded)
                        && let Some(row) = self.lib.rows.get_mut(usize::from(k))
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

    async fn load_window<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>) {
        let b = &mut self.book;
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

    async fn save_state<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, r: &SaveRec) {
        let data = r.state.encode();
        let path = self.path.file(BOOKS_DIR, r.id, "pos");
        // In place when it exists (one sector, nothing allocated).
        let mut done = false;
        if let Ok(mut f) = vol.open(path, OpenMode::ReadWrite).await
            && vol.rewrite(&mut f, &data).await.unwrap_or(false)
        {
            done = vol.write_entry(&mut f).await.is_ok();
        }
        if !done
            && let Ok(mut f) = vol.open(path, OpenMode::Truncate).await
            && vol.write(&mut f, &data).await.is_ok()
        {
            let _ = vol.close(f).await;
        }
        // The library's progress for this book.
        let progress = permille(r.state.pos.offset, r.text_len);
        if let Ok(mut lf) = vol.open(LIBRARY_PATH, OpenMode::ReadWrite).await {
            lf.set_position(files::lib_record_at(r.rec) + LIB_PROGRESS_AT as u64);
            if vol.write(&mut lf, &progress.to_le_bytes()).await.is_ok() {
                let _ = vol.close(lf).await;
            }
        }
        self.flush = true;
        if let Some(k) = r.row.checked_sub(self.lib.first)
            && k < u16::from(self.lib.loaded)
            && let Some(row) = self.lib.rows.get_mut(usize::from(k))
        {
            row.progress = progress;
        }
    }

    async fn load_toc<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, first: u32) {
        let b = &mut self.book;
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
    async fn toc_entry<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, i: u32) -> Option<TocEntry> {
        if let Some(e) = self.book.toc_row(i) {
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
    async fn page_of<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>, offset: u32) -> Option<(u32, Mark)> {
        let b = &mut self.book;
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
        let mut raw = [0u8; IDX_CACHE * INDEX_ENTRY];
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

    async fn find_page_no<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>) -> bool {
        let top = self.book.top.offset;
        if !self.book.known(top) {
            return false;
        }
        let was = self.book.search;
        match self.page_of(vol, top).await {
            Some((k, _)) => {
                self.book.page_no = Some(k);
                true
            }
            None => self.book.search != was || self.book.search.is_some(),
        }
    }

    /// Work on the pending jump. True if it did card work or finished.
    async fn resolve<D: BlockDevice>(&mut self, vol: &mut Volume<'_, D>) -> bool {
        let Some(p) = self.book.pending else { return false };
        let top = self.book.top;
        match p {
            Pending::Prev | Pending::Up => {
                let was = self.book.search;
                match self.page_of(vol, top.offset.saturating_sub(1)).await {
                    Some((k, m)) => {
                        self.book.pending = None;
                        let skip = if p == Pending::Up { Skip::Before(top.offset) } else { Skip::None };
                        self.book.view(m, skip);
                        self.book.page_no = Some(k);
                        true
                    }
                    None => self.book.search != was || self.book.search.is_some(),
                }
            }
            Pending::At(at) => {
                let was = self.book.search;
                match self.page_of(vol, at).await {
                    Some((k, m)) => {
                        self.book.pending = None;
                        self.book.view(m, Skip::None);
                        self.book.page_no = Some(k);
                        true
                    }
                    None => self.book.search != was || self.book.search.is_some(),
                }
            }
            Pending::Toc(i) => {
                self.book.pending = None;
                if let Some(e) = self.toc_entry(vol, i).await {
                    self.book.view(e.mark(), Skip::None);
                }
                true
            }
            Pending::Chapter(next) => {
                // Binary search of the contents by offset (each probe one
                // read; the contents are in reading order).
                let (mut lo, mut hi) = (0u32, self.book.toc_count);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let Some(e) = self.toc_entry(vol, mid).await else { break };
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
                        && let Some(e) = self.toc_entry(vol, i).await
                        && e.offset == top.offset
                    {
                        k = i.checked_sub(1);
                    }
                    k.unwrap_or(u32::MAX)
                };
                self.book.pending = None;
                if let Some(e) = self.toc_entry(vol, target).await {
                    self.book.view(e.mark(), Skip::None);
                }
                true
            }
        }
    }

    // ----- layout -------------------------------------------------------

    async fn layout_step<D: BlockDevice>(
        &mut self,
        vol: &mut Volume<'_, D>,
        p: Params,
        m: &dyn Metrics,
    ) -> bool {
        let b = &mut self.book;
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
            self.work = Work::Layout(LayoutJob {
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
            let mut raw = [0u8; 64 * INDEX_ENTRY];
            for (k, mk) in job.tail[..64].iter().enumerate() {
                raw[k * INDEX_ENTRY..(k + 1) * INDEX_ENTRY].copy_from_slice(&files::encode_mark(*mk));
            }
            job.idx.set_position(INDEX_HEADER as u64 + u64::from(job.written) * INDEX_ENTRY as u64);
            let _ = vol.write(&mut job.idx, &raw).await;
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
        let mut raw = [0u8; TAIL * INDEX_ENTRY];
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

impl ScanJob {
    fn new() -> ScanJob {
        ScanJob {
            stack: [None; SCAN_DEPTH],
            depth: 0,
            entry: DirEntry::new(),
            path: PathBuf::new(),
            lib: None,
            count: 0,
            keys: [0; MAX_BOOKS],
            meta: None,
            started: false,
        }
    }
}
