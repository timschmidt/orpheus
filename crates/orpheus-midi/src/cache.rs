//! A block cache that answers the parser's [`Need`]s from a file read in
//! 512-byte blocks (the card's sectors) — the player's card source (SYN-3).
//!
//! The parser asks for at most [`WINDOW`] bytes at any offset, one track
//! window at a time; a file's tracks lie in different places, so the
//! cache keeps up to [`SLOTS`] blocks (least recently used out) and a
//! `Need` costs a card read only when one of the (at most two) blocks it
//! spans is missing. The cache holds only tags; the block memory is lent
//! by the caller on every call (the firmware's pool, after the player), so
//! the cache is a plain value that lives in the pool next to the player
//! (`bytemuck::Zeroable`: all-zero is "empty").
//!
//! Use: `answer(need, mem, &mut buf)`; on `Err(Miss { block })` read the
//! file at `block × BLOCK` into `slot(block, mem)` and report how many
//! bytes came with `filled`, then answer again. A short read (the file's
//! end, a card error) leaves a short block: the answer is short and the
//! parser ends that track (it trusts nothing).

use crate::smf::{Need, WINDOW};

/// Bytes per block (a card sector).
pub const BLOCK: usize = 512;
/// Most blocks the cache keeps.
pub const SLOTS: usize = 32;

/// The block to read before the `Need` can be answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Miss {
    /// Block number (file offset / [`BLOCK`]).
    pub block: u32,
}

/// Tags of the cached blocks (see the module documentation).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug)]
pub struct BlockCache {
    /// Block number + 1 per slot (0: empty).
    tag: [u32; SLOTS],
    /// Valid bytes per slot.
    len: [u16; SLOTS],
    /// Last use per slot.
    used: [u32; SLOTS],
    /// Slots in use (`mem.len() / BLOCK`, at most [`SLOTS`]).
    n: u8,
    /// The slot a `slot` call handed out for filling, + 1.
    filling: u8,
    /// The block it is for.
    want: u32,
    clock: u32,
    /// Needs answered.
    pub hits: u32,
    /// Blocks read.
    pub misses: u32,
}

impl BlockCache {
    /// Empty, for `slots` blocks of memory (at most [`SLOTS`]).
    pub fn reset(&mut self, slots: usize) {
        self.tag = [0; SLOTS];
        self.len = [0; SLOTS];
        self.used = [0; SLOTS];
        self.n = slots.min(SLOTS) as u8;
        self.filling = 0;
        self.want = 0;
        self.clock = 0;
        self.hits = 0;
        self.misses = 0;
    }

    /// Blocks it holds at most.
    #[must_use]
    pub fn slots(&self) -> usize {
        usize::from(self.n)
    }

    fn find(&self, block: u32) -> Option<usize> {
        self.tag.iter().take(self.slots()).position(|&t| t == block.wrapping_add(1))
    }

    /// Copy the bytes of `need` into `out` (≥ [`WINDOW`] bytes; a longer
    /// `need` is cut to it) and return how many (fewer when a block is
    /// short), or the first block that must be read first.
    pub fn answer(&mut self, need: Need, mem: &[u8], out: &mut [u8]) -> Result<usize, Miss> {
        let want = usize::from(need.len).min(WINDOW).min(out.len());
        let first = need.offset / BLOCK as u32;
        let last = (u64::from(need.offset) + want.saturating_sub(1) as u64) / BLOCK as u64;
        let last = last.min(u64::from(u32::MAX)) as u32;
        for b in first..=last {
            if self.find(b).is_none() {
                return Err(Miss { block: b });
            }
        }
        self.clock = self.clock.wrapping_add(1);
        let mut n = 0;
        let mut at = u64::from(need.offset);
        while n < want {
            let Some(s) = self.find((at / BLOCK as u64) as u32) else { break };
            let (Some(used), Some(&have)) = (self.used.get_mut(s), self.len.get(s)) else { break };
            *used = self.clock;
            let off = (at % BLOCK as u64) as usize;
            let have = usize::from(have);
            if off >= have {
                break;
            }
            let k = (have - off).min(want - n);
            let (Some(src), Some(dst)) =
                (mem.get(s * BLOCK + off..s * BLOCK + off + k), out.get_mut(n..n + k))
            else {
                break;
            };
            dst.copy_from_slice(src);
            n += k;
            at += k as u64;
            if have < BLOCK {
                // A short block (the file's end, a failed read): nothing
                // after it.
                break;
            }
        }
        self.hits = self.hits.wrapping_add(1);
        Ok(n)
    }

    /// The memory to read `block` into (the least recently used slot; its
    /// old block is forgotten). The slot is tagged only by
    /// [`BlockCache::filled`]: a read that never completes leaves it empty,
    /// not stale bytes under the new tag. `None` without memory.
    pub fn slot<'m>(&mut self, block: u32, mem: &'m mut [u8]) -> Option<&'m mut [u8]> {
        let n = self.slots().min(mem.len() / BLOCK);
        if n == 0 {
            return None;
        }
        let s = (0..n)
            .zip(self.tag.iter().zip(&self.used))
            .min_by_key(|&(_, (&t, &u))| (t != 0, u))
            .map_or(0, |(i, _)| i);
        let (Some(tag), Some(len)) = (self.tag.get_mut(s), self.len.get_mut(s)) else { return None };
        *tag = 0;
        *len = 0;
        self.filling = s as u8 + 1;
        self.want = block;
        mem.get_mut(s * BLOCK..(s + 1) * BLOCK)
    }

    /// The read into the slot `slot` handed out brought `n` bytes of
    /// `block`.
    pub fn filled(&mut self, block: u32, n: usize) {
        let Some(s) = self.filling.checked_sub(1).map(usize::from) else { return };
        self.filling = 0;
        if self.want != block {
            return;
        }
        self.clock = self.clock.wrapping_add(1);
        let (Some(tag), Some(len), Some(used)) =
            (self.tag.get_mut(s), self.len.get_mut(s), self.used.get_mut(s))
        else {
            return;
        };
        *tag = block.wrapping_add(1);
        *len = n.min(BLOCK) as u16;
        *used = self.clock;
        self.misses = self.misses.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn file(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + (i >> 9)) as u8).collect()
    }

    /// Answer `need` from `data`, reading blocks as asked; returns the
    /// bytes and the reads.
    fn serve(c: &mut BlockCache, mem: &mut [u8], data: &[u8], need: Need) -> (Vec<u8>, u32) {
        let mut reads = 0;
        let mut out = [0u8; WINDOW];
        loop {
            match c.answer(need, mem, &mut out) {
                Ok(n) => return (out[..n].to_vec(), reads),
                Err(Miss { block }) => {
                    reads += 1;
                    assert!(reads <= 2, "a need spans two blocks at most");
                    let s = c.slot(block, mem).unwrap();
                    let at = block as usize * BLOCK;
                    let n = data.len().saturating_sub(at).min(BLOCK);
                    if n > 0 {
                        s[..n].copy_from_slice(&data[at..at + n]);
                    }
                    c.filled(block, n);
                }
            }
        }
    }

    fn cache(slots: usize) -> (BlockCache, Vec<u8>) {
        let mut c = <BlockCache as bytemuck::Zeroable>::zeroed();
        c.reset(slots);
        (c, std::vec![0u8; slots * BLOCK])
    }

    #[test]
    fn answers_equal_the_file_at_any_offset() {
        let data = file(5000);
        let (mut c, mut mem) = cache(4);
        for off in [0u32, 1, 63, 448, 449, 511, 512, 1000, 4990, 4999, 5000, 6000] {
            for len in [1u16, 7, 34, 64] {
                let (got, _) = serve(&mut c, &mut mem, &data, Need { offset: off, len });
                let a = (off as usize).min(data.len());
                let b = (off as usize + len as usize).min(data.len());
                assert_eq!(got, data[a..b], "offset {off} len {len}");
            }
        }
    }

    #[test]
    fn a_need_across_two_blocks_reads_both_once() {
        let data = file(2048);
        let (mut c, mut mem) = cache(4);
        let (got, reads) = serve(&mut c, &mut mem, &data, Need { offset: 500, len: 64 });
        assert_eq!(got, data[500..564]);
        assert_eq!(reads, 2);
        let (_, reads) = serve(&mut c, &mut mem, &data, Need { offset: 520, len: 64 });
        assert_eq!(reads, 0, "cached");
        assert_eq!((c.misses, c.hits), (2, 2));
    }

    /// Tracks in different places, fewer than the slots: after the first
    /// read of each block nothing is read again (the player's case).
    #[test]
    fn interleaved_tracks_within_the_slots_read_each_block_once() {
        let data = file(16 * BLOCK);
        let (mut c, mut mem) = cache(8);
        let starts = [0usize, 2048, 4096, 6144];
        let mut reads = 0;
        for step in 0..32 {
            for s in starts {
                let off = (s + step * WINDOW) as u32;
                let (got, r) = serve(&mut c, &mut mem, &data, Need { offset: off, len: 64 });
                assert_eq!(got, data[off as usize..off as usize + 64]);
                reads += r;
            }
        }
        // 4 tracks × 2048 bytes each = 16 blocks, each read once.
        assert_eq!(reads, 16);
    }

    /// The least recently used block goes.
    #[test]
    fn lru_eviction() {
        let data = file(8 * BLOCK);
        let (mut c, mut mem) = cache(2);
        let need = |b: usize| Need { offset: (b * BLOCK) as u32, len: 16 };
        assert_eq!(serve(&mut c, &mut mem, &data, need(0)).1, 1);
        assert_eq!(serve(&mut c, &mut mem, &data, need(1)).1, 1);
        assert_eq!(serve(&mut c, &mut mem, &data, need(0)).1, 0);
        assert_eq!(serve(&mut c, &mut mem, &data, need(2)).1, 1, "evicts block 1");
        assert_eq!(serve(&mut c, &mut mem, &data, need(0)).1, 0);
        assert_eq!(serve(&mut c, &mut mem, &data, need(1)).1, 1);
    }

    /// A failed read (0 bytes) or a slot never filled: no stale bytes under
    /// the new tag, the answer is short, the next need reads again.
    #[test]
    fn a_failed_read_answers_short_and_is_not_kept() {
        let data = file(2048);
        let (mut c, mut mem) = cache(2);
        serve(&mut c, &mut mem, &data, Need { offset: 0, len: 64 });
        let mut out = [0u8; WINDOW];
        let Err(Miss { block }) = c.answer(Need { offset: 1024, len: 64 }, &mem, &mut out) else { panic!() };
        // Handed out, never filled: the next answer misses again.
        let _ = c.slot(block, &mut mem).unwrap();
        assert_eq!(c.answer(Need { offset: 1024, len: 64 }, &mem, &mut out), Err(Miss { block: 2 }));
        let _ = c.slot(block, &mut mem).unwrap();
        c.filled(block, 0);
        assert_eq!(c.answer(Need { offset: 1024, len: 64 }, &mem, &mut out), Ok(0));
        // Block 0 is still answered (it was the other slot).
        assert!(c.answer(Need { offset: 10, len: 64 }, &mem, &mut out).is_ok_and(|n| n == 64));
    }

    #[test]
    fn no_memory_no_slot() {
        let (mut c, _) = cache(4);
        assert!(c.slot(0, &mut []).is_none());
        let (mut c, mut mem) = cache(0);
        assert!(c.slot(0, &mut mem).is_none());
    }
}
