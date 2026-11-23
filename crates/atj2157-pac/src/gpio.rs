//! GPIO and multi-function pin control, `0xC01C0000` (datasheet §9.4).
//! Layout \[HW\]/\[code\] (01-hardware §5); bank registers per 32 pins.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC01C_0000;

/// Debug/SWD routing word (`[1:0]` pad set, bit 4 enable). \[code\]
pub const DEBUG_SEL: Reg = Reg::at(BASE);

/// Per-pin function word. \[HW\]
#[must_use]
pub const fn mfp(pin: usize) -> Reg {
    Reg::at(BASE + 0x04 + 4 * pin)
}
pub mod mfp_bits {
    use super::{Field, bit};
    pub const FUNC: Field = Field::bits(4, 0);
    pub const OUT_EN: u32 = bit(6);
    pub const IN_EN: u32 = bit(7);
    /// Pull-up/down and other bits `[11:8]`. \[inferred\]
    pub const PULL: Field = Field::bits(11, 8);
    pub const DRIVE: Field = Field::bits(14, 12);
    /// Function 0 = plain GPIO. \[inferred\] from vendor use.
    pub const FUNC_GPIO: u32 = 0;
}

/// Output latch for pins `32·bank..` (the port's header calls it OUTEN;
/// vendor use says output latch: U-GPIO-1). \[code\]
#[must_use]
pub const fn odat(bank: usize) -> Reg {
    Reg::at(BASE + 0x200 + 4 * bank)
}
/// Bit-set register for the output latch. \[code\]
#[must_use]
pub const fn bsr(bank: usize) -> Reg {
    Reg::at(BASE + 0x210 + 4 * bank)
}
/// Bit-reset register for the output latch. \[code\]
#[must_use]
pub const fn brr(bank: usize) -> Reg {
    Reg::at(BASE + 0x220 + 4 * bank)
}
/// Input data. \[code\]
#[must_use]
pub const fn idat(bank: usize) -> Reg {
    Reg::at(BASE + 0x230 + 4 * bank)
}

/// Pins used on the YM03 (board facts belong to `yoton-bsp`; the numbers
/// are repeated here only for the address arithmetic tests).
pub const PIN_BACKLIGHT: usize = 22;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(mfp(17).addr(), 0xC01C_0048);
        assert_eq!(odat(1).addr(), 0xC01C_0204);
        let _ = bit(0);
    }
}
