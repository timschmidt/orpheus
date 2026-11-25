//! A simulated 8 MiB NOR (host only): erase sets a 4 KiB sector to `0xFF`,
//! program only clears bits within one 256-byte page, as the real chip.
//! It can lose power: after `cut_after` operations the next one is torn
//! (a program clears only some of its bits, an erase leaves the sector
//! half erased) and every later one fails. Any erase or program touching
//! sector 0 or `0x7E0000` and above is counted in `forbidden` and refused,
//! so every test also proves the guards.

use std::vec::Vec;

use orpheus_proto::layout::{NOR_SIZE, PAGE, SECTOR, full};

use crate::install::Flash;
use crate::ofw::{HEADER_SIZE, Header, LOAD_ADDR, version_bytes};
use orpheus_proto::slot::{self, LAYOUT_FULL, SlotHeader};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct SimNor {
    pub data: Vec<u8>,
    /// Erase/program operations carried out.
    pub ops: usize,
    /// Lose power before operation number `cut_after` (0 = before the first).
    pub cut_after: Option<usize>,
    /// Seed for the torn operation's pattern.
    pub seed: u32,
    /// Power lost: everything fails.
    pub dead: bool,
    /// Writes refused because they touched sector 0 or the vendor data.
    pub forbidden: usize,
    /// Programs that would have needed an erase (set a 0 bit back to 1).
    pub unerased: usize,
}

impl Default for SimNor {
    fn default() -> Self {
        Self::new()
    }
}

/// Error of the simulated chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimError {
    PowerLost,
    Forbidden,
    Range,
}

impl SimNor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: std::vec![0xFF; NOR_SIZE as usize],
            ops: 0,
            cut_after: None,
            seed: 1,
            dead: false,
            forbidden: 0,
            unerased: 0,
        }
    }

    /// Put bytes in place without counting (test set-up).
    pub fn load(&mut self, addr: u32, bytes: &[u8]) {
        self.data[addr as usize..addr as usize + bytes.len()].copy_from_slice(bytes);
    }

    fn rand(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    /// Count an operation; `Ok(true)` = carry it out, `Ok(false)` = tear it.
    fn op(&mut self) -> Result<bool, SimError> {
        if self.dead {
            return Err(SimError::PowerLost);
        }
        if self.cut_after == Some(self.ops) {
            self.dead = true;
            return Ok(false);
        }
        self.ops += 1;
        Ok(true)
    }

    fn forbidden_range(addr: u32, len: u32) -> bool {
        addr < SECTOR || addr.saturating_add(len) > full::FREE_END
    }

    /// Restart after a power cut (the chip keeps its contents).
    pub fn power_on(&mut self) {
        self.dead = false;
        self.cut_after = None;
    }

    /// A read function over the contents (for `slots::stage2_pick`).
    pub fn reader(&self) -> impl FnMut(u32, &mut [u8]) -> bool + '_ {
        |a, buf| match self.data.get(a as usize..a as usize + buf.len()) {
            Some(s) => {
                buf.copy_from_slice(s);
                true
            }
            None => false,
        }
    }
}

impl Flash for SimNor {
    type Error = SimError;

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), SimError> {
        if self.dead {
            return Err(SimError::PowerLost);
        }
        let s = self.data.get(addr as usize..addr as usize + buf.len()).ok_or(SimError::Range)?;
        buf.copy_from_slice(s);
        Ok(())
    }

    fn erase_sector(&mut self, addr: u32) -> Result<(), SimError> {
        if Self::forbidden_range(addr, SECTOR) {
            self.forbidden += 1;
            return Err(SimError::Forbidden);
        }
        if !addr.is_multiple_of(SECTOR) {
            return Err(SimError::Range);
        }
        let full = self.op()?;
        let a = addr as usize;
        if full {
            self.data[a..a + SECTOR as usize].fill(0xFF);
        } else {
            // Torn: an unpredictable part of the sector erased.
            for i in 0..SECTOR as usize {
                if self.rand() % 3 != 0 {
                    self.data[a + i] = 0xFF;
                }
            }
            return Err(SimError::PowerLost);
        }
        Ok(())
    }

    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), SimError> {
        let len = data.len() as u32;
        if Self::forbidden_range(addr, len) {
            self.forbidden += 1;
            return Err(SimError::Forbidden);
        }
        if len == 0 || len > PAGE || (addr % PAGE) + len > PAGE {
            return Err(SimError::Range);
        }
        let full = self.op()?;
        let a = addr as usize;
        for (i, &b) in data.iter().enumerate() {
            let cur = self.data[a + i];
            if b & !cur != 0 {
                self.unerased += 1;
            }
            // Torn: only some of the bits that should clear do.
            let mask = if full { 0xFF } else { self.rand() as u8 };
            self.data[a + i] = cur & !(!b & mask);
        }
        if full { Ok(()) } else { Err(SimError::PowerLost) }
    }
}

/// A small but complete slot image: header sector, `res` bytes of
/// resident image, `pages` (≥ 1) 8 KiB pages; filled from `seed`.
#[must_use]
pub fn image(seed: u8, res: u32, pages: u32) -> Vec<u8> {
    let pages_off = (0x1000 + res).div_ceil(0x1000) * 0x1000;
    let len = pages_off + pages * slot::PAGE_SIZE;
    let mut img = std::vec![0xFFu8; len as usize];
    let mut x = u32::from(seed).wrapping_mul(2_654_435_761) | 1;
    for b in &mut img[0x1000..] {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        // Some erased runs, as real page stores have.
        *b = if x % 7 == 0 { 0xFF } else { x as u8 };
    }
    // Whole erased pages too (the installer skips programming them).
    let last = img.len() - 256;
    img[last..].fill(0xFF);
    let resident = &img[0x1000..(0x1000 + res) as usize];
    let mut build_id = [0u8; 16];
    build_id[0] = seed;
    let h = SlotHeader {
        layout: LAYOUT_FULL,
        resident_off: 0x1000,
        resident_len: res,
        load_addr: LOAD_ADDR,
        entry: LOAD_ADDR | 1,
        resident_sum: orpheus_proto::hash::sum32(resident),
        pages_off,
        code_pages: pages,
        rodata_pages: 0,
        pages_crc: 0,
        res_off: 0,
        res_len: 0,
        verified: 0xFFFF_FFFF,
        build_id,
    };
    img[..128].copy_from_slice(&h.to_bytes());
    img
}

/// The `.ofw` header for `img` (its build id, SHA-256 and length).
#[must_use]
pub fn ofw_header(img: &[u8], version: &str, changes: &str) -> [u8; HEADER_SIZE] {
    Header {
        layout: LAYOUT_FULL,
        image_len: img.len() as u32,
        changes_len: 0,
        version: version_bytes(version),
        build_id: SlotHeader::from_bytes(img).map(|h| h.build_id).unwrap_or([0; 16]),
        sha256: Sha256::digest(img).into(),
    }
    .to_bytes(changes)
}
