//! The index on a `orpheus-fs` volume: [`Storage`], [`DirSource`] and
//! [`NameSource`] over one [`Volume`], for the firmware's card task (and the
//! host tests, over a RAM disk).
//!
//! `orpheus-fs` is async and the index traits are synchronous. [`CardIo`]
//! runs every file-system call to completion with [`spin`]: it polls the
//! call's future in a loop with the caller's task [`Context`]. The firmware
//! calls the builder from inside a `poll_fn` of the task that owns the
//! volume, so a timer the SD driver awaits registers with a real task
//! waker (a stray wake-up later is harmless). Nothing else runs on that
//! executor while it spins: callers keep the work per call bounded
//! ([`crate::Builder::step`] budgets) and give time back between calls.
//!
//! Files: index files go to `/.orpheus/index/X/NAME`, where `X` is the last
//! hex digit of the directory identity (16 shards keep each directory
//! short, so a lookup by name stays cheap with thousands of indexed
//! folders); the merge scratch files to `/.orpheus/index/sort-a.tmp` and
//! `sort-b.tmp`, or `sort-c.tmp`/`sort-d.tmp` with [`Mode::Write`]`{ alt:
//! true }` (a second build can run while the first one is suspended). The
//! directories are created on first use; `orpheus-fs` refuses writes outside
//! `/.orpheus/` anyway. Open files are kept in a [`Handles`] table that the
//! caller owns, so they survive between calls; [`CardIo::close_all`] writes
//! their directory entries back. [`CardIo::new_in`] puts the [`Storage`]
//! files flat into another folder instead (the music library's
//! `/.orpheus/library`).

use core::cell::RefCell;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll};

use orpheus_fs::{BlockDevice, DirCursor, DirEntry, File, Locator, OpenMode, Volume};

use crate::{DirSource, FileName, MAX_NAME, NameSource, SrcEntry, Storage, mtime_of};

/// The file-system error of a volume on `D`.
pub type FsError<D> = orpheus_fs::Error<<D as BlockDevice>::Error>;

/// The directory under which every index file lives.
pub const INDEX_DIR: &str = "/.orpheus/index";

/// Files kept open at once (the index and the two scratch files).
const OPEN: usize = 3;

/// Longest path this module builds: `/.orpheus/index/X/` + 16.
const PATH: usize = 40;

/// Run `f` to completion, polling it with `cx` in a loop.
#[inline(always)]
pub fn spin<F: Future>(cx: &mut Context<'_>, f: F) -> F::Output {
    let mut f = pin!(f);
    spin_pinned(cx, f.as_mut())
}

/// [`spin`] over a future already pinned in the caller's frame: the
/// future is not moved into another frame (2026-11-25: once `spin` stopped
/// being inlined into [`spin_in`], the file-open future sat in both frames,
/// +1.5 KB on the RT task's deepest chain).
pub fn spin_pinned<F: Future>(cx: &mut Context<'_>, mut f: core::pin::Pin<&mut F>) -> F::Output {
    let _sync = orpheus_fs::SyncPoll::enter();
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(cx) {
            return v;
        }
    }
}

/// [`spin`] in a frame of its own: `make` builds the future there, so it
/// takes no room in the caller's frame. A caller with several file-system
/// calls otherwise holds all their futures at once, under the deepest of
/// them (`Inner::slot`: 2.5 KB of the RT task's stack, VER-10).
#[inline(never)]
pub fn spin_in<F: Future>(cx: &mut Context<'_>, make: impl FnOnce() -> F) -> F::Output {
    let f = pin!(make());
    spin_pinned(cx, f)
}

/// Files opened by a [`CardIo`] (plain values, about 250 bytes). Keep the
/// table between calls of one session and call [`CardIo::close_all`] at
/// its end (before the volume is used for anything that might touch these
/// files, and before it is unmounted).
#[derive(Clone, Default)]
pub struct Handles {
    open: [Option<(FileName, File)>; OPEN],
    next: u8,
}

impl Handles {
    #[must_use]
    pub const fn new() -> Handles {
        Handles { open: [None, None, None], next: 0 }
    }

    /// No file is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.iter().all(Option::is_none)
    }
}

/// Where a file was found ([`CardIo::place`]): its directory entry and
/// the volume's serial number (12 bytes). A later session given it
/// ([`CardIo::reuse`]) reopens the file from there with
/// [`Volume::open_at`] (the entry read again and its name checked) instead
/// of walking `/.orpheus/index/X/NAME` from the root: on a card with a
/// large root folder that walk was tens to hundreds of sector reads
/// whenever the path-walk cache had lost `/.orpheus` (AU-PERF,
/// 2026-11-25: one 31-ms stretch in every listing piece). A place of
/// another volume, or one whose entry is gone or holds another name, is
/// not used: the file is looked for by path again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Place {
    loc: Locator,
    serial: u32,
}

impl Place {
    /// The file's directory entry.
    #[must_use]
    pub fn locator(&self) -> Locator {
        self.loc
    }
}

/// What a session may do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Queries only: files are opened read-only, nothing is created.
    Read,
    /// Building. `alt`: use the second pair of scratch files.
    Write { alt: bool },
}

/// [`DirSource::next_entry`]'s view of a `orpheus-fs` entry (also for
/// [`crate::Signature::add`] in a pass the caller makes itself). Hidden:
/// the FAT hidden or system attribute.
#[must_use]
pub fn src_entry(e: &DirEntry) -> SrcEntry {
    SrcEntry {
        loc: e.locator(),
        is_dir: e.is_dir(),
        hidden: e.attributes() & 0x06 != 0,
        size: e.size(),
        mtime: mtime_of(&e.modified()),
        name_len: e.name().len(),
    }
}

/// `.` and `..` (FAT subdirectories list them; they are not entries).
#[must_use]
pub fn is_dot(e: &DirEntry) -> bool {
    matches!(e.name(), "." | "..")
}

/// Where a file of the index lives (`base`: flat in that directory
/// instead, [`CardIo::new_in`]).
fn path<'b>(f: FileName, alt: bool, base: Option<&str>, buf: &'b mut [u8; PATH]) -> &'b str {
    let name = f.as_str();
    let mut n = 0;
    let mut put = |s: &[u8]| {
        for &b in s {
            if let Some(d) = buf.get_mut(n) {
                *d = b;
                n += 1;
            }
        }
    };
    put(base.unwrap_or(INDEX_DIR).as_bytes());
    put(b"/");
    if base.is_some() {
        put(name.as_bytes());
    } else if f == FileName::TEMP_A || f == FileName::TEMP_B {
        if alt {
            put(if f == FileName::TEMP_A { b"sort-c.tmp" } else { b"sort-d.tmp" });
        } else {
            put(name.as_bytes());
        }
    } else {
        // `DDDDDDDD-SF.idx`: shard by the last hex digit of the directory.
        if let Some(&c) = name.as_bytes().get(7) {
            put(&[c, b'/']);
        }
        put(name.as_bytes());
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

struct Inner<'a, 'v, 'c, D: BlockDevice> {
    vol: &'a mut Volume<'v, D>,
    cx: &'a mut Context<'c>,
    h: &'a mut Handles,
    mode: Mode,
    base: Option<&'static str>,
    /// The place of one file: given ([`CardIo::reuse`]), or where a
    /// read-only session found it by path ([`CardIo::place`]).
    kept: Option<(FileName, Place)>,
}

impl<'v, 'c, D: BlockDevice> Inner<'_, 'v, 'c, D> {
    /// The table slot of `f`, opening it (and with `create`, creating it
    /// and its directories) if needed. `None`: it does not exist.
    fn slot(&mut self, f: FileName, create: bool) -> Result<Option<usize>, FsError<D>> {
        if let Some(i) = self.h.open.iter().position(|o| o.as_ref().is_some_and(|(n, _)| *n == f)) {
            return Ok(Some(i));
        }
        let (write, alt) = match self.mode {
            Mode::Read => (false, false),
            Mode::Write { alt } => (true, alt),
        };
        let mut pb = [0u8; PATH];
        let p = path(f, alt, self.base, &mut pb);
        let file = match self.open_read(f, p) {
            Some(r) => r,
            None => spin_in(self.cx, || {
                self.vol.open(p, if write { OpenMode::ReadWrite } else { OpenMode::Read })
            }),
        };
        let file = match file {
            Ok(file) => file,
            Err(orpheus_fs::Error::NotFound) if create && write => {
                // Parents one by one (`/.orpheus`, `/.orpheus/index`, the shard;
                // or the base directory's).
                for (i, _) in p.match_indices('/').skip(1) {
                    let _ = spin_in(self.cx, || self.vol.create_dir(&p[..i]));
                }
                spin_in(self.cx, || self.vol.open(p, OpenMode::Create))?
            }
            Err(orpheus_fs::Error::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let i = match self.h.open.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                // Full: close the oldest.
                let i = usize::from(self.h.next) % OPEN;
                self.h.next = self.h.next.wrapping_add(1);
                if let Some((_, old)) = self.h.open[i].take() {
                    spin_in(self.cx, || self.vol.close(old))?;
                }
                i
            }
        };
        self.h.open[i] = Some((f, file));
        Ok(Some(i))
    }

    /// A read-only session opens `f` (at path `p`) from its kept place if
    /// that is still good, else by path, keeping where it found it. `None`
    /// in a writing session (it opens by path).
    #[inline(never)]
    fn open_read(&mut self, f: FileName, p: &str) -> Option<Result<File, FsError<D>>> {
        if self.mode != Mode::Read {
            return None;
        }
        let serial = self.vol.serial();
        if let Some((k, at)) = self.kept
            && k == f
            && at.serial == serial
        {
            let name = p.rsplit('/').next().unwrap_or(p);
            match spin_in(self.cx, || self.vol.open_at(at.loc, name)) {
                Ok(file) => return Some(Ok(file)),
                // Gone, or another file there: by path.
                Err(orpheus_fs::Error::NotFound | orpheus_fs::Error::IsADirectory) => self.kept = None,
                Err(e) => return Some(Err(e)),
            }
        }
        Some(spin_in(self.cx, || self.vol.open_located(p)).map(|(file, loc)| {
            self.kept = Some((f, Place { loc, serial }));
            file
        }))
    }

    /// Run `op` on file `f` (opened as [`Inner::slot`] does); `None` if
    /// it does not exist.
    fn on<R>(
        &mut self,
        f: FileName,
        create: bool,
        op: impl FnOnce(&mut Volume<'v, D>, &mut Context<'c>, &mut File) -> Result<R, FsError<D>>,
    ) -> Result<Option<R>, FsError<D>> {
        let Some(i) = self.slot(f, create)? else { return Ok(None) };
        let Some(Some((_, file))) = self.h.open.get_mut(i) else { return Err(orpheus_fs::Error::Corrupt) };
        op(self.vol, self.cx, file).map(Some)
    }
}

/// One session's access to the volume: a [`Storage`], a [`NameSource`]
/// and directory sources, all sharing the volume. Build it inside a
/// `poll_fn` (or any place that has the task's [`Context`]).
pub struct CardIo<'a, 'v, 'c, D: BlockDevice> {
    inner: RefCell<Inner<'a, 'v, 'c, D>>,
}

impl<'a, 'v, 'c, D: BlockDevice> CardIo<'a, 'v, 'c, D> {
    pub fn new(vol: &'a mut Volume<'v, D>, cx: &'a mut Context<'c>, h: &'a mut Handles, mode: Mode) -> Self {
        CardIo { inner: RefCell::new(Inner { vol, cx, h, mode, base: None, kept: None }) }
    }

    /// A session whose [`Storage`] files live flat in `dir` (at most 22
    /// bytes, e.g. `/.orpheus/library`) instead of the index directory: no
    /// shards; [`Mode::Write`]'s `alt` has no effect.
    pub fn new_in(
        vol: &'a mut Volume<'v, D>,
        cx: &'a mut Context<'c>,
        h: &'a mut Handles,
        mode: Mode,
        dir: &'static str,
    ) -> Self {
        CardIo { inner: RefCell::new(Inner { vol, cx, h, mode, base: Some(dir), kept: None }) }
    }

    /// Reopen `f` from `at` (where an earlier session found it,
    /// [`CardIo::place`]) when this read-only session first needs it,
    /// instead of walking its path; if `at` is not good any more (another
    /// volume, the entry gone or renamed), by path as usual. Call it
    /// before the session first uses `f`.
    #[inline(never)]
    pub fn reuse(&self, f: &FileName, at: Place) {
        self.inner.borrow_mut().kept = Some((*f, at));
    }

    /// Where this read-only session found `f` (by path, or at the place
    /// given to [`CardIo::reuse`]): keep it for the next session. The
    /// place given if `f` was not opened; `None` if it was neither given
    /// nor found.
    #[must_use]
    #[inline(never)]
    pub fn place(&self, f: &FileName) -> Option<Place> {
        self.inner.borrow().kept.as_ref().filter(|(k, _)| k == f).map(|&(_, at)| at)
    }

    /// The index files.
    pub fn store(&self) -> CardStore<'_, 'a, 'v, 'c, D> {
        CardStore { io: &self.inner }
    }

    /// Names by locator (for [`crate::Index::find`]), through `e`.
    pub fn names<'r>(&'r self, e: &'r mut DirEntry) -> CardNames<'r, 'a, 'v, 'c, D> {
        CardNames { io: &self.inner, e }
    }

    /// The directory that `start` (a cursor at its first entry) lists.
    /// `cur` is where the source is: keep it between calls (a build reads
    /// the directory across many steps). Entries are read through `e`.
    pub fn dir<'r>(
        &'r self,
        start: DirCursor,
        cur: &'r mut DirCursor,
        e: &'r mut DirEntry,
    ) -> CardDir<'r, 'a, 'v, 'c, D> {
        CardDir { io: &self.inner, start, cur, e }
    }

    /// Run `f` on the volume (e.g. to read entries for a window).
    pub fn with_vol<R>(&self, f: impl FnOnce(&mut Volume<'v, D>, &mut Context<'c>) -> R) -> R {
        let mut io = self.inner.borrow_mut();
        let io = &mut *io;
        f(io.vol, io.cx)
    }

    /// Close every open file (writes back their directory entries).
    pub fn close_all(&self) -> Result<(), FsError<D>> {
        let mut io = self.inner.borrow_mut();
        let io = &mut *io;
        let mut r = Ok(());
        for o in &mut io.h.open {
            if let Some((_, f)) = o.take() {
                let c = spin_in(io.cx, || io.vol.close(f));
                if r.is_ok() {
                    r = c;
                }
            }
        }
        r
    }
}

/// [`Storage`] of a [`CardIo`].
pub struct CardStore<'r, 'a, 'v, 'c, D: BlockDevice> {
    io: &'r RefCell<Inner<'a, 'v, 'c, D>>,
}

impl<D: BlockDevice> Storage for CardStore<'_, '_, '_, '_, D> {
    type Error = FsError<D>;

    fn len(&mut self, file: FileName) -> Result<Option<u32>, Self::Error> {
        self.io.borrow_mut().on(file, false, |_, _, f| Ok(u32::try_from(f.size()).unwrap_or(u32::MAX)))
    }

    fn read_at(&mut self, file: FileName, offset: u32, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let r = self.io.borrow_mut().on(file, false, |vol, cx, f| {
            f.set_position(u64::from(offset));
            spin_in(cx, || vol.read_exact_or_eof(f, buf))
        })?;
        Ok(r.unwrap_or(0))
    }

    fn write_at(&mut self, file: FileName, offset: u32, data: &[u8]) -> Result<(), Self::Error> {
        let r = self.io.borrow_mut().on(file, true, |vol, cx, f| {
            f.set_position(u64::from(offset));
            spin_in(cx, || vol.write(f, data))
        })?;
        r.map(|_| ()).ok_or(orpheus_fs::Error::NotFound)
    }

    fn truncate(&mut self, file: FileName, len: u32) -> Result<(), Self::Error> {
        // A missing file is as good as an empty one (no scratch files are
        // created for a directory that fits in one run).
        self.io
            .borrow_mut()
            .on(file, false, |vol, cx, f| spin_in(cx, || vol.set_len(f, u64::from(len))))
            .map(|_| ())
    }

    fn sync(&mut self, file: FileName) -> Result<(), Self::Error> {
        // The entry (size), then every dirty sector: data before anything
        // written later.
        self.io.borrow_mut().on(file, false, |vol, cx, f| spin_in(cx, || vol.flush_file(f))).map(|_| ())
    }

    fn remove(&mut self, file: FileName) -> Result<(), Self::Error> {
        let mut io = self.io.borrow_mut();
        let io = &mut *io;
        if let Some(o) = io.h.open.iter_mut().find(|o| o.as_ref().is_some_and(|(n, _)| *n == file))
            && let Some((_, f)) = o.take()
        {
            // The entry must name its clusters before they can be freed.
            spin_in(io.cx, || io.vol.close(f))?;
        }
        if io.kept.is_some_and(|(k, _)| k == file) {
            io.kept = None;
        }
        let alt = matches!(io.mode, Mode::Write { alt: true });
        let mut pb = [0u8; PATH];
        let p = path(file, alt, io.base, &mut pb);
        match spin_in(io.cx, || io.vol.remove(p)) {
            Ok(()) | Err(orpheus_fs::Error::NotFound) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// [`NameSource`] of a [`CardIo`].
pub struct CardNames<'r, 'a, 'v, 'c, D: BlockDevice> {
    io: &'r RefCell<Inner<'a, 'v, 'c, D>>,
    e: &'r mut DirEntry,
}

impl<D: BlockDevice> NameSource for CardNames<'_, '_, '_, '_, D> {
    type Error = FsError<D>;

    fn name_of(&mut self, loc: Locator, name: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        let mut io = self.io.borrow_mut();
        let io = &mut *io;
        match spin_in(io.cx, || io.vol.open_locator(loc, self.e)) {
            Ok(()) => {}
            Err(orpheus_fs::Error::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        }
        let n = self.e.name().as_bytes();
        let k = n.len().min(name.len());
        name[..k].copy_from_slice(&n[..k]);
        Ok(Some(n.len()))
    }
}

/// [`DirSource`] of a [`CardIo`].
pub struct CardDir<'r, 'a, 'v, 'c, D: BlockDevice> {
    io: &'r RefCell<Inner<'a, 'v, 'c, D>>,
    start: DirCursor,
    cur: &'r mut DirCursor,
    e: &'r mut DirEntry,
}

impl<D: BlockDevice> DirSource for CardDir<'_, '_, '_, '_, D> {
    type Error = FsError<D>;

    fn rewind(&mut self) -> Result<(), Self::Error> {
        *self.cur = self.start;
        Ok(())
    }

    fn next_entry(&mut self, name: &mut [u8]) -> Result<Option<SrcEntry>, Self::Error> {
        let mut io = self.io.borrow_mut();
        let io = &mut *io;
        loop {
            if !spin_in(io.cx, || io.vol.read_dir(self.cur, self.e))? {
                return Ok(None);
            }
            if is_dot(self.e) {
                continue;
            }
            let s = src_entry(self.e);
            let n = self.e.name().as_bytes();
            let k = n.len().min(name.len()).min(MAX_NAME);
            name[..k].copy_from_slice(&n[..k]);
            return Ok(Some(s));
        }
    }
}
