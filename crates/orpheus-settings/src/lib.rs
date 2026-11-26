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
/// Keys tracked in the RAM index.
pub const MAX_KEYS: usize = 128;

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
    /// More than [`MAX_KEYS`] distinct keys.
    TooManyKeys,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::Nor(e)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    key: u16,
    /// Absolute offset of the record header.
    at: u32,
    len: u16,
}

/// A wear-levelled key-value store over `N` NOR sectors.
pub struct Store<F: Nor, const N: usize> {
    nor: F,
    active: usize,
    seq: u32,
    /// Next free offset inside the active sector.
    head: u32,
    index: [Slot; MAX_KEYS],
    n_keys: usize,
    /// Statistics.
    pub erases: u32,
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
        }
    }
    !c
}

fn word(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn pad4(n: u32) -> u32 {
    n.div_ceil(4) * 4
}

impl<F: Nor, const N: usize> Store<F, N> {
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
        let mut st = Self {
            nor,
            active: 0,
            seq: 0,
            head: HDR,
            index: [Slot::default(); MAX_KEYS],
            n_keys: 0,
            erases: 0,
        };
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
                self.put_index(key, base + off, len);
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
        let mut buf = [0u8; MAX_VALUE];
        let b = &mut buf[..usize::from(len)];
        self.nor.read(at + REC_HDR, b).await?;
        Ok(crc32(b) == crc)
    }

    fn put_index(&mut self, key: u16, at: u32, len: u16) {
        if let Some(s) = self.index[..self.n_keys].iter_mut().find(|s| s.key == key) {
            s.at = at;
            s.len = len;
        } else if self.n_keys < MAX_KEYS {
            self.index[self.n_keys] = Slot { key, at, len };
            self.n_keys += 1;
        }
    }

    /// Read a value; returns its length, or `None` if absent.
    pub async fn get(&mut self, key: u16, buf: &mut [u8]) -> Result<Option<usize>, Error<F::Error>> {
        let Some(s) = self.index[..self.n_keys].iter().find(|s| s.key == key).copied() else {
            return Ok(None);
        };
        if s.len == DELETED {
            return Ok(None);
        }
        let n = usize::from(s.len);
        if buf.len() < n {
            return Err(Error::BufferTooSmall);
        }
        self.nor.read(s.at + REC_HDR, &mut buf[..n]).await?;
        Ok(Some(n))
    }

    /// Keys currently present.
    /// The keys that hold a value.
    pub fn keys(&self) -> impl Iterator<Item = u16> + '_ {
        self.index[..self.n_keys].iter().filter(|s| s.len != DELETED).map(|s| s.key)
    }

    /// Store `value` under `key`, replacing any older value.
    pub async fn set(&mut self, key: u16, value: &[u8]) -> Result<(), Error<F::Error>> {
        if value.len() > MAX_VALUE || key == 0xFFFF {
            return Err(Error::TooLarge);
        }
        if self.index[..self.n_keys].iter().all(|s| s.key != key) && self.n_keys >= MAX_KEYS {
            return Err(Error::TooManyKeys);
        }
        self.append(key, value.len() as u16, value).await
    }

    /// Delete the value under `key` (nothing happens if there is none).
    pub async fn remove(&mut self, key: u16) -> Result<(), Error<F::Error>> {
        if self.index[..self.n_keys].iter().any(|s| s.key == key && s.len != DELETED) {
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
        let at = base + self.head;
        self.write_record(at, key, len, value).await?;
        self.head += need;
        self.put_index(key, at, len);
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
        let mut h = [0u8; 8];
        h[0..2].copy_from_slice(&key.to_le_bytes());
        h[2..4].copy_from_slice(&len.to_le_bytes());
        h[4..8].copy_from_slice(&crc.to_le_bytes());
        self.nor.program(at, &h).await?;
        let mut chunk = [0xFFu8; 64];
        for (i, part) in value.chunks(64).enumerate() {
            let n = pad4(part.len() as u32) as usize;
            chunk[..n].fill(0xFF);
            chunk[..part.len()].copy_from_slice(part);
            self.nor.program(at + REC_HDR + (i * 64) as u32, &chunk[..n]).await?;
        }
        // Commit last.
        self.nor.program(at + 8, &0u32.to_le_bytes()).await?;
        Ok(())
    }

    /// Move live records into the next sector and retire the old one.
    async fn compact(&mut self, need: u32) -> Result<(), Error<F::Error>> {
        let live: u32 = self.index[..self.n_keys]
            .iter()
            .filter(|s| s.len != DELETED)
            .map(|s| REC_HDR + pad4(u32::from(s.len)))
            .sum();
        if HDR + live + need > SECTOR {
            return Err(Error::Full);
        }
        let old = self.active;
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
        let mut buf = [0u8; MAX_VALUE];
        for i in 0..self.n_keys {
            let s = self.index[i];
            if s.len == DELETED {
                continue;
            }
            let n = usize::from(s.len);
            self.nor.read(s.at + REC_HDR, &mut buf[..n]).await?;
            self.write_record(nbase + off, s.key, s.len, &buf[..n]).await?;
            self.index[i].at = nbase + off;
            off += REC_HDR + pad4(u32::from(s.len));
        }
        // Activate the new sector, then retire the old one.
        self.nor.program(nbase + 8, &STATE_ACTIVE.to_le_bytes()).await?;
        self.nor.program(old as u32 * SECTOR + 8, &STATE_RETIRED.to_le_bytes()).await?;
        // Drop deletion markers from the index.
        let mut w = 0;
        for r in 0..self.n_keys {
            if self.index[r].len != DELETED {
                self.index[w] = self.index[r];
                w += 1;
            }
        }
        self.n_keys = w;
        self.active = next;
        self.seq = seq;
        self.head = off;
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
