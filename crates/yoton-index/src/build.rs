//! Building an index: external merge sort in a borrowed scratch buffer.
//!
//! Phases, each advanced by [`Builder::step`] with bounded work:
//!
//! 1. **Scan**: entries from the [`DirSource`] are packed into the scratch
//!    as items `[record 32 B][name length u16][name]` with a `u16` offset
//!    table growing down from the top. When the next item might not fit,
//!    the offsets are heap-sorted and the items written as one run
//!    `[bytes u32][items u32][items…]` to scratch file A. A directory that
//!    fits in one run goes straight to the index.
//! 2. **Merge**: passes merge up to `k` runs at a time (`k` = how many
//!    input buffers of max(largest item seen, 512 B) fit next to the
//!    output buffers, at most [`MAX_K`]; ≥ 2 for any names in
//!    [`MIN_SCRATCH`]), ping-ponging between scratch files A and B. The pass
//!    that starts with ≤ `k` runs writes the index rows instead (names
//!    dropped) and the letter runs to the free scratch file.
//! 3. **Finish**: the letter runs are appended to the index, the file is
//!    synced, the header is written (it was zero until now), synced, and
//!    the scratch files are removed.
//!
//! Scratch layout: `[0, 512)` output buffer, `[512, 576)` letter-run
//! buffer, `[576, len)` the run being collected (scan) or the `k` input
//! buffers (merge). Output is written in whole, aligned sectors and input
//! refills end on sector boundaries, so `yoton-fs` moves them without
//! read-modify-write. Nothing in the scratch is needed between steps
//! except the run being collected and the buffered input/output, and
//! [`Builder::suspend`] writes or rewinds those, so the scratch can be
//! handed back (e.g. to the decoder) between two build steps.

use core::cmp::Ordering;

use yoton_apps::browse::{EntryKind, is_hidden_name, kind_of, letter_of};

use crate::key::KeyPrefix;
use crate::query::Index;
use crate::record::{
    HEADER, Header, LETTER_RUN, RECORD, Rec, Row, Signature, cmp_full, crc32_update, encode, rd16, rd32,
    wr16, wr32,
};
use crate::{DirSource, Error, FileName, MAX_NAME, Spec, Storage};

const OUT: usize = 576;
const RECBUF: usize = 512;
const SECTOR: usize = 512;
const ITEM_HDR: usize = RECORD + 2;
const MAX_ITEM: usize = ITEM_HDR + MAX_NAME;
const MIN_SLOT: usize = 800;
const RUN_HDR: usize = 8;
const NO_LETTER: u32 = u32::MAX;
/// Most runs merged at once (bounds the builder's own size).
pub const MAX_K: usize = 12;
/// Smallest usable scratch buffer: output buffers plus two inputs.
pub const MIN_SCRATCH: usize = OUT + 2 * MIN_SLOT;
/// Largest part of the scratch used (offsets are 16 bits).
const MAX_AREA: usize = 0xFFFF;

const _: () = assert!(MIN_SLOT >= MAX_ITEM);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Start,
    Scan,
    Merge,
    Finish,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Slot {
    /// Next file byte not yet in the buffer.
    fpos: u32,
    /// End of this run in the file.
    fend: u32,
    /// Unconsumed bytes in the slot's buffer.
    bs: u16,
    be: u16,
    live: bool,
}

/// Where a build is: a plain value (about 300 bytes) that holds no scratch
/// contents. [`Builder::suspend`] returns it and [`Suspended::resume`]
/// continues with any scratch buffer of sufficient size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Suspended {
    dir: u32,
    spec: Spec,
    quick: u64,
    phase: Phase,
    sig: Signature,
    rows: u32,
    dirs: u32,
    // Scan: the run being collected.
    n: u16,
    used: u16,
    /// Largest item seen; sizes the merge inputs.
    max_item: u16,
    /// Runs merged at once (set when the scan ends).
    k: u8,
    // Runs on file.
    inp: u8,
    runs: u32,
    in_len: u32,
    pass: u8,
    next_run: u32,
    runs_left: u32,
    final_pass: bool,
    // Output stream.
    out_pos: u32,
    ob: u16,
    out_runs: u32,
    run_start: u32,
    run_items: u32,
    // Merge group.
    group: bool,
    kg: u8,
    slots: [Slot; MAX_K],
    // Final output.
    written: u32,
    rec_crc: u32,
    lr_letter: u32,
    lr_n: u32,
    lr_pos: u32,
    lrb: u8,
    run_crc: u32,
    copied: u32,
}

/// How far a build is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Progress {
    /// Reading the directory: entries seen so far.
    Scanning { entries: u32 },
    /// Merging: pass number (0 = first) and runs still to merge in it.
    Merging { pass: u8, runs_left: u32 },
    /// Writing the letter runs and the header.
    Finishing,
    /// The index is complete and open.
    Done(Index),
}

/// An index build in progress. Holds the scratch buffer for its lifetime;
/// [`Builder::suspend`] gives it back without losing work.
pub struct Builder<'s> {
    scratch: &'s mut [u8],
    st: Suspended,
}

fn tmp(i: u8) -> FileName {
    if i == 0 { FileName::TEMP_A } else { FileName::TEMP_B }
}

fn usable(scratch: &[u8]) -> usize {
    scratch.len().min(OUT + MAX_AREA)
}

/// Input buffer size needed: the largest item, at least a sector.
fn slot_min(max_item: u16) -> usize {
    usize::from(max_item).max(SECTOR)
}

/// Runs merged at once with a scratch of `len` usable bytes (≥ 2 for any
/// names when `len ≥ MIN_SCRATCH`).
fn k_for(len: usize, max_item: u16) -> usize {
    ((len - OUT) / slot_min(max_item)).min(MAX_K)
}

fn slot_size(len: usize, kg: usize) -> usize {
    ((len - OUT) / kg.max(1)).min(MAX_AREA)
}

/// Item at `off` in `buf`: (record, name).
fn item(buf: &[u8], off: usize) -> (Rec<'_>, &[u8]) {
    let n = usize::from(rd16(buf, off + RECORD));
    (Rec(&buf[off..off + RECORD]), &buf[off + ITEM_HDR..off + ITEM_HDR + n])
}

fn item_len(buf: &[u8], off: usize) -> usize {
    ITEM_HDR + usize::from(rd16(buf, off + RECORD))
}

fn cmp_at(sort: yoton_apps::services::SortBy, items: &[u8], a: usize, b: usize) -> Ordering {
    let (ra, na) = item(items, a);
    let (rb, nb) = item(items, b);
    cmp_full(sort, &ra, na, &rb, nb)
}

/// Heap sort of the `u16` offsets in `slots` by the items they point at.
fn heapsort(slots: &mut [u8], items: &[u8], sort: yoton_apps::services::SortBy) {
    heapsort_by(slots, |a, b| cmp_at(sort, items, a, b) == Ordering::Less);
}

/// Heap sort of the `u16` values in `slots` by `less` (applied to the
/// values).
pub(crate) fn heapsort_by(slots: &mut [u8], mut less: impl FnMut(usize, usize) -> bool) {
    let n = slots.len() / 2;
    let get = |s: &[u8], i: usize| usize::from(rd16(s, 2 * i));
    let swap = |s: &mut [u8], i: usize, j: usize| {
        let (a, b) = (rd16(s, 2 * i), rd16(s, 2 * j));
        wr16(s, 2 * i, b);
        wr16(s, 2 * j, a);
    };
    let mut sift = |s: &mut [u8], mut root: usize, end: usize| {
        loop {
            let mut child = 2 * root + 1;
            if child >= end {
                break;
            }
            if child + 1 < end && less(get(s, child), get(s, child + 1)) {
                child += 1;
            }
            if less(get(s, root), get(s, child)) {
                swap(s, root, child);
                root = child;
            } else {
                break;
            }
        }
    };
    for start in (0..n / 2).rev() {
        sift(slots, start, n);
    }
    for end in (1..n).rev() {
        swap(slots, 0, end);
        sift(slots, 0, end);
    }
}

/// Append `data` to the stream buffered in `buf[..*fill]` that goes to
/// `file` at `*pos`. Full-buffer writes end on a multiple of `buf.len()`
/// in the file (whole sectors for the 512-byte output buffer), also after
/// a partial flush.
pub(crate) fn push<S: Storage>(
    store: &mut S,
    file: FileName,
    buf: &mut [u8],
    fill: &mut usize,
    pos: &mut u32,
    mut data: &[u8],
) -> Result<(), S::Error> {
    while !data.is_empty() {
        let cap = buf.len() - *pos as usize % buf.len();
        let n = (cap - *fill).min(data.len());
        buf[*fill..*fill + n].copy_from_slice(&data[..n]);
        *fill += n;
        data = &data[n..];
        if *fill == cap {
            flush(store, file, buf, fill, pos)?;
        }
    }
    Ok(())
}

pub(crate) fn flush<S: Storage>(
    store: &mut S,
    file: FileName,
    buf: &[u8],
    fill: &mut usize,
    pos: &mut u32,
) -> Result<(), S::Error> {
    if *fill > 0 {
        store.write_at(file, *pos, &buf[..*fill])?;
        *pos += *fill as u32;
        *fill = 0;
    }
    Ok(())
}

impl Suspended {
    /// Continue with `scratch` (any contents). `None` if it is too small
    /// for this build: below [`MIN_SCRATCH`], or, once merging, narrower
    /// than the merge fan-in chosen at the end of the scan needs (`k`
    /// inputs of max(largest item, 512 B), plus 576 B) — keep `self` and
    /// try again with a larger buffer.
    #[must_use]
    pub fn resume<'s>(&self, scratch: &'s mut [u8]) -> Option<Builder<'s>> {
        let len = usable(scratch);
        let merging = self.phase == Phase::Merge;
        if len < MIN_SCRATCH || (merging && slot_size(len, usize::from(self.k)) < slot_min(self.max_item)) {
            return None;
        }
        Some(Builder { scratch, st: *self })
    }

    /// The index file this build writes.
    #[must_use]
    pub fn file(&self) -> FileName {
        FileName::index(self.dir, self.spec)
    }
}

impl<'s> Builder<'s> {
    /// Start building the index of directory `dir` (its identity, e.g. the
    /// first cluster) for `spec`. `quick` is stored for [`Index::quick`].
    /// `None` if `scratch` is smaller than [`MIN_SCRATCH`].
    pub fn new(scratch: &'s mut [u8], dir: u32, spec: Spec, quick: u64) -> Option<Builder<'s>> {
        if usable(scratch) < MIN_SCRATCH {
            return None;
        }
        let st = Suspended {
            dir,
            spec,
            quick,
            phase: Phase::Start,
            sig: Signature::default(),
            rows: 0,
            dirs: 0,
            n: 0,
            used: 0,
            max_item: 0,
            k: 0,
            inp: 0,
            runs: 0,
            in_len: 0,
            pass: 0,
            next_run: 0,
            runs_left: 0,
            final_pass: false,
            out_pos: 0,
            ob: 0,
            out_runs: 0,
            run_start: 0,
            run_items: 0,
            group: false,
            kg: 0,
            slots: [Slot::default(); MAX_K],
            written: 0,
            rec_crc: 0,
            lr_letter: NO_LETTER,
            lr_n: 0,
            lr_pos: 0,
            lrb: 0,
            run_crc: 0,
            copied: 0,
        };
        Some(Builder { scratch, st })
    }

    /// The index file this build writes.
    #[must_use]
    pub fn file(&self) -> FileName {
        self.st.file()
    }

    /// Do at most about `budget` units of work: directory entries read
    /// while scanning, rows merged while merging, letter-run bytes / 32
    /// while finishing. One unit is about 32–800 bytes of card I/O; a step
    /// may also sort and write one scratch-full run (≤ the scratch size).
    pub fn step<S, D, E>(&mut self, store: &mut S, src: &mut D, budget: u32) -> Result<Progress, Error<E>>
    where
        S: Storage<Error = E>,
        D: DirSource<Error = E>,
    {
        let budget = budget.max(1);
        match self.st.phase {
            Phase::Start => {
                let file = self.file();
                store.truncate(file, 0)?;
                self.scratch[..HEADER].fill(0);
                store.write_at(file, 0, &self.scratch[..HEADER])?;
                store.truncate(FileName::TEMP_A, 0)?;
                store.truncate(FileName::TEMP_B, 0)?;
                src.rewind()?;
                self.st.phase = Phase::Scan;
                Ok(Progress::Scanning { entries: 0 })
            }
            Phase::Scan => self.scan(store, src, budget),
            Phase::Merge => self.merge(store, budget),
            Phase::Finish => self.finish(store, budget),
            Phase::Done => Ok(Progress::Done(self.index())),
        }
    }

    /// Stop here and give the scratch back. The run being collected is
    /// written out as a (short) run, buffered output is flushed and inputs
    /// rewound to their next item; nothing is lost.
    pub fn suspend<S: Storage>(mut self, store: &mut S) -> Result<Suspended, Error<S::Error>> {
        match self.st.phase {
            Phase::Scan if self.st.n > 0 => self.flush_run(store)?,
            Phase::Merge => {
                self.flush_out(store)?;
                self.flush_letters(store)?;
                for s in &mut self.st.slots[..usize::from(self.st.kg)] {
                    if s.live {
                        s.fpos -= u32::from(s.be - s.bs);
                        s.bs = 0;
                        s.be = 0;
                    }
                }
            }
            _ => {}
        }
        Ok(self.st)
    }

    fn index(&self) -> Index {
        Index::from_header(self.file(), &self.header())
    }

    fn header(&self) -> Header {
        Header {
            dir: self.st.dir,
            spec: self.st.spec,
            rows: self.st.rows,
            dirs: self.st.dirs,
            letter_runs: self.st.lr_n,
            sig: self.st.sig,
            quick: self.st.quick,
            rec_crc: self.st.rec_crc,
            run_crc: self.st.run_crc,
        }
    }

    // ------------------------------------------------------------ scan

    fn scan<S, D, E>(&mut self, store: &mut S, src: &mut D, budget: u32) -> Result<Progress, Error<E>>
    where
        S: Storage<Error = E>,
        D: DirSource<Error = E>,
    {
        let len = usable(self.scratch);
        let area_len = len - OUT;
        for _ in 0..budget {
            let free = area_len - usize::from(self.st.used) - 2 * usize::from(self.st.n);
            if free < MAX_ITEM + 2 {
                self.flush_run(store)?;
            }
            let at = OUT + usize::from(self.st.used);
            let Some(e) = src.next_entry(&mut self.scratch[at + ITEM_HDR..at + MAX_ITEM])? else {
                self.end_scan(store)?;
                return Ok(if self.st.phase == Phase::Finish {
                    Progress::Finishing
                } else {
                    Progress::Merging { pass: 0, runs_left: self.st.runs }
                });
            };
            if e.name_len > MAX_NAME {
                return Err(Error::BadName);
            }
            let Ok(name) = core::str::from_utf8(&self.scratch[at + ITEM_HDR..at + ITEM_HDR + e.name_len])
            else {
                return Err(Error::BadName);
            };
            self.st.sig.add(&e, name);
            if !self.st.spec.keeps(name, &e) {
                continue;
            }
            let row = Row {
                loc: e.loc,
                size: e.size,
                mtime: e.mtime,
                letter: letter_of(name),
                kind: kind_of(name, e.is_dir),
                hidden: e.hidden || is_hidden_name(name),
            };
            let key = KeyPrefix::of(name);
            let mut rec = [0u8; RECORD];
            encode(&row, &key, &mut rec);
            self.scratch[at..at + RECORD].copy_from_slice(&rec);
            wr16(self.scratch, at + RECORD, e.name_len as u16);
            let n = usize::from(self.st.n) + 1;
            wr16(self.scratch, len - 2 * n, self.st.used);
            self.st.n += 1;
            self.st.used += (ITEM_HDR + e.name_len) as u16;
            self.st.max_item = self.st.max_item.max((ITEM_HDR + e.name_len) as u16);
            self.st.rows += 1;
            if row.kind == EntryKind::Dir {
                self.st.dirs += 1;
            }
        }
        Ok(Progress::Scanning { entries: self.st.sig.entries })
    }

    /// Sort the offsets of the collected items.
    fn sort_collected(&mut self) {
        let len = usable(self.scratch);
        let n = usize::from(self.st.n);
        let (lo, hi) = self.scratch[..len].split_at_mut(len - 2 * n);
        heapsort(hi, &lo[OUT..OUT + usize::from(self.st.used)], self.st.spec.sort);
    }

    /// Write the collected items as one sorted run to the scan file.
    fn flush_run<S: Storage>(&mut self, store: &mut S) -> Result<(), S::Error> {
        let file = tmp(self.st.inp);
        let mut hdr = [0u8; RUN_HDR];
        wr32(&mut hdr, 0, u32::from(self.st.used));
        wr32(&mut hdr, 4, u32::from(self.st.n));
        let mut pos = self.st.in_len;
        let mut fill = 0;
        let len = usable(self.scratch);
        let n = usize::from(self.st.n);
        self.sort_collected();
        let (lo, hi) = self.scratch[..len].split_at_mut(len - 2 * n);
        let (out, rest) = lo.split_at_mut(OUT);
        let items = &rest[..usize::from(self.st.used)];
        let out = &mut out[..RECBUF];
        push(store, file, out, &mut fill, &mut pos, &hdr)?;
        for p in 0..n {
            let off = usize::from(rd16(hi, 2 * p));
            let l = item_len(items, off);
            push(store, file, out, &mut fill, &mut pos, &items[off..off + l])?;
        }
        flush(store, file, out, &mut fill, &mut pos)?;
        self.st.in_len = pos;
        self.st.runs += 1;
        self.st.n = 0;
        self.st.used = 0;
        Ok(())
    }

    fn end_scan<S: Storage>(&mut self, store: &mut S) -> Result<(), Error<S::Error>> {
        if self.st.runs == 0 {
            // Everything is in the scratch: sort and write the rows.
            self.begin_final();
            let n = usize::from(self.st.n);
            self.sort_collected();
            for p in 0..n {
                let len = usable(self.scratch);
                let hi = len - 2 * n;
                let off = OUT + usize::from(rd16(self.scratch, hi + 2 * p));
                let mut rec = [0u8; RECORD];
                rec.copy_from_slice(&self.scratch[off..off + RECORD]);
                self.emit_row(store, &rec)?;
            }
            self.st.n = 0;
            self.st.used = 0;
            self.end_final(store)?;
            return Ok(());
        }
        if self.st.n > 0 {
            self.flush_run(store)?;
        }
        self.st.k = k_for(usable(self.scratch), self.st.max_item) as u8;
        self.st.phase = Phase::Merge;
        self.begin_pass(store)?;
        Ok(())
    }

    // ----------------------------------------------------------- merge

    fn begin_pass<S: Storage>(&mut self, store: &mut S) -> Result<(), S::Error> {
        let k = usize::from(self.st.k);
        self.st.runs_left = self.st.runs;
        self.st.next_run = 0;
        self.st.out_runs = 0;
        self.st.group = false;
        if self.st.runs as usize <= k {
            self.begin_final();
            store.truncate(self.letters_file(), 0)?;
        } else {
            self.st.final_pass = false;
            self.st.out_pos = 0;
            self.st.ob = 0;
            store.truncate(tmp(1 - self.st.inp), 0)?;
        }
        Ok(())
    }

    fn begin_final(&mut self) {
        self.st.final_pass = true;
        self.st.out_pos = HEADER as u32;
        self.st.ob = 0;
        self.st.written = 0;
        self.st.rec_crc = 0;
        self.st.lr_letter = NO_LETTER;
        self.st.lr_n = 0;
        self.st.lr_pos = 0;
        self.st.lrb = 0;
        self.st.run_crc = 0;
    }

    fn out_file(&self) -> FileName {
        if self.st.final_pass { self.file() } else { tmp(1 - self.st.inp) }
    }

    fn out_push<S: Storage>(&mut self, store: &mut S, data: &[u8]) -> Result<(), S::Error> {
        let file = self.out_file();
        let mut fill = usize::from(self.st.ob);
        let r = push(store, file, &mut self.scratch[..RECBUF], &mut fill, &mut self.st.out_pos, data);
        self.st.ob = fill as u16;
        r
    }

    fn flush_out<S: Storage>(&mut self, store: &mut S) -> Result<(), S::Error> {
        let file = self.out_file();
        let mut fill = usize::from(self.st.ob);
        let r = flush(store, file, &self.scratch[..RECBUF], &mut fill, &mut self.st.out_pos);
        self.st.ob = fill as u16;
        r
    }

    /// The letter runs go to the scratch file the final pass does not read.
    fn letters_file(&self) -> FileName {
        tmp(1 - self.st.inp)
    }

    fn flush_letters<S: Storage>(&mut self, store: &mut S) -> Result<(), S::Error> {
        let file = self.letters_file();
        let mut fill = usize::from(self.st.lrb);
        let r = flush(store, file, &self.scratch[RECBUF..OUT], &mut fill, &mut self.st.lr_pos);
        self.st.lrb = fill as u8;
        r
    }

    /// One row of the final order.
    fn emit_row<S: Storage>(&mut self, store: &mut S, rec: &[u8; RECORD]) -> Result<(), S::Error> {
        self.out_push(store, rec)?;
        self.st.rec_crc = crc32_update(self.st.rec_crc, rec);
        let letter = Rec(rec).letter_code();
        if letter != self.st.lr_letter {
            let mut e = [0u8; LETTER_RUN];
            wr32(&mut e, 0, self.st.written);
            wr32(&mut e, 4, letter);
            self.st.run_crc = crc32_update(self.st.run_crc, &e);
            let file = self.letters_file();
            let mut fill = usize::from(self.st.lrb);
            let r = push(store, file, &mut self.scratch[RECBUF..OUT], &mut fill, &mut self.st.lr_pos, &e);
            self.st.lrb = fill as u8;
            r?;
            self.st.lr_letter = letter;
            self.st.lr_n += 1;
        }
        self.st.written += 1;
        Ok(())
    }

    fn end_final<S: Storage>(&mut self, store: &mut S) -> Result<(), Error<S::Error>> {
        self.flush_out(store)?;
        self.flush_letters(store)?;
        if self.st.written != self.st.rows {
            return Err(Error::Corrupt);
        }
        self.st.copied = 0;
        self.st.phase = Phase::Finish;
        Ok(())
    }

    fn start_group<S: Storage>(&mut self, store: &mut S) -> Result<(), Error<S::Error>> {
        let kg = u32::from(self.st.k).min(self.st.runs_left) as usize;
        let file = tmp(self.st.inp);
        for i in 0..kg {
            let mut hdr = [0u8; RUN_HDR];
            if store.read_at(file, self.st.next_run, &mut hdr)? != RUN_HDR {
                return Err(Error::Corrupt);
            }
            let start = self.st.next_run + RUN_HDR as u32;
            let end = start.checked_add(rd32(&hdr, 0)).ok_or(Error::Corrupt)?;
            self.st.slots[i] = Slot { fpos: start, fend: end, bs: 0, be: 0, live: true };
            self.st.next_run = end;
        }
        self.st.kg = kg as u8;
        self.st.runs_left -= kg as u32;
        if !self.st.final_pass {
            self.st.run_start = self.st.out_pos + u32::from(self.st.ob);
            self.st.run_items = 0;
            self.out_push(store, &[0u8; RUN_HDR])?;
        }
        self.st.group = true;
        Ok(())
    }

    /// Make sure slot `i` holds its whole head item (or is exhausted).
    fn load_head<S: Storage>(&mut self, store: &mut S, i: usize) -> Result<(), Error<S::Error>> {
        let len = usable(self.scratch);
        let b = slot_size(len, usize::from(self.st.kg));
        let file = tmp(self.st.inp);
        let region = &mut self.scratch[OUT + i * b..OUT + (i + 1) * b];
        let s = &mut self.st.slots[i];
        loop {
            let avail = usize::from(s.be - s.bs);
            if avail >= ITEM_HDR {
                let nl = usize::from(rd16(region, usize::from(s.bs) + RECORD));
                if nl > MAX_NAME {
                    return Err(Error::Corrupt);
                }
                if avail >= ITEM_HDR + nl {
                    return Ok(());
                }
            }
            if s.fpos >= s.fend {
                if avail == 0 {
                    s.live = false;
                    return Ok(());
                }
                return Err(Error::Corrupt);
            }
            region.copy_within(usize::from(s.bs)..usize::from(s.be), 0);
            let mut want = (b - avail).min((s.fend - s.fpos) as usize);
            if want > SECTOR {
                // End on a sector boundary so that later reads are whole
                // sectors.
                want -= (s.fpos as usize + want) % SECTOR;
            }
            if store.read_at(file, s.fpos, &mut region[avail..avail + want])? != want {
                return Err(Error::Corrupt);
            }
            s.fpos += want as u32;
            s.bs = 0;
            s.be = (avail + want) as u16;
        }
    }

    fn merge<S: Storage>(&mut self, store: &mut S, budget: u32) -> Result<Progress, Error<S::Error>> {
        let mut left = budget;
        while left > 0 {
            if !self.st.group {
                if self.st.runs_left == 0 {
                    self.end_pass(store)?;
                    if self.st.phase != Phase::Merge {
                        return Ok(Progress::Finishing);
                    }
                    continue;
                }
                self.start_group(store)?;
            }
            let kg = usize::from(self.st.kg);
            for i in 0..kg {
                if self.st.slots[i].live {
                    self.load_head(store, i)?;
                }
            }
            let len = usable(self.scratch);
            let b = slot_size(len, kg);
            let ins = &self.scratch[OUT..];
            let mut best: Option<usize> = None;
            for i in 0..kg {
                let s = self.st.slots[i];
                if !s.live {
                    continue;
                }
                best = Some(match best {
                    None => i,
                    Some(j) => {
                        let (ri, ni) = item(ins, i * b + usize::from(s.bs));
                        let (rj, nj) = item(ins, j * b + usize::from(self.st.slots[j].bs));
                        if cmp_full(self.st.spec.sort, &ri, ni, &rj, nj) == Ordering::Less { i } else { j }
                    }
                });
            }
            let Some(i) = best else {
                self.end_group(store)?;
                continue;
            };
            let at = OUT + i * b + usize::from(self.st.slots[i].bs);
            let l = item_len(self.scratch, at);
            self.st.slots[i].bs += l as u16;
            if self.st.final_pass {
                let mut rec = [0u8; RECORD];
                rec.copy_from_slice(&self.scratch[at..at + RECORD]);
                self.emit_row(store, &rec)?;
            } else {
                let file = self.out_file();
                let mut fill = usize::from(self.st.ob);
                let (out, ins) = self.scratch.split_at_mut(OUT);
                let item = &ins[at - OUT..at - OUT + l];
                let r = push(store, file, &mut out[..RECBUF], &mut fill, &mut self.st.out_pos, item);
                self.st.ob = fill as u16;
                r?;
                self.st.run_items += 1;
            }
            left -= 1;
        }
        Ok(Progress::Merging { pass: self.st.pass, runs_left: self.st.runs_left })
    }

    fn end_group<S: Storage>(&mut self, store: &mut S) -> Result<(), Error<S::Error>> {
        self.st.group = false;
        if self.st.final_pass {
            return Ok(());
        }
        self.flush_out(store)?;
        let mut hdr = [0u8; RUN_HDR];
        wr32(&mut hdr, 0, self.st.out_pos - self.st.run_start - RUN_HDR as u32);
        wr32(&mut hdr, 4, self.st.run_items);
        store.write_at(tmp(1 - self.st.inp), self.st.run_start, &hdr)?;
        self.st.out_runs += 1;
        Ok(())
    }

    fn end_pass<S: Storage>(&mut self, store: &mut S) -> Result<(), Error<S::Error>> {
        if self.st.final_pass {
            return self.end_final(store);
        }
        store.truncate(tmp(self.st.inp), 0)?;
        self.st.inp = 1 - self.st.inp;
        self.st.runs = self.st.out_runs;
        self.st.pass += 1;
        self.begin_pass(store)?;
        Ok(())
    }

    // ---------------------------------------------------------- finish

    fn finish<S: Storage>(&mut self, store: &mut S, budget: u32) -> Result<Progress, Error<S::Error>> {
        let file = self.file();
        let total = self.st.lr_n * LETTER_RUN as u32;
        let base = HEADER as u32 + self.st.rows * RECORD as u32;
        let chunk_max =
            (budget as usize).saturating_mul(RECORD).clamp(512, usable(self.scratch) - OUT) as u32;
        if self.st.copied < total {
            let n = (total - self.st.copied).min(chunk_max) as usize;
            let letters = self.letters_file();
            let buf = &mut self.scratch[OUT..OUT + n];
            if store.read_at(letters, self.st.copied, buf)? != n {
                return Err(Error::Corrupt);
            }
            store.write_at(file, base + self.st.copied, buf)?;
            self.st.copied += n as u32;
            if self.st.copied < total {
                return Ok(Progress::Finishing);
            }
        }
        store.sync(file)?;
        let header = self.header();
        let hb = &mut self.scratch[..HEADER];
        hb.fill(0);
        header.encode(hb);
        store.write_at(file, 0, hb)?;
        store.sync(file)?;
        store.remove(FileName::TEMP_A)?;
        store.remove(FileName::TEMP_B)?;
        self.st.phase = Phase::Done;
        Ok(Progress::Done(self.index()))
    }
}

/// Recomputes a directory's [`Signature`] in bounded steps (compare it
/// with [`Index::signature`] to decide whether an index is current).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Verifier {
    sig: Signature,
    started: bool,
    done: bool,
}

impl Verifier {
    #[must_use]
    pub fn new() -> Verifier {
        Verifier::default()
    }

    /// Read up to `budget` entries; `Some` with the signature once the
    /// directory is exhausted. `name` must hold [`MAX_NAME`] bytes.
    pub fn step<D: DirSource>(
        &mut self,
        src: &mut D,
        name: &mut [u8],
        budget: u32,
    ) -> Result<Option<Signature>, Error<D::Error>> {
        if self.done {
            return Ok(Some(self.sig));
        }
        if !self.started {
            src.rewind()?;
            self.started = true;
        }
        for _ in 0..budget.max(1) {
            let Some(e) = src.next_entry(name)? else {
                self.done = true;
                return Ok(Some(self.sig));
            };
            let s =
                name.get(..e.name_len).and_then(|b| core::str::from_utf8(b).ok()).ok_or(Error::BadName)?;
            self.sig.add(&e, s);
        }
        Ok(None)
    }
}
