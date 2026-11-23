//! Firmware slot header `"YFW1"` (docs/plan/02-architecture.md §2.3).
//!
//! The first 4 KiB sector of a slot. Stage 2 reads the first 64 bytes; the
//! rest is for the firmware and the host.
//!
//! | Offset | Field |
//! |---|---|
//! | `+0x00` | magic `"YFW1"` |
//! | `+0x04` | format version (1) |
//! | `+0x08` | layout id (1 = dual, 2 = full) |
//! | `+0x0C` | header crc32 over `0x00..0x7C` with this field … see [`HEADER_CRC_SPAN`] |
//! | `+0x10` | resident: offset in slot |
//! | `+0x14` | resident: length |
//! | `+0x18` | resident: load address |
//! | `+0x1C` | resident: entry (Thumb bit set) |
//! | `+0x20` | resident: sum32 |
//! | `+0x24` | pages: offset in slot |
//! | `+0x28` | pages: code page count |
//! | `+0x2C` | pages: rodata page count |
//! | `+0x30` | pages: crc32 |
//! | `+0x34` | resources: directory offset |
//! | `+0x38` | resources: directory length |
//! | `+0x3C` | verified: `0xFFFF_FFFF` until the firmware checked the pages, then 0 |
//! | `+0x40` | build id (16 bytes) |
//! | `+0x50..0x7C` | reserved (0) |
//! | `+0x7C` | header crc32 over `0x00..0x7C` with `verified` (+0x3C) read as `0xFFFF_FFFF` |

use crate::hash::crc32_update;
use crate::{magic, rd32, wr32};

pub const MAGIC: u32 = magic(b"YFW1");
pub const VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 0x80;
pub const HEADER_CRC_SPAN: usize = 0x7C;
pub const PAGE_SIZE: u32 = 8 * 1024;

pub const LAYOUT_DUAL: u32 = 1;
pub const LAYOUT_FULL: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotHeader {
    pub layout: u32,
    pub resident_off: u32,
    pub resident_len: u32,
    pub load_addr: u32,
    pub entry: u32,
    pub resident_sum: u32,
    pub pages_off: u32,
    pub code_pages: u32,
    pub rodata_pages: u32,
    pub pages_crc: u32,
    pub res_off: u32,
    pub res_len: u32,
    pub verified: u32,
    pub build_id: [u8; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    Short,
    Magic,
    Version,
    Crc,
}

fn header_crc(b: &[u8; HEADER_SIZE]) -> u32 {
    let mut tmp = [0u8; HEADER_CRC_SPAN];
    tmp.copy_from_slice(&b[..HEADER_CRC_SPAN]);
    wr32(&mut tmp, 0x3C, 0xFFFF_FFFF);
    crc32_update(0xFFFF_FFFF, &tmp) ^ 0xFFFF_FFFF
}

impl SlotHeader {
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        let w = [
            (0x00, MAGIC),
            (0x04, VERSION),
            (0x08, self.layout),
            (0x10, self.resident_off),
            (0x14, self.resident_len),
            (0x18, self.load_addr),
            (0x1C, self.entry),
            (0x20, self.resident_sum),
            (0x24, self.pages_off),
            (0x28, self.code_pages),
            (0x2C, self.rodata_pages),
            (0x30, self.pages_crc),
            (0x34, self.res_off),
            (0x38, self.res_len),
            (0x3C, self.verified),
        ];
        for (o, v) in w {
            wr32(&mut b, o, v);
        }
        b[0x40..0x50].copy_from_slice(&self.build_id);
        let crc = header_crc(&b);
        wr32(&mut b, HEADER_CRC_SPAN, crc);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, HeaderError> {
        let b: &[u8; HEADER_SIZE] =
            b.get(..HEADER_SIZE).and_then(|s| s.try_into().ok()).ok_or(HeaderError::Short)?;
        let r = |o| rd32(b, o).unwrap_or(0);
        if r(0) != MAGIC {
            return Err(HeaderError::Magic);
        }
        if r(4) != VERSION {
            return Err(HeaderError::Version);
        }
        if r(HEADER_CRC_SPAN) != header_crc(b) {
            return Err(HeaderError::Crc);
        }
        let mut build_id = [0u8; 16];
        build_id.copy_from_slice(&b[0x40..0x50]);
        Ok(Self {
            layout: r(0x08),
            resident_off: r(0x10),
            resident_len: r(0x14),
            load_addr: r(0x18),
            entry: r(0x1C),
            resident_sum: r(0x20),
            pages_off: r(0x24),
            code_pages: r(0x28),
            rodata_pages: r(0x2C),
            pages_crc: r(0x30),
            res_off: r(0x34),
            res_len: r(0x38),
            verified: r(0x3C),
            build_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_verified_flag_outside_crc() {
        let h = SlotHeader {
            layout: LAYOUT_DUAL,
            resident_off: 0x1000,
            resident_len: 0x8000,
            load_addr: 0x100000,
            entry: 0x100001,
            verified: 0xFFFF_FFFF,
            ..Default::default()
        };
        let mut b = h.to_bytes();
        assert_eq!(SlotHeader::from_bytes(&b), Ok(h));
        // The firmware clears `verified` in place without rewriting the CRC.
        wr32(&mut b, 0x3C, 0);
        assert_eq!(SlotHeader::from_bytes(&b).map(|x| x.verified), Ok(0));
        b[0x14] ^= 1;
        assert_eq!(SlotHeader::from_bytes(&b), Err(HeaderError::Crc));
    }
}
