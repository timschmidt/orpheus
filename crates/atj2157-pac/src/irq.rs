//! Interrupt numbers (NVIC lines). \[DS\] §3.2.1 table 3-1; T0, USB, DMA5
//! \[HW\]. 64 lines, 4 priority levels (top two bits of each priority byte).
//! Every line beyond the \[HW\] ones is unproven until a driver uses it
//! (U-IRQ-1).

/// Watchdog in interrupt mode ("WD_2Hz").
pub const WATCHDOG: u16 = 1;
pub const TIMER0: u16 = 2;
pub const TIMER1: u16 = 3;
pub const TIMER2: u16 = 4;
pub const TIMER3: u16 = 5;
/// RTC alarm.
pub const RTC: u16 = 6;
pub const I2S_RX0: u16 = 7;
pub const I2S_RX1: u16 = 8;
pub const I2S_TX0: u16 = 10;
pub const I2S_TX1: u16 = 11;
pub const USB: u16 = 12;
pub const SPI0: u16 = 13;
pub const SPI1: u16 = 14;
pub const SPI2: u16 = 15;
pub const NAND: u16 = 16;
/// "SDC": the SD/MMC host.
pub const SD: u16 = 17;
pub const SDIO: u16 = 18;
pub const TWI0: u16 = 19;
pub const TWI1: u16 = 20;
pub const UART0: u16 = 21;
pub const UART1: u16 = 22;
pub const UART2: u16 = 23;
pub const UART3: u16 = 24;
/// DMA channel `n` (0..9) interrupts on line `DMA0 + n`.
pub const DMA0: u16 = 26;
/// GPIO external interrupt controller (GPIO INTC, datasheet §10.2.5).
pub const EXTIRQ: u16 = 36;
pub const DAC: u16 = 39;
pub const ADC01: u16 = 40;
pub const ADC23: u16 = 41;
pub const LCD: u16 = 42;
/// "MPU" as printed; meaning unclear (not the Cortex-M MemManage fault).
pub const MPU: u16 = 48;
/// Special LCD DMA channel (`SDMA`, `0xC0071000`). Datasheet ch. 5 names the
/// channel; system.md §6 guessed 36, which is EXTIRQ. \[DS\]
pub const SDMA: u16 = 50;

/// DMA channel `n`'s line.
#[must_use]
pub const fn dma(n: u16) -> u16 {
    DMA0 + n
}

/// Lines printed "NC" in the datasheet table: candidates for software-pended
/// interrupts (the RT executor, U-IRQ-2). Lines 0, 43, 45, 49 are printed
/// "-" and 51–63 are absent. \[DS\]
pub const UNUSED: [u16; 7] = [9, 25, 37, 38, 44, 46, 47];

/// Number of NVIC lines.
pub const COUNT: usize = 64;

/// Priority bits implemented (4 levels).
pub const PRIO_BITS: u8 = 2;

/// NVIC priority byte for level `l` (0 = highest, 3 = lowest).
#[must_use]
pub const fn prio(level: u8) -> u8 {
    level << (8 - PRIO_BITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_lines() {
        // Rockbox port header: ATJ_IRQ_USB, ATJ_IRQ_DMA (channel 5), ATJ_IRQ_TIMER0.
        assert_eq!(USB, 12);
        assert_eq!(dma(5), 31);
        assert_eq!(TIMER0, 2);
        assert_eq!(dma(9), 35);
        assert!(dma(9) < EXTIRQ);
        assert_eq!(prio(3), 0xC0);
        for l in UNUSED {
            assert!(![WATCHDOG, RTC, USB, EXTIRQ, DAC, LCD, MPU, SDMA].contains(&l));
            assert!(!(DMA0..=dma(9)).contains(&l));
        }
    }
}
