//! Settings store (docs/plan/03-hal-drivers.md §6, WP-PG-2).
//!
//! A log of records over `N` NOR sectors (4 KiB each, erased = `0xFF`,
//! programming only clears bits). One sector is *active*; records are
//! appended to it. When it is full the live records are copied into the
//! next erased sector, which becomes active, and the old one is erased.
//!
//! Layout (all little-endian, 4-byte aligned):
//!
//! ```text
//! sector: [ magic "YSET" | sequence u32 | state u32 | reserved u32 ] records…
//!   state: 0xFFFF_FFFF erased-but-claimed, 0x0000_FFFF active, 0 retired
//! record: [ key u16 | len u16 | crc32 u32 | commit u32 ] data (padded to 4)
//!   commit: 0xFFFF_FFFF = not committed (ignored), 0 = valid
//!   len 0xFFFF = deletion marker (no data)
//! ```
//!
//! A record counts only once its commit word was programmed, which is the
//! last write; a power cut at any step leaves either the old or the new
//! value of that key and never disturbs others. The newest valid record
//! for a key (highest sector sequence, then highest offset) wins.
//!
//! Only the active sector holds live records, so a store holds at most
//! what one sector does: 4,080 bytes of records. A two-byte setting is a
//! 16-byte record, so 255 of them fill a sector with no room to compact.
//! The RAM index ([`MAX_KEYS`] keys, 4 bytes each) is sized so that it is
//! full before the sector is: 192 keys of the firmware's mix (two-byte
//! settings, the 332-byte radio presets, the 6-byte Bluetooth address)
//! are 3,404 bytes, which leaves 676 bytes to write in before the next
//! compaction (VER-4, 2026-11-26; the index was 128 keys of 8 bytes).
//!
//! The format does not depend on the index size: a store written with a
//! bigger index mounts with a smaller one as long as it holds no more
//! keys than that index; beyond it, the smaller index keeps the first keys
//! in log order and does not see the others (its next compaction drops
//! them; nothing else is disturbed).

#![warn(missing_docs)]
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

/// Erase unit of the NOR region, bytes.
pub const SECTOR: u32 = 4096;
/// Sector header magic, `"YSET"`.
pub const MAGIC: u32 = 0x5445_5359;
const HDR: u32 = 16;
const REC_HDR: u32 = 12;
const STATE_ACTIVE: u32 = 0x0000_FFFF;
const STATE_RETIRED: u32 = 0;
const DELETED: u16 = 0xFFFF;
/// Largest value.
pub const MAX_VALUE: usize = 1024;
/// Keys tracked in the RAM index (the default of [`Store`]'s `K`): 192
/// since 2026-11-26 (was 128, full with every app's settings; module
/// documentation for why not more).
pub const MAX_KEYS: usize = 192;
// An index slot keeps a record offset in 15 bits.
const _: () = assert!(SECTOR <= 0x8000);

/// The NOR region the store lives in. Addresses are offsets from its
/// start; `erase` takes a sector-aligned offset.
#[allow(async_fn_in_trait)]
pub trait Nor {
    /// The driver's error.
    type Error;
    /// Read `buf.len()` bytes at `off`.
    async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), Self::Error>;
    /// Program bytes (only clears bits); `off` and `data.len()` multiples of 4.
    async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), Self::Error>;
    /// Erase the sector at `sector_off`.
    async fn erase(&mut self, sector_off: u32) -> Result<(), Self::Error>;
}

/// Why a store operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// The NOR driver failed.
    Nor(E),
    /// The value is longer than [`MAX_VALUE`] (or the key is reserved).
    TooLarge,
    /// No room left, even after compaction.
    Full,
    /// The caller's buffer is shorter than the value.
    BufferTooSmall,
    /// More distinct keys than the index holds ([`MAX_KEYS`] by default).
    TooManyKeys,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::Nor(e)
    }
}

/// One key of the index: 4 bytes (was 8 with an absolute offset and the
/// length; the length is read back from the record header when needed).
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    key: u16,
    /// Offset of the newest record's header inside the active sector, with
    /// [`Slot::DELETED`] set for a deletion marker.
    at: u16,
}

impl Slot {
    const DELETED: u16 = 0x8000;
    fn new(key: u16, off: u32, deleted: bool) -> Self {
        Slot { key, at: off as u16 | if deleted { Self::DELETED } else { 0 } }
    }
    fn off(self) -> u32 {
        u32::from(self.at & !Self::DELETED)
    }
    fn deleted(self) -> bool {
        self.at & Self::DELETED != 0
    }
}

/// A wear-levelled key-value store over `N` NOR sectors with an index of
/// `K` keys (4 bytes of RAM each).
pub struct Store<F: Nor, const N: usize, const K: usize = MAX_KEYS> {
    nor: F,
    active: usize,
    seq: u32,
    /// Next free offset inside the active sector.
    head: u32,
    index: [Slot; K],
    n_keys: usize,
    /// Statistics.
    pub erases: u32,
}

/// CRC-32 (IEEE) register update: [`crc32`] is `!crc32_update(!0, data)`.
fn crc32_update(mut c: u32, data: &[u8]) -> u32 {
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
        }
    }
    c
}

fn crc32(data: &[u8]) -> u32 {
    !crc32_update(0xFFFF_FFFF, data)
}

/// NOR is read and copied in pieces of this size: no value-sized buffer
/// in `set`'s future, which the firmware keeps in the UI task's static
/// (VER-4, 2026-11-26: a 1 KiB copy buffer and a 1 KiB check buffer).
const PIECE: usize = 64;

fn word(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn pad4(n: u32) -> u32 {
    n.div_ceil(4) * 4
}

impl<F: Nor, const N: usize, const K: usize> Store<F, N, K> {
    /// Mount: find the active sector (or format an empty region) and build
    /// the index.
    pub async fn mount(mut nor: F) -> Result<Self, Error<F::Error>> {
        const { assert!(N >= 2) };
        let mut best: Option<(usize, u32)> = None;
        for s in 0..N {
            let mut h = [0u8; HDR as usize];
            nor.read(s as u32 * SECTOR, &mut h).await?;
            if word(&h, 0) == MAGIC && word(&h, 8) == STATE_ACTIVE {
                let seq = word(&h, 4);
                if best.is_none_or(|(_, b)| seq > b) {
                    best = Some((s, seq));
                }
            }
        }
        let mut st =
            Self { nor, active: 0, seq: 0, head: HDR, index: [Slot::default(); K], n_keys: 0, erases: 0 };
        match best {
            Some((s, seq)) => {
                st.active = s;
                st.seq = seq;
                st.scan().await?;
            }
            None => {
                st.format_sector(0, 1).await?;
                st.active = 0;
                st.seq = 1;
                st.head = HDR;
            }
        }
        Ok(st)
    }

    /// [`mount`](Self::mount), tried up to `tries` times with a fresh
    /// driver from `make`: a NOR read error at boot left the player on
    /// default settings until the next restart (2026-11-26: `store=none`
    /// once, the owner's settings intact in NOR and mounted on the next
    /// boot). A read error never formats or writes anything. Returns the
    /// store and the number of failed tries before it.
    pub async fn mount_retrying(
        mut make: impl FnMut() -> F,
        tries: u8,
    ) -> Result<(Self, u8), Error<F::Error>> {
        let mut failed = 0u8;
        loop {
            match Self::mount(make()).await {
                Ok(s) => return Ok((s, failed)),
                Err(e) if failed.saturating_add(1) >= tries.max(1) => return Err(e),
                Err(_) => failed += 1,
            }
        }
    }

    async fn format_sector(&mut self, s: usize, seq: u32) -> Result<(), Error<F::Error>> {
        let base = s as u32 * SECTOR;
        self.nor.erase(base).await?;
        self.erases += 1;
        let mut h = [0xFFu8; HDR as usize];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&seq.to_le_bytes());
        self.nor.program(base, &h[..8]).await?;
        self.nor.program(base + 8, &STATE_ACTIVE.to_le_bytes()).await?;
        Ok(())
    }

    /// Walk the active sector's records and index the committed ones.
    async fn scan(&mut self) -> Result<(), Error<F::Error>> {
        self.n_keys = 0;
        let base = self.active as u32 * SECTOR;
        let mut off = HDR;
        while off + REC_HDR <= SECTOR {
            let mut rh = [0u8; REC_HDR as usize];
            self.nor.read(base + off, &mut rh).await?;
            let key = u16::from_le_bytes([rh[0], rh[1]]);
            let len = u16::from_le_bytes([rh[2], rh[3]]);
            if key == 0xFFFF && len == 0xFFFF && word(&rh, 4) == 0xFFFF_FFFF {
                // Erased: end of log. If anything was programmed after it
                // (foreign data, bit rot), appending here would program
                // over it and corrupt the new record: the next append
                // compacts into a freshly erased sector instead (VER-4
                // finding F-SET-1).
                if !self.erased_from(base + off, base + SECTOR).await? {
                    off = SECTOR;
                }
                break;
            }
            let dlen = if len == DELETED { 0 } else { u32::from(len) };
            let total = REC_HDR + pad4(dlen);
            if dlen as usize > MAX_VALUE || off + total > SECTOR {
                // Torn header: nothing valid can follow.
                off = SECTOR;
                break;
            }
            if word(&rh, 8) == 0 && self.check(base + off, len, word(&rh, 4)).await? {
                self.put_index(key, off, len == DELETED);
            }
            off += total;
        }
        self.head = off;
        Ok(())
    }

    /// Whether `[from, to)` reads as erased.
    async fn erased_from(&mut self, mut from: u32, to: u32) -> Result<bool, Error<F::Error>> {
        let mut buf = [0u8; 64];
        while from < to {
            let n = (to - from).min(buf.len() as u32);
            let b = &mut buf[..n as usize];
            self.nor.read(from, b).await?;
            if b.iter().any(|&x| x != 0xFF) {
                return Ok(false);
            }
            from += n;
        }
        Ok(true)
    }

    async fn check(&mut self, at: u32, len: u16, crc: u32) -> Result<bool, Error<F::Error>> {
        if len == DELETED {
            return Ok(crc == 0);
        }
        Ok(self.crc_at(at + REC_HDR, len).await? == crc)
    }

    /// CRC-32 of the `len` bytes at absolute `at`, read piece by piece.
    async fn crc_at(&mut self, at: u32, len: u16) -> Result<u32, Error<F::Error>> {
        let mut c = 0xFFFF_FFFF;
        let mut b = [0u8; PIECE];
        let mut done = 0;
        while done < u32::from(len) {
            let n = (u32::from(len) - done).min(PIECE as u32) as usize;
            self.nor.read(at + done, &mut b[..n]).await?;
            c = crc32_update(c, &b[..n]);
            done += n as u32;
        }
        Ok(!c)
    }

    /// Index the record at `off` of the active sector as `key`'s newest.
    /// A key beyond the index's `K` is not seen (only a store written with
    /// a bigger index has one: module documentation).
    fn put_index(&mut self, key: u16, off: u32, deleted: bool) {
        let slot = Slot::new(key, off, deleted);
        if let Some(s) = self.index[..self.n_keys].iter_mut().find(|s| s.key == key) {
            *s = slot;
        } else if self.n_keys < K {
            self.index[self.n_keys] = slot;
            self.n_keys += 1;
        }
    }

    /// The data length of the record whose header is at absolute `at`
    /// (read back from NOR: the index keeps no lengths). `None` if the
    /// header no longer reads as a value the store wrote.
    async fn len_at(&mut self, at: u32) -> Result<Option<u16>, Error<F::Error>> {
        let mut b = [0u8; 4];
        self.nor.read(at, &mut b).await?;
        let len = u16::from_le_bytes([b[2], b[3]]);
        Ok((usize::from(len) <= MAX_VALUE).then_some(len))
    }

    /// Read a value; returns its length, or `None` if absent.
    pub async fn get(&mut self, key: u16, buf: &mut [u8]) -> Result<Option<usize>, Error<F::Error>> {
        let Some(s) = self.index[..self.n_keys].iter().find(|s| s.key == key).copied() else {
            return Ok(None);
        };
        if s.deleted() {
            return Ok(None);
        }
        let at = self.active as u32 * SECTOR + s.off();
        let Some(len) = self.len_at(at).await? else { return Ok(None) };
        let n = usize::from(len);
        if buf.len() < n {
            return Err(Error::BufferTooSmall);
        }
        self.nor.read(at + REC_HDR, &mut buf[..n]).await?;
        Ok(Some(n))
    }

    /// The keys that hold a value.
    pub fn keys(&self) -> impl Iterator<Item = u16> + '_ {
        self.index[..self.n_keys].iter().filter(|s| !s.deleted()).map(|s| s.key)
    }

    /// Keys in the index, deletion markers included (each takes a slot
    /// until the next compaction).
    #[must_use]
    pub fn indexed(&self) -> usize {
        self.n_keys
    }

    /// Store `value` under `key`, replacing any older value.
    pub async fn set(&mut self, key: u16, value: &[u8]) -> Result<(), Error<F::Error>> {
        if value.len() > MAX_VALUE || key == 0xFFFF {
            return Err(Error::TooLarge);
        }
        if self.index[..self.n_keys].iter().all(|s| s.key != key) && self.n_keys >= K {
            return Err(Error::TooManyKeys);
        }
        self.append(key, value.len() as u16, value).await
    }

    /// Delete the value under `key` (nothing happens if there is none).
    pub async fn remove(&mut self, key: u16) -> Result<(), Error<F::Error>> {
        if self.index[..self.n_keys].iter().any(|s| s.key == key && !s.deleted()) {
            self.append(key, DELETED, &[]).await?;
        }
        Ok(())
    }

    async fn append(&mut self, key: u16, len: u16, value: &[u8]) -> Result<(), Error<F::Error>> {
        let need = REC_HDR + pad4(value.len() as u32);
        if self.head + need > SECTOR {
            self.compact(need).await?;
        }
        let base = self.active as u32 * SECTOR;
        let off = self.head;
        self.write_record(base + off, key, len, value).await?;
        self.head += need;
        self.put_index(key, off, len == DELETED);
        Ok(())
    }

    async fn write_record(
        &mut self,
        at: u32,
        key: u16,
        len: u16,
        value: &[u8],
    ) -> Result<(), Error<F::Error>> {
        let crc = if len == DELETED { 0 } else { crc32(value) };
        self.write_header(at, key, len, crc).await?;
        let mut chunk = [0xFFu8; PIECE];
        for (i, part) in value.chunks(PIECE).enumerate() {
            let n = pad4(part.len() as u32) as usize;
            chunk[..n].fill(0xFF);
            chunk[..part.len()].copy_from_slice(part);
            self.nor.program(at + REC_HDR + (i * PIECE) as u32, &chunk[..n]).await?;
        }
        // Commit last.
        self.nor.program(at + 8, &0u32.to_le_bytes()).await?;
        Ok(())
    }

    /// The record's first 8 bytes: key, length, CRC (not yet committed).
    async fn write_header(&mut self, at: u32, key: u16, len: u16, crc: u32) -> Result<(), Error<F::Error>> {
        let mut h = [0u8; 8];
        h[0..2].copy_from_slice(&key.to_le_bytes());
        h[2..4].copy_from_slice(&len.to_le_bytes());
        h[4..8].copy_from_slice(&crc.to_le_bytes());
        self.nor.program(at, &h).await?;
        Ok(())
    }

    /// Copy a value record from absolute `from` to `to` piece by piece:
    /// the same bytes [`write_record`](Self::write_record) would write
    /// for its value, committed last.
    async fn copy_record(
        &mut self,
        from: u32,
        to: u32,
        key: u16,
        len: u16,
        crc: u32,
    ) -> Result<(), Error<F::Error>> {
        self.write_header(to, key, len, crc).await?;
        let mut chunk = [0xFFu8; PIECE];
        let mut done = 0;
        while done < u32::from(len) {
            let n = (u32::from(len) - done).min(PIECE as u32) as usize;
            let p = pad4(n as u32) as usize;
            chunk[..p].fill(0xFF);
            self.nor.read(from + REC_HDR + done, &mut chunk[..n]).await?;
            self.nor.program(to + REC_HDR + done, &chunk[..p]).await?;
            done += n as u32;
        }
        self.nor.program(to + 8, &0u32.to_le_bytes()).await?;
        Ok(())
    }

    /// Move live records into the next sector and retire the old one.
    ///
    /// The index is not touched until the new sector is active; it is then
    /// rebuilt by scanning that sector (which drops the deletion markers).
    /// A NOR error part-way leaves the index on the old sector, which is
    /// also the active one in NOR.
    async fn compact(&mut self, need: u32) -> Result<(), Error<F::Error>> {
        let old = self.active;
        let obase = old as u32 * SECTOR;
        let mut live = 0u32;
        for i in 0..self.n_keys {
            let s = self.index[i];
            if s.deleted() {
                continue;
            }
            if let Some(len) = self.len_at(obase + s.off()).await? {
                live += REC_HDR + pad4(u32::from(len));
            }
        }
        if HDR + live + need > SECTOR {
            return Err(Error::Full);
        }
        let next = (old + 1) % N;
        let seq = self.seq.wrapping_add(1);
        // New sector: erase and write its header but keep it inactive
        // (state erased) until all records are copied.
        let nbase = next as u32 * SECTOR;
        self.nor.erase(nbase).await?;
        self.erases += 1;
        let mut h = [0xFFu8; 8];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&seq.to_le_bytes());
        self.nor.program(nbase, &h).await?;
        let mut off = HDR;
        for i in 0..self.n_keys {
            let s = self.index[i];
            if s.deleted() {
                continue;
            }
            let at = obase + s.off();
            let mut rh = [0u8; REC_HDR as usize];
            self.nor.read(at, &mut rh).await?;
            let len = u16::from_le_bytes([rh[2], rh[3]]);
            let crc = word(&rh, 4);
            // A value that no longer reads as it did at mount (bit rot) is
            // not carried over: the next mount would refuse it as well.
            if usize::from(len) > MAX_VALUE || self.crc_at(at + REC_HDR, len).await? != crc {
                continue;
            }
            let total = REC_HDR + pad4(u32::from(len));
            if off + total > SECTOR {
                // The old sector read differently from the first pass:
                // stay on it.
                return Err(Error::Full);
            }
            self.copy_record(at, nbase + off, s.key, len, crc).await?;
            off += total;
        }
        // Activate the new sector, then retire the old one.
        self.nor.program(nbase + 8, &STATE_ACTIVE.to_le_bytes()).await?;
        self.active = next;
        self.seq = seq;
        self.scan().await?;
        self.nor.program(obase + 8, &STATE_RETIRED.to_le_bytes()).await?;
        Ok(())
    }

    /// Give the storage back (tests).
    pub fn into_inner(self) -> F {
        self.nor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::block_on;
    use std::collections::HashMap;
    use std::vec;
    use std::vec::Vec;

    /// Simulated NOR: bit-clear programming, 4 KiB erase, and an optional
    /// power cut after `budget` write operations (later writes are lost,
    /// the one that hits the budget is applied only halfway).
    #[derive(Clone)]
    struct SimNor {
        mem: Vec<u8>,
        budget: Option<u32>,
        ops: u32,
        /// Reads that fail before reads work (shared across clones).
        fail_reads: std::rc::Rc<core::cell::Cell<u32>>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Cut;

    impl SimNor {
        fn new(sectors: usize) -> Self {
            Self {
                mem: vec![0xFF; sectors * SECTOR as usize],
                budget: None,
                ops: 0,
                fail_reads: std::rc::Rc::new(core::cell::Cell::new(0)),
            }
        }
        fn tick(&mut self) -> Result<bool, Cut> {
            self.ops += 1;
            match self.budget {
                Some(b) if self.ops > b + 1 => Err(Cut),
                Some(b) if self.ops == b + 1 => Ok(true), // torn op
                _ => Ok(false),
            }
        }
    }

    impl Nor for SimNor {
        type Error = Cut;
        async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), Cut> {
            if self.fail_reads.get() > 0 {
                self.fail_reads.set(self.fail_reads.get() - 1);
                return Err(Cut);
            }
            let o = off as usize;
            buf.copy_from_slice(&self.mem[o..o + buf.len()]);
            Ok(())
        }
        async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), Cut> {
            assert!(off % 4 == 0 && data.len() % 4 == 0, "unaligned program");
            let torn = self.tick()?;
            let n = if torn { data.len() / 2 } else { data.len() };
            for (i, &b) in data[..n].iter().enumerate() {
                self.mem[off as usize + i] &= b;
            }
            if torn { Err(Cut) } else { Ok(()) }
        }
        async fn erase(&mut self, s: u32) -> Result<(), Cut> {
            assert!(s % SECTOR == 0);
            let torn = self.tick()?;
            let end = if torn { s + SECTOR / 3 } else { s + SECTOR };
            self.mem[s as usize..end as usize].fill(0xFF);
            if torn { Err(Cut) } else { Ok(()) }
        }
    }

    type S = Store<SimNor, 4>;

    fn get(s: &mut S, k: u16) -> Option<Vec<u8>> {
        let mut b = [0u8; MAX_VALUE];
        block_on(s.get(k, &mut b)).unwrap().map(|n| b[..n].to_vec())
    }

    /// 2026-11-26: one NOR read error at boot left the player on default
    /// settings (`store=none`); the mount is retried with a fresh driver
    /// and the stored values come back, and nothing is erased.
    #[test]
    fn regress_mount_survives_a_read_error() {
        let mut nor = SimNor::new(4);
        {
            let mut s = block_on(S::mount(nor.clone())).unwrap();
            block_on(s.set(0x0701, &[1, 0])).unwrap();
            nor = s.nor.clone();
        }
        let erases_before = nor.ops;
        nor.fail_reads.set(1);
        let (mut s, failed) = block_on(S::mount_retrying(|| nor.clone(), 3)).unwrap();
        assert_eq!(failed, 1);
        assert_eq!(get(&mut s, 0x0701), Some(vec![1, 0]));
        assert_eq!(s.nor.ops, erases_before, "a retried mount wrote to NOR");
        nor.fail_reads.set(5);
        assert!(block_on(S::mount_retrying(|| nor.clone(), 3)).is_err());
    }

    /// 2026-11-26 (VER-4, found while making the index smaller): a value
    /// whose bytes changed in NOR after the mount (bit rot) was copied by
    /// the next compaction with a fresh CRC, so the corruption became a
    /// valid record. Now the copy checks the CRC and leaves it out, as a
    /// mount would; the other keys are copied.
    #[test]
    fn regress_compaction_does_not_bless_a_rotted_value() {
        let mut s = block_on(S::mount(SimNor::new(4))).unwrap();
        block_on(s.set(1, &[0x55; 40])).unwrap();
        block_on(s.set(2, &[7, 7])).unwrap();
        // Key 1's data starts after the sector and record headers.
        let at = (s.active as u32 * SECTOR + HDR + REC_HDR) as usize + 5;
        s.nor.mem[at] &= 0xFE;
        let erases = s.erases;
        let mut i = 0u16;
        while s.erases == erases {
            block_on(s.set(2, &i.to_le_bytes())).unwrap();
            i += 1;
        }
        assert_eq!(get(&mut s, 1), None, "the rotted value was carried over");
        let mut s = block_on(S::mount(s.into_inner())).unwrap();
        assert_eq!(get(&mut s, 1), None);
        assert_eq!(get(&mut s, 2), Some((i - 1).to_le_bytes().to_vec()));
    }

    #[test]
    fn basic_set_get_remove_remount() {
        let mut s = block_on(S::mount(SimNor::new(4))).unwrap();
        block_on(s.set(1, b"hello")).unwrap();
        block_on(s.set(2, &[7; 100])).unwrap();
        block_on(s.set(1, b"world!")).unwrap();
        assert_eq!(get(&mut s, 1).as_deref(), Some(&b"world!"[..]));
        block_on(s.remove(2)).unwrap();
        assert_eq!(get(&mut s, 2), None);
        let nor = s.into_inner();
        let mut s = block_on(S::mount(nor)).unwrap();
        assert_eq!(get(&mut s, 1).as_deref(), Some(&b"world!"[..]));
        assert_eq!(get(&mut s, 2), None);
        assert_eq!(s.keys().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn compaction_and_wear_levelling() {
        let mut s = block_on(S::mount(SimNor::new(4))).unwrap();
        for i in 0..2000u32 {
            let k = (i % 10) as u16;
            block_on(s.set(k, &i.to_le_bytes().repeat(8))).unwrap();
        }
        for k in 0..10u16 {
            let last = (1990 + u32::from(k)).to_le_bytes().repeat(8);
            assert_eq!(get(&mut s, k), Some(last));
        }
        // Erases spread over all four sectors.
        assert!(s.erases > 8);
        let nor = s.into_inner();
        let mut s = block_on(S::mount(nor)).unwrap();
        assert_eq!(get(&mut s, 3), Some(1993u32.to_le_bytes().repeat(8)));
    }

    #[test]
    fn full_is_an_error_not_corruption() {
        let mut s = block_on(Store::<SimNor, 2>::mount(SimNor::new(2))).unwrap();
        let mut ok = 0;
        for k in 0..100u16 {
            match block_on(s.set(k, &[k as u8; 200])) {
                Ok(()) => ok += 1,
                Err(Error::Full) => break,
                Err(e) => panic!("{e:?}"),
            }
        }
        assert!(ok >= 15);
        let mut b = [0u8; MAX_VALUE];
        assert_eq!(block_on(s.get(0, &mut b)).unwrap(), Some(200));
    }

    /// Power cut at every write step of a workload: after remount every key
    /// holds either its value before or after the interrupted operation.
    #[test]
    fn power_cut_at_every_step() {
        // Reference run: which operations does the workload do?
        let workload: Vec<(u16, Vec<u8>)> =
            (0..300u32).map(|i| ((i % 7) as u16, vec![(i % 251) as u8; (i % 90) as usize + 1])).collect();
        let mut clean = block_on(S::mount(SimNor::new(4))).unwrap();
        let base_ops = clean.nor.ops;
        for (k, v) in &workload {
            block_on(clean.set(*k, v)).unwrap();
        }
        let total = clean.nor.ops - base_ops;
        for cut in (0..total).step_by(3) {
            let mut nor = SimNor::new(4);
            let s = block_on(S::mount(nor.clone())).unwrap();
            nor = s.into_inner();
            nor.budget = Some(nor.ops + cut);
            let mut s = block_on(S::mount(nor)).unwrap();
            let mut model: HashMap<u16, Vec<u8>> = HashMap::new();
            let mut pending = None;
            for (k, v) in &workload {
                match block_on(s.set(*k, v)) {
                    Ok(()) => {
                        model.insert(*k, v.clone());
                    }
                    Err(Error::Nor(Cut)) => {
                        pending = Some((*k, v.clone()));
                        break;
                    }
                    Err(e) => panic!("{e:?}"),
                }
            }
            let mut nor = s.into_inner();
            nor.budget = None;
            let mut s = block_on(S::mount(nor)).unwrap();
            for k in 0..7u16 {
                let got = get(&mut s, k);
                let old = model.get(&k).cloned();
                let ok = match &pending {
                    Some((pk, pv)) if *pk == k => got == old || got.as_ref() == Some(pv),
                    _ => got == old,
                };
                assert!(ok, "cut {cut}: key {k} = {got:?}, expected {old:?} (pending {pending:?})");
            }
            // The store keeps working after the cut.
            block_on(s.set(9, b"after")).unwrap();
            assert_eq!(get(&mut s, 9).as_deref(), Some(&b"after"[..]));
        }
    }

    /// Bytes programmed after the end of the log (foreign data, bit rot)
    /// are not programmed over: the next value goes to a fresh sector and
    /// reads back intact, before and after a remount.
    ///
    /// Regression: VER-4 finding F-SET-1 (2026-11-24, fuzzing): the next
    /// record was programmed over them and read back corrupted.
    #[test]
    fn regress_settings_garbage_after_the_log_end() {
        let mut s = block_on(S::mount(SimNor::new(4))).unwrap();
        block_on(s.set(1, b"kept")).unwrap();
        let mut nor = s.into_inner();
        // After the first record (16 + 12 + 4 bytes): an erased header,
        // then stray bytes.
        nor.mem[100] = 0x75;
        let mut s = block_on(S::mount(nor)).unwrap();
        block_on(s.set(2, &[0x1E; 90])).unwrap();
        assert_eq!(get(&mut s, 2), Some(vec![0x1E; 90]));
        let mut s = block_on(S::mount(s.into_inner())).unwrap();
        assert_eq!(get(&mut s, 2), Some(vec![0x1E; 90]));
        assert_eq!(get(&mut s, 1).as_deref(), Some(&b"kept"[..]));
    }
}
