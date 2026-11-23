//! TWI (I²C) controllers TWI0 `0xC0110000` and TWI1 `0xC0120000`
//! (datasheet §7.5, memory map §3.4).
//!
//! Master and slave, 7-bit addresses, 100/400 kbit/s, 8-byte TX and RX FIFOs,
//! IRQ and DMA, and a "command list" engine (`CMD`: start, address bytes,
//! restart, data count, stop) that runs a whole transfer. On the YM03 the
//! tuner and accelerometer sit on GPIO24 (SCL) / GPIO25 (SDA), which the
//! datasheet gives MFP function 14 = `TWI1_CLK` / `TWI1_DAT` (§10.4.25–26), so
//! TWI1 is the candidate controller (U-FM-1). The stock firmware bit-bangs the
//! bus instead and never touches these blocks.
//!
//! Sources: datasheet V1.2 \[DS\] only; nothing here has run on the player.
//! The register layout equals the Actions "owl" I²C controller that Linux
//! supports; the `GBCC` encoding is \[inferred\] from that.
//!
//! Unknown: the SCL source clock (the datasheet says HOSC; the CMU register at
//! `+0x28` is named `CMU_SPTWILK`, hinting at a shared SPI2/TWI clock: U-TWI-1);
//! the IRQ lines 19/20 (U-IRQ-1). Gates `CLKEN0` b18/19, resets `RST0`
//! b18/19; DMA selectors 0x13/0x14.

use crate::{Field, Reg, bit};

/// TWI0 base.
pub const BASE: usize = 0xC011_0000;
/// Number of TWI blocks.
pub const COUNT: usize = 2;
/// SCL source clock per the datasheet (HOSC). \[DS\] §7.5.3.2
pub const CLOCK_HZ: u32 = 24_000_000;

/// Base address of TWI`n`, n = 0..1. \[DS\] §7.5.2
#[must_use]
pub const fn base(n: usize) -> usize {
    BASE + 0x1_0000 * n
}

pub const CTL_OFFSET: usize = 0x00;
pub const CLKDIV_OFFSET: usize = 0x04;
pub const STAT_OFFSET: usize = 0x08;
pub const ADDR_OFFSET: usize = 0x0C;
pub const TXDAT_OFFSET: usize = 0x10;
pub const RXDAT_OFFSET: usize = 0x14;
pub const CMD_OFFSET: usize = 0x18;
pub const FIFOCTL_OFFSET: usize = 0x1C;
pub const FIFOSTAT_OFFSET: usize = 0x20;
pub const DATCNT_OFFSET: usize = 0x24;
pub const RCNT_OFFSET: usize = 0x28;

/// TWIx_CTL. Reset 0. \[DS\] §7.5.3.1
#[must_use]
pub const fn ctl(n: usize) -> Reg {
    Reg::at(base(n) + CTL_OFFSET)
}
/// TWIx_CLKDIV: SCL = HOSC / (CLKDIV · 16), master only. Reset 0. \[DS\] §7.5.3.2
#[must_use]
pub const fn clkdiv(n: usize) -> Reg {
    Reg::at(base(n) + CLKDIV_OFFSET)
}
/// TWIx_STAT. Reset 0. \[DS\] §7.5.3.3
#[must_use]
pub const fn stat(n: usize) -> Reg {
    Reg::at(base(n) + STAT_OFFSET)
}
/// TWIx_ADDR: own slave address `[7:1]`. Reset 0. \[DS\] §7.5.3.4
#[must_use]
pub const fn addr(n: usize) -> Reg {
    Reg::at(base(n) + ADDR_OFFSET)
}
/// TWIx_TXDAT: byte `[7:0]` into the TX FIFO (data, or in master mode the
/// address byte `addr << 1 | R/W`). \[DS\] §7.5.3.5
#[must_use]
pub const fn txdat(n: usize) -> Reg {
    Reg::at(base(n) + TXDAT_OFFSET)
}
/// TWIx_RXDAT: byte `[7:0]` from the RX FIFO. \[DS\] §7.5.3.6
#[must_use]
pub const fn rxdat(n: usize) -> Reg {
    Reg::at(base(n) + RXDAT_OFFSET)
}
/// TWIx_CMD: command list. Reset 0. \[DS\] §7.5.3.7
#[must_use]
pub const fn cmd(n: usize) -> Reg {
    Reg::at(base(n) + CMD_OFFSET)
}
/// TWIx_FIFOCTL. Reset 0. \[DS\] §7.5.3.8
#[must_use]
pub const fn fifoctl(n: usize) -> Reg {
    Reg::at(base(n) + FIFOCTL_OFFSET)
}
/// TWIx_FIFOSTAT. Reset `0x0000_0015`. \[DS\] §7.5.3.9
#[must_use]
pub const fn fifostat(n: usize) -> Reg {
    Reg::at(base(n) + FIFOSTAT_OFFSET)
}
/// TWIx_DATCNT: data bytes for the command list `[7:0]`. Reset 0. \[DS\] §7.5.3.10
#[must_use]
pub const fn datcnt(n: usize) -> Reg {
    Reg::at(base(n) + DATCNT_OFFSET)
}
/// TWIx_RCNT: data bytes remaining `[7:0]`, read-only. \[DS\] §7.5.3.11
#[must_use]
pub const fn rcnt(n: usize) -> Reg {
    Reg::at(base(n) + RCNT_OFFSET)
}

pub mod ctl {
    use super::{Field, bit};
    /// RX IRQ level: 0 = 1 byte, 1 = 4 bytes. \[DS\]
    pub const IRQC: u32 = bit(10);
    /// TX DMA request enable. \[DS\]
    pub const DRQTE: u32 = bit(9);
    /// RX DMA request enable. \[DS\]
    pub const DRQRE: u32 = bit(8);
    /// FIFO bus: 0 = AHB (CPU), 1 = DMA. \[DS\]
    pub const BUSSEL: u32 = bit(7);
    /// IRQ enable. \[DS\]
    pub const IRQE: u32 = bit(6);
    /// Enable; enabling resets the state machine to idle. \[DS\]
    pub const EN: u32 = bit(5);
    /// Generate bus condition (master): see `GBCC_*`. \[DS\] field, values \[inferred\].
    pub const GBCC: Field = Field::bits(3, 2);
    /// Write 1 to release the bus. \[DS\]
    pub const RB: u32 = bit(1);
    /// When receiving: 0 = ACK, 1 = NACK at the 9th clock. \[DS\]
    pub const GRAS: u32 = bit(0);
    pub const GBCC_NONE: u32 = 0;
    pub const GBCC_START: u32 = 1;
    pub const GBCC_STOP: u32 = 2;
    pub const GBCC_RESTART: u32 = 3;
}

pub mod clkdiv {
    use super::Field;
    pub const CLKDIV: Field = Field::bits(7, 0);
    /// 24 MHz / (15 · 16) = 100 kHz. \[DS\] formula
    pub const CLKDIV_100K: u32 = 15;
    /// 24 MHz / (4 · 16) = 375 kHz (400 kHz is not reachable exactly). \[DS\] formula
    pub const CLKDIV_400K: u32 = 4;
}

pub mod stat {
    use super::bit;
    /// Slave: general call received. \[DS\]
    pub const SRGC: u32 = bit(10);
    /// Slave: own address matched. \[DS\]
    pub const SAMB: u32 = bit(9);
    /// Last byte was 0 = address, 1 = data. \[DS\]
    pub const LBST: u32 = bit(8);
    /// Byte (incl. ACK bit) transferred; write 1 to clear. \[DS\]
    pub const TCB: u32 = bit(7);
    /// Bus busy (between START and STOP). \[DS\]
    pub const BBB: u32 = bit(6);
    /// START/restart detected; write 1 to clear. \[DS\]
    pub const STAD: u32 = bit(5);
    /// STOP detected; write 1 to clear. \[DS\]
    pub const STPD: u32 = bit(4);
    /// IRQ pending; write 1 to clear. \[DS\]
    pub const IRQP: u32 = bit(2);
    /// Bus error (misplaced START/STOP); write 1 to clear. \[DS\]
    pub const BEB: u32 = bit(1);
    /// Last received acknowledge: 1 = ACK, 0 = NACK. \[DS\]
    pub const RACK: u32 = bit(0);
}

pub mod addr {
    use super::Field;
    /// Own 7-bit slave address. \[DS\]
    pub const SDAD: Field = Field::bits(7, 1);
}

pub mod cmd {
    use super::{Field, bit};
    /// Execute the command list. \[DS\]
    pub const SECL: u32 = bit(15);
    /// Slave only: 0 = write, 1 = read. \[DS\]
    pub const WRS: u32 = bit(12);
    /// **1 = master**, 0 = slave. \[DS\]
    pub const MSS: u32 = bit(11);
    /// Send STOP at the end. \[DS\]
    pub const SE: u32 = bit(10);
    /// NACK the last byte read. \[DS\]
    pub const NS: u32 = bit(9);
    /// Data phase enabled (length in `DATCNT`). \[DS\]
    pub const DE: u32 = bit(8);
    /// Address bytes sent after the restart (0..7). \[DS\]
    pub const SAS: Field = Field::bits(7, 5);
    /// Send a restart. \[DS\]
    pub const RBE: u32 = bit(4);
    /// Address bytes after the start, slave address plus register address (0..7). \[DS\]
    pub const AS: Field = Field::bits(3, 1);
    /// Send START. \[DS\]
    pub const SBE: u32 = bit(0);
}

pub mod fifoctl {
    use super::bit;
    /// Reset the TX FIFO (self-clearing). \[DS\]
    pub const TFR: u32 = bit(2);
    /// Reset the RX FIFO (self-clearing). \[DS\]
    pub const RFR: u32 = bit(1);
    /// Ignore NACK on write (continue the command list). \[DS\]
    pub const NIB: u32 = bit(0);
}

pub mod fifostat {
    use super::{Field, bit};
    /// TX FIFO level. \[DS\]
    pub const TFD: Field = Field::bits(15, 12);
    /// RX FIFO level. \[DS\]
    pub const RFD: Field = Field::bits(11, 8);
    /// Slave FIFO mode: 0 = master writes, 1 = master reads. \[DS\]
    pub const WRS: u32 = bit(6);
    pub const TFF: u32 = bit(5);
    /// TX FIFO empty (reset 1). \[DS\]
    pub const TFE: u32 = bit(4);
    pub const RFF: u32 = bit(3);
    /// RX FIFO empty (reset 1). \[DS\]
    pub const RFE: u32 = bit(2);
    /// NACK received while writing; write 1 to clear. \[DS\]
    pub const RNB: u32 = bit(1);
    /// Command list complete (reset 1). \[DS\]
    pub const CECB: u32 = bit(0);
}

pub mod datcnt {
    use super::Field;
    pub const TC: Field = Field::bits(7, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn datasheet_addresses() {
        assert_eq!(base(0), 0xC011_0000);
        assert_eq!(base(1), 0xC012_0000);
        assert_eq!(ctl(1).addr(), 0xC012_0000);
        assert_eq!(cmd(1).addr(), 0xC012_0018);
        assert_eq!(rcnt(1).addr(), 0xC012_0028);
        assert_eq!(fifostat(0).addr(), 0xC011_0020);
        for n in 0..COUNT {
            assert_increasing(&[
                ctl(n).addr(),
                clkdiv(n).addr(),
                stat(n).addr(),
                addr(n).addr(),
                txdat(n).addr(),
                rxdat(n).addr(),
                cmd(n).addr(),
                fifoctl(n).addr(),
                fifostat(n).addr(),
                datcnt(n).addr(),
                rcnt(n).addr(),
            ]);
        }
    }

    #[test]
    fn fields_disjoint() {
        use ctl::*;
        assert_disjoint(&[IRQC, DRQTE, DRQRE, BUSSEL, IRQE, EN, GBCC.mask(), RB, GRAS]);
        use stat::*;
        assert_disjoint(&[SRGC, SAMB, LBST, TCB, BBB, STAD, STPD, IRQP, BEB, RACK]);
        {
            use cmd::*;
            assert_disjoint(&[SECL, WRS, MSS, SE, NS, DE, SAS.mask(), RBE, AS.mask(), SBE]);
        }
        assert_disjoint(&[fifoctl::TFR, fifoctl::RFR, fifoctl::NIB]);
        use fifostat::*;
        assert_disjoint(&[TFD.mask(), RFD.mask(), WRS, TFF, TFE, RFF, RFE, RNB, CECB]);
    }

    #[test]
    fn clock_divisors() {
        assert_eq!(CLOCK_HZ / (clkdiv::CLKDIV_100K * 16), 100_000);
        assert_eq!(CLOCK_HZ / (clkdiv::CLKDIV_400K * 16), 375_000);
        // FIFOSTAT reset value from the datasheet.
        assert_eq!(fifostat::TFE | fifostat::RFE | fifostat::CECB, 0x15);
    }
}
