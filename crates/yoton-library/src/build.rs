//! Building the library: a folder walk that reads tags into the catalogue,
//! then a fixed plan of external sorts that derive every table from it.
//!
//! Stages ([`PLAN`]), each advanced by [`Builder::step`] with bounded work:
//!
//! 1. **Scan**: breadth-first walk with a queue file (`walk.tmp`: folder
//!    locator, depth, name). A folder the previous library knows is first
//!    read once for its signature; unchanged, its catalogue bytes are
//!    copied; changed, it is read again and every file whose stamps match
//!    its old item keeps it. New folders are read once. Catalogue items go
//!    straight to the new slot file; one `DIRS` row per folder with tracks
//!    to `dirs.tmp`.
//! 2. **Dirs**: `dirs.tmp` sorted by folder identity → `DIRS`.
//! 3. **Strings**: every field's value at the start of each run of equal
//!    values along the catalogue, sorted by (kind, unknown last, sort key,
//!    raw string) → `STR` (one entry per folded-equal group) and a join
//!    list (first catalogue ordinal, field, string reference).
//! 4. **Join**: the join list sorted by ordinal.
//! 5. **Tracks**: catalogue + join list → track items sorted by (album
//!    artist, album, disc, track, title, locator) → `TRK`; artist and album
//!    groups and the artists' letter runs to temporary lists.
//! 6. **Songs**, **Genre**, **Recent**, **ByLoc**: orders of `TRK` (the
//!    genre order also yields its group tables and letter runs).
//! 7. Temporary lists copied into their sections; **AlbYear**,
//!    **AlbName**, **GAlbYear**: orders of the album tables.
//! 8. **Finish**: sync, header, sync; temporary files and the previous
//!    slot removed.
//!
//! Scratch layout of the sort stages: `[0, 1024)` input buffer, five
//! 512-byte stream buffers, 640 bytes of auxiliary space, the rest for the
//! sorter. Scan: old-catalogue buffer 1 KiB, three 512-byte output
//! buffers, folder name 128 B, entry name 768 B, the rest (≤ 16 KiB) the
//! tag window.

use core::cmp::Ordering;

use yoton_apps::browse::{EntryKind, is_hidden_name, kind_of};
use yoton_fs::Locator;
use yoton_index::{Error, ExtSort, FileName, Signature, SrcEntry, Storage};
use yoton_meta::text::CodePage;
use yoton_meta::window::{Budget, ReadAt, read_budgeted};
use yoton_meta::{Opts, Tags};
use yoton_player::name_hash;

use crate::collate;
use crate::format::{
    CatFixed, DIR_ROW, GRP_ROW, Group, HEADER, Header, Kind, LETTER_RUN, Sec, SecInfo, TRK_ROW, be32, cat,
    cat_fields, grp, rd16, rd32, rd64, trk, wr16, wr32,
};
use crate::io::{In, Out};
use crate::query::Library;
use crate::{FileFormat, TagSource, flags};

/// Smallest scratch buffer a build accepts.
pub const MIN_SCRATCH: usize = 8192;

const fn fname(s: &str) -> FileName {
    match FileName::new(s) {
        Some(f) => f,
        None => panic!("bad file name"),
    }
}

/// The two library slots.
pub(crate) const SLOTS: [FileName; 2] = [fname("lib-a.ydb"), fname("lib-b.ydb")];

const T_WALK: FileName = fname("walk.tmp");
const T_DIRS: FileName = fname("dirs.tmp");
const T_JOIN: FileName = fname("join.tmp");
const T_JSORT: FileName = fname("jsort.tmp");
const T_TART: FileName = fname("t-art.tmp");
const T_TALB: FileName = fname("t-alb.tmp");
const T_TLET: FileName = fname("t-let.tmp");
const T_SLET: FileName = fname("s-let.tmp");
const T_GGEN: FileName = fname("g-gen.tmp");
const T_GART: FileName = fname("g-art.tmp");
const T_GALB: FileName = fname("g-alb.tmp");
const T_GLET: FileName = fname("g-let.tmp");
const T_ALET: FileName = fname("a-let.tmp");
const T_SA: FileName = fname("sort-a.tmp");
const T_SB: FileName = fname("sort-b.tmp");
const TEMPS: [FileName; 15] = [
    T_WALK, T_DIRS, T_JOIN, T_JSORT, T_TART, T_TALB, T_TLET, T_SLET, T_GGEN, T_GART, T_GALB, T_GLET, T_ALET,
    T_SA, T_SB,
];

// Sort-stage scratch layout.
const B0: usize = 1024;
const BN: usize = 512;
const AUX: usize = 640;
const SORT_AT: usize = B0 + 5 * BN + AUX;
const AUX_FIELD: usize = 128;

// Scan scratch layout.
const S_OLD: usize = 1024;
const S_NAME: usize = 128;
const S_ENTRY: usize = 768;
const S_WIN: usize = S_OLD + 3 * BN + S_NAME + S_ENTRY;
const MAX_WIN: usize = 16 * 1024;
const WALK_ITEM: usize = 10;
/// Units of work charged for one file whose tags are read.
const FILE_COST: u32 = 8;
const MASK31: u32 = 0x7FFF_FFFF;

const _: () = assert!(S_ENTRY >= yoton_index::MAX_NAME);
const _: () = assert!(S_OLD >= cat::MAX);
const _: () = assert!(SORT_AT + yoton_index::MIN_SORT_SCRATCH <= MIN_SCRATCH);
const _: () = assert!(S_WIN + 4096 <= MIN_SCRATCH);
const _: () = assert!(AUX >= 4 + 400 && AUX >= cat::FIELDS * AUX_FIELD);

/// What a build does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// Volume serial (put into every [`crate::TrackRef`]).
    pub serial: u32,
    /// The time now in the encoding of [`yoton_index::mtime_of`] (0 if
    /// the clock is not set): "first seen" of files new since the last
    /// build. A first build uses each file's mtime instead.
    pub now: u32,
    /// Sort artists and albums without a leading "The"/"A"/"An".
    pub ignore_articles: bool,
    /// Per-file tag read limits.
    pub budget: Budget,
    /// Deepest folder level walked (the root is 0).
    pub max_depth: u8,
    pub code_page: CodePage,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            serial: 0,
            now: 0,
            ignore_articles: false,
            budget: Budget::default(),
            max_depth: 16,
            code_page: CodePage::default(),
        }
    }
}

/// Counters of the scan.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ScanStats {
    /// Folders listed.
    pub dirs: u32,
    /// Audio files seen.
    pub files: u32,
    /// Catalogue items written.
    pub tracks: u32,
    /// Items kept from the previous library without a read.
    pub reused: u32,
    /// Files whose tags were read.
    pub read: u32,
    /// Read calls and bytes for tags.
    pub tag_reads: u32,
    pub tag_bytes: u64,
    /// Tag reads the budget cut short.
    pub truncated: u32,
}

/// How far a build is.
#[allow(clippy::large_enum_variant)] // returned by value, not stored
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Progress {
    /// Walking the card.
    Scanning { dirs: u32, files: u32, tracks: u32 },
    /// Deriving the tables: stage `stage` of `stages`.
    Indexing { stage: u8, stages: u8 },
    /// Complete: the new library (the previous slot has been removed).
    Done(Library),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Js {
    Dirs,
    Strings,
    Join,
    Tracks,
    Songs,
    Genre,
    Recent,
    ByLoc,
    AlbYear,
    AlbName,
    GAlbYear,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Scan,
    Sort(Js),
    Copy(FileName, Sec),
    Finish,
}

const PLAN: [Stage; 22] = [
    Stage::Scan,
    Stage::Sort(Js::Dirs),
    Stage::Sort(Js::Strings),
    Stage::Sort(Js::Join),
    Stage::Sort(Js::Tracks),
    Stage::Sort(Js::Songs),
    Stage::Sort(Js::Genre),
    Stage::Sort(Js::Recent),
    Stage::Sort(Js::ByLoc),
    Stage::Copy(T_TART, Sec::Artists),
    Stage::Copy(T_TALB, Sec::Albums),
    Stage::Copy(T_GGEN, Sec::Genres),
    Stage::Copy(T_GART, Sec::GArtists),
    Stage::Copy(T_GALB, Sec::GAlbums),
    Stage::Copy(T_TLET, Sec::LArtists),
    Stage::Copy(T_SLET, Sec::LSongs),
    Stage::Copy(T_GLET, Sec::LGenres),
    Stage::Sort(Js::AlbYear),
    Stage::Sort(Js::AlbName),
    Stage::Sort(Js::GAlbYear),
    Stage::Copy(T_ALET, Sec::LAlbums),
    Stage::Finish,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dest {
    None,
    Sec(Sec),
    Tmp(FileName),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Src {
    Sec(Sec),
    Tmp(FileName),
}

impl Js {
    fn outs(self) -> [Dest; 5] {
        use Dest::{None as N, Sec as S, Tmp as T};
        match self {
            Js::Dirs => [S(Sec::Dirs), N, N, N, N],
            Js::Strings => [S(Sec::Str), T(T_JOIN), N, N, N],
            Js::Join => [T(T_JSORT), N, N, N, N],
            Js::Tracks => [N, S(Sec::Trk), T(T_TART), T(T_TALB), T(T_TLET)],
            Js::Songs => [S(Sec::Songs), T(T_SLET), N, N, N],
            Js::Genre => [S(Sec::Genre), T(T_GGEN), T(T_GART), T(T_GALB), T(T_GLET)],
            Js::Recent => [S(Sec::Recent), N, N, N, N],
            Js::ByLoc => [S(Sec::ByLoc), N, N, N, N],
            Js::AlbYear => [S(Sec::AlbYear), N, N, N, N],
            Js::AlbName => [S(Sec::AlbName), T(T_ALET), N, N, N],
            Js::GAlbYear => [S(Sec::GAlbYear), N, N, N, N],
        }
    }

    fn input(self) -> (Src, Option<Src>) {
        match self {
            Js::Dirs => (Src::Tmp(T_DIRS), None),
            Js::Strings => (Src::Sec(Sec::Cat), None),
            Js::Join => (Src::Tmp(T_JOIN), None),
            Js::Tracks => (Src::Sec(Sec::Cat), Some(Src::Tmp(T_JSORT))),
            Js::Songs | Js::Genre | Js::Recent | Js::ByLoc => (Src::Sec(Sec::Trk), None),
            Js::AlbYear | Js::AlbName => (Src::Sec(Sec::Albums), None),
            Js::GAlbYear => (Src::Sec(Sec::GAlbums), None),
        }
    }

    /// Input record size (0: catalogue items).
    fn rec(self) -> usize {
        match self {
            Js::Dirs => DIR_ROW,
            Js::Strings | Js::Tracks => 0,
            Js::Join => JOIN_REC,
            Js::Songs | Js::Genre | Js::Recent | Js::ByLoc => TRK_ROW,
            Js::AlbYear | Js::AlbName | Js::GAlbYear => GRP_ROW,
        }
    }

    fn cmp(self) -> fn(&[u8], &[u8]) -> Ordering {
        match self {
            Js::Dirs => pre::<4>,
            Js::Strings => str_cmp,
            Js::Join => pre::<5>,
            Js::Tracks => pre::<24>,
            Js::Songs => pre::<16>,
            Js::Genre | Js::Recent => pre::<8>,
            Js::ByLoc => pre::<12>,
            Js::AlbYear | Js::AlbName | Js::GAlbYear => pre::<14>,
        }
    }
}

const JOIN_REC: usize = 9;

fn pre<const N: usize>(a: &[u8], b: &[u8]) -> Ordering {
    a[..N.min(a.len())].cmp(&b[..N.min(b.len())])
}

/// String items: `[kind][unknown][key length u16 BE][key][raw length][raw][first ordinal u32 BE][field]`.
struct StrItem<'a> {
    kind: u8,
    empty: u8,
    key: &'a [u8],
    raw: &'a [u8],
    tail: &'a [u8],
}

fn str_item(a: &[u8]) -> StrItem<'_> {
    let kl = usize::from(u16::from_be_bytes([a[2], a[3]]));
    let k_end = 4 + kl;
    let rl = usize::from(a[k_end]);
    let r_end = k_end + 1 + rl;
    StrItem { kind: a[0], empty: a[1], key: &a[4..k_end], raw: &a[k_end + 1..r_end], tail: &a[r_end..] }
}

fn str_cmp(a: &[u8], b: &[u8]) -> Ordering {
    let (x, y) = (str_item(a), str_item(b));
    (x.kind, x.empty, x.key, x.raw, x.tail).cmp(&(y.kind, y.empty, y.key, y.raw, y.tail))
}

/// The previous library's catalogue and folder table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Old {
    file: FileName,
    cat: u32,
    dirs: u32,
    dirs_rows: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct OldDir {
    sig: Signature,
    off: u32,
    len: u32,
    tracks: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CurSt {
    Open,
    /// New folder: signature, subfolders and tags in one pass.
    Single,
    /// Known folder, first pass: signature and subfolders.
    Sig,
    /// Unchanged: copying the old items.
    Copy,
    /// Changed: second pass, tags of new and changed files.
    Tags,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Cur {
    loc: Option<Locator>,
    id: u32,
    depth: u8,
    item_off: u32,
    name_len: u8,
    name_ok: bool,
    st: CurSt,
    opened: bool,
    last: Option<Locator>,
    sig: Signature,
    cat_start: u32,
    tracks: u32,
    old: Option<OldDir>,
    old_in: In,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Scan {
    q_read: u32,
    root_done: bool,
    active: bool,
    walk: Out,
    dirs: Out,
    cat: Out,
    cur: Cur,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sub {
    Init,
    Feed,
    Merge,
    Drain,
}

/// State of a sort stage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Sj {
    sub: Sub,
    sorter: ExtSort,
    ins: [In; 2],
    outs: [Out; 5],
    /// Feed: records read. Drain: items handed out.
    n: u32,
    m: u32,
    /// Tracks feed: current string of each field. Strings drain: [0] the
    /// current reference, [1] its kind, [2] kinds started. Songs: [0]
    /// previous title. Genre: [0] artist row, [1] album row of the open
    /// groups.
    cur: [u32; 5],
    groups: [Group; 3],
    rows: [u32; 3],
    str_in: In,
    str_base: u32,
    last_letter: u32,
    /// The auxiliary scratch holds valid previous values.
    aux_ok: bool,
    have_prev: bool,
}

/// The state of the current stage (one at a time: a union saves RAM).
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Work {
    Scan(Scan),
    Sort(Sj),
}

/// A build stopped between two steps: a plain value (no scratch
/// contents). [`Suspended::resume`] continues it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Suspended {
    cfg: Config,
    stage: u8,
    started: bool,
    slot: u8,
    old: Option<Old>,
    scratch_len: u32,
    hdr: Header,
    end: u32,
    w: Work,
    stats: ScanStats,
}

/// A library build in progress; holds the scratch buffer for its
/// lifetime, [`Builder::suspend`] gives it back.
pub struct Builder<'s> {
    scratch: &'s mut [u8],
    st: Suspended,
}

const NO_FILE: FileName = fname("-");

fn empty_in() -> In {
    In::new(NO_FILE, 0, 0)
}

fn empty_out() -> Out {
    Out::new(NO_FILE, 0)
}

impl Sj {
    fn new() -> Sj {
        Sj {
            sub: Sub::Init,
            sorter: ExtSort::new(T_SA, T_SB),
            ins: [empty_in(); 2],
            outs: [empty_out(); 5],
            n: 0,
            m: 0,
            cur: [u32::MAX; 5],
            groups: [Group::EMPTY; 3],
            rows: [0; 3],
            str_in: empty_in(),
            str_base: 0,
            last_letter: u32::MAX,
            aux_ok: false,
            have_prev: false,
        }
    }
}

impl Suspended {
    /// Continue with `scratch` (any contents, at least as long as the one
    /// the build started with); `None` if it is shorter.
    #[must_use]
    pub fn resume<'s>(&self, scratch: &'s mut [u8]) -> Option<Builder<'s>> {
        let len = self.scratch_len as usize;
        if scratch.len() < len {
            return None;
        }
        Some(Builder { scratch: &mut scratch[..len], st: *self })
    }

    /// Scan counters so far.
    #[must_use]
    pub fn stats(&self) -> ScanStats {
        self.stats
    }
}

struct Regions<'a> {
    b0: &'a mut [u8],
    b: [&'a mut [u8]; 5],
    aux: &'a mut [u8],
}

fn regions(s: &mut [u8]) -> (Regions<'_>, &mut [u8]) {
    let (b0, r) = s.split_at_mut(B0);
    let (b1, r) = r.split_at_mut(BN);
    let (b2, r) = r.split_at_mut(BN);
    let (b3, r) = r.split_at_mut(BN);
    let (b4, r) = r.split_at_mut(BN);
    let (b5, r) = r.split_at_mut(BN);
    let (aux, sort) = r.split_at_mut(AUX);
    (Regions { b0, b: [b1, b2, b3, b4, b5], aux }, sort)
}

fn short<E>(_: crate::io::Short) -> Error<E> {
    Error::Corrupt
}

/// `s` cut to at most `n` bytes at a character boundary.
fn trunc(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut k = n;
    while !s.is_char_boundary(k) {
        k -= 1;
    }
    &s[..k]
}

struct Rd<'a, C>(&'a mut C);

impl<C: TagSource> ReadAt for Rd<'_, C> {
    type Error = C::Error;
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, C::Error> {
        self.0.read_at(offset, buf)
    }
}

impl<'s> Builder<'s> {
    /// Start a build (incremental when a library exists). `None` if
    /// `scratch` is shorter than [`MIN_SCRATCH`].
    pub fn new(scratch: &'s mut [u8], cfg: Config) -> Option<Builder<'s>> {
        if scratch.len() < MIN_SCRATCH {
            return None;
        }
        let len = scratch.len().clamp(MIN_SCRATCH, S_WIN + MAX_WIN);
        let cur = Cur {
            loc: None,
            id: 0,
            depth: 0,
            item_off: u32::MAX,
            name_len: 0,
            name_ok: false,
            st: CurSt::Open,
            opened: false,
            last: None,
            sig: Signature::default(),
            cat_start: 0,
            tracks: 0,
            old: None,
            old_in: empty_in(),
        };
        let st = Suspended {
            cfg,
            stage: 0,
            started: false,
            slot: 0,
            old: None,
            scratch_len: len as u32,
            hdr: Header::default(),
            end: HEADER as u32,
            w: Work::Scan(Scan {
                q_read: 0,
                root_done: false,
                active: false,
                walk: empty_out(),
                dirs: empty_out(),
                cat: empty_out(),
                cur,
            }),
            stats: ScanStats::default(),
        };
        Some(Builder { scratch: &mut scratch[..len], st })
    }

    /// Scan counters so far.
    #[must_use]
    pub fn stats(&self) -> ScanStats {
        self.st.stats
    }

    /// Do at most about `budget` units of work: folder entries read while
    /// scanning (a file whose tags are read counts 8), items or 512-byte
    /// blocks while indexing. A step may also sort and write one
    /// scratch-full run.
    pub fn step<S, C, E>(&mut self, store: &mut S, src: &mut C, budget: u32) -> Result<Progress, Error<E>>
    where
        S: Storage<Error = E>,
        C: TagSource<Error = E>,
    {
        let budget = budget.max(1);
        if !self.st.started {
            self.start(store)?;
            return Ok(Progress::Scanning { dirs: 0, files: 0, tracks: 0 });
        }
        let stages = PLAN.len() as u8;
        match PLAN.get(usize::from(self.st.stage)) {
            None => Ok(Progress::Done(Library::from_parts(
                SLOTS[usize::from(self.st.slot)],
                self.st.slot,
                &self.st.hdr,
            ))),
            Some(Stage::Scan) => self.scan(store, src, budget),
            Some(&Stage::Sort(js)) => {
                if self.sort(store, js, budget)? {
                    self.st.stage += 1;
                    self.st.w = Work::Sort(Sj::new());
                }
                Ok(Progress::Indexing { stage: self.st.stage, stages })
            }
            Some(&Stage::Copy(from, to)) => {
                if self.copy(store, from, to, budget)? {
                    self.st.stage += 1;
                    self.st.w = Work::Sort(Sj::new());
                }
                Ok(Progress::Indexing { stage: self.st.stage, stages })
            }
            Some(Stage::Finish) => self.finish(store),
        }
    }

    /// Stop here and give the scratch back; nothing is lost.
    pub fn suspend<S: Storage>(mut self, store: &mut S) -> Result<Suspended, Error<S::Error>> {
        let st = &mut self.st;
        match (PLAN.get(usize::from(st.stage)), &mut st.w) {
            (Some(Stage::Scan), Work::Scan(sc)) if st.started => {
                let (old, rest) = self.scratch.split_at_mut(S_OLD);
                let _ = old;
                let (b_cat, rest) = rest.split_at_mut(BN);
                let (b_walk, rest) = rest.split_at_mut(BN);
                let b_dirs = &rest[..BN];
                sc.cat.flush(store, b_cat)?;
                sc.walk.flush(store, b_walk)?;
                sc.dirs.flush(store, b_dirs)?;
                sc.cur.old_in.rewind();
                sc.cur.opened = false;
                sc.cur.name_ok = false;
            }
            (Some(&Stage::Sort(js)), Work::Sort(sj)) if sj.sub != Sub::Init => {
                let (r, sort) = regions(self.scratch);
                sj.sorter.suspend(store, sort, &js.cmp())?;
                for (o, b) in sj.outs.iter_mut().zip(r.b) {
                    o.flush(store, b)?;
                }
                for i in &mut sj.ins {
                    i.rewind();
                }
                sj.str_in.rewind();
                sj.aux_ok = false;
            }
            (Some(Stage::Copy(..)), Work::Sort(sj)) if sj.sub != Sub::Init => {
                let (r, _) = regions(self.scratch);
                sj.outs[0].flush(store, r.b[0])?;
                sj.ins[0].rewind();
            }
            _ => {}
        }
        Ok(self.st)
    }

    // ------------------------------------------------------------ start

    fn start<S: Storage>(&mut self, store: &mut S) -> Result<(), Error<S::Error>> {
        let st = &mut self.st;
        let prev = Library::open(store)?;
        let (slot, generation) = match prev {
            Some(l) => (1 - l.slot(), l.generation().wrapping_add(1)),
            None => (0, 1),
        };
        st.old = prev.map(|l| {
            let (cat, _) = l.section(Sec::Cat);
            let (dirs, dl) = l.section(Sec::Dirs);
            Old { file: l.file(), cat, dirs, dirs_rows: dl / DIR_ROW as u32 }
        });
        st.slot = slot;
        st.hdr = Header {
            options: u16::from(st.cfg.ignore_articles),
            generation,
            serial: st.cfg.serial,
            stamp: st.cfg.now,
            ..Header::default()
        };
        let file = SLOTS[usize::from(slot)];
        store.truncate(file, 0)?;
        self.scratch[..HEADER].fill(0);
        store.write_at(file, 0, &self.scratch[..HEADER])?;
        for t in TEMPS {
            store.truncate(t, 0)?;
        }
        st.end = HEADER as u32;
        if let Work::Scan(sc) = &mut st.w {
            sc.walk = Out::new(T_WALK, 0);
            sc.dirs = Out::new(T_DIRS, 0);
            sc.cat = Out::new(file, HEADER as u32);
        }
        st.started = true;
        Ok(())
    }

    fn file(&self) -> FileName {
        SLOTS[usize::from(self.st.slot)]
    }

    // ------------------------------------------------------------- scan

    fn scan<S, C, E>(&mut self, store: &mut S, src: &mut C, budget: u32) -> Result<Progress, Error<E>>
    where
        S: Storage<Error = E>,
        C: TagSource<Error = E>,
    {
        let len = self.st.scratch_len as usize;
        let (s_old, rest) = self.scratch.split_at_mut(S_OLD);
        let (s_cat, rest) = rest.split_at_mut(BN);
        let (s_walk, rest) = rest.split_at_mut(BN);
        let (s_dirs, rest) = rest.split_at_mut(BN);
        let (s_name, rest) = rest.split_at_mut(S_NAME);
        let (s_entry, rest) = rest.split_at_mut(S_ENTRY);
        let win = &mut rest[..len - S_WIN];
        let st = &mut self.st;
        let mut left = budget;
        let Work::Scan(sc) = &mut st.w else { return Err(Error::Corrupt) };
        while left > 0 {
            if !sc.active {
                if !next_dir(store, sc, s_walk, s_name)? {
                    // The walk is complete.
                    sc.cat.flush(store, s_cat)?;
                    sc.dirs.flush(store, s_dirs)?;
                    st.hdr.secs[Sec::Cat as usize] =
                        SecInfo { off: sc.cat.base, len: sc.cat.written(), crc: sc.cat.crc };
                    st.end = sc.cat.base + sc.cat.written();
                    store.remove(T_WALK)?;
                    st.stage += 1;
                    st.w = Work::Sort(Sj::new());
                    return Ok(Progress::Indexing { stage: st.stage, stages: PLAN.len() as u8 });
                }
                continue;
            }
            let cur = &mut sc.cur;
            if !cur.name_ok {
                load_name(store, cur, s_name)?;
            }
            match cur.st {
                CurSt::Open => {
                    left -= 1;
                    let Some(id) = src.open_dir(cur.loc, None)? else {
                        sc.active = false;
                        continue;
                    };
                    cur.id = id;
                    cur.opened = true;
                    cur.last = None;
                    cur.sig = Signature::default();
                    cur.cat_start = sc.cat.written();
                    cur.tracks = 0;
                    cur.old = match st.old {
                        Some(o) => old_dir(store, &o, id)?,
                        None => None,
                    };
                    cur.st = if cur.old.is_some() { CurSt::Sig } else { CurSt::Single };
                }
                CurSt::Copy => {
                    left -= 1;
                    if cur.old_in.is_empty() {
                        end_dir(store, sc, s_dirs, &mut st.stats)?;
                        continue;
                    }
                    let n = (cur.old_in.remaining() as usize).min(s_old.len());
                    cur.old_in.need(store, s_old, n)?.map_err(short)?;
                    let b = cur.old_in.take(s_old, n);
                    sc.cat.put(store, s_cat, b)?;
                }
                CurSt::Single | CurSt::Sig | CurSt::Tags => {
                    if !cur.opened {
                        if src.open_dir(cur.loc, cur.last)?.is_none() {
                            // Gone while suspended: keep what was read.
                            end_dir(store, sc, s_dirs, &mut st.stats)?;
                            continue;
                        }
                        cur.opened = true;
                    }
                    left -= 1;
                    let Some(e) = src.next_entry(s_entry)? else {
                        match cur.st {
                            CurSt::Sig => {
                                let od = cur.old.unwrap_or(OldDir {
                                    sig: Signature::default(),
                                    off: 0,
                                    len: 0,
                                    tracks: 0,
                                });
                                let old = st.old.map_or(NO_FILE, |o| o.file);
                                let base = st.old.map_or(0, |o| o.cat) + od.off;
                                cur.old_in = In::new(old, base, base + od.len);
                                if od.sig == cur.sig {
                                    cur.st = CurSt::Copy;
                                    cur.tracks = od.tracks;
                                    st.stats.tracks += od.tracks;
                                    st.stats.reused += od.tracks;
                                    st.stats.files += od.tracks;
                                } else {
                                    cur.st = CurSt::Tags;
                                    cur.opened = false;
                                    cur.last = None;
                                }
                            }
                            _ => end_dir(store, sc, s_dirs, &mut st.stats)?,
                        }
                        continue;
                    };
                    cur.last = Some(e.loc);
                    let Some(name) = s_entry.get(..e.name_len).and_then(|b| core::str::from_utf8(b).ok())
                    else {
                        continue;
                    };
                    let visible = !e.hidden && !is_hidden_name(name);
                    if matches!(cur.st, CurSt::Single | CurSt::Sig) {
                        cur.sig.add(&e, name);
                        if e.is_dir && visible && cur.depth < st.cfg.max_depth {
                            let nm = trunc(name, 127);
                            let mut h = [0u8; WALK_ITEM];
                            wr32(&mut h, 0, e.loc.dir);
                            wr32(&mut h, 4, e.loc.index);
                            h[8] = cur.depth + 1;
                            h[9] = nm.len() as u8;
                            sc.walk.put(store, s_walk, &h)?;
                            sc.walk.put(store, s_walk, nm.as_bytes())?;
                        }
                    }
                    let audio = !e.is_dir && visible && kind_of(name, false) == EntryKind::Audio;
                    if audio && matches!(cur.st, CurSt::Single | CurSt::Tags) {
                        st.stats.files += 1;
                        let folder = core::str::from_utf8(&s_name[..usize::from(cur.name_len)]).unwrap_or("");
                        let initial = st.old.is_none();
                        let cost = file(
                            store,
                            src,
                            &st.cfg,
                            initial,
                            &e,
                            name,
                            folder,
                            cur,
                            &mut sc.cat,
                            s_cat,
                            s_old,
                            win,
                            &mut st.stats,
                        )?;
                        left = left.saturating_sub(cost);
                    }
                }
            }
        }
        Ok(Progress::Scanning { dirs: st.stats.dirs, files: st.stats.files, tracks: st.stats.tracks })
    }

    // ------------------------------------------------------------ sorts

    /// One step of sort stage `js`; `true` when it is complete.
    fn sort<S: Storage>(&mut self, store: &mut S, js: Js, budget: u32) -> Result<bool, Error<S::Error>> {
        let file = self.file();
        let st = &mut self.st;
        let (mut r, sort) = regions(self.scratch);
        let cmp = js.cmp();
        let Work::Sort(sj) = &mut st.w else { return Err(Error::Corrupt) };
        if sj.sub == Sub::Init {
            sj.sorter.reset(store)?;
            let (a, b) = js.input();
            let open = |store: &mut S, s: Src, hdr: &Header| -> Result<In, S::Error> {
                Ok(match s {
                    Src::Sec(sec) => {
                        let i = hdr.secs[sec as usize];
                        In::new(file, i.off, i.off + i.len)
                    }
                    Src::Tmp(t) => In::new(t, 0, store.len(t)?.unwrap_or(0)),
                })
            };
            sj.ins[0] = open(store, a, &st.hdr)?;
            if let Some(b) = b {
                sj.ins[1] = open(store, b, &st.hdr)?;
            }
            for (o, d) in sj.outs.iter_mut().zip(js.outs()) {
                *o = match d {
                    Dest::None => empty_out(),
                    Dest::Sec(_) => Out::new(file, st.end),
                    Dest::Tmp(t) => {
                        store.truncate(t, 0)?;
                        Out::new(t, 0)
                    }
                };
            }
            let s = st.hdr.secs[Sec::Str as usize];
            sj.str_in = In::new(file, s.off, s.off + s.len);
            sj.str_base = s.off;
            if js == Js::Strings {
                // Kinds whose start offset is set.
                sj.cur[2] = 0;
            }
            sj.sub = Sub::Feed;
        }
        let mut left = budget;
        while left > 0 {
            match sj.sub {
                Sub::Init => unreachable_init()?,
                Sub::Feed => {
                    left -= 1;
                    if !feed(store, js, st.cfg.ignore_articles, sj, &mut r, sort)? {
                        sj.sorter.seal(store, sort, &cmp)?;
                        sj.sub = Sub::Merge;
                    }
                }
                Sub::Merge => {
                    if sj.sorter.merge(store, sort, &cmp, left)? {
                        sj.sub = Sub::Drain;
                        sj.aux_ok = false;
                    }
                    left = 0;
                }
                Sub::Drain => {
                    left -= 1;
                    let Some(item) = sj.sorter.next(store, &mut *sort, &cmp)? else {
                        end_drain(store, js, &mut st.hdr, sj, &mut r)?;
                        // Exactly one section per stage.
                        for (o, d) in sj.outs.iter_mut().zip(js.outs()) {
                            if let Dest::Sec(sec) = d {
                                st.hdr.secs[sec as usize] =
                                    SecInfo { off: o.base, len: o.written(), crc: o.crc };
                                st.end = o.base + o.written();
                            }
                        }
                        if js == Js::Strings {
                            st.hdr.str_end = sj.outs[0].written();
                            let mut k = sj.cur[2] as usize;
                            while k < 4 {
                                st.hdr.kinds[k] = st.hdr.str_end;
                                k += 1;
                            }
                        }
                        if js == Js::Tracks {
                            st.hdr.tracks = sj.outs[1].written() / TRK_ROW as u32;
                        }
                        return Ok(true);
                    };
                    drain(store, js, st.cfg.ignore_articles, &mut st.hdr, sj, &mut r, item)?;
                }
            }
        }
        Ok(false)
    }

    fn copy<S: Storage>(
        &mut self,
        store: &mut S,
        from: FileName,
        to: Sec,
        budget: u32,
    ) -> Result<bool, Error<S::Error>> {
        let file = self.file();
        let st = &mut self.st;
        let (r, sort) = regions(self.scratch);
        let Work::Sort(sj) = &mut st.w else { return Err(Error::Corrupt) };
        if sj.sub == Sub::Init {
            sj.ins[0] = In::new(from, 0, store.len(from)?.unwrap_or(0));
            sj.outs[0] = Out::new(file, st.end);
            sj.sub = Sub::Feed;
        }
        for _ in 0..budget {
            let i = &mut sj.ins[0];
            if i.is_empty() {
                let o = &mut sj.outs[0];
                o.flush(store, r.b[0])?;
                st.hdr.secs[to as usize] = SecInfo { off: o.base, len: o.written(), crc: o.crc };
                st.end = o.base + o.written();
                store.remove(from)?;
                return Ok(true);
            }
            let n = (i.remaining() as usize).min(sort.len());
            i.need(store, sort, n)?.map_err(short)?;
            let b = i.take(sort, n);
            sj.outs[0].put(store, r.b[0], b)?;
        }
        Ok(false)
    }

    fn finish<S: Storage>(&mut self, store: &mut S) -> Result<Progress, Error<S::Error>> {
        let file = self.file();
        let st = &mut self.st;
        store.sync(file)?;
        let len = store.len(file)?.unwrap_or(0);
        if u64::from(len) != st.hdr.file_len() {
            return Err(Error::Corrupt);
        }
        let hb = &mut self.scratch[..HEADER];
        st.hdr.encode(hb);
        store.write_at(file, 0, hb)?;
        store.sync(file)?;
        for t in TEMPS {
            store.remove(t)?;
        }
        store.remove(SLOTS[usize::from(1 - st.slot)])?;
        st.stage += 1;
        Ok(Progress::Done(Library::from_parts(file, st.slot, &st.hdr)))
    }
}

fn unreachable_init<E>() -> Result<(), Error<E>> {
    Err(Error::Corrupt)
}

// ---------------------------------------------------------------- scan

/// Start the next folder: the root, then the walk queue. `false` when the
/// queue is exhausted.
fn next_dir<S: Storage>(
    store: &mut S,
    sc: &mut Scan,
    s_walk: &mut [u8],
    s_name: &mut [u8],
) -> Result<bool, Error<S::Error>> {
    let cur = &mut sc.cur;
    cur.st = CurSt::Open;
    cur.opened = false;
    cur.last = None;
    cur.old = None;
    if !sc.root_done {
        sc.root_done = true;
        cur.loc = None;
        cur.depth = 0;
        cur.item_off = u32::MAX;
        cur.name_len = 0;
        cur.name_ok = true;
        sc.active = true;
        return Ok(true);
    }
    if sc.q_read >= sc.walk.written() {
        return Ok(false);
    }
    if sc.q_read + (WALK_ITEM + 127) as u32 > sc.walk.pos {
        sc.walk.flush(store, s_walk)?;
    }
    cur.item_off = sc.q_read;
    load_name(store, cur, s_name)?;
    sc.q_read += (WALK_ITEM + usize::from(cur.name_len)) as u32;
    sc.active = true;
    Ok(true)
}

/// Read the current folder's walk item (locator, depth, name).
fn load_name<S: Storage>(store: &mut S, cur: &mut Cur, s_name: &mut [u8]) -> Result<(), Error<S::Error>> {
    if cur.item_off == u32::MAX {
        cur.name_len = 0;
        cur.name_ok = true;
        return Ok(());
    }
    let mut b = [0u8; WALK_ITEM + 127];
    let got = store.read_at(T_WALK, cur.item_off, &mut b)?;
    if got < WALK_ITEM || got < WALK_ITEM + usize::from(b[9]) {
        return Err(Error::Corrupt);
    }
    cur.loc = Some(Locator { dir: rd32(&b, 0), index: rd32(&b, 4) });
    cur.depth = b[8];
    cur.name_len = b[9];
    let n = usize::from(b[9]);
    s_name[..n].copy_from_slice(&b[WALK_ITEM..WALK_ITEM + n]);
    cur.name_ok = true;
    Ok(())
}

/// The previous library's row for folder `id` (binary search of `DIRS`).
fn old_dir<S: Storage>(store: &mut S, o: &Old, id: u32) -> Result<Option<OldDir>, Error<S::Error>> {
    let (mut lo, mut hi) = (0u32, o.dirs_rows);
    let mut b = [0u8; DIR_ROW];
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if store.read_at(o.file, o.dirs + mid * DIR_ROW as u32, &mut b)? != DIR_ROW {
            return Err(Error::Corrupt);
        }
        let k = rd32(&b, 0);
        match k.cmp(&id) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                return Ok(Some(OldDir {
                    sig: Signature {
                        entries: rd32(&b, 4),
                        name_bytes: rd32(&b, 8),
                        max_mtime: rd32(&b, 12),
                        hash: rd64(&b, 16),
                    },
                    off: rd32(&b, 24),
                    len: rd32(&b, 28),
                    tracks: rd32(&b, 32),
                }));
            }
        }
    }
    Ok(None)
}

fn end_dir<S: Storage>(
    store: &mut S,
    sc: &mut Scan,
    s_dirs: &mut [u8],
    stats: &mut ScanStats,
) -> Result<(), S::Error> {
    let cur = &mut sc.cur;
    if cur.tracks > 0 {
        let mut b = [0u8; DIR_ROW];
        wr32(&mut b, 0, cur.id);
        wr32(&mut b, 4, cur.sig.entries);
        wr32(&mut b, 8, cur.sig.name_bytes);
        wr32(&mut b, 12, cur.sig.max_mtime);
        b[16..24].copy_from_slice(&cur.sig.hash.to_le_bytes());
        wr32(&mut b, 24, cur.cat_start);
        wr32(&mut b, 28, sc.cat.written() - cur.cat_start);
        wr32(&mut b, 32, cur.tracks);
        sc.dirs.put(store, s_dirs, &b)?;
    }
    sc.active = false;
    stats.dirs += 1;
    Ok(())
}

/// The leading number of a file name ("07 - Song.mp3" → 7), ≤ 999.
fn leading_track(name: &str) -> u16 {
    let d: &str = &name[..name.bytes().take_while(u8::is_ascii_digit).count()];
    match d.parse::<u16>() {
        Ok(n) if (1..=999).contains(&n) && d.len() <= 3 => n,
        _ => 0,
    }
}

/// One audio file: keep its old item or read its tags. Returns the work
/// units spent.
#[allow(clippy::too_many_arguments)]
fn file<S, C, E>(
    store: &mut S,
    src: &mut C,
    cfg: &Config,
    initial: bool,
    e: &SrcEntry,
    name: &str,
    folder: &str,
    cur: &mut Cur,
    cat_out: &mut Out,
    s_cat: &mut [u8],
    s_old: &mut [u8],
    win: &mut [u8],
    stats: &mut ScanStats,
) -> Result<u32, Error<E>>
where
    S: Storage<Error = E>,
    C: TagSource<Error = E>,
{
    let hash = name_hash(name);
    let mut added = None;
    if cur.st == CurSt::Tags {
        let oi = &mut cur.old_in;
        loop {
            if oi.is_empty() || oi.need(store, s_old, 2)?.is_err() {
                break;
            }
            let l = usize::from(rd16(s_old, usize::from(oi.lo)));
            if !(cat::FIXED..=cat::MAX).contains(&l) || oi.need(store, s_old, l)?.is_err() {
                break;
            }
            let lo = usize::from(oi.lo);
            let f = CatFixed::decode(&s_old[lo..lo + l]);
            let (a, b) = (f.loc.index & MASK31, e.loc.index & MASK31);
            if a < b {
                oi.take(s_old, l);
                continue;
            }
            if a == b && f.name_hash == hash && f.loc.dir == e.loc.dir {
                if f.size == e.size as u32 && f.mtime == e.mtime {
                    let mut it = [0u8; cat::MAX];
                    it[..l].copy_from_slice(oi.take(s_old, l));
                    // The contiguity bit of exFAT may have changed.
                    wr32(&mut it, 6, e.loc.index);
                    cat_out.put(store, s_cat, &it[..l])?;
                    cur.tracks += 1;
                    stats.tracks += 1;
                    stats.reused += 1;
                    return Ok(1);
                }
                added = Some(f.added);
                oi.take(s_old, l);
            }
            break;
        }
    }
    let Some(flen) = src.open_file(e.loc)? else { return Ok(1) };
    let format = FileFormat::of(name);
    let mut tags = Tags::default();
    let mut info = yoton_meta::window::ReadInfo::default();
    if let Some(tf) = format.tag_format() {
        let opts = Opts { code_page: cfg.code_page };
        info = read_budgeted(&mut Rd(src), flen, tf, opts, win, cfg.budget, &mut tags)?;
        stats.read += 1;
        stats.tag_reads += u32::from(info.reads);
        stats.tag_bytes += u64::from(info.bytes);
        if info.truncated {
            stats.truncated += 1;
        }
    }
    let mut fl = 0u8;
    if info.truncated {
        fl |= flags::TRUNCATED;
    }
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let title = if tags.title.is_empty() {
        fl |= flags::TITLE_FROM_NAME;
        trunc(stem, 127)
    } else {
        tags.title.as_str()
    };
    let album = if tags.album.is_empty() {
        fl |= flags::ALBUM_FROM_FOLDER;
        trunc(folder, 127)
    } else {
        tags.album.as_str()
    };
    let mut track = tags.track;
    if track == 0 {
        track = leading_track(name);
        if track != 0 {
            fl |= flags::TRACK_FROM_NAME;
        }
    }
    let fixed = CatFixed {
        loc: e.loc,
        name_hash: hash,
        size: e.size as u32,
        mtime: e.mtime,
        added: added.unwrap_or(if initial || cfg.now == 0 { e.mtime } else { cfg.now }),
        duration_ms: info.duration_ms.unwrap_or(0),
        year: tags.year,
        track,
        disc: tags.disc,
        format: format as u8,
        flags: fl,
    };
    let it = &mut win[..cat::MAX];
    fixed.encode(it);
    let mut o = cat::FIXED;
    for f in [title, tags.artist.as_str(), album, tags.album_artist.as_str(), tags.genre.as_str()] {
        let f = trunc(f, 127);
        it[o] = f.len() as u8;
        it[o + 1..o + 1 + f.len()].copy_from_slice(f.as_bytes());
        o += 1 + f.len();
    }
    wr16(it, 0, o as u16);
    cat_out.put(store, s_cat, &it[..o])?;
    cur.tracks += 1;
    stats.tracks += 1;
    Ok(FILE_COST)
}

// ---------------------------------------------------------- sort stages

/// Read the next input record into `rec`; `false` at the end.
fn next_rec<S: Storage>(
    store: &mut S,
    i: &mut In,
    buf: &mut [u8],
    size: usize,
    rec: &mut [u8],
) -> Result<Option<usize>, Error<S::Error>> {
    if i.is_empty() {
        return Ok(None);
    }
    let n = if size == 0 {
        i.need(store, buf, 2)?.map_err(short)?;
        let l = usize::from(rd16(buf, usize::from(i.lo)));
        if !(cat::FIXED..=cat::MAX).contains(&l) {
            return Err(Error::Corrupt);
        }
        l
    } else {
        size
    };
    i.need(store, buf, n)?.map_err(short)?;
    rec[..n].copy_from_slice(i.take(buf, n));
    Ok(Some(n))
}

const KINDS: [Kind; cat::FIELDS] = [Kind::Title, Kind::Artist, Kind::Album, Kind::Artist, Kind::Genre];

/// Feed one input record; `false` at the end of the input.
fn feed<S: Storage>(
    store: &mut S,
    js: Js,
    articles: bool,
    sj: &mut Sj,
    r: &mut Regions<'_>,
    sort: &mut [u8],
) -> Result<bool, Error<S::Error>> {
    let mut rec = [0u8; cat::MAX];
    let Some(n) = next_rec(store, &mut sj.ins[0], r.b0, js.rec(), &mut rec)? else {
        return Ok(false);
    };
    let rec = &rec[..n];
    let ord = sj.n;
    sj.n += 1;
    let cmp = js.cmp();
    let mut it = [0u8; 64];
    let be = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_be_bytes());
    let be16 = |b: &mut [u8], o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_be_bytes());
    let len = match js {
        Js::Dirs => {
            be(&mut it, 0, rd32(rec, 0));
            it[4..4 + DIR_ROW].copy_from_slice(rec);
            4 + DIR_ROW
        }
        Js::Join => {
            it[..JOIN_REC].copy_from_slice(rec);
            JOIN_REC
        }
        Js::Strings => {
            let mut fields = cat_fields(rec);
            if fields[cat::ALBUM_ARTIST].is_empty() {
                fields[cat::ALBUM_ARTIST] = fields[cat::ARTIST];
            }
            for (f, v) in fields.iter().enumerate() {
                let prev = &mut r.aux[f * AUX_FIELD..(f + 1) * AUX_FIELD];
                if sj.aux_ok && prev[0] as usize == v.len() && &prev[1..1 + v.len()] == *v {
                    continue;
                }
                prev[0] = v.len() as u8;
                prev[1..1 + v.len()].copy_from_slice(v);
                let s = core::str::from_utf8(v).unwrap_or("");
                let mut si = [0u8; 4 + 400 + 1 + 127 + 5];
                si[0] = KINDS[f] as u8;
                si[1] = u8::from(v.is_empty());
                let kl = collate::key(s, KINDS[f], articles, &mut si[4..404]);
                si[2..4].copy_from_slice(&(kl as u16).to_be_bytes());
                let mut o = 4 + kl;
                si[o] = v.len() as u8;
                si[o + 1..o + 1 + v.len()].copy_from_slice(v);
                o += 1 + v.len();
                si[o..o + 4].copy_from_slice(&ord.to_be_bytes());
                si[o + 4] = f as u8;
                sj.sorter.push(store, sort, &si[..o + 5], &cmp)?;
            }
            sj.aux_ok = true;
            return Ok(true);
        }
        Js::Tracks => {
            // Apply the string runs that start at or before this item.
            let ji = &mut sj.ins[1];
            loop {
                if ji.is_empty() {
                    break;
                }
                ji.need(store, r.b[0], JOIN_REC)?.map_err(short)?;
                let lo = usize::from(ji.lo);
                if be32(r.b[0], lo) > ord {
                    break;
                }
                let j = ji.take(r.b[0], JOIN_REC);
                if let Some(c) = sj.cur.get_mut(usize::from(j[4])) {
                    *c = rd32(j, 5);
                }
            }
            if sj.cur.contains(&u32::MAX) {
                return Err(Error::Corrupt);
            }
            let f = CatFixed::decode(rec);
            be(&mut it, 0, sj.cur[cat::ALBUM_ARTIST]);
            be(&mut it, 4, sj.cur[cat::ALBUM]);
            be16(&mut it, 8, f.disc);
            be16(&mut it, 10, f.track);
            be(&mut it, 12, sj.cur[cat::TITLE]);
            be(&mut it, 16, f.loc.dir);
            be(&mut it, 20, f.loc.index);
            wr32(&mut it, 24, f.name_hash);
            wr32(&mut it, 28, sj.cur[cat::ARTIST]);
            wr32(&mut it, 32, sj.cur[cat::GENRE]);
            wr32(&mut it, 36, f.duration_ms);
            wr32(&mut it, 40, f.added);
            wr16(&mut it, 44, f.year);
            it[46] = f.format;
            it[47] = f.flags;
            wr32(&mut it, 48, f.size);
            wr32(&mut it, 52, f.mtime);
            56
        }
        Js::Songs => {
            be(&mut it, 0, rd32(rec, trk::TITLE));
            be(&mut it, 4, rd32(rec, trk::ALBUM_ROW));
            be16(&mut it, 8, rd16(rec, trk::DISC));
            be16(&mut it, 10, rd16(rec, trk::TRACK));
            be(&mut it, 12, ord);
            16
        }
        Js::Genre => {
            be(&mut it, 0, rd32(rec, trk::GENRE));
            be(&mut it, 4, ord);
            wr32(&mut it, 8, rd32(rec, trk::ARTIST_ROW));
            wr32(&mut it, 12, rd32(rec, trk::ALBUM_ROW));
            wr32(&mut it, 16, rd32(rec, trk::AARTIST));
            wr32(&mut it, 20, rd32(rec, trk::ALBUM));
            wr16(&mut it, 24, rd16(rec, trk::YEAR));
            wr16(&mut it, 26, rd16(rec, trk::DISC));
            wr32(&mut it, 28, rd32(rec, trk::ARTIST));
            32
        }
        Js::Recent => {
            be(&mut it, 0, u32::MAX - rd32(rec, trk::ADDED));
            be(&mut it, 4, ord);
            8
        }
        Js::ByLoc => {
            be(&mut it, 0, rd32(rec, 0));
            be(&mut it, 4, rd32(rec, 4));
            be(&mut it, 8, ord);
            12
        }
        Js::AlbYear | Js::GAlbYear | Js::AlbName => {
            let year = match rd16(rec, grp::YEAR) {
                0 => u16::MAX,
                y => y,
            };
            if js == Js::AlbName {
                be(&mut it, 0, rd32(rec, grp::NAME));
                be(&mut it, 4, rd32(rec, grp::SUB));
                be16(&mut it, 8, year);
            } else {
                be(&mut it, 0, rd32(rec, grp::PARENT));
                be16(&mut it, 4, year);
                be(&mut it, 6, rd32(rec, grp::NAME));
            }
            be(&mut it, 10, ord);
            14
        }
    };
    sj.sorter.push(store, sort, &it[..len], &cmp)?;
    Ok(true)
}

/// Letter of the string at `r` (read through the pool stream).
fn letter_at<S: Storage>(
    store: &mut S,
    sj: &mut Sj,
    b0: &mut [u8],
    r: u32,
    kind: Kind,
    articles: bool,
) -> Result<u32, Error<S::Error>> {
    let target = sj.str_base + r;
    let si = &mut sj.str_in;
    if target < si.at() {
        // References only grow along a list; a smaller one means a reread.
        si.pos = target;
        si.lo = 0;
        si.hi = 0;
    }
    si.skip_to(target);
    si.need(store, b0, 1)?.map_err(short)?;
    let n = usize::from(b0[usize::from(si.lo)]);
    si.need(store, b0, 1 + n)?.map_err(short)?;
    let s = si.take(b0, 1 + n);
    let s = core::str::from_utf8(&s[1..]).unwrap_or("");
    Ok(collate::letter_code(s, kind, articles))
}

/// Append a letter run if `letter` starts one at `row`.
fn letter_run<S: Storage>(
    store: &mut S,
    sj_last: &mut u32,
    out: &mut Out,
    buf: &mut [u8],
    row: u32,
    letter: u32,
) -> Result<(), S::Error> {
    if letter != *sj_last {
        let mut b = [0u8; LETTER_RUN];
        wr32(&mut b, 0, row);
        wr32(&mut b, 4, letter);
        out.put(store, buf, &b)?;
        *sj_last = letter;
    }
    Ok(())
}

fn drain<S: Storage>(
    store: &mut S,
    js: Js,
    articles: bool,
    hdr: &mut Header,
    sj: &mut Sj,
    r: &mut Regions<'_>,
    item: &[u8],
) -> Result<(), Error<S::Error>> {
    let pos = sj.m;
    sj.m += 1;
    let le = |v: u32| v.to_le_bytes();
    match js {
        Js::Dirs => sj.outs[0].put(store, r.b[0], &item[4..4 + DIR_ROW])?,
        Js::Join => sj.outs[0].put(store, r.b[0], item)?,
        Js::Songs | Js::Recent | Js::ByLoc | Js::AlbYear | Js::GAlbYear | Js::AlbName => {
            let idx = match js {
                Js::Songs => be32(item, 12),
                Js::Recent => be32(item, 4),
                Js::ByLoc => be32(item, 8),
                _ => be32(item, 10),
            };
            sj.outs[0].put(store, r.b[0], &le(idx))?;
            let lettered = match js {
                Js::Songs => Some((be32(item, 0), Kind::Title)),
                Js::AlbName => Some((be32(item, 0), Kind::Album)),
                _ => None,
            };
            if let Some((sr, kind)) = lettered {
                let l = if sj.have_prev && sj.cur[0] == sr {
                    sj.last_letter
                } else {
                    letter_at(store, sj, r.b0, sr, kind, articles)?
                };
                sj.cur[0] = sr;
                sj.have_prev = true;
                let (last, out) = (&mut sj.last_letter, &mut sj.outs[1]);
                letter_run(store, last, out, r.b[1], pos, l)?;
            }
        }
        Js::Strings => {
            let s = str_item(item);
            if !sj.aux_ok && sj.have_prev {
                // Recompute the previous key from the pool (the
                // auxiliary space was lent out).
                let o = &mut sj.outs[0];
                o.flush(store, r.b[0])?;
                let mut b = [0u8; 128];
                let got = store.read_at(o.file, o.base + sj.cur[0], &mut b)?;
                let n = usize::from(b[0]);
                if got < 1 + n {
                    return Err(Error::Corrupt);
                }
                let raw = core::str::from_utf8(&b[1..1 + n]).unwrap_or("");
                let kind = kind_of_code(sj.cur[1] as u8);
                let kl = collate::key(raw, kind, articles, &mut r.aux[4..404]);
                r.aux[0] = sj.cur[1] as u8;
                r.aux[1] = u8::from(n == 0);
                r.aux[2..4].copy_from_slice(&(kl as u16).to_be_bytes());
                sj.aux_ok = true;
            }
            let same = sj.have_prev && {
                let kl = usize::from(u16::from_be_bytes([r.aux[2], r.aux[3]]));
                r.aux[0] == s.kind && r.aux[1] == s.empty && &r.aux[4..4 + kl] == s.key
            };
            if !same {
                let o = &mut sj.outs[0];
                let at = o.written();
                let mut k = sj.cur[2] as usize;
                while k <= usize::from(s.kind) && k < 4 {
                    hdr.kinds[k] = at;
                    k += 1;
                }
                sj.cur[2] = k as u32;
                o.put(store, r.b[0], &[s.raw.len() as u8])?;
                o.put(store, r.b[0], s.raw)?;
                sj.cur[0] = at;
                sj.cur[1] = u32::from(s.kind);
                sj.have_prev = true;
                r.aux[0] = s.kind;
                r.aux[1] = s.empty;
                r.aux[2..4].copy_from_slice(&(s.key.len() as u16).to_be_bytes());
                r.aux[4..4 + s.key.len()].copy_from_slice(s.key);
                sj.aux_ok = true;
            }
            let mut j = [0u8; JOIN_REC];
            j[..4].copy_from_slice(&s.tail[..4]);
            j[4] = s.tail[4];
            wr32(&mut j, 5, sj.cur[0]);
            sj.outs[1].put(store, r.b[1], &j)?;
        }
        Js::Tracks => {
            let aa = be32(item, 0);
            let album = be32(item, 4);
            let disc = u16::from_be_bytes([item[8], item[9]]);
            let year = rd16(item, 44);
            let artist = rd32(item, 28);
            let new_artist = sj.groups[0].count == 0 || sj.groups[0].name != aa;
            let new_album = new_artist || sj.groups[1].name != album;
            if new_album && sj.groups[1].count > 0 {
                sj.outs[3].put(store, r.b[3], &sj.groups[1].encode())?;
                sj.rows[1] += 1;
            }
            if new_artist && sj.groups[0].count > 0 {
                sj.groups[0].children = sj.rows[1] - sj.groups[0].child;
                sj.outs[2].put(store, r.b[2], &sj.groups[0].encode())?;
                sj.rows[0] += 1;
            }
            if new_artist {
                sj.groups[0] = Group { name: aa, first: pos, child: sj.rows[1], ..Group::EMPTY };
                let l = letter_at(store, sj, r.b0, aa, Kind::Artist, articles)?;
                let (last, out) = (&mut sj.last_letter, &mut sj.outs[4]);
                letter_run(store, last, out, r.b[4], sj.rows[0], l)?;
            }
            if new_album {
                sj.groups[1] = Group { name: album, sub: aa, first: pos, parent: sj.rows[0], ..Group::EMPTY };
            }
            sj.groups[0].add(year, disc, artist);
            sj.groups[1].add(year, disc, artist);
            let mut row = [0u8; TRK_ROW];
            wr32(&mut row, 0, be32(item, 16));
            wr32(&mut row, 4, be32(item, 20));
            wr32(&mut row, 8, rd32(item, 24));
            wr32(&mut row, trk::TITLE, be32(item, 12));
            wr32(&mut row, trk::ARTIST, artist);
            wr32(&mut row, trk::ALBUM, album);
            wr32(&mut row, trk::AARTIST, aa);
            wr32(&mut row, trk::GENRE, rd32(item, 32));
            wr32(&mut row, trk::ALBUM_ROW, sj.rows[1]);
            wr32(&mut row, trk::ARTIST_ROW, sj.rows[0]);
            wr32(&mut row, trk::DURATION, rd32(item, 36));
            wr32(&mut row, trk::ADDED, rd32(item, 40));
            wr16(&mut row, trk::YEAR, year);
            wr16(&mut row, trk::TRACK, u16::from_be_bytes([item[10], item[11]]));
            wr16(&mut row, trk::DISC, disc);
            row[trk::FORMAT] = item[46];
            row[trk::FLAGS] = item[47];
            wr32(&mut row, trk::SIZE, rd32(item, 48));
            wr32(&mut row, trk::MTIME, rd32(item, 52));
            sj.outs[1].put(store, r.b[1], &row)?;
        }
        Js::Genre => {
            let genre = be32(item, 0);
            let idx = be32(item, 4);
            let artist_row = rd32(item, 8);
            let album_row = rd32(item, 12);
            let aa = rd32(item, 16);
            let album = rd32(item, 20);
            let year = rd16(item, 24);
            let disc = rd16(item, 26);
            let artist = rd32(item, 28);
            sj.outs[0].put(store, r.b[0], &le(idx))?;
            let new_genre = sj.groups[0].count == 0 || sj.groups[0].name != genre;
            let new_gart = new_genre || sj.cur[0] != artist_row;
            let new_galb = new_gart || sj.cur[1] != album_row;
            if new_galb && sj.groups[2].count > 0 {
                sj.outs[3].put(store, r.b[3], &sj.groups[2].encode())?;
                sj.rows[2] += 1;
            }
            if new_gart && sj.groups[1].count > 0 {
                sj.groups[1].children = sj.rows[2] - sj.groups[1].child;
                sj.outs[2].put(store, r.b[2], &sj.groups[1].encode())?;
                sj.rows[1] += 1;
            }
            if new_genre && sj.groups[0].count > 0 {
                sj.groups[0].children = sj.rows[1] - sj.groups[0].child;
                sj.outs[1].put(store, r.b[1], &sj.groups[0].encode())?;
                sj.rows[0] += 1;
            }
            if new_genre {
                sj.groups[0] = Group { name: genre, first: pos, child: sj.rows[1], ..Group::EMPTY };
                let l = letter_at(store, sj, r.b0, genre, Kind::Genre, articles)?;
                let (last, out) = (&mut sj.last_letter, &mut sj.outs[4]);
                letter_run(store, last, out, r.b[4], sj.rows[0], l)?;
            }
            if new_gart {
                sj.groups[1] = Group {
                    name: aa,
                    sub: genre,
                    first: pos,
                    child: sj.rows[2],
                    parent: sj.rows[0],
                    ..Group::EMPTY
                };
                sj.cur[0] = artist_row;
            }
            if new_galb {
                sj.groups[2] = Group {
                    name: album,
                    sub: aa,
                    first: pos,
                    child: album_row,
                    parent: sj.rows[1],
                    ..Group::EMPTY
                };
                sj.cur[1] = album_row;
            }
            for g in &mut sj.groups {
                g.add(year, disc, artist);
            }
        }
    }
    Ok(())
}

/// Close the open groups at the end of a drain.
fn end_drain<S: Storage>(
    store: &mut S,
    js: Js,
    _hdr: &mut Header,
    sj: &mut Sj,
    r: &mut Regions<'_>,
) -> Result<(), Error<S::Error>> {
    match js {
        Js::Tracks => {
            if sj.groups[1].count > 0 {
                sj.outs[3].put(store, r.b[3], &sj.groups[1].encode())?;
                sj.rows[1] += 1;
            }
            if sj.groups[0].count > 0 {
                sj.groups[0].children = sj.rows[1] - sj.groups[0].child;
                sj.outs[2].put(store, r.b[2], &sj.groups[0].encode())?;
                sj.rows[0] += 1;
            }
        }
        Js::Genre => {
            if sj.groups[2].count > 0 {
                sj.outs[3].put(store, r.b[3], &sj.groups[2].encode())?;
                sj.rows[2] += 1;
            }
            if sj.groups[1].count > 0 {
                sj.groups[1].children = sj.rows[2] - sj.groups[1].child;
                sj.outs[2].put(store, r.b[2], &sj.groups[1].encode())?;
                sj.rows[1] += 1;
            }
            if sj.groups[0].count > 0 {
                sj.groups[0].children = sj.rows[1] - sj.groups[0].child;
                sj.outs[1].put(store, r.b[1], &sj.groups[0].encode())?;
                sj.rows[0] += 1;
            }
        }
        _ => {}
    }
    for (o, b) in sj.outs.iter_mut().zip(r.b.iter_mut()) {
        o.flush(store, b)?;
    }
    sj.sorter.remove(store)?;
    Ok(())
}

fn kind_of_code(c: u8) -> Kind {
    match c {
        0 => Kind::Artist,
        1 => Kind::Album,
        2 => Kind::Genre,
        _ => Kind::Title,
    }
}
