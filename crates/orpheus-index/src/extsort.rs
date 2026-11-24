//! A generic external merge sort of variable-length byte items with a
//! caller-supplied order, in a borrowed scratch buffer — the machinery of
//! [`crate::Builder`] without the directory-index specifics (used by the
//! music library, AU-12).
//!
//! Life cycle: [`ExtSort::reset`], then [`ExtSort::push`] for every item,
//! [`ExtSort::seal`], [`ExtSort::merge`] until it answers `true`, then
//! [`ExtSort::next`] until it answers `None`. Every call takes the store,
//! the scratch buffer and the comparison; the sorter itself is a plain
//! value (about 260 bytes) that keeps nothing in the scratch across
//! [`ExtSort::suspend`], so the scratch can be lent out between two calls.
//! Between suspensions the caller passes the same scratch length.
//!
//! Scratch layout: `[0, 512)` output buffer of the merge passes, `[512,
//! len)` either the items being collected (`[length u16][bytes]`, with a
//! `u16` offset table growing down from the top) or `k` merge input
//! buffers. A run on file is `[bytes u32][items u32][items…]`. When all
//! items fit in the scratch nothing is written: they are sorted in place
//! and handed out from memory.

use core::cmp::Ordering;

use crate::build::{flush, heapsort_by, push};
use crate::record::{rd16, rd32, wr16, wr32};
use crate::{Error, FileName, Storage};

const OUT: usize = 512;
const SECTOR: usize = 512;
const RUN_HDR: usize = 8;
const ITEM_HDR: usize = 2;
const MAX_AREA: usize = 0xFFFF;
/// Most runs merged at once.
pub const SORT_MAX_K: usize = 12;
/// Largest item (bytes, without its length prefix).
pub const MAX_SORT_ITEM: usize = 1024;
/// Smallest scratch: the output buffer and two inputs of the largest item.
pub const MIN_SORT_SCRATCH: usize = OUT + 2 * (ITEM_HDR + MAX_SORT_ITEM) + 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Collect,
    Merge,
    Drain,
    MemDrain,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Slot {
    fpos: u32,
    fend: u32,
    bs: u16,
    be: u16,
    live: bool,
}

/// An external sort in progress (see the module documentation).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExtSort {
    files: [FileName; 2],
    phase: Phase,
    items: u32,
    // Collect.
    n: u16,
    used: u16,
    max_item: u16,
    mem_next: u16,
    // Runs on file.
    k: u8,
    inp: u8,
    runs: u32,
    in_len: u32,
    pass: u8,
    next_run: u32,
    runs_left: u32,
    // Intermediate output.
    out_pos: u32,
    ob: u16,
    out_runs: u32,
    run_start: u32,
    run_items: u32,
    group: bool,
    kg: u8,
    slots: [Slot; SORT_MAX_K],
}

fn usable(scratch: &[u8]) -> usize {
    scratch.len().min(OUT + MAX_AREA)
}

fn slot_min(max_item: u16) -> usize {
    usize::from(max_item).max(SECTOR)
}

fn slot_size(len: usize, kg: usize) -> usize {
    ((len - OUT) / kg.max(1)).min(MAX_AREA)
}

fn item_at(buf: &[u8], off: usize) -> &[u8] {
    let n = usize::from(rd16(buf, off));
    &buf[off + ITEM_HDR..off + ITEM_HDR + n]
}

impl ExtSort {
    /// A sorter that keeps its runs in the two scratch files `a` and `b`.
    #[must_use]
    pub const fn new(a: FileName, b: FileName) -> ExtSort {
        ExtSort {
            files: [a, b],
            phase: Phase::Collect,
            items: 0,
            n: 0,
            used: 0,
            max_item: 0,
            mem_next: 0,
            k: 0,
            inp: 0,
            runs: 0,
            in_len: 0,
            pass: 0,
            next_run: 0,
            runs_left: 0,
            out_pos: 0,
            ob: 0,
            out_runs: 0,
            run_start: 0,
            run_items: 0,
            group: false,
            kg: 0,
            slots: [Slot { fpos: 0, fend: 0, bs: 0, be: 0, live: false }; SORT_MAX_K],
        }
    }

    /// Start a new sort (empties the scratch files).
    pub fn reset<S: Storage>(&mut self, store: &mut S) -> Result<(), S::Error> {
        *self = ExtSort::new(self.files[0], self.files[1]);
        store.truncate(self.files[0], 0)?;
        store.truncate(self.files[1], 0)
    }

    /// Delete the scratch files (after the last item, or to abandon).
    pub fn remove<S: Storage>(&self, store: &mut S) -> Result<(), S::Error> {
        store.remove(self.files[0])?;
        store.remove(self.files[1])
    }

    /// Items pushed so far.
    #[must_use]
    pub fn items(&self) -> u32 {
        self.items
    }

    /// Merge pass (0 = first) and runs still to merge in it.
    #[must_use]
    pub fn progress(&self) -> (u8, u32) {
        (self.pass, self.runs_left)
    }

    /// All items have been handed out by [`ExtSort::next`].
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    fn tmp(&self, i: u8) -> FileName {
        self.files[usize::from(i & 1)]
    }

    /// Add one item (at most [`MAX_SORT_ITEM`] bytes). May sort and write
    /// a scratch-full run.
    pub fn push<S, C>(
        &mut self,
        store: &mut S,
        scratch: &mut [u8],
        item: &[u8],
        cmp: &C,
    ) -> Result<(), Error<S::Error>>
    where
        S: Storage,
        C: Fn(&[u8], &[u8]) -> Ordering,
    {
        if self.phase != Phase::Collect || item.len() > MAX_SORT_ITEM {
            return Err(Error::Corrupt);
        }
        let len = usable(scratch);
        if len < MIN_SORT_SCRATCH {
            return Err(Error::ScratchTooSmall);
        }
        let need = ITEM_HDR + item.len();
        let free = len - OUT - usize::from(self.used) - 2 * usize::from(self.n);
        if free < need + 2 {
            self.flush_run(store, scratch, cmp)?;
        }
        let at = OUT + usize::from(self.used);
        wr16(scratch, at, item.len() as u16);
        scratch[at + ITEM_HDR..at + need].copy_from_slice(item);
        let n = usize::from(self.n) + 1;
        wr16(scratch, len - 2 * n, self.used);
        self.n += 1;
        self.used += need as u16;
        self.max_item = self.max_item.max(need as u16);
        self.items += 1;
        Ok(())
    }

    fn sort_collected<C: Fn(&[u8], &[u8]) -> Ordering>(&self, scratch: &mut [u8], cmp: &C) {
        let len = usable(scratch);
        let n = usize::from(self.n);
        let (lo, hi) = scratch[..len].split_at_mut(len - 2 * n);
        let items = &lo[OUT..OUT + usize::from(self.used)];
        heapsort_by(hi, |a, b| cmp(item_at(items, a), item_at(items, b)) == Ordering::Less);
    }

    /// Write collected items `from..n` (sorted) as one run.
    fn write_run<S: Storage>(
        &mut self,
        store: &mut S,
        scratch: &mut [u8],
        from: usize,
    ) -> Result<(), S::Error> {
        let file = self.tmp(self.inp);
        let len = usable(scratch);
        let n = usize::from(self.n);
        let (lo, hi) = scratch[..len].split_at_mut(len - 2 * n);
        let (out, rest) = lo.split_at_mut(OUT);
        let items = &rest[..usize::from(self.used)];
        let mut bytes = 0u32;
        for p in from..n {
            let off = usize::from(rd16(hi, 2 * p));
            bytes += (ITEM_HDR + usize::from(rd16(items, off))) as u32;
        }
        let mut hdr = [0u8; RUN_HDR];
        wr32(&mut hdr, 0, bytes);
        wr32(&mut hdr, 4, (n - from) as u32);
        let mut pos = self.in_len;
        let mut fill = 0;
        push(store, file, out, &mut fill, &mut pos, &hdr)?;
        for p in from..n {
            let off = usize::from(rd16(hi, 2 * p));
            let l = ITEM_HDR + usize::from(rd16(items, off));
            push(store, file, out, &mut fill, &mut pos, &items[off..off + l])?;
        }
        flush(store, file, out, &mut fill, &mut pos)?;
        self.in_len = pos;
        self.runs += 1;
        self.n = 0;
        self.used = 0;
        Ok(())
    }

    fn flush_run<S, C>(&mut self, store: &mut S, scratch: &mut [u8], cmp: &C) -> Result<(), S::Error>
    where
        S: Storage,
        C: Fn(&[u8], &[u8]) -> Ordering,
    {
        if self.n == 0 {
            return Ok(());
        }
        self.sort_collected(scratch, cmp);
        self.write_run(store, scratch, 0)
    }

    /// No more items. Small inputs are sorted in memory; otherwise the last
    /// run is written and the merge planned.
    pub fn seal<S, C>(&mut self, store: &mut S, scratch: &mut [u8], cmp: &C) -> Result<(), Error<S::Error>>
    where
        S: Storage,
        C: Fn(&[u8], &[u8]) -> Ordering,
    {
        if self.phase != Phase::Collect {
            return Err(Error::Corrupt);
        }
        if self.runs == 0 {
            self.sort_collected(scratch, cmp);
            self.mem_next = 0;
            self.phase = Phase::MemDrain;
            return Ok(());
        }
        self.flush_run(store, scratch, cmp)?;
        let len = usable(scratch);
        self.k = ((len - OUT) / slot_min(self.max_item)).clamp(1, SORT_MAX_K) as u8;
        self.phase = Phase::Merge;
        self.begin_pass(store)?;
        Ok(())
    }

    fn begin_pass<S: Storage>(&mut self, store: &mut S) -> Result<(), S::Error> {
        self.runs_left = self.runs;
        self.next_run = 0;
        self.out_runs = 0;
        self.group = false;
        self.out_pos = 0;
        self.ob = 0;
        if self.runs > u32::from(self.k) {
            store.truncate(self.tmp(1 - self.inp), 0)?;
        }
        Ok(())
    }

    fn start_group<S: Storage>(&mut self, store: &mut S, final_pass: bool) -> Result<(), Error<S::Error>> {
        let kg = u32::from(self.k).min(self.runs_left) as usize;
        let file = self.tmp(self.inp);
        for i in 0..kg {
            let mut hdr = [0u8; RUN_HDR];
            if store.read_at(file, self.next_run, &mut hdr)? != RUN_HDR {
                return Err(Error::Corrupt);
            }
            let start = self.next_run + RUN_HDR as u32;
            let end = start.checked_add(rd32(&hdr, 0)).ok_or(Error::Corrupt)?;
            self.slots[i] = Slot { fpos: start, fend: end, bs: 0, be: 0, live: true };
            self.next_run = end;
        }
        self.kg = kg as u8;
        self.runs_left -= kg as u32;
        self.group = !final_pass;
        Ok(())
    }

    /// Make sure slot `i` holds its whole head item (or is exhausted).
    fn load_head<S: Storage>(
        &mut self,
        store: &mut S,
        scratch: &mut [u8],
        i: usize,
    ) -> Result<(), Error<S::Error>> {
        let len = usable(scratch);
        let b = slot_size(len, usize::from(self.kg));
        let file = self.tmp(self.inp);
        let region = &mut scratch[OUT + i * b..OUT + (i + 1) * b];
        let s = &mut self.slots[i];
        loop {
            let avail = usize::from(s.be - s.bs);
            if avail >= ITEM_HDR {
                let nl = usize::from(rd16(region, usize::from(s.bs)));
                if nl > MAX_SORT_ITEM {
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

    /// The slot whose head item is smallest, after loading every head.
    fn pick<S, C>(
        &mut self,
        store: &mut S,
        scratch: &mut [u8],
        cmp: &C,
    ) -> Result<Option<usize>, Error<S::Error>>
    where
        S: Storage,
        C: Fn(&[u8], &[u8]) -> Ordering,
    {
        let kg = usize::from(self.kg);
        for i in 0..kg {
            if self.slots[i].live {
                self.load_head(store, scratch, i)?;
            }
        }
        let b = slot_size(usable(scratch), kg);
        let ins = &scratch[OUT..];
        let mut best: Option<usize> = None;
        for i in 0..kg {
            let s = self.slots[i];
            if !s.live {
                continue;
            }
            best = Some(match best {
                None => i,
                Some(j) => {
                    let a = item_at(ins, i * b + usize::from(s.bs));
                    let c = item_at(ins, j * b + usize::from(self.slots[j].bs));
                    if cmp(a, c) == Ordering::Less { i } else { j }
                }
            });
        }
        Ok(best)
    }

    /// Run the intermediate merge passes, about `budget` items per call.
    /// `true` once the items can be read with [`ExtSort::next`].
    pub fn merge<S, C>(
        &mut self,
        store: &mut S,
        scratch: &mut [u8],
        cmp: &C,
        budget: u32,
    ) -> Result<bool, Error<S::Error>>
    where
        S: Storage,
        C: Fn(&[u8], &[u8]) -> Ordering,
    {
        let mut left = budget.max(1);
        loop {
            match self.phase {
                Phase::Collect => return Err(Error::Corrupt),
                Phase::Drain | Phase::MemDrain | Phase::Done => return Ok(true),
                Phase::Merge => {}
            }
            if self.runs <= u32::from(self.k) {
                // The final pass is read by `next`.
                self.start_group(store, true)?;
                self.phase = Phase::Drain;
                return Ok(true);
            }
            if left == 0 {
                return Ok(false);
            }
            if !self.group {
                if self.runs_left == 0 {
                    self.end_pass(store)?;
                    continue;
                }
                self.start_group(store, false)?;
                self.run_start = self.out_pos + u32::from(self.ob);
                self.run_items = 0;
                self.out_write(store, scratch, &[0u8; RUN_HDR])?;
            }
            let Some(i) = self.pick(store, scratch, cmp)? else {
                self.end_group(store, scratch)?;
                continue;
            };
            let b = slot_size(usable(scratch), usize::from(self.kg));
            let at = OUT + i * b + usize::from(self.slots[i].bs);
            let l = ITEM_HDR + usize::from(rd16(scratch, at));
            self.slots[i].bs += l as u16;
            let file = self.tmp(1 - self.inp);
            let mut fill = usize::from(self.ob);
            let (out, ins) = scratch.split_at_mut(OUT);
            let r = push(store, file, out, &mut fill, &mut self.out_pos, &ins[at - OUT..at - OUT + l]);
            self.ob = fill as u16;
            r?;
            self.run_items += 1;
            left -= 1;
        }
    }

    fn out_write<S: Storage>(
        &mut self,
        store: &mut S,
        scratch: &mut [u8],
        data: &[u8],
    ) -> Result<(), S::Error> {
        let file = self.tmp(1 - self.inp);
        let mut fill = usize::from(self.ob);
        let r = push(store, file, &mut scratch[..OUT], &mut fill, &mut self.out_pos, data);
        self.ob = fill as u16;
        r
    }

    fn flush_out<S: Storage>(&mut self, store: &mut S, scratch: &mut [u8]) -> Result<(), S::Error> {
        let file = self.tmp(1 - self.inp);
        let mut fill = usize::from(self.ob);
        let r = flush(store, file, &scratch[..OUT], &mut fill, &mut self.out_pos);
        self.ob = fill as u16;
        r
    }

    fn end_group<S: Storage>(&mut self, store: &mut S, scratch: &mut [u8]) -> Result<(), S::Error> {
        self.group = false;
        self.flush_out(store, scratch)?;
        let mut hdr = [0u8; RUN_HDR];
        wr32(&mut hdr, 0, self.out_pos - self.run_start - RUN_HDR as u32);
        wr32(&mut hdr, 4, self.run_items);
        store.write_at(self.tmp(1 - self.inp), self.run_start, &hdr)?;
        self.out_runs += 1;
        Ok(())
    }

    fn end_pass<S: Storage>(&mut self, store: &mut S) -> Result<(), S::Error> {
        store.truncate(self.tmp(self.inp), 0)?;
        self.inp = 1 - self.inp;
        self.runs = self.out_runs;
        self.pass += 1;
        self.begin_pass(store)
    }

    /// The next item in order, `None` after the last. The slice lives in
    /// the scratch until the next call.
    pub fn next<'a, S, C>(
        &mut self,
        store: &mut S,
        scratch: &'a mut [u8],
        cmp: &C,
    ) -> Result<Option<&'a [u8]>, Error<S::Error>>
    where
        S: Storage,
        C: Fn(&[u8], &[u8]) -> Ordering,
    {
        match self.phase {
            Phase::Done => Ok(None),
            Phase::Collect | Phase::Merge => Err(Error::Corrupt),
            Phase::MemDrain => {
                let n = usize::from(self.n);
                let p = usize::from(self.mem_next);
                if p >= n {
                    self.phase = Phase::Done;
                    self.n = 0;
                    self.used = 0;
                    return Ok(None);
                }
                self.mem_next += 1;
                let len = usable(scratch);
                let off = usize::from(rd16(scratch, len - 2 * n + 2 * p));
                Ok(Some(item_at(&scratch[OUT..], off)))
            }
            Phase::Drain => {
                let Some(i) = self.pick(store, scratch, cmp)? else {
                    self.phase = Phase::Done;
                    return Ok(None);
                };
                let b = slot_size(usable(scratch), usize::from(self.kg));
                let at = OUT + i * b + usize::from(self.slots[i].bs);
                let l = ITEM_HDR + usize::from(rd16(scratch, at));
                self.slots[i].bs += l as u16;
                Ok(Some(&scratch[at + ITEM_HDR..at + l]))
            }
        }
    }

    /// Stop here so that the scratch can be lent out: a run being collected
    /// is written, buffered output flushed, inputs rewound to their head
    /// item. Continue with any scratch of the same length.
    pub fn suspend<S, C>(&mut self, store: &mut S, scratch: &mut [u8], cmp: &C) -> Result<(), S::Error>
    where
        S: Storage,
        C: Fn(&[u8], &[u8]) -> Ordering,
    {
        match self.phase {
            Phase::Collect => self.flush_run(store, scratch, cmp)?,
            Phase::MemDrain => {
                let from = usize::from(self.mem_next);
                if from < usize::from(self.n) {
                    // Sorted already: the rest becomes the only run, read
                    // back by `next` as a one-input final pass.
                    let start = self.in_len;
                    self.write_run(store, scratch, from)?;
                    self.slots[0] =
                        Slot { fpos: start + RUN_HDR as u32, fend: self.in_len, bs: 0, be: 0, live: true };
                    self.k = 1;
                    self.kg = 1;
                    self.runs_left = 0;
                    self.group = false;
                    self.phase = Phase::Drain;
                } else {
                    self.n = 0;
                    self.used = 0;
                    self.phase = Phase::Done;
                }
            }
            Phase::Merge | Phase::Drain => {
                if self.phase == Phase::Merge && self.group {
                    self.flush_out(store, scratch)?;
                }
                for s in &mut self.slots[..usize::from(self.kg)] {
                    if s.live {
                        s.fpos -= u32::from(s.be - s.bs);
                        s.bs = 0;
                        s.be = 0;
                    }
                }
            }
            Phase::Done => {}
        }
        Ok(())
    }
}
