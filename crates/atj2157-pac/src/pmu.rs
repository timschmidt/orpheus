//! PMU and the low-rate ADCs, `0xC0010000` (datasheet §4.1, §9.3).
//!
//! **Regulator and charger fields are dangerous** (a lithium cell is
//! attached): only values the stock firmware uses may be written
//! (CLAUDE.md rule 3; docs/hw/system.md §3). This module defines addresses
//! and read-side fields; writers live in the HAL's `pmu` driver with each
//! value's provenance.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC001_0000;

/// VOUT_CTL. \[DS\] §4.1.4.1. The ROM-state recreation sets USBVCC_EN then
/// USBVDD_EN \[HW\].
pub const VOUT_CTL: Reg = Reg::at(BASE);
pub mod vout {
    use super::{Field, bit};
    pub const USBVCC_EN: u32 = bit(29);
    pub const USBVDD_EN: u32 = bit(28);
    pub const AVDD_EN: u32 = bit(22);
    pub const AVDD_VOL: Field = Field::bits(21, 20);
    pub const AVCC_SET: Field = Field::bits(19, 17);
    pub const VCC_SET: Field = Field::bits(12, 10);
    /// VDD in S3 standby. \[DS\]
    pub const VDD_SET_S3: Field = Field::bits(9, 5);
    /// Core VDD. Codes in 01-hardware §7 (0x0C = 1.0 V … 0x18 = 1.3 V).
    pub const VDD_SET: Field = Field::bits(4, 0);
}

pub const CHG_CTL: Reg = Reg::at(BASE + 0x10);
pub const CHG_DET: Reg = Reg::at(BASE + 0x14);
pub const SYSTEM_SET: Reg = Reg::at(BASE + 0x1C);
pub const POWER_CTL: Reg = Reg::at(BASE + 0x20);
pub const WKEN_CTL: Reg = Reg::at(BASE + 0x24);
pub const WAKE_PD: Reg = Reg::at(BASE + 0x28);
/// ONOFF_CTL: power key. Bit 1 = short-press pending (W1C) \[HW\].
pub const ONOFF_CTL: Reg = Reg::at(BASE + 0x2C);
pub mod onoff {
    use super::bit;
    pub const SHORT_PD: u32 = bit(1);
}

/// PMUADC_CTL: channel enables of the low-rate ADC. \[DS\] §9.3.4.1
pub const PMUADC_CTL: Reg = Reg::at(BASE + 0x38);
pub mod pmuadc {
    use super::bit;
    /// LRADCn enable bits (n = 1..10). \[inferred\]: position not yet
    /// checked against §9.3.4.1 (WP-HAL-0).
    #[must_use]
    pub const fn lradc_en(n: u8) -> u32 {
        bit(3 + n)
    }
    pub const BAT_EN: u32 = bit(1);
}
pub const BATADC_DATA: Reg = Reg::at(BASE + 0x3C);
pub const DC5VADC_DATA: Reg = Reg::at(BASE + 0x40);
pub const SENSADC_DATA: Reg = Reg::at(BASE + 0x44);
pub const AVCCADC_DATA: Reg = Reg::at(BASE + 0x48);
/// LRADCn_DATA, n = 1..10, `[9:0]`. Key ladder 1 = LRADC1 (`+0x50`), ladder 2
/// = LRADC6 (`+0x64`). \[HW\]
#[must_use]
pub const fn lradc_data(n: usize) -> Reg {
    Reg::at(BASE + 0x50 + 4 * (n - 1))
}
pub const ADC_DATA: Field = Field::bits(9, 0);
pub const SVCCADC_DATA: Reg = Reg::at(BASE + 0x7C);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladders() {
        assert_eq!(lradc_data(1).addr(), 0xC001_0050);
        assert_eq!(lradc_data(6).addr(), 0xC001_0064);
    }
}
