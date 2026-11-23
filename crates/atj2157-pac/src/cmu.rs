//! Clock management unit, `0xC0001000` (datasheet §4.3), and the oscillator
//! / PLL block at `0xC0000100` (datasheet §4.2).

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC000_1000;

/// CMU_SYSCLK. `[1:0]` source (0 32k, 1 HOSC, 2 COREPLL, 3 DEVPLL), `[5:4]`
/// divider. ROM state `0x03000002` \[HW\]; field meanings \[code\] system.md §1.
pub const SYSCLK: Reg = Reg::at(BASE);
pub mod sysclk {
    use super::Field;
    pub const SRC: Field = Field::bits(1, 0);
    pub const DIV: Field = Field::bits(5, 4);
    pub const SRC_32K: u32 = 0;
    pub const SRC_HOSC: u32 = 1;
    pub const SRC_COREPLL: u32 = 2;
    pub const SRC_DEVPLL: u32 = 3;
}

/// CMU_DEVCLKEN0 — device clock gates. \[DS\] §4.3.3.1.
pub const CLKEN0: Reg = Reg::at(BASE + 0x08);
/// CMU_DEVCLKEN1. \[DS\] §4.3.3.2.
pub const CLKEN1: Reg = Reg::at(BASE + 0x0C);

pub mod clken0 {
    use super::bit;
    pub const DMA: u32 = bit(0);
    pub const NAND: u32 = bit(1);
    pub const SD0: u32 = bit(2);
    pub const SD1: u32 = bit(3);
    pub const SPI0: u32 = bit(6);
    pub const SPI1: u32 = bit(7);
    pub const SPI2: u32 = bit(8);
    /// SPI0 cache controller = SRAM2 clock. \[DS\]; crt0 sets it \[HW\].
    pub const SPI0_CACHE: u32 = bit(9);
    /// SPI1 cache controller = SRAM3 clock. \[DS\]; crt0 sets it \[HW\].
    pub const SPI1_CACHE: u32 = bit(10);
    pub const LCD: u32 = bit(13);
    pub const SEC: u32 = bit(14);
    pub const USB: u32 = bit(15);
    pub const TWI0: u32 = bit(18);
    pub const TWI1: u32 = bit(19);
    pub const UART0: u32 = bit(20);
    pub const UART1: u32 = bit(21);
    pub const UART2: u32 = bit(22);
    pub const UART3: u32 = bit(23);
    pub const TIMER: u32 = bit(24);
    pub const PWM: u32 = bit(25);
    /// Reset default 1. \[DS\]
    pub const LRADC: u32 = bit(27);
}

pub mod clken1 {
    use super::bit;
    pub const ADC01: u32 = bit(0);
    pub const ADC23: u32 = bit(1);
    pub const DAC: u32 = bit(2);
    pub const I2STX0_MCLK: u32 = bit(3);
    pub const I2STX1_MCLK: u32 = bit(4);
    pub const I2SRX0_MCLK: u32 = bit(5);
    pub const I2SRX1_MCLK: u32 = bit(6);
    pub const I2SRX2_MCLK: u32 = bit(7);
    pub const I2SSRD: u32 = bit(8);
    pub const DAC_ANALOG: u32 = bit(11);
    /// DSP accelerator. \[code\] audio.md §7.1.
    pub const DSP: u32 = bit(13);
    /// eFuse controller (read-only for yotonrs). \[code\] system.md §5.5.
    pub const EFUSE: u32 = bit(31);
}

/// CMU_TIMERnCLK, n = 0..3: `[1:0]` 0 = CK24M, 1 = CK24M/24 (1 MHz). \[DS\] §4.3.3.9.
#[must_use]
pub const fn timer_clk(n: usize) -> Reg {
    Reg::at(BASE + 0x40 + 4 * n)
}
pub const TIMERCLK_24M: u32 = 0;
pub const TIMERCLK_1M: u32 = 1;

/// CMU_PWMnCLK, n = 0..8. \[DS\] §4.3.3.13.
#[must_use]
pub const fn pwm_clk(n: usize) -> Reg {
    Reg::at(BASE + 0x50 + 4 * n)
}

/// Oscillator / PLL block. \[code\] system.md §1.
pub const OSC_BASE: usize = 0xC000_0100;
/// HOSC control; bit 0 set by the port's timer start-up (`|= 1`). \[HW\]
pub const HOSC_CTL: Reg = Reg::at(OSC_BASE + 0x04);
/// COREPLL: `0x88` = 48 MHz as the ROM sets it (6 MHz × 8, enable bit 7). \[HW\]
pub const COREPLL_CTL: Reg = Reg::at(OSC_BASE + 0x14);
pub mod corepll {
    use super::{Field, bit};
    pub const EN: u32 = bit(7);
    /// Multiplier N: f = 6 MHz × N. \[code\] system.md §1.
    pub const N: Field = Field::bits(6, 0);
}
