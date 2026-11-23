//! Reset management unit, `0xC0000000` (datasheet §4.4). Reset bits are
//! active low: `0` holds the block in reset.

use crate::{Reg, bit};

pub const BASE: usize = 0xC000_0000;

/// MRCR0 — module reset control 0. \[DS\] §4.4.3; bits 9/10/24 \[HW\].
pub const RST0: Reg = Reg::at(BASE);
/// MRCR1 — module reset control 1. \[DS\] §4.4.3.
pub const RST1: Reg = Reg::at(BASE + 0x04);

/// Bits of [`RST0`] (same positions as [`crate::cmu::CLKEN0`] in most cases).
pub mod rst0 {
    use super::bit;
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
    pub const LCD: u32 = bit(13);
    pub const USB: u32 = bit(15);
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
}

/// Bits of [`RST1`].
pub mod rst1 {
    use super::bit;
    pub const ADC: u32 = bit(0);
    pub const DAC: u32 = bit(2);
    pub const I2STX0: u32 = bit(3);
    pub const I2STX1: u32 = bit(4);
    pub const I2SRX0: u32 = bit(5);
    pub const I2SRX1: u32 = bit(6);
    /// DSP accelerator (not in the datasheet table). \[code\] audio.md §7.1.
    pub const DSP: u32 = bit(13);
}
