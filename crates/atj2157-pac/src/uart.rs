//! UART0..3 at `0xC00D0000 + 0x10000·n` (datasheet §7.4, memory map §3.4).
//!
//! Four identical blocks: 16-byte TX and RX FIFOs, 5–8 data bits, IRQ and DMA
//! modes, RTS/CTS auto flow control, clocked from HOSC (24 MHz). UART0 is the
//! vendor debug port (TX pad disabled in the shipped config), UART1 the
//! Bluetooth module (GPIO38 = UART1_TX, GPIO39 = UART1_RX, MFP function 10
//! \[DS\] §10.4.35–36).
//!
//! Sources: datasheet V1.2 \[DS\]; vendor use from system.md §7 and
//! bluetooth.md §2, re-checked in the kernel text and `BTENGINE.AP` \[code\].
//! The Rockbox port does not use the UARTs, so nothing here is \[HW\].
//!
//! Unknown: the IRQ lines (21–24 per the datasheet table) are unproven
//! (U-IRQ-1); the vendor never enables flow control (no RTS/CTS pins seen).
//! Clock gates `CLKEN0` b20–23, resets `RST0` b20–23 (`cmu`, `rmu`); DMA
//! selectors 1–4 (`dma`).

use crate::{Field, Reg, bit};

/// UART0 base.
pub const BASE: usize = 0xC00D_0000;
/// Number of UART blocks.
pub const COUNT: usize = 4;
/// UART clock = HOSC. \[DS\] §7.4.2
pub const CLOCK_HZ: u32 = 24_000_000;

/// Base address of UART`n`, n = 0..3. \[DS\] §7.4.4
#[must_use]
pub const fn base(n: usize) -> usize {
    BASE + 0x1_0000 * n
}

pub const CTL_OFFSET: usize = 0x00;
pub const RXDAT_OFFSET: usize = 0x04;
pub const TXDAT_OFFSET: usize = 0x08;
pub const STA_OFFSET: usize = 0x0C;
pub const BR_OFFSET: usize = 0x10;

/// UARTx_CTL. Reset 0. \[DS\] §7.4.5.1; vendor words below \[code\].
#[must_use]
pub const fn ctl(n: usize) -> Reg {
    Reg::at(base(n) + CTL_OFFSET)
}
/// UARTx_RXDAT: one byte `[7:0]` from the RX FIFO; DMA source for UART1 RX. \[DS\] §7.4.5.2
#[must_use]
pub const fn rxdat(n: usize) -> Reg {
    Reg::at(base(n) + RXDAT_OFFSET)
}
/// UARTx_TXDAT: one byte `[7:0]` into the TX FIFO; DMA destination for UART1 TX. \[DS\] §7.4.5.3
#[must_use]
pub const fn txdat(n: usize) -> Reg {
    Reg::at(base(n) + TXDAT_OFFSET)
}
/// UARTx_STA. Reset `0x0010_0422` (CTSS follows the pin). \[DS\] §7.4.5.4
#[must_use]
pub const fn sta(n: usize) -> Reg {
    Reg::at(base(n) + STA_OFFSET)
}
/// UARTx_BR: separate TX and RX baud divisors. Reset `0x0028_0028`. \[DS\] §7.4.5.5
#[must_use]
pub const fn br(n: usize) -> Reg {
    Reg::at(base(n) + BR_OFFSET)
}

pub mod ctl {
    use super::{Field, bit};
    /// Receiver on (1 = normal, 0 = disabled). \[DS\]
    pub const RXENABLE: u32 = bit(31);
    /// Transmitter on. \[DS\]
    pub const TXENABLE: u32 = bit(30);
    pub const TX_FIFO_EN: u32 = bit(29);
    pub const RX_FIFO_EN: u32 = bit(28);
    /// TX FIFO fed by 0 = CPU, 1 = DMA. \[DS\]
    pub const TX_FIFO_SEL: u32 = bit(26);
    /// RX FIFO drained by 0 = CPU, 1 = DMA. \[DS\] **Disagreement:**
    /// bluetooth.md §2 / system.md §7 guess "RX DMA request/IRQ enable"; the
    /// vendor clears it to read `RXDAT` by CPU, which fits the datasheet. \[code\]
    pub const RX_FIFO_SEL: u32 = bit(24);
    /// RX FIFO level that raises RTS in auto flow: 14, 12, 8, 4 bytes. \[DS\]
    pub const AFL: Field = Field::bits(23, 22);
    /// Loopback. \[DS\]
    pub const LBEN: u32 = bit(20);
    /// TX IRQ enable. \[DS\]
    pub const TXIE: u32 = bit(19);
    /// RX IRQ enable. \[DS\]
    pub const RXIE: u32 = bit(18);
    /// TX DMA request enable. \[DS\]; set for UART1 \[code\].
    pub const TXDE: u32 = bit(17);
    /// RX DMA request enable. \[DS\]; set for UART1 \[code\].
    pub const RXDE: u32 = bit(16);
    /// Module enable (clock on). \[DS\]
    pub const EN: u32 = bit(15);
    /// RTS asserted (no effect with `AFE`). \[DS\]
    pub const RTSE: u32 = bit(13);
    /// RTS/CTS auto flow control. \[DS\]
    pub const AFE: u32 = bit(12);
    /// RX IRQ/DRQ level: 0 = ≥ 1 byte, 1 = 4 (IRQ only), 2 = 8, 3 = 12. DMA
    /// burst mode needs ≥ 8; DMA single mode needs 0. \[DS\]
    pub const RDIC: Field = Field::bits(11, 10);
    /// TX IRQ/DRQ level (bytes free), encoding as `RDIC`. \[DS\]
    pub const TDIC: Field = Field::bits(9, 8);
    /// CTS enable. \[DS\]
    pub const CTSE: u32 = bit(7);
    /// Parity: bit 6 PEN, bit 5 stick, bit 4 even; see `PRS_*`. \[DS\]
    pub const PRS: Field = Field::bits(6, 4);
    /// Wake-up enable (UART0 only). \[DS\]
    pub const WUEN: u32 = bit(3);
    /// 0 = 1 stop bit, 1 = 2. \[DS\]
    pub const STPS: u32 = bit(2);
    /// Data bits: 0..3 = 5..8. \[DS\]
    pub const DWLS: Field = Field::bits(1, 0);

    pub const LEVEL_1: u32 = 0;
    pub const LEVEL_4: u32 = 1;
    pub const LEVEL_8: u32 = 2;
    pub const LEVEL_12: u32 = 3;
    pub const PRS_NONE: u32 = 0b000;
    pub const PRS_ODD: u32 = 0b100;
    /// Stick parity 1.
    pub const PRS_MARK: u32 = 0b101;
    pub const PRS_EVEN: u32 = 0b110;
    /// Stick parity 0.
    pub const PRS_SPACE: u32 = 0b111;
    pub const DWLS_8: u32 = 3;
}

pub mod sta {
    use super::{Field, bit};
    /// Woken (UART0 only). \[DS\]
    pub const WSTA: u32 = bit(24);
    /// Parity error; write 1 to clear. \[DS\]
    pub const PAER: u32 = bit(23);
    /// Stop-bit error; write 1 to clear. \[DS\]
    pub const STER: u32 = bit(22);
    /// TX busy: 0 = FIFO empty and shifter idle. \[DS\]; vendor waits for 0 \[code\].
    pub const UTBB: u32 = bit(21);
    /// Free TX FIFO entries (16 at reset). \[DS\]
    pub const TXFL: Field = Field::bits(20, 16);
    /// Bytes in the RX FIFO. \[DS\]
    pub const RXFL: Field = Field::bits(15, 11);
    /// TX FIFO empty. \[DS\]
    pub const TFES: u32 = bit(10);
    /// RX FIFO full. \[DS\]
    pub const RFFS: u32 = bit(9);
    /// RTS pin state. \[DS\]
    pub const RTSS: u32 = bit(8);
    /// CTS pin state. \[DS\]
    pub const CTSS: u32 = bit(7);
    /// TX FIFO full. \[DS\]
    pub const TFFU: u32 = bit(6);
    /// RX FIFO empty. \[DS\]; polled by the vendor \[code\].
    pub const RFEM: u32 = bit(5);
    /// Receive error; write 1 to clear. \[DS\], \[code\]
    pub const RXST: u32 = bit(4);
    /// TX FIFO error; write 1 to clear (vendor writes it around every TX DMA). \[DS\], \[code\]
    pub const TFER: u32 = bit(3);
    /// RX FIFO error; write 1 to clear. \[DS\], \[code\]
    pub const RXER: u32 = bit(2);
    /// TX IRQ pending (reset 1); write 1 to clear. \[DS\]
    pub const TIP: u32 = bit(1);
    /// RX IRQ pending; write 1 to clear. \[DS\]
    pub const RIP: u32 = bit(0);
}

pub mod br {
    use super::Field;
    /// TX divisor: baud = HOSC / divisor. \[DS\]
    pub const TXBRDIV: Field = Field::bits(31, 16);
    /// RX divisor, same formula. \[DS\]
    pub const RXBRDIV: Field = Field::bits(15, 0);
}

/// `BR` word for `baud` with the same divisor in both halves (rounded to the
/// nearest divisor), as the vendor writes it. \[DS\] formula.
#[must_use]
pub const fn br_value(baud: u32) -> u32 {
    let div = (CLOCK_HZ + baud / 2) / baud;
    br::TXBRDIV.val(div) | br::RXBRDIV.val(div)
}

/// Debug UART0 `CTL`: RX/TX on, both FIFOs on (CPU), enable, 8N1. Kernel text
/// `0x2196` (system.md §7). \[code\]
pub const VENDOR_CTL_DEBUG: u32 = 0xF000_8003;
/// Bluetooth UART1 `CTL`: as the debug word plus both FIFOs on DMA, both DRQ
/// enables, RX/TX levels 8 bytes (burst-8 DMA). `BTENGINE.AP` file `0x66A8`
/// (bluetooth.md §2.1). \[code\]
pub const VENDOR_CTL_BT: u32 = 0xF503_8A03;
/// `BR` for 115200 baud (divisor 208 = 115 385 baud, +0.16 %), debug and
/// Bluetooth. Kernel text `0x2180`, `BTENGINE.AP` `0x66A0`. \[code\]
/// **Disagreement:** bluetooth.md §2 gives `0x00D00150` and calls the low half
/// unexplained; both cited instructions encode `mov.w #0x00D000D0`
/// (system.md §7 is right): the halves are the TX and RX divisors.
pub const VENDOR_BR_115200: u32 = 0x00D0_00D0;
/// `BR` for 1 500 000 baud (divisor 16). bluetooth.md §2 (B68). \[code\]
pub const VENDOR_BR_1M5: u32 = 0x0010_0010;
/// Written to `STA` at init: clears RIP, TIP, RXER, TFER, RXST. \[code\]
pub const VENDOR_STA_CLEAR: u32 = 0x1F;
/// Written to `STA` after draining RX by CPU: clears RXST and RXER. \[code\]
pub const VENDOR_STA_RX_ERR_CLEAR: u32 = 0x14;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn vendor_addresses() {
        // system.md §7, bluetooth.md §2 [code]: UART1 DMA source/destination and BAUD.
        assert_eq!(base(0), 0xC00D_0000);
        assert_eq!(base(1), 0xC00E_0000);
        assert_eq!(rxdat(1).addr(), 0xC00E_0004);
        assert_eq!(txdat(1).addr(), 0xC00E_0008);
        assert_eq!(sta(1).addr(), 0xC00E_000C);
        assert_eq!(br(1).addr(), 0xC00E_0010);
    }

    #[test]
    fn datasheet_addresses() {
        assert_eq!(base(2), 0xC00F_0000);
        assert_eq!(base(3), 0xC010_0000);
        assert_eq!(ctl(3).addr(), 0xC010_0000);
        assert_eq!(br(3).addr(), 0xC010_0010);
        for n in 0..COUNT {
            assert_increasing(&[
                ctl(n).addr(),
                rxdat(n).addr(),
                txdat(n).addr(),
                sta(n).addr(),
                br(n).addr(),
            ]);
        }
    }

    #[test]
    fn fields_disjoint() {
        use ctl::*;
        assert_disjoint(&[
            RXENABLE,
            TXENABLE,
            TX_FIFO_EN,
            RX_FIFO_EN,
            TX_FIFO_SEL,
            RX_FIFO_SEL,
            AFL.mask(),
            LBEN,
            TXIE,
            RXIE,
            TXDE,
            RXDE,
            EN,
            RTSE,
            AFE,
            RDIC.mask(),
            TDIC.mask(),
            CTSE,
            PRS.mask(),
            WUEN,
            STPS,
            DWLS.mask(),
        ]);
        use sta::*;
        assert_disjoint(&[
            WSTA,
            PAER,
            STER,
            UTBB,
            TXFL.mask(),
            RXFL.mask(),
            TFES,
            RFFS,
            RTSS,
            CTSS,
            TFFU,
            RFEM,
            RXST,
            TFER,
            RXER,
            TIP,
            RIP,
        ]);
        assert_disjoint(&[br::TXBRDIV.mask(), br::RXBRDIV.mask()]);
    }

    #[test]
    fn vendor_words_decode() {
        use ctl::*;
        let base_word = RXENABLE | TXENABLE | TX_FIFO_EN | RX_FIFO_EN | EN | DWLS.val(DWLS_8);
        assert_eq!(VENDOR_CTL_DEBUG, base_word);
        assert_eq!(
            VENDOR_CTL_BT,
            base_word | TX_FIFO_SEL | RX_FIFO_SEL | TXDE | RXDE | RDIC.val(LEVEL_8) | TDIC.val(LEVEL_8)
        );
        assert_eq!(br_value(115_200), VENDOR_BR_115200);
        assert_eq!(br_value(1_500_000), VENDOR_BR_1M5);
        assert_eq!(br::TXBRDIV.get(0x0028_0028), 40); // reset: 600 kbaud
        assert_eq!(VENDOR_STA_CLEAR, sta::RIP | sta::TIP | sta::RXER | sta::TFER | sta::RXST);
        assert_eq!(VENDOR_STA_RX_ERR_CLEAR, sta::RXST | sta::RXER);
        assert_eq!(sta::UTBB, 0x0020_0000);
        assert_eq!(sta::RFEM, 0x20);
    }
}
