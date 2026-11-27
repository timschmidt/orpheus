//! EPUB 2 and 3 books to the normalised stream (05 §7.15, BOOK-2), in
//! bounded steps like the other formats' conversion: each [`Epub::step`]
//! does at most one 512-byte read of the book, a few hundred bytes of
//! scratch-table reads or writes, and at most [`STEP_OUT`] bytes of
//! decompression with the parsing that follows.
//!
//! The order of work:
//! 1. the zip's end record (from the end of the file, back over a
//!    comment), the central directory walked for `META-INF/container.xml`
//!    (or, if it is missing or names nothing, the first `*.opf`);
//! 2. the package document (OPF): metadata, the manifest (to the
//!    [`Table::Manifest`] scratch table: id, path, kind) and the spine (to
//!    [`Table::Spine`]);
//! 3. the contents: EPUB 3's navigation document, else (or if it has no
//!    entries) the NCX; its entries go to the book's contents file
//!    ([`Table::Toc`], offsets not yet known) and where they point to
//!    [`Table::Targets`]. A book with neither gets contents from its
//!    headings, as the other formats;
//! 4. every spine item in order, one at a time, through the XHTML
//!    converter into one stream (so positions, layout and the screens are
//!    those of BOOK-1). Before an item, the contents entries that point
//!    into it are watched for ([`crate::convert::Watch`], 16 at a time,
//!    refilled as they resolve): an entry takes the offset of the
//!    paragraph its anchor is in (or starts), or of the item's start if
//!    it names no fragment or the fragment is never found;
//! 5. entries never resolved (pointing outside the spine, or out of
//!    reading order past the targets looked at) take their predecessor's
//!    offset.
//!
//! Refused, with the reason told: archives that are not zip, zip64,
//! multi-disk; books without a package document or spine. Parts that
//! cannot be read (encrypted, another method, over the size caps, a
//! damaged stream or CRC, a truncated file) become a one-line note in the
//! text (`[damaged]` …) and reading goes on. Deflate bombs stop at the
//! entry's declared size, at [`PART_CAP`] per part and [`BOOK_CAP`] per
//! book.
//!
//! Memory: [`Epub`] holds no window. A deflated entry needs the lent
//! [`crate::inflate::Scratch`] ([`crate::inflate::SCRATCH`] bytes) for
//! every step from its first byte to its last; a step without one waits
//! ([`Step::NeedScratch`]) or, if an entry was half decoded, gives up
//! ([`Step::Lost`]: the caller starts again).

use crate::convert::{Converter, Format, ROOM, WATCH, Watch};
use crate::files::{TOC_ENTRY, encode_toc};
use crate::inflate::{Inflate, SCRATCH, Scratch, Status};
use crate::norm::Sink;
use crate::opf::{self, CONTENT, Doc, Metadata, Out, Paths};
use crate::zip::{self, CdWalk, DEFLATED, End, Entry, Name, STORED, ZipError, crc32, find_end, local_data};

/// A contents entry's offset before its anchor is found.
pub const UNRESOLVED: u32 = u32::MAX;
/// Bytes of a manifest record: id hash, path hash, kind.
pub const MAN_REC: usize = 16;
/// Bytes of a spine record: the idref's hash.
pub const SPINE_REC: usize = 4;
/// Bytes of a target record: path hash, fragment hash, has fragment.
pub const TGT_REC: usize = 16;
/// Manifest and spine items kept (more are ignored).
pub const MAX_ITEMS: u32 = 8192;
/// Contents entries kept.
pub const MAX_TOC: u32 = 4096;
/// The most a package document (container, OPF, navigation) may inflate
/// to.
pub const DOC_CAP: u32 = 4 << 20;
/// The most one content document may inflate to.
pub const PART_CAP: u32 = 16 << 20;
/// The most a book may inflate to in all.
pub const BOOK_CAP: u64 = 64 << 20;
/// Inflated bytes one step makes at most.
pub const STEP_OUT: usize = 4096;
const BUF: usize = 512;
/// Table records one step reads at most.
const SCAN: u32 = 16;
/// Target records looked past without finding a part's first one.
const SKIP: u32 = 64;
/// Records or offsets one step writes at most.
const WRITES: u32 = 8;

/// The job's scratch tables and the book's contents file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Table {
    Manifest,
    Spine,
    Targets,
    /// The contents file (`files::TOC_ENTRY` bytes per entry).
    Toc,
}

/// The card, as the job sees it.
#[allow(async_fn_in_trait)]
pub trait Store {
    /// The book file's bytes from `at` (fewer at its end, none on an
    /// error).
    async fn read(&mut self, at: u32, buf: &mut [u8]) -> usize;
    /// A table's bytes from `at`; false if they are not all there.
    async fn get(&mut self, t: Table, at: u32, buf: &mut [u8]) -> bool;
    /// Write a table's bytes at `at` (appending or in place).
    async fn put(&mut self, t: Table, at: u32, data: &[u8]) -> bool;
}

/// Why a book cannot be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fail {
    NotZip,
    Zip64,
    MultiDisk,
    /// No package document (or it cannot be read).
    NoPackage,
    /// A package document without a spine.
    Empty,
}

/// What a step did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Busy,
    /// Waits for the scratch (nothing half done).
    NeedScratch,
    /// The scratch was not lent in the middle of an entry: start again.
    Lost,
    Done,
    Failed(Fail),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Look {
    Container,
    Package,
    /// Any `*.opf` (the container did not help).
    AnyPackage,
    Nav,
    Ncx,
    Part,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Before a part: entries to watch.
    Load,
    /// During a part: more entries as watched ones resolve.
    Refill,
    /// After a part: the rest point at its start.
    Finish,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    End {
        at: u32,
    },
    Find {
        look: Look,
        pos: u32,
    },
    Local {
        look: Look,
    },
    Data {
        look: Look,
    },
    Spine,
    Manifest {
        idref: u32,
        i: u32,
    },
    /// `from`: where the scan started (it wraps to the start once).
    Targets {
        i: u32,
        matched: bool,
        skipped: u32,
        mode: Mode,
        from: u32,
    },
    PartEnd,
    Finish,
    Fixup {
        i: u32,
        last: u32,
    },
    Done,
    Failed(Fail),
}

/// Who reads the entry's bytes.
#[allow(clippy::large_enum_variant)]
enum Stage {
    Doc(Doc),
    Text(Converter),
}

/// The paths a package document of `kind` is read with (container: its
/// fixed path, the package's path read into `nav` until it is known
/// good; package: its own path, the navigation document's and the
/// NCX's).
fn paths<'a>(kind: opf::Kind, package: &'a mut Name, nav: &'a mut Name, ncx: &'a mut Name) -> Paths<'a> {
    match kind {
        opf::Kind::Container => Paths { base: b"META-INF/container.xml", path: nav, ncx },
        opf::Kind::Package => Paths { base: package.as_bytes(), path: nav, ncx },
        opf::Kind::Nav => Paths { base: nav.as_bytes(), path: package, ncx },
        opf::Kind::Ncx => Paths { base: ncx.as_bytes(), path: package, ncx: nav },
    }
}

/// An EPUB conversion (or, [`Epub::library`], just its title and author).
pub struct Epub {
    st: St,
    len: u32,
    meta_only: bool,
    end: End,
    walk: CdWalk,
    want: u64,
    ent: Entry,
    /// The entry: where its next bytes are, compressed bytes not read.
    next: u32,
    left: u32,
    inf: Inflate,
    inflated: bool,
    deflating: bool,
    /// Bytes out of the entry, their CRC, and those not yet parsed (in
    /// the window, or in `buf` for a stored entry).
    out_n: u32,
    crc: u32,
    pend: usize,
    total: u64,
    buf: [u8; BUF],
    buf_n: usize,
    buf_at: usize,
    stage: Stage,
    guess: Option<Entry>,
    /// The package document's path (or the first `*.opf` seen), the
    /// navigation document's, the NCX's.
    package: Name,
    nav: Name,
    ncx: Name,
    has_nav: bool,
    has_ncx: bool,
    meta: Metadata,
    /// Contents entries watched for in the part being read.
    watch: Watch,
    man_n: u32,
    spine_n: u32,
    /// Contents entries written; from the navigation (not headings).
    pub toc_n: u32,
    nav_toc: bool,
    /// Spine item being read, its path and where its text starts.
    k: u32,
    part: u64,
    part_start: u32,
    /// The part's head was seen (its encoding).
    head: bool,
    /// Targets: the first not looked at for a later part; more of this
    /// part's run waiting for a watch slot.
    tgt_at: u32,
    run_next: Option<u32>,
    note: Option<&'static str>,
    writes: u32,
    text_len: u32,
    /// Parts that could not be read (or not wholly).
    pub damaged: u16,
}

fn rd32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn rd64(b: &[u8], o: usize) -> u64 {
    u64::from(rd32(b, o)) | (u64::from(rd32(b, o + 4)) << 32)
}

impl Epub {
    /// Convert the book of `len` bytes.
    #[must_use]
    // Inlined: `shelf::begin_epub` and `epub_head` then build it straight
    // into the job instead of a 2.9 KB temporary copied after (PG-2).
    #[inline(always)]
    pub fn new(len: u32) -> Epub {
        Epub {
            st: St::End { at: len.saturating_sub(BUF as u32) },
            len,
            meta_only: false,
            end: End::default(),
            walk: CdWalk::new(0),
            want: 0,
            ent: Entry::default(),
            next: 0,
            left: 0,
            inf: Inflate::new(),
            inflated: false,
            deflating: false,
            out_n: 0,
            crc: 0,
            pend: 0,
            total: 0,
            buf: [0; BUF],
            buf_n: 0,
            buf_at: 0,
            stage: Stage::Text(Converter::new(Format::Epub, &[], false)),
            guess: None,
            package: Name::new(),
            nav: Name::new(),
            ncx: Name::new(),
            has_nav: false,
            has_ncx: false,
            meta: Metadata::default(),
            watch: Watch::default(),
            man_n: 0,
            spine_n: 0,
            toc_n: 0,
            nav_toc: false,
            k: 0,
            part: 0,
            part_start: 0,
            head: false,
            tgt_at: 0,
            run_next: None,
            note: None,
            writes: 0,
            text_len: 0,
            damaged: 0,
        }
    }

    /// Read only the title, author and language (the library search).
    #[must_use]
    #[inline(always)]
    pub fn library(len: u32) -> Epub {
        // Not `Epub { meta_only: true, ..Epub::new(len) }`: the struct
        // update built a second 2.8 KB copy on the stack (the card task's
        // deepest path, `shelf::epub_head`'s frame 5,784 B; PG-2).
        let mut e = Epub::new(len);
        e.meta_only = true;
        e
    }

    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.meta
    }

    /// Bytes of normalised text (once done).
    #[must_use]
    pub fn text_len(&self) -> u32 {
        self.text_len
    }

    /// Bytes of normalised text so far.
    #[must_use]
    pub fn offset(&self) -> u32 {
        match &self.stage {
            Stage::Text(c) => c.offset(),
            Stage::Doc(_) => 0,
        }
    }

    /// Progress, per mille of the spine.
    #[must_use]
    pub fn permille(&self) -> u16 {
        if self.spine_n == 0 || !matches!(self.stage, Stage::Text(_)) {
            return 0;
        }
        let part = if self.ent.csize > 0 && matches!(self.st, St::Data { look: Look::Part }) {
            u64::from(self.ent.csize - self.left) * 1000 / u64::from(self.ent.csize)
        } else {
            0
        };
        ((u64::from(self.k) * 1000 + part) / u64::from(self.spine_n)).min(999) as u16
    }

    /// An entry the job reads now needs the scratch (a deflated one).
    #[must_use]
    pub fn needs_scratch(&self) -> bool {
        matches!(self.st, St::Data { .. }) && self.ent.method == DEFLATED
    }

    fn fail(&mut self, f: Fail) {
        self.st = St::Failed(f);
    }

    fn find(&mut self, look: Look, hash: u64) {
        self.want = hash;
        self.walk = CdWalk::new(self.end.bias);
        self.st = St::Find { look, pos: self.end.cd_at };
    }

    /// One piece of the conversion. `scratch`: the lent memory for
    /// deflated entries (the same bytes, untouched, from an entry's first
    /// step to its last); `out`: the stream's sink, emptied by the caller
    /// (it needs [`ROOM`] bytes free to go on).
    pub async fn step<S: Store>(
        &mut self,
        s: &mut S,
        scratch: Option<&mut [u8]>,
        out: &mut dyn Sink,
    ) -> Step {
        self.writes = 0;
        match self.st {
            St::Done => return Step::Done,
            St::Failed(f) => return Step::Failed(f),
            St::End { at } => self.end_step(s, at).await,
            St::Find { look, pos } => self.find_step(s, look, pos).await,
            St::Local { look } => self.local_step(s, look).await,
            St::Data { look } => return self.data_step(s, look, scratch, out).await,
            St::Spine => self.spine_step(s).await,
            St::Manifest { idref, i } => self.manifest_step(s, idref, i).await,
            St::Targets { i, matched, skipped, mode, from } => {
                self.targets_step(s, i, matched, skipped, mode, from).await;
            }
            St::PartEnd => self.part_end(s, out).await,
            St::Finish => self.finish(s, out).await,
            St::Fixup { i, last } => self.fixup(s, i, last).await,
        }
        match self.st {
            St::Done => Step::Done,
            St::Failed(f) => Step::Failed(f),
            _ => Step::Busy,
        }
    }

    // ----- the archive ---------------------------------------------------

    async fn end_step<S: Store>(&mut self, s: &mut S, at: u32) {
        let n = (self.len - at.min(self.len)).min(BUF as u32) as usize;
        let n = s.read(at, &mut self.buf[..n]).await;
        match find_end(&self.buf[..n], at, self.len) {
            Some(Ok(e)) => {
                self.end = e;
                self.find(Look::Container, zip::name_hash(b"META-INF/container.xml"));
            }
            Some(Err(ZipError::Zip64)) => self.fail(Fail::Zip64),
            Some(Err(ZipError::MultiDisk)) => self.fail(Fail::MultiDisk),
            Some(Err(ZipError::NotZip)) => self.fail(Fail::NotZip),
            None => {
                let back = self.len - at;
                if at == 0 || n == 0 || back >= (zip::END as u32 + zip::MAX_COMMENT + BUF as u32) {
                    self.fail(Fail::NotZip);
                } else {
                    self.st = St::End { at: at.saturating_sub((BUF - zip::END + 1) as u32) };
                }
            }
        }
    }

    async fn find_step<S: Store>(&mut self, s: &mut S, look: Look, pos: u32) {
        let cd_end = self.end.cd_at.saturating_add(self.end.cd_size).min(self.len);
        if pos >= cd_end || self.walk.bad {
            return self.missing(look);
        }
        let n = (cd_end - pos).min(BUF as u32) as usize;
        let n = s.read(pos, &mut self.buf[..n]).await;
        if n == 0 {
            return self.missing(look);
        }
        let mut at = 0;
        while at < n {
            // Names are kept (in `package`) while a `*.opf` is looked for.
            let keep = (look == Look::Container && self.guess.is_none()) || look == Look::AnyPackage;
            let (used, e) = self.walk.feed(&self.buf[at..n], keep.then_some(&mut self.package));
            at += used;
            let Some(e) = e else { continue };
            let opf = keep && self.package.ends_with(b".opf") && !self.package.cut;
            if look == Look::Container && opf {
                self.guess = Some(e);
            }
            let hit = if look == Look::AnyPackage { opf } else { e.hash == self.want };
            if hit {
                self.ent = e;
                self.st = St::Local { look };
                return;
            }
        }
        self.st = St::Find { look, pos: pos + n as u32 };
    }

    /// The entry is not in the archive.
    fn missing(&mut self, look: Look) {
        match look {
            Look::Container => match self.guess {
                Some(g) => {
                    self.ent = g;
                    self.st = St::Local { look: Look::Package };
                }
                None => self.fail(Fail::NoPackage),
            },
            Look::Package => self.find(Look::AnyPackage, 0),
            Look::AnyPackage => self.fail(Fail::NoPackage),
            Look::Nav => self.after_nav(),
            Look::Ncx => self.start_text(),
            Look::Part => {
                self.k += 1;
                self.st = St::Spine;
            }
        }
    }

    async fn local_step<S: Store>(&mut self, s: &mut S, look: Look) {
        let at = self.ent.local;
        let n = (self.len - at.min(self.len)).min(BUF as u32) as usize;
        let n = s.read(at, &mut self.buf[..n]).await;
        let cap = if look == Look::Part { PART_CAP } else { DOC_CAP };
        let why = if !self.ent.readable() {
            Some(if self.ent.encrypted() { "[encrypted]" } else { "[unsupported]" })
        } else if self.ent.usize > cap || (look == Look::Part && self.total >= BOOK_CAP) {
            Some("[too large]")
        } else {
            None
        };
        let off = local_data(&self.buf[..n]).map(|o| o as usize);
        let (Some(off), None) = (off, why) else {
            return self.unreadable(look, why.unwrap_or("[damaged]"));
        };
        let first = n.saturating_sub(off).min(self.ent.csize as usize);
        self.buf_at = off.min(n);
        self.buf_n = self.buf_at + first;
        self.next = at.saturating_add((off + first) as u32);
        self.left = self.ent.csize - first as u32;
        self.inf = Inflate::new();
        self.inflated = false;
        self.deflating = false;
        self.out_n = 0;
        self.crc = 0;
        self.pend = 0;
        let doc = match look {
            Look::Container => Some(Doc::new(opf::Kind::Container)),
            Look::Package | Look::AnyPackage => {
                let mut d = Doc::new(opf::Kind::Package);
                d.stop_at_meta = self.meta_only;
                Some(d)
            }
            Look::Nav => Some(Doc::new(opf::Kind::Nav)),
            Look::Ncx => Some(Doc::new(opf::Kind::Ncx)),
            Look::Part => None,
        };
        if let Some(d) = doc {
            self.stage = Stage::Doc(d);
        }
        self.head = false;
        self.st = St::Data { look };
    }

    /// The entry cannot be read.
    fn unreadable(&mut self, look: Look, why: &'static str) {
        match look {
            Look::Part => {
                self.damaged = self.damaged.saturating_add(1);
                self.note = Some(why);
                self.st = St::PartEnd;
            }
            Look::Package | Look::AnyPackage => self.fail(Fail::NoPackage),
            _ => self.missing(look),
        }
    }

    // ----- an entry's data -----------------------------------------------

    async fn data_step<S: Store>(
        &mut self,
        s: &mut S,
        look: Look,
        scratch: Option<&mut [u8]>,
        out: &mut dyn Sink,
    ) -> Step {
        let deflated = self.ent.method == DEFLATED;
        // Less than the full window and tables counts as none.
        let mut scratch = scratch.filter(|m| m.len() >= SCRATCH);
        if deflated && self.deflating && scratch.is_none() {
            return Step::Lost;
        }
        if self.pend == 0 {
            let done = if deflated { self.inflated } else { self.left == 0 && self.buf_at == self.buf_n };
            if done {
                self.entry_end(s, look, out, false).await;
                return self.state();
            }
            if self.buf_at == self.buf_n && self.left > 0 {
                let n = self.left.min(BUF as u32) as usize;
                let got = s.read(self.next, &mut self.buf[..n]).await;
                self.buf_at = 0;
                self.buf_n = got;
                self.next = self.next.saturating_add(got as u32);
                self.left -= got as u32;
                if got == 0 {
                    // The file ends inside the entry.
                    self.left = 0;
                    self.entry_end(s, look, out, true).await;
                }
                return self.state();
            }
            if deflated {
                let Some(mem) = scratch.as_deref_mut().filter(|m| m.len() >= SCRATCH) else {
                    return Step::NeedScratch;
                };
                let Some(mut sc) = Scratch::new(mem) else { return Step::NeedScratch };
                self.deflating = true;
                let end = self.left == 0;
                match self.inf.run(&mut sc, &self.buf[self.buf_at..self.buf_n], end, STEP_OUT) {
                    Ok(r) => {
                        self.buf_at += r.used;
                        self.made(&sc, r.made);
                        self.inflated = r.status == Status::Done;
                        if r.status == Status::More && end && self.buf_at == self.buf_n {
                            self.entry_end(s, look, out, true).await;
                            return self.state();
                        }
                    }
                    Err(_) => {
                        self.entry_end(s, look, out, true).await;
                        return self.state();
                    }
                }
            } else {
                let n = self.buf_n - self.buf_at;
                let (a, b) = (self.buf_at, self.buf_n);
                self.crc = crc32(self.crc, &self.buf[a..b]);
                self.out_n = self.out_n.saturating_add(n as u32);
                self.total += n as u64;
                self.pend = n;
            }
            if self.over() {
                self.pend = 0;
                self.entry_end(s, look, out, true).await;
                return self.state();
            }
        }
        let sc = match scratch {
            Some(mem) if deflated => Scratch::new(mem),
            _ => None,
        };
        self.consume(s, look, sc.as_ref(), out).await;
        self.state()
    }

    fn state(&self) -> Step {
        match self.st {
            St::Done => Step::Done,
            St::Failed(f) => Step::Failed(f),
            _ => Step::Busy,
        }
    }

    /// Inflated bytes made: counted and summed.
    fn made(&mut self, sc: &Scratch<'_>, n: usize) {
        let (a, b) = self.inf.made(sc, n);
        self.crc = crc32(crc32(self.crc, a), b);
        self.out_n = self.out_n.saturating_add(n as u32);
        self.total += n as u64;
        self.pend = n;
    }

    /// More out than the entry says it holds, or than the caps allow.
    fn over(&self) -> bool {
        self.out_n > self.ent.usize || self.total > BOOK_CAP
    }

    /// The pending bytes (two pieces: the window may wrap).
    fn pending<'a>(
        ent: &Entry,
        buf: &'a [u8],
        (at, n, pend): (usize, usize, usize),
        inf: &Inflate,
        sc: Option<&'a Scratch<'_>>,
    ) -> (&'a [u8], &'a [u8]) {
        if ent.method == STORED {
            let end = (at + pend).min(n);
            (&buf[at..end], &[][..])
        } else {
            match sc {
                Some(sc) => inf.made(sc, pend),
                None => (&[][..], &[][..]),
            }
        }
    }

    fn feed_doc(&mut self, sc: Option<&Scratch<'_>>) -> usize {
        let Epub { ent, buf, buf_at, buf_n, pend, inf, stage, package, nav, ncx, .. } = self;
        let (a, b) = Self::pending(ent, buf, (*buf_at, *buf_n, *pend), inf, sc);
        let Stage::Doc(d) = stage else { return 0 };
        let mut p = paths(d.kind(), package, nav, ncx);
        let mut used = d.feed(a, &mut p);
        if used == a.len() {
            used += d.feed(b, &mut p);
        }
        used
    }

    fn taken(&mut self, n: usize) {
        self.pend -= n;
        if self.ent.method == STORED {
            self.buf_at += n;
        }
    }

    /// Pending bytes to the document parser or the converter, until they
    /// are used, the sink is full, or enough was written.
    async fn consume<S: Store>(
        &mut self,
        s: &mut S,
        look: Look,
        sc: Option<&Scratch<'_>>,
        out: &mut dyn Sink,
    ) {
        for _ in 0..64 {
            if self.pend == 0 || self.writes >= WRITES {
                return;
            }
            if look == Look::Part {
                if out.room() < ROOM {
                    return;
                }
                let used = self.feed_text(sc, out);
                self.taken(used);
                let mut wrote = false;
                if let Stage::Text(c) = &mut self.stage
                    && let Some(e) = c.take_toc()
                {
                    self.put_toc(s, &e).await;
                    wrote = true;
                }
                if let Some((i, off)) = self.watch.take_hit() {
                    self.resolve(s, i, off).await;
                    wrote = true;
                    if let Some(i) = self.run_next
                        && self.watch.len() < WATCH
                    {
                        self.run_next = None;
                        self.st = St::Targets { i, matched: true, skipped: 0, mode: Mode::Refill, from: 0 };
                        return;
                    }
                }
                if used == 0 && !wrote {
                    return;
                }
            } else {
                let used = self.feed_doc(sc);
                self.taken(used);
                let mut wrote = false;
                while let Stage::Doc(d) = &mut self.stage
                    && let Some(o) = d.take()
                {
                    self.record(s, o).await;
                    wrote = true;
                }
                if self.meta_only
                    && let Stage::Doc(d) = &self.stage
                    && d.meta_done()
                {
                    self.meta = *d.meta();
                    self.st = St::Done;
                    return;
                }
                if used == 0 && !wrote {
                    return;
                }
            }
        }
    }

    fn feed_text(&mut self, sc: Option<&Scratch<'_>>, out: &mut dyn Sink) -> usize {
        let Epub { ent, buf, buf_at, buf_n, pend, inf, stage, head, watch, .. } = self;
        let (a, b) = Self::pending(ent, buf, (*buf_at, *buf_n, *pend), inf, sc);
        let Stage::Text(c) = stage else { return 0 };
        if !*head {
            // The part's encoding from its first bytes.
            *head = true;
            c.begin_part(a, a.len() + b.len() == ent.usize as usize);
        }
        let mut used = c.feed_watched(a, out, Some(&mut *watch));
        if used == a.len() {
            used += c.feed_watched(b, out, Some(watch));
        }
        used
    }

    async fn put_toc<S: Store>(&mut self, s: &mut S, e: &crate::norm::TocEntry) {
        if self.toc_n < MAX_TOC && s.put(Table::Toc, self.toc_n * TOC_ENTRY as u32, &encode_toc(e)).await {
            self.toc_n += 1;
        }
        self.writes += 1;
    }

    /// Contents entry `i` points at `offset`.
    async fn resolve<S: Store>(&mut self, s: &mut S, i: u32, offset: u32) {
        let _ = s.put(Table::Toc, i * TOC_ENTRY as u32, &offset.to_le_bytes()).await;
        self.writes += 1;
    }

    /// A package document's record to its table.
    async fn record<S: Store>(&mut self, s: &mut S, o: Out) {
        if self.meta_only {
            return;
        }
        self.writes += 1;
        match o {
            Out::Item { id, path, kind } if self.man_n < MAX_ITEMS => {
                let mut r = [0u8; MAN_REC];
                r[..4].copy_from_slice(&id.to_le_bytes());
                r[4..12].copy_from_slice(&path.to_le_bytes());
                r[12] = kind;
                if s.put(Table::Manifest, self.man_n * MAN_REC as u32, &r).await {
                    self.man_n += 1;
                }
            }
            Out::Spine { idref } if self.spine_n < MAX_ITEMS => {
                if s.put(Table::Spine, self.spine_n * SPINE_REC as u32, &idref.to_le_bytes()).await {
                    self.spine_n += 1;
                }
            }
            Out::Toc { entry, target } if self.toc_n < MAX_TOC => {
                let mut r = [0u8; TGT_REC];
                r[..8].copy_from_slice(&target.path.to_le_bytes());
                r[8..12].copy_from_slice(&target.frag.unwrap_or(0).to_le_bytes());
                r[12] = u8::from(target.frag.is_some());
                let i = self.toc_n;
                if s.put(Table::Targets, i * TGT_REC as u32, &r).await
                    && s.put(Table::Toc, i * TOC_ENTRY as u32, &encode_toc(&entry)).await
                {
                    self.toc_n += 1;
                }
            }
            _ => {}
        }
    }

    /// The entry's bytes are all read (or it broke off: `broken`).
    async fn entry_end<S: Store>(&mut self, s: &mut S, look: Look, out: &mut dyn Sink, broken: bool) {
        let bad = broken || self.out_n != self.ent.usize || self.crc != self.ent.crc;
        self.pend = 0;
        if look == Look::Part {
            if bad {
                self.damaged = self.damaged.saturating_add(1);
                self.note = Some("[damaged]");
            }
            self.st = St::PartEnd;
            return;
        }
        let _ = out;
        {
            let Epub { stage, package, nav, ncx, .. } = self;
            let Stage::Doc(d) = stage else { return };
            d.finish(&mut paths(d.kind(), package, nav, ncx));
        }
        while let Stage::Doc(d) = &mut self.stage
            && let Some(o) = d.take()
        {
            self.record(s, o).await;
        }
        let Stage::Doc(d) = &self.stage else { return };
        match look {
            Look::Container => {
                if d.found() {
                    // The container's path was read into `nav` (the
                    // `*.opf` seen stays in `package` until then).
                    self.package = self.nav;
                    self.find(Look::Package, self.package.hash());
                } else {
                    self.missing(Look::Container);
                }
            }
            Look::Package | Look::AnyPackage => {
                self.meta = *d.meta();
                self.has_nav = d.found();
                self.has_ncx = d.has_ncx();
                if self.meta_only {
                    self.st = St::Done;
                } else if self.spine_n == 0 {
                    self.fail(Fail::Empty);
                } else if self.has_nav {
                    self.find(Look::Nav, self.nav.hash());
                } else {
                    self.after_nav();
                }
            }
            Look::Nav => self.after_nav(),
            _ => self.start_text(),
        }
    }

    /// The navigation document is read (or missing): the NCX if it gave
    /// nothing.
    fn after_nav(&mut self) {
        if self.has_ncx && self.toc_n == 0 {
            self.find(Look::Ncx, self.ncx.hash());
        } else {
            self.start_text();
        }
    }

    fn start_text(&mut self) {
        self.nav_toc = self.toc_n > 0;
        let mut c = Converter::new(Format::Epub, &[], false);
        c.headings(!self.nav_toc);
        self.stage = Stage::Text(c);
        self.k = 0;
        self.tgt_at = 0;
        self.st = St::Spine;
    }

    // ----- the spine ---------------------------------------------------

    async fn spine_step<S: Store>(&mut self, s: &mut S) {
        if self.k >= self.spine_n {
            self.st = St::Finish;
            return;
        }
        let mut r = [0u8; SPINE_REC];
        if !s.get(Table::Spine, self.k * SPINE_REC as u32, &mut r).await {
            self.st = St::Finish;
            return;
        }
        self.st = St::Manifest { idref: u32::from_le_bytes(r), i: 0 };
    }

    async fn manifest_step<S: Store>(&mut self, s: &mut S, idref: u32, mut i: u32) {
        let mut r = [0u8; MAN_REC];
        for _ in 0..SCAN {
            if i >= self.man_n || !s.get(Table::Manifest, i * MAN_REC as u32, &mut r).await {
                // Not in the manifest.
                self.k += 1;
                self.st = St::Spine;
                return;
            }
            if rd32(&r, 0) == idref {
                if r[12] != CONTENT {
                    self.k += 1;
                    self.st = St::Spine;
                    return;
                }
                self.part = rd64(&r, 4);
                self.part_start = match &self.stage {
                    Stage::Text(c) => c.anchor_at(),
                    Stage::Doc(_) => 0,
                };
                self.note = None;
                self.head = false;
                self.watch.clear();
                if self.nav_toc {
                    let i = self.tgt_at;
                    self.st = St::Targets { i, matched: false, skipped: 0, mode: Mode::Load, from: i };
                } else {
                    self.find(Look::Part, self.part);
                }
                return;
            }
            i += 1;
        }
        self.st = St::Manifest { idref, i };
    }

    /// Contents entries pointing into the part: watched (or, without a
    /// fragment or after the part, given its start). The scan starts after
    /// the last part's entries (contents in reading order); finding none
    /// there, it looks once from the start (contents out of order).
    async fn targets_step<S: Store>(
        &mut self,
        s: &mut S,
        mut i: u32,
        mut matched: bool,
        mut skipped: u32,
        mode: Mode,
        from: u32,
    ) {
        let go_on = |e: &mut Epub| match mode {
            Mode::Load => e.find(Look::Part, e.part),
            Mode::Refill => e.st = St::Data { look: Look::Part },
            Mode::Finish => e.st = St::PartEnd,
        };
        let mut r = [0u8; TGT_REC];
        for _ in 0..SCAN {
            let wrap = mode == Mode::Load && !matched && from > 0;
            if i >= self.toc_n || !s.get(Table::Targets, i * TGT_REC as u32, &mut r).await {
                if matched {
                    self.tgt_at = i;
                }
                self.run_next = None;
                if wrap {
                    self.st = St::Targets { i: 0, matched, skipped: 0, mode, from: 0 };
                    return;
                }
                return go_on(self);
            }
            if rd64(&r, 0) == self.part {
                matched = true;
                if r[12] == 0 || mode == Mode::Finish {
                    let at = self.part_start;
                    self.resolve(s, i, at).await;
                } else {
                    if !self.watch.add(rd32(&r, 8), i) {
                        self.run_next = Some(i);
                        return go_on(self);
                    }
                }
                i += 1;
                if self.writes >= WRITES {
                    break;
                }
            } else if matched {
                self.tgt_at = i;
                self.run_next = None;
                return go_on(self);
            } else {
                skipped += 1;
                if skipped > SKIP
                    || (from == 0 && i + 1 == self.tgt_at && mode == Mode::Load && self.tgt_at > 0)
                {
                    self.run_next = None;
                    if wrap && skipped > SKIP {
                        self.st = St::Targets { i: 0, matched, skipped: 0, mode, from: 0 };
                        return;
                    }
                    return go_on(self);
                }
                i += 1;
            }
        }
        self.st = St::Targets { i, matched, skipped, mode, from };
    }

    /// The part ended: its last text, a note if it was damaged, its
    /// unresolved entries to its start.
    async fn part_end<S: Store>(&mut self, s: &mut S, out: &mut dyn Sink) {
        if out.room() < ROOM {
            return;
        }
        let Stage::Text(c) = &mut self.stage else {
            self.k += 1;
            self.st = St::Spine;
            return;
        };
        if self.head {
            self.head = false;
            c.end_part(out);
        }
        if let Some(n) = self.note.take() {
            c.note(n, out);
        }
        if let Some(e) = c.take_toc() {
            return self.put_toc(s, &e).await;
        }
        if let Some((i, off)) = self.watch.take_hit() {
            return self.resolve(s, i, off).await;
        }
        if let Some(i) = self.watch.pop_unresolved() {
            let at = self.part_start;
            return self.resolve(s, i, at).await;
        }
        if let Some(i) = self.run_next.take() {
            self.st = St::Targets { i, matched: true, skipped: 0, mode: Mode::Finish, from: 0 };
            return;
        }
        self.k += 1;
        self.st = St::Spine;
    }

    async fn finish<S: Store>(&mut self, s: &mut S, out: &mut dyn Sink) {
        if out.room() < ROOM {
            return;
        }
        let Stage::Text(c) = &mut self.stage else { return };
        c.finish(out);
        self.text_len = c.offset();
        if let Some(e) = c.take_toc() {
            self.put_toc(s, &e).await;
        }
        self.st = if self.nav_toc { St::Fixup { i: 0, last: 0 } } else { St::Done };
    }

    /// Entries never resolved take their predecessor's offset.
    async fn fixup<S: Store>(&mut self, s: &mut S, mut i: u32, mut last: u32) {
        let mut b = [0u8; 4];
        for _ in 0..SCAN {
            if i >= self.toc_n {
                self.st = St::Done;
                return;
            }
            if s.get(Table::Toc, i * TOC_ENTRY as u32, &mut b).await {
                let off = u32::from_le_bytes(b);
                if off == UNRESOLVED || off > self.text_len {
                    self.resolve(s, i, last).await;
                } else {
                    last = off;
                }
            }
            i += 1;
        }
        self.st = St::Fixup { i, last };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_fit() {
        const { assert!(TOC_ENTRY >= 4 && MAN_REC >= 13 && TGT_REC >= 13) };
        assert_eq!(Epub::new(0).permille(), 0);
    }
}
