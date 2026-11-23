//! Reset management unit, `0xC0000000` (datasheet §4.4). Reset bits are
//! active low: `0` holds the block in reset, `1` releases it; all reset to 0.
//! A reset pulse is "clear, short delay, set" (system.md §2).
//!
//! Bit positions in [`RST0`] match [`crate::cmu::CLKEN0`] for every block
//! that has both.

use crate::{Reg, bit};

pub const BASE: usize = 0xC000_0000;

/// MRCR0 — module reset control 0. \[DS\] §4.4.3.1; bits 0, 2, 6, 9, 10, 13,
/// 15, 16, 24, 25 used by the port \[HW\].
pub const RST0: Reg = Reg::at(BASE);
/// MRCR1 — module reset control 1 (audio). \[DS\] §4.4.3.2; bits 0, 2 \[HW\].
pub const RST1: Reg = Reg::at(BASE + 0x04);

/// Chip revision ("ictype"), read by the vendor kernel; revision 3 takes a
/// different eFuse path. Not in the datasheet. Read-only use. \[code\]
/// system.md §1.1.
pub const IC_REV: Reg = Reg::at(BASE + 0xA0);

/// Bits of [`RST0`]. \[DS\] §4.4.3.1 unless noted.
pub mod rst0 {
    use super::bit;
    /// DMA controller; also pulsed by the vendor with SD, LCD and DSP init.
    pub const DMA: u32 = bit(0);
    pub const NAND: u32 = bit(1);
    pub const SD0: u32 = bit(2);
    pub const SD1: u32 = bit(3);
    pub const SPI0: u32 = bit(6);
    pub const SPI1: u32 = bit(7);
    pub const SPI2: u32 = bit(8);
    /// SPI0 cache controller. Holds SRAM2 \[DS\]; released by crt0 \[HW\].
    pub const SPI0_CACHE: u32 = bit(9);
    /// SPI1 cache controller. Holds SRAM3 \[DS\]; released by crt0 \[HW\].
    pub const SPI1_CACHE: u32 = bit(10);
    /// Image/video block (datasheet: unnamed R/W bit). \[code\] system.md §2.
    pub const IMG: u32 = bit(12);
    /// The datasheet's description says "clock enable" (copy error); it is
    /// the LCD controller reset. \[DS\]; \[HW\].
    pub const LCD: u32 = bit(13);
    pub const USB: u32 = bit(15);
    /// Second USB reset (`USBRESET2`). \[DS\]; \[HW\].
    pub const USB2: u32 = bit(16);
    pub const TWI0: u32 = bit(18);
    pub const TWI1: u32 = bit(19);
    pub const UART0: u32 = bit(20);
    pub const UART1: u32 = bit(21);
    pub const UART2: u32 = bit(22);
    pub const UART3: u32 = bit(23);
    /// All four timers. \[HW\] (port `timer_hw_init`).
    pub const TIMER: u32 = bit(24);
    pub const PWM: u32 = bit(25);
    /// SRAM2 + SRAM3 bring-up pulse of crt0 (01-hardware §3). \[HW\]
    pub const SRAM23: u32 = SPI0_CACHE | SPI1_CACHE;
}

/// Bits of [`RST1`]. \[DS\] §4.4.3.2 unless noted.
pub mod rst1 {
    use super::bit;
    /// ADC (`ADCRESET`, both pairs).
    pub const ADC: u32 = bit(0);
    pub const DAC: u32 = bit(2);
    pub const I2STX0: u32 = bit(3);
    pub const I2STX1: u32 = bit(4);
    pub const I2SRX0: u32 = bit(5);
    pub const I2SRX1: u32 = bit(6);
    /// DSP accelerator (datasheet: unnamed R/W bit). \[code\] audio.md §7.1.
    pub const DSP: u32 = bit(13);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::assert_disjoint;

    #[test]
    fn addresses() {
        assert_eq!(RST0.addr(), 0xC000_0000);
        assert_eq!(RST1.addr(), 0xC000_0004);
        assert_eq!(rst0::SRAM23, 0x600);
        let _ = bit(0);
    }

    #[test]
    fn same_positions_as_clock_gates() {
        use crate::cmu::clken0;
        assert_eq!(rst0::SD0, clken0::SD0);
        assert_eq!(rst0::SPI0, clken0::SPI0);
        assert_eq!(rst0::SPI0_CACHE, clken0::SPI0_CACHE);
        assert_eq!(rst0::LCD, clken0::LCD);
        assert_eq!(rst0::USB, clken0::USB);
        assert_eq!(rst0::TIMER, clken0::TIMER);
        assert_eq!(rst0::PWM, clken0::PWM);
        assert_eq!(rst1::DSP, crate::cmu::clken1::DSP);
    }

    #[test]
    fn disjoint() {
        use rst0::*;
        assert_disjoint(&[
            DMA, NAND, SD0, SD1, SPI0, SPI1, SPI2, SPI0_CACHE, SPI1_CACHE, IMG, LCD, USB, USB2, TWI0, TWI1,
            UART0, UART1, UART2, UART3, TIMER, PWM,
        ]);
        assert_disjoint(&[
            rst1::ADC,
            rst1::DAC,
            rst1::I2STX0,
            rst1::I2STX1,
            rst1::I2SRX0,
            rst1::I2SRX1,
            rst1::DSP,
        ]);
    }
}
