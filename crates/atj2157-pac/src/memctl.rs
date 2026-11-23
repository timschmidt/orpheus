//! Memory controller, `0xC0040000`: SRAM2/3 windows and bus-master enables.
//! Values \[HW\] (crt0 of the Rockbox port); meanings \[inferred\] system.md §10.

use crate::Reg;

pub const BASE: usize = 0xC004_0000;

/// Bus-master memory enables; the ROM-state recreation sets `|= 0xFF`. \[HW\]
pub const MASTER_EN: Reg = Reg::at(BASE + 0x04);
/// SRAM2 window start/end (`0x200000` / `0x207FFF`). \[HW\]
pub const SRAM2_START: Reg = Reg::at(BASE + 0x10);
pub const SRAM2_END: Reg = Reg::at(BASE + 0x14);
/// SRAM3 window start/end (`0x300000` / `0x307FFF`). \[HW\]
pub const SRAM3_START: Reg = Reg::at(BASE + 0x18);
pub const SRAM3_END: Reg = Reg::at(BASE + 0x1C);
/// Second pair of window registers (`0x300` block): base, 0. \[HW\]
pub const SRAM2_MAP: Reg = Reg::at(BASE + 0x300);
pub const SRAM2_MAP_HI: Reg = Reg::at(BASE + 0x304);
pub const SRAM3_MAP: Reg = Reg::at(BASE + 0x308);
pub const SRAM3_MAP_HI: Reg = Reg::at(BASE + 0x30C);
/// Bank-tag registers `+0x80 + 4n` (vendor virtual windows; unused). \[code\]
#[must_use]
pub const fn bank_tag(n: usize) -> Reg {
    Reg::at(BASE + 0x80 + 4 * n)
}
