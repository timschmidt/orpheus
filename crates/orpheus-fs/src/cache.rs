//! Write-back sector cache for metadata (FAT, allocation bitmap, directories)
//! and partial data sectors.
//!
//! The owner provides the slots (`&mut [CacheSlot]`), so the firmware decides
//! how much RAM the cache gets. Two slots are the minimum; eight (4 KiB) is
//! the planned size.
//!
//! Ordering rule (02 §6 "write safety"): before a dirty *directory* sector is
//! written, every dirty allocation (FAT/bitmap) and data sector is written
//! first. A power cut can then leave allocated clusters that no entry points
//! to (a leak), but never an entry that points at clusters still marked free.

use crate::Error;
use crate::block::{BlockDevice, SECTOR_SIZE, Sector};

/// What a cached sector holds; decides the write-back order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// FAT or allocation bitmap.
    Alloc = 1,
    /// File data (partial-sector writes).
    Data = 2,
    /// Directory entries, boot sector, FSINFO.
    Meta = 3,
}

const VALID: u8 = 0x80;
const DIRTY: u8 = 0x40;
const KIND_MASK: u8 = 0x0F;

/// One cache slot: a sector and its tag. 520 bytes.
#[derive(Clone)]
pub struct CacheSlot {
    data: Sector,
    lba: u32,
    stamp: u16,
    state: u8,
}

impl CacheSlot {
    /// An empty slot, for building the slot array in a `static`.
    pub const EMPTY: CacheSlot = CacheSlot { data: [0; SECTOR_SIZE], lba: 0, stamp: 0, state: 0 };

    fn kind(&self) -> u8 {
        self.state & KIND_MASK
    }
    fn dirty(&self) -> bool {
        self.state & (VALID | DIRTY) == VALID | DIRTY
    }
}

impl Default for CacheSlot {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Where FAT copies live, so a write-back of a FAT sector updates every copy.
#[derive(Clone, Copy, Default)]
pub(crate) struct Mirror {
    pub start: u32,
    pub len: u32,
    pub copies: u8,
}

pub(crate) struct Cache<'c> {
    slots: &'c mut [CacheSlot],
    clock: u16,
    pub mirror: Mirror,
    /// Statistics: device reads/writes issued for metadata.
    pub reads: u32,
    pub writes: u32,
}

impl<'c> Cache<'c> {
    pub fn new(slots: &'c mut [CacheSlot]) -> Self {
        for s in slots.iter_mut() {
            s.state = 0;
        }
        Cache { slots, clock: 0, mirror: Mirror::default(), reads: 0, writes: 0 }
    }

    /// The slots back (their contents are not flushed).
    pub fn into_slots(self) -> &'c mut [CacheSlot] {
        self.slots
    }

    fn find(&self, lba: u32) -> Option<usize> {
        self.slots.iter().position(|s| s.state & VALID != 0 && s.lba == lba)
    }

    fn touch(&mut self, i: usize) {
        self.clock = self.clock.wrapping_add(1);
        if self.clock == 0 {
            // renormalise on wrap so that recency stays comparable
            for s in self.slots.iter_mut() {
                s.stamp = 0;
            }
            self.clock = 1;
        }
        if let Some(s) = self.slots.get_mut(i) {
            s.stamp = self.clock;
        }
    }

    async fn write_slot<D: BlockDevice>(&mut self, dev: &mut D, i: usize) -> Result<(), Error<D::Error>> {
        let Some(s) = self.slots.get(i) else {
            return Ok(());
        };
        if !s.dirty() {
            return Ok(());
        }
        let lba = s.lba;
        let m = self.mirror;
        dev.write(lba, core::slice::from_ref(&s.data)).await.map_err(Error::Io)?;
        self.writes += 1;
        if m.copies > 1 && lba >= m.start && lba - m.start < m.len {
            for k in 1..u32::from(m.copies) {
                let Some(copy) = lba.checked_add(k * m.len) else {
                    break;
                };
                let s = &self.slots[i];
                dev.write(copy, core::slice::from_ref(&s.data)).await.map_err(Error::Io)?;
                self.writes += 1;
            }
        }
        if let Some(s) = self.slots.get_mut(i) {
            s.state &= !DIRTY;
        }
        Ok(())
    }

    /// Write back dirty slots of the given kinds (Alloc and Data first).
    pub async fn flush_kinds<D: BlockDevice>(
        &mut self,
        dev: &mut D,
        meta_too: bool,
    ) -> Result<(), Error<D::Error>> {
        for pass in [Kind::Alloc as u8, Kind::Data as u8, Kind::Meta as u8] {
            if pass == Kind::Meta as u8 && !meta_too {
                break;
            }
            // lowest LBA first within a pass
            loop {
                let mut best: Option<usize> = None;
                for (i, s) in self.slots.iter().enumerate() {
                    if s.dirty() && s.kind() == pass {
                        match best {
                            Some(b) if self.slots[b].lba <= s.lba => {}
                            _ => best = Some(i),
                        }
                    }
                }
                let Some(b) = best else { break };
                self.write_slot(dev, b).await?;
            }
        }
        Ok(())
    }

    pub async fn flush<D: BlockDevice>(&mut self, dev: &mut D) -> Result<(), Error<D::Error>> {
        self.flush_kinds(dev, true).await
    }

    /// Choose a victim slot and make it clean.
    async fn evict<D: BlockDevice>(&mut self, dev: &mut D) -> Result<usize, Error<D::Error>> {
        if self.slots.is_empty() {
            return Err(Error::BufferTooSmall);
        }
        let mut victim = 0usize;
        let mut best = u32::MAX;
        for (i, s) in self.slots.iter().enumerate() {
            if s.state & VALID == 0 {
                return Ok(i);
            }
            // prefer clean slots: dirty ones cost a write
            let age = u32::from(self.clock.wrapping_sub(s.stamp));
            let score = if s.dirty() { 0x1_0000 } else { 0 } + (0xFFFF - age.min(0xFFFF));
            if score < best {
                best = score;
                victim = i;
            }
        }
        if self.slots[victim].dirty() {
            if self.slots[victim].kind() == Kind::Meta as u8 {
                self.flush_kinds(dev, false).await?;
            }
            self.write_slot(dev, victim).await?;
        }
        self.slots[victim].state = 0;
        Ok(victim)
    }

    /// Get the slot holding `lba`, reading it if needed. With `fill_zero`
    /// the sector is not read but zero-filled (it is about to be overwritten
    /// or lies beyond any valid data).
    pub async fn load<D: BlockDevice>(
        &mut self,
        dev: &mut D,
        lba: u32,
        kind: Kind,
        fill_zero: bool,
    ) -> Result<usize, Error<D::Error>> {
        if let Some(i) = self.find(lba) {
            if fill_zero {
                // a stale copy (e.g. of a freed directory cluster) must not
                // leak into a fresh sector
                let s = &mut self.slots[i];
                s.data = [0; SECTOR_SIZE];
                s.state = (s.state & !KIND_MASK) | kind as u8;
            }
            self.touch(i);
            return Ok(i);
        }
        let i = self.evict(dev).await?;
        let s = &mut self.slots[i];
        if fill_zero {
            s.data = [0; SECTOR_SIZE];
        } else {
            dev.read(lba, core::slice::from_mut(&mut s.data)).await.map_err(Error::Io)?;
            self.reads += 1;
        }
        let s = &mut self.slots[i];
        s.lba = lba;
        s.state = VALID | kind as u8;
        self.touch(i);
        Ok(i)
    }

    pub fn data(&self, i: usize) -> &Sector {
        &self.slots[i].data
    }

    /// Mutable access; marks the slot dirty.
    pub fn data_mut(&mut self, i: usize) -> &mut Sector {
        let s = &mut self.slots[i];
        s.state |= DIRTY;
        &mut s.data
    }

    /// Before a direct read of `[lba, lba + n)`: write back dirty slots there.
    pub async fn flush_range<D: BlockDevice>(
        &mut self,
        dev: &mut D,
        lba: u32,
        n: u32,
    ) -> Result<(), Error<D::Error>> {
        for i in 0..self.slots.len() {
            let s = &self.slots[i];
            if s.dirty() && s.lba >= lba && s.lba - lba < n {
                if s.kind() == Kind::Meta as u8 {
                    self.flush_kinds(dev, false).await?;
                }
                self.write_slot(dev, i).await?;
            }
        }
        Ok(())
    }

    /// After a direct write of `[lba, lba + n)`: drop stale copies.
    pub fn invalidate_range(&mut self, lba: u32, n: u32) {
        for s in self.slots.iter_mut() {
            if s.state & VALID != 0 && s.lba >= lba && s.lba - lba < n {
                s.state = 0;
            }
        }
    }

    pub fn dirty_count(&self) -> usize {
        self.slots.iter().filter(|s| s.dirty()).count()
    }
}
