//! I2S transmitters TX0/TX1 at `0xC0182000 + 0x100·n` and receivers RX0/RX1
//! at `0xC0183000 + 0x100·n` (datasheet §8.3).
//!
//! Sources: datasheet §8.3 (layout); docs/hw/audio.md §5.3–5.4 (vendor driver,
//! never called by the stock firmware) \[code\]. The port does not use I2S.
//! On the YM03 the vendor's pin groups (GPIO18–25) are taken by LCD reset,
//! backlight and I2C, so external I2S needs another pin option (audio.md Q8).
//!
//! DMA: TX FIFO data `0xC018200C` / `0xC018210C` (device codes `0x0D`/`0x0E`),
//! RX `0xC018300C` / `0xC018310C`. Interrupts: RX0/RX1 = 7/8, TX0/TX1 = 10/11.
//! Clocks: `cmu::I2STX0CLK` … `I2SRX1CLK`, gates CLKEN1 b3–b6.
//!
//! Internal routes useful without pins \[DS\]: TX0 can take its data from the
//! DAC FIFOs (`TX FIFOCTL.FIFO_SEL`, `dac::digctl::MULT_DEVICE`), and
//! [`TX_MODE`] loops TXn back into RXn — a candidate digital DAC → capture
//! self-test path (U-AUD-6), untested.

use crate::{Field, Reg, bit};

pub const TX_BASE: usize = 0xC018_2000;
pub const RX_BASE: usize = 0xC018_3000;
/// Instances of each direction.
pub const COUNT: usize = 2;
/// Distance between instances.
pub const STRIDE: usize = 0x100;

#[must_use]
pub const fn tx_base(n: usize) -> usize {
    TX_BASE + STRIDE * n
}
#[must_use]
pub const fn rx_base(n: usize) -> usize {
    RX_BASE + STRIDE * n
}

pub const CTL_OFFSET: usize = 0x00;
pub const FIFOCTL_OFFSET: usize = 0x04;
pub const FIFOSTA_OFFSET: usize = 0x08;
pub const DAT_FIFO_OFFSET: usize = 0x0C;
pub const SRDCTL_OFFSET: usize = 0x10;
pub const SRDSTA_OFFSET: usize = 0x14;
/// TX only.
pub const FIFO_CNT_OFFSET: usize = 0x18;

/// I2STXn_CTL. \[DS\] §8.3.3.1/8. Reset `0x60` (fields in [`txctl`]).
#[must_use]
pub const fn tx_ctl(n: usize) -> Reg {
    Reg::at(tx_base(n) + CTL_OFFSET)
}
/// I2STXn_FIFOCTL. \[DS\] §8.3.3.2/9. Reset 0 (fields in [`txfifoctl`]).
#[must_use]
pub const fn tx_fifoctl(n: usize) -> Reg {
    Reg::at(tx_base(n) + FIFOCTL_OFFSET)
}
/// I2STXn_FIFOSTA. \[DS\] §8.3.3.3/10 (fields in [`txfifosta`]).
#[must_use]
pub const fn tx_fifosta(n: usize) -> Reg {
    Reg::at(tx_base(n) + FIFOSTA_OFFSET)
}
/// I2STXn_DAT_FIFO (write-only), 24-bit data in `[31:8]` ([`dat`]). DMA
/// destination \[DS\] §8.3.3.4/11; vendor address \[code\].
#[must_use]
pub const fn tx_dat_fifo(n: usize) -> Reg {
    Reg::at(tx_base(n) + DAT_FIFO_OFFSET)
}
/// I2STXn_SRDCTL: slave-mode sample-rate detection. \[DS\] §8.3.3.5/12
/// ([`srdctl`]).
#[must_use]
pub const fn tx_srdctl(n: usize) -> Reg {
    Reg::at(tx_base(n) + SRDCTL_OFFSET)
}
/// I2STXn_SRDSTA. \[DS\] §8.3.3.6/13 ([`srdsta`]).
#[must_use]
pub const fn tx_srdsta(n: usize) -> Reg {
    Reg::at(tx_base(n) + SRDSTA_OFFSET)
}
/// I2STXn_FIFO_CNT: samples sent ([`fifo_cnt`]). \[DS\] §8.3.3.7/14
#[must_use]
pub const fn tx_fifo_cnt(n: usize) -> Reg {
    Reg::at(tx_base(n) + FIFO_CNT_OFFSET)
}
/// I2STX_MODE: loopback and clock sharing ([`tx_mode`]). \[DS\] §8.3.3.15. Reset 0.
pub const TX_MODE: Reg = Reg::at(TX_BASE + 0x200);

/// I2SRXn_CTL. \[DS\] §8.3.5.1/7. Reset `0x60` (fields in [`rxctl`]).
#[must_use]
pub const fn rx_ctl(n: usize) -> Reg {
    Reg::at(rx_base(n) + CTL_OFFSET)
}
/// I2SRXn_FIFOCTL. \[DS\] §8.3.5.2/8 ([`rxfifoctl`]).
#[must_use]
pub const fn rx_fifoctl(n: usize) -> Reg {
    Reg::at(rx_base(n) + FIFOCTL_OFFSET)
}
/// I2SRXn_FIFOSTA. \[DS\] §8.3.5.3/9 ([`rxfifosta`]). Reset `0x40`.
#[must_use]
pub const fn rx_fifosta(n: usize) -> Reg {
    Reg::at(rx_base(n) + FIFOSTA_OFFSET)
}
/// I2SRXn_DAT_FIFO (read-only), DMA source. \[DS\] §8.3.5.4/10; vendor
/// address \[code\].
#[must_use]
pub const fn rx_dat_fifo(n: usize) -> Reg {
    Reg::at(rx_base(n) + DAT_FIFO_OFFSET)
}
/// I2SRXn_SRDCTL. \[DS\] §8.3.5.5/11 ([`srdctl`]).
#[must_use]
pub const fn rx_srdctl(n: usize) -> Reg {
    Reg::at(rx_base(n) + SRDCTL_OFFSET)
}
/// I2SRXn_SRDSTA. \[DS\] §8.3.5.6/12 ([`srdsta`]).
#[must_use]
pub const fn rx_srdsta(n: usize) -> Reg {
    Reg::at(rx_base(n) + SRDSTA_OFFSET)
}

/// Fields of I2STXn_CTL. Vendor writes 0, then `TXBCLKSET` (unless a driver
/// flag says otherwise) and `TXEN` \[code\].
pub mod txctl {
    use super::{Field, bit};
    /// TDM: 0 4 channels, 1 8. \[DS\]
    pub const TDMTX_CHAN: u32 = bit(20);
    /// TDM: 0 TDM-A (I2S-style), 1 TDM-B (left-justified). \[DS\]
    pub const TDMTX_MODE: u32 = bit(19);
    /// Frame sync: 0 pulse on LRCLK rise, 1 50 % rising, 2 50 % falling. \[DS\]
    pub const TDMTX_SYNC: Field = Field::bits(18, 17);
    /// 0 master, 1 slave. \[DS\]
    pub const TXMODE: u32 = bit(7);
    /// Slave-mode MCLK: 0 internal, 1 MCLK pad (reset 1). \[DS\]
    pub const TX_SMCLK: u32 = bit(6);
    /// Effective width: 0 16, 1 20, 2 24 bit (reset 2). \[DS\]
    pub const TXWIDTH: Field = Field::bits(5, 4);
    /// Slot width: 0 32 bit, 1 16 bit. \[DS\]
    pub const TXBCLKSET: u32 = bit(3);
    /// Format: 0 I2S, 1 left-, 2 right-justified (not with 16-bit slots), 3 TDM. \[DS\]
    pub const TXMODELSEL: Field = Field::bits(2, 1);
    pub const TXEN: u32 = bit(0);
}

/// Fields of I2SRXn_CTL. Vendor writes `RXMODE` (slave) and optionally
/// `TDMRX_CHAN`, then `RXEN` \[code\] (audio.md §5.4, consistent).
pub mod rxctl {
    use super::{Field, bit};
    pub const TDMRX_CHAN: u32 = bit(16);
    pub const TDMRX_MODE: u32 = bit(15);
    pub const TDMRX_SYNC: Field = Field::bits(14, 13);
    /// 0 master, 1 slave. \[DS\]
    pub const RXMODE: u32 = bit(7);
    pub const RX_SMCLK: u32 = bit(6);
    pub const RXWIDTH: Field = Field::bits(5, 4);
    pub const RXBCLKSET: u32 = bit(3);
    pub const RXMODELSEL: Field = Field::bits(2, 1);
    pub const RXEN: u32 = bit(0);
}

/// Shared encodings of the CTL fields. \[DS\]
pub const WIDTH_16: u32 = 0;
pub const WIDTH_20: u32 = 1;
pub const WIDTH_24: u32 = 2;
pub const FMT_I2S: u32 = 0;
pub const FMT_LEFT: u32 = 1;
pub const FMT_RIGHT: u32 = 2;
pub const FMT_TDM: u32 = 3;

/// Fields of I2STXn_FIFOCTL. Vendor `0x10 | fmt16 << 7`, then `FIFO_RST`,
/// `FIFO_DEN` \[code\].
pub mod txfifoctl {
    use super::{Field, bit};
    /// DMA width: 0 32-bit, 1 16-bit. \[DS\]
    pub const FIFO_DMAWIDTH: u32 = bit(7);
    /// Input: 0 CPU, 1 DMA. \[DS\]
    pub const FIFO_IS: Field = Field::bits(5, 4);
    pub const IS_CPU: u32 = 0;
    pub const IS_DMA: u32 = 1;
    /// TX0 only: 0 own FIFO, 1 take data from the DAC FIFO0/1. \[DS\]; vendor
    /// loopback path sets it (audio.md §4 type 3, "b3 = loopback") \[code\].
    pub const FIFO_SEL: u32 = bit(3);
    /// Half-empty IRQ enable. \[DS\]
    pub const FIFO_IEN: u32 = bit(2);
    /// Half-empty DMA request enable. \[DS\]
    pub const FIFO_DEN: u32 = bit(1);
    /// Run: 0 holds the FIFO in reset. \[DS\]
    pub const FIFO_RST: u32 = bit(0);
}

/// Fields of I2SRXn_FIFOCTL. \[DS\]
pub mod rxfifoctl {
    use super::{Field, bit};
    pub const FIFO_DMAWIDTH: u32 = bit(7);
    /// Output: 0 CPU, 1 DMA. \[DS\]
    pub const FIFO_OS: Field = Field::bits(5, 4);
    pub const OS_CPU: u32 = 0;
    pub const OS_DMA: u32 = 1;
    /// Half-full IRQ enable. \[DS\]
    pub const FIFO_IEN: u32 = bit(2);
    pub const FIFO_DEN: u32 = bit(1);
    pub const FIFO_RST: u32 = bit(0);
}

/// Fields of I2STXn_FIFOSTA. \[DS\]
pub mod txfifosta {
    use super::{Field, bit};
    /// Underrun error (read while empty); W1C.
    pub const FIFO_ER: u32 = bit(8);
    /// Half-empty pending; W1C.
    pub const FIFO_IP: u32 = bit(7);
    pub const FIFO_FULL: u32 = bit(6);
    /// Free levels (0 without clock).
    pub const FIFO_STATUS: Field = Field::bits(4, 0);
}

/// Fields of I2SRXn_FIFOSTA. \[DS\]
pub mod rxfifosta {
    use super::{Field, bit};
    /// Overrun error (written while full); W1C.
    pub const FIFO_ER: u32 = bit(8);
    /// Half-full pending; W1C.
    pub const FIFO_IP: u32 = bit(7);
    pub const FIFO_EMPTY: u32 = bit(6);
    pub const FIFO_STATUS: Field = Field::bits(4, 0);
}

/// Data field of the FIFO data registers. \[DS\]
pub mod dat {
    use super::Field;
    pub const DAT: Field = Field::bits(31, 8);
}

/// Fields of the SRDCTL registers (TX and RX). \[DS\]. The TX0 table prints
/// "11:6 reserved" over `SRD_IE` (b8); TX1/RX tables say 11:9 — a datasheet typo.
pub mod srdctl {
    use super::{Field, bit};
    /// Mute output on a detected rate / width change (RX: until `SRC_PD` is
    /// cleared). \[DS\]
    pub const MUTE_EN: u32 = bit(12);
    /// Interrupt on a detection result change. \[DS\]
    pub const SRD_IE: u32 = bit(8);
    /// Count period: 0 2 LRCLK, 1 4 LRCLK. \[DS\]
    pub const CNT_TIM: Field = Field::bits(5, 4);
    /// Threshold `(SRD_TH + 1)·8`. \[DS\]
    pub const SRD_TH: Field = Field::bits(3, 1);
    /// Slave-mode rate detection enable. \[DS\]
    pub const SRD_EN: u32 = bit(0);
}

/// Fields of the SRDSTA registers (TX and RX). \[DS\]
pub mod srdsta {
    use super::{Field, bit};
    /// SRDCLK / LRCLK count.
    pub const CNT: Field = Field::bits(24, 12);
    /// Detection timeout pending; W1C.
    pub const TO_PD: u32 = bit(11);
    /// Rate change pending; W1C.
    pub const SRC_PD: u32 = bit(10);
    /// Channel-width change pending; W1C.
    pub const CHW_PD: u32 = bit(8);
    /// BCLK/LRCLK: 0 32, 1 64, 2 128, 3 256; 4–7 error.
    pub const WL: Field = Field::bits(2, 0);
}

/// Fields of I2STXn_FIFO_CNT. \[DS\]
pub mod fifo_cnt {
    use super::{Field, bit};
    /// Overflow pending; W1C.
    pub const IP: u32 = bit(18);
    pub const IE: u32 = bit(17);
    /// Enable; 0 resets the count.
    pub const EN: u32 = bit(16);
    pub const CNT: Field = Field::bits(15, 0);
}

/// Fields of [`TX_MODE`]. \[DS\]
pub mod tx_mode {
    use super::bit;
    /// TX0 → RX0 data loopback.
    pub const LPEN0: u32 = bit(10);
    /// TX1 → RX1 data loopback.
    pub const LPEN1: u32 = bit(9);
    /// TX0 and RX0 share TX0's MCLK/BCLK/LRCLK.
    pub const SC_T0R0_EN: u32 = bit(4);
    pub const SC_T1R1_EN: u32 = bit(3);
    pub const SC_T0T1_EN: u32 = bit(2);
    pub const SC_R0R1_EN: u32 = bit(1);
}

/// Vendor TX/RX FIFO control before run (`0x10`: input/output = DMA; OR
/// `FIFO_DMAWIDTH` for 16-bit). audio.md §5.3–5.4 \[code\]
pub const VENDOR_FIFOCTL_DMA_SEL: u32 = txfifoctl::IS_DMA << 4;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn vendor_addresses() {
        // audio.md §5.3–5.4 (vendor DMA addresses); port does not use I2S.
        assert_eq!(tx_base(0), 0xC018_2000);
        assert_eq!(tx_base(1), 0xC018_2100);
        assert_eq!(rx_base(0), 0xC018_3000);
        assert_eq!(rx_base(1), 0xC018_3100);
        assert_eq!(tx_dat_fifo(0).addr(), 0xC018_200C);
        assert_eq!(tx_dat_fifo(1).addr(), 0xC018_210C);
        assert_eq!(rx_dat_fifo(0).addr(), 0xC018_300C);
        assert_eq!(rx_dat_fifo(1).addr(), 0xC018_310C);
    }

    #[test]
    fn datasheet_list() {
        let tx = [
            tx_ctl(0),
            tx_fifoctl(0),
            tx_fifosta(0),
            tx_dat_fifo(0),
            tx_srdctl(0),
            tx_srdsta(0),
            tx_fifo_cnt(0),
            tx_ctl(1),
            tx_fifoctl(1),
            tx_fifosta(1),
            tx_dat_fifo(1),
            tx_srdctl(1),
            tx_srdsta(1),
            tx_fifo_cnt(1),
            TX_MODE,
        ];
        assert_increasing(&tx.map(Reg::addr));
        assert_eq!(tx_fifo_cnt(0).addr(), 0xC018_2018);
        assert_eq!(tx_srdsta(1).addr(), 0xC018_2114);
        assert_eq!(tx_fifo_cnt(1).addr(), 0xC018_2118);
        assert_eq!(TX_MODE.addr(), 0xC018_2200);
        let rx = [
            rx_ctl(0),
            rx_fifoctl(0),
            rx_fifosta(0),
            rx_dat_fifo(0),
            rx_srdctl(0),
            rx_srdsta(0),
            rx_ctl(1),
            rx_fifoctl(1),
            rx_fifosta(1),
            rx_dat_fifo(1),
            rx_srdctl(1),
            rx_srdsta(1),
        ];
        assert_increasing(&rx.map(Reg::addr));
        assert_eq!(rx_srdctl(0).addr(), 0xC018_3010);
        assert_eq!(rx_srdsta(1).addr(), 0xC018_3114);
        assert_eq!(VENDOR_FIFOCTL_DMA_SEL, 0x10);
    }

    #[test]
    fn fields_disjoint() {
        let m = crate::Field::mask;
        {
            use txctl::*;
            assert_disjoint(&[
                TDMTX_CHAN,
                TDMTX_MODE,
                m(TDMTX_SYNC),
                TXMODE,
                TX_SMCLK,
                m(TXWIDTH),
                TXBCLKSET,
                m(TXMODELSEL),
                TXEN,
            ]);
        }
        {
            use rxctl::*;
            assert_disjoint(&[
                TDMRX_CHAN,
                TDMRX_MODE,
                m(TDMRX_SYNC),
                RXMODE,
                RX_SMCLK,
                m(RXWIDTH),
                RXBCLKSET,
                m(RXMODELSEL),
                RXEN,
            ]);
        }
        {
            use txfifoctl::*;
            assert_disjoint(&[FIFO_DMAWIDTH, m(FIFO_IS), FIFO_SEL, FIFO_IEN, FIFO_DEN, FIFO_RST]);
        }
        {
            use rxfifoctl::*;
            assert_disjoint(&[FIFO_DMAWIDTH, m(FIFO_OS), FIFO_IEN, FIFO_DEN, FIFO_RST]);
        }
        {
            use txfifosta::*;
            assert_disjoint(&[FIFO_ER, FIFO_IP, FIFO_FULL, m(FIFO_STATUS)]);
        }
        {
            use rxfifosta::*;
            assert_disjoint(&[FIFO_ER, FIFO_IP, FIFO_EMPTY, m(FIFO_STATUS)]);
        }
        {
            use srdctl::*;
            assert_disjoint(&[MUTE_EN, SRD_IE, m(CNT_TIM), m(SRD_TH), SRD_EN]);
        }
        {
            use srdsta::*;
            assert_disjoint(&[m(CNT), TO_PD, SRC_PD, CHW_PD, m(WL)]);
        }
        {
            use fifo_cnt::*;
            assert_disjoint(&[IP, IE, EN, m(CNT)]);
        }
        {
            use tx_mode::*;
            assert_disjoint(&[LPEN0, LPEN1, SC_T0R0_EN, SC_T1R1_EN, SC_T0T1_EN, SC_R0R1_EN]);
        }
    }
}
