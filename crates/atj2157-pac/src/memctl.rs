//! Memory controller, `0xC0040000`: SRAM2/3 windows, bus-master enables and
//! bank-tag registers. The datasheet lists the block in the memory map (§3.4)
//! but has no register chapter. Values \[HW\] (crt0 of the Rockbox port);
//! meanings \[inferred\] system.md §10.
//!
//! The SDK header of the sister chip ATJ2127 names a "MemoryController" with
//! ROM-patch and page-miss registers at a different base (`0xC00A0000`);
//! nothing there matches this block's use, so it is not used as a source.
//!
//! Unknown: bank-tag window sizes and count (U-MEM-1).

use crate::Reg;

pub const BASE: usize = 0xC004_0000;

/// Bus-master memory enables; the ROM-state recreation sets `|= 0xFF`, the
/// vendor UDISK `|= 0x7F`. Needed before USB DMA works. \[HW\]
pub const MASTER_EN: Reg = Reg::at(BASE + 0x04);
pub const MASTER_EN_ALL: u32 = 0xFF;
/// SRAM2 window start/end (`0x200000` / `0x207FFF`). \[HW\]
pub const SRAM2_START: Reg = Reg::at(BASE + 0x10);
pub const SRAM2_END: Reg = Reg::at(BASE + 0x14);
/// SRAM3 window start/end (`0x300000` / `0x307FFF`). \[HW\]
pub const SRAM3_START: Reg = Reg::at(BASE + 0x18);
pub const SRAM3_END: Reg = Reg::at(BASE + 0x1C);
/// Second pair of window registers (`0x300` block): base, 0 (purpose
/// \[inferred\]: DMA alias base). \[HW\]
pub const SRAM2_MAP: Reg = Reg::at(BASE + 0x300);
pub const SRAM2_MAP_HI: Reg = Reg::at(BASE + 0x304);
pub const SRAM3_MAP: Reg = Reg::at(BASE + 0x308);
pub const SRAM3_MAP_HI: Reg = Reg::at(BASE + 0x30C);

/// Values crt0 writes (01-hardware §3). \[HW\]
pub const SRAM2_WINDOW: (u32, u32) = (0x0020_0000, 0x0020_7FFF);
pub const SRAM3_WINDOW: (u32, u32) = (0x0030_0000, 0x0030_7FFF);

/// Bank-tag registers `+0x80 + 4n`, n = 1..17 (vendor virtual windows of
/// 128 KiB; unused by orpheus). \[code\] system.md §10.
#[must_use]
pub const fn bank_tag(n: usize) -> Reg {
    Reg::at(BASE + 0x80 + 4 * n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt0_addresses() {
        assert_eq!(MASTER_EN.addr(), 0xC004_0004);
        assert_eq!(SRAM2_START.addr(), 0xC004_0010);
        assert_eq!(SRAM3_END.addr(), 0xC004_001C);
        assert_eq!(SRAM2_MAP.addr(), 0xC004_0300);
        assert_eq!(SRAM3_MAP_HI.addr(), 0xC004_030C);
        assert_eq!(bank_tag(17).addr(), 0xC004_00C4);
    }
}
