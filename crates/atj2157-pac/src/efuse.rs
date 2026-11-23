//! eFuse controller, `0xC0020000`. **READ-ONLY for yotonrs** (CLAUDE.md
//! rule 3): this module exposes only addresses and [`RoReg`] read accessors;
//! it has no [`crate::Reg`] and no write path. The memory map (datasheet
//! §3.4) prints the block as "-"; there is no register chapter. Everything
//! here is \[code\] from the ROM and kernel reader (system.md §5.5).
//!
//! Reading the fuses needs writes to the control registers and to PMU
//! registers (`VOUT_CTL` = [`crate::pmu::VENDOR_EFUSE_READ`], `PMU+0x18`) and
//! a key write; that sequence is documented below for reference only and is
//! not implemented without a user decision. The latched words read back
//! without the sequence may be stale (U-EFUSE-1).
//!
//! Sequence (vendor, \[code\]): `CLKEN1 |= EFUSE`; prepare PMU; `CTL1 &=
//! !0x200`, `&= !0x8000`; `CTL0 |= 4`, wait bit 0; write [`VENDOR_KEY`] to
//! `DATA0` until `CTL0` bit 1; read `DATA0 + 4n`; `CTL0 &= !4`; `CTL1 |=
//! 0x200`; restore PMU. Chip revision 3 also writes a password to `+0x08`
//! and uses per-word keys.

use crate::{Field, RoReg};

pub const BASE: usize = 0xC002_0000;

/// Control 0: bit 2 read enable, bit 0 ready, bit 1 key accepted. \[code\]
pub const CTL0: RoReg = RoReg::at(BASE);
/// Control 1: bits 9 and 15 cleared for a read, bit 21 password accepted
/// (rev 3). \[code\]
pub const CTL1: RoReg = RoReg::at(BASE + 0x04);
/// Password register (rev 3 only). Address only. \[code\]
pub const PASSWORD_ADDR: usize = BASE + 0x08;
/// Number of 32-bit fuse words.
pub const WORDS: usize = 8;

/// Fuse word `n` (0..7) as latched at `+0x0C + 4n`. \[code\]
#[must_use]
pub const fn data(n: usize) -> RoReg {
    RoReg::at(BASE + 0x0C + 4 * n)
}

/// Key the vendor writes to `+0x0C` to start a read (words 0–3); words 4–7
/// use [`VENDOR_KEY_HI`] on rev 3. \[code\]
pub const VENDOR_KEY: u32 = 0x004B_4559;
pub const VENDOR_KEY_HI: u32 = 0x0097_E38C;
/// Rev-3 password. \[code\]
pub const VENDOR_PASSWORD: u32 = 0x4143_5453;

/// Known word contents. \[code\] system.md §5.5, audio.md §1.4.
pub mod word {
    use super::Field;
    /// Word 0 `[31:29]`: IC type (0 = ATJ2157, 1/5 = ATJ2155, 2 = ATJ2159).
    pub const W0_IC_TYPE: Field = Field::bits(31, 29);
    /// Word 2 `[15:11]`: battery-ADC trim, bit 4 = subtract, `[3:0]` magnitude.
    pub const W2_BATADC_TRIM: Field = Field::bits(15, 11);
    /// Word 3: boot options read by the ROM (bit 2 ADFU after all sources
    /// failed, bit 3 ADFU immediately, bit 4 skip one boot source).
    pub const W3_ADFU_ON_FAIL: u32 = 1 << 2;
    pub const W3_ADFU_NOW: u32 = 1 << 3;
    pub const W3_SKIP_SOURCE: u32 = 1 << 4;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(CTL0.addr(), 0xC002_0000);
        assert_eq!(CTL1.addr(), 0xC002_0004);
        assert_eq!(data(0).addr(), 0xC002_000C);
        assert_eq!(data(WORDS - 1).addr(), 0xC002_0028);
        assert_eq!(word::W0_IC_TYPE.mask(), 0xE000_0000);
    }
}
