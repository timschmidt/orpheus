//! SPI controllers SPI0 (`0xC00A0000`), SPI1 (`0xC00B0000`) and SPI2
//! (`0xC00C0000`). Datasheet §7.1–§7.3 and memory map §3.4.
//!
//! The three blocks have **different register sets and status-bit
//! positions** (SPI0 `BUSY` is bit 6, SPI1/2 `BUSY` is bit 0), so each has its
//! own submodule: [`spi0`] (the NOR flash, with the cache/descrambler extras),
//! [`spi1`] (adds slave mode, IRQs, 32-bit FIFO, QPI and an OPI/DDR PSRAM
//! interface) and [`spi2`] (SPI1 without the multi-I/O and PSRAM parts).
//!
//! Sources: datasheet V1.2 \[DS\]; SPI0 use by the Rockbox port
//! (`pager-atj2157.c`, `atj2157.h`) and `payloads/spinor.h` \[HW\]; the mask
//! ROM NOR read at `0x2224` and system.md §9.1 \[code\].
//!
//! Unknown: the randomizer seed (the datasheet lists a "32 bit seed" but no
//! register for it; SPI0 offsets `0x14`, `0x20`–`0x2C` are undocumented:
//! U-SPI-1); whether the descrambler key stream depends only on the NOR
//! address (U-NOR-3); the "cache interface" fields belong to the SPI cache
//! controller at `0xC0050000`, which has no register documentation (U-XIP-1).
//! Clocks: `CMU_SPI0CLK`/`SPI1CLK`/`SPTWILK` (`cmu`), gates `CLKEN0` b6–8,
//! IRQ 13–15, DMA selectors 8–10 (`irq`, `dma`).

use crate::Reg;

/// Number of SPI blocks.
pub const COUNT: usize = 3;

/// Base address of SPI`n`, n = 0..2. \[DS\] §3.4, §7.1.4, §7.2.4, §7.3.3.
#[must_use]
pub const fn base(n: usize) -> usize {
    0xC00A_0000 + 0x1_0000 * n
}

/// Register offsets common to all three blocks. \[DS\]
pub const CTL_OFFSET: usize = 0x00;
pub const STA_OFFSET: usize = 0x04;
pub const TXDAT_OFFSET: usize = 0x08;
pub const RXDAT_OFFSET: usize = 0x0C;
pub const BC_OFFSET: usize = 0x10;

/// SPI0: the NOR flash controller (master only, 8-bit FIFO 16 deep). \[DS\] §7.1;
/// register use \[HW\].
///
/// **Disagreement:** §7.1.1 says "no DMA interface", yet `CTL` has DRQ
/// enables and the ROM and the port read NOR through DMA channel 0
/// (mode `0x00200088`) \[HW\]. The hardware wins.
pub mod spi0 {
    use crate::{Field, Reg, bit};

    pub const BASE: usize = 0xC00A_0000;

    /// SPI0_CTL. Reset `0x0000_0008`. \[DS\] §7.1.5.1; \[HW\] for the bits the port uses.
    pub const CTL: Reg = Reg::at(BASE + super::CTL_OFFSET);
    /// SPI0_STA. Reset `0x0000_0014`. \[DS\] §7.1.5.2; TXEM, BUSY, RXEM \[HW\].
    pub const STA: Reg = Reg::at(BASE + super::STA_OFFSET);
    /// SPI0_TXDAT: write one byte `[7:0]` to the 16-byte TX FIFO. \[DS\] §7.1.5.3, \[HW\]
    pub const TXDAT: Reg = Reg::at(BASE + super::TXDAT_OFFSET);
    /// SPI0_RXDAT: read one byte `[7:0]` from the 16-byte RX FIFO; DMA source for
    /// NOR reads. \[DS\] §7.1.5.4, \[HW\]. The datasheet adds "only when
    /// `RAND_RXEN` disable": descrambled reads have only been done by DMA (U-NOR-3).
    pub const RXDAT: Reg = Reg::at(BASE + super::RXDAT_OFFSET);
    /// SPI0_BC: receive byte count for read-only mode. Reset 0. \[DS\] §7.1.5.5, \[HW\]
    pub const BC: Reg = Reg::at(BASE + super::BC_OFFSET);
    /// CACHE_ERROR_ADDR: address of the last cache read error (CRC).
    /// Reset `0x00FF_FFE0`. \[DS\] §7.1.5.6
    pub const CACHE_ERROR_ADDR: Reg = Reg::at(BASE + 0x18);
    /// CACHE_MODE: NOR ready/busy auto-check. Reset 0. \[DS\] §7.1.5.7
    pub const CACHE_MODE: Reg = Reg::at(BASE + 0x1C);
    /// SPI0_CHECKRB_COUNTER: auto check RB counter `[11:0]`. Reset `0xFFF`. \[DS\] §7.1.5.8
    pub const CHECKRB_COUNTER: Reg = Reg::at(BASE + 0x30);
    /// SPI0_SNOR_CRM: continuous-read mode byte `[7:0]` sent after the address
    /// in 4x reads (`0xEB`). Reset `0xA5`. \[DS\] §7.1.5.9, §7.1.2.6
    pub const SNOR_CRM: Reg = Reg::at(BASE + 0x34);

    pub mod ctl {
        use super::{Field, bit};
        /// FIFO clock: 0 CPU clock, 1 DMA clock. Set with `SPI_RDRQ_EN` for DMA
        /// reads \[HW\]. **Disagreement:** system.md §9.1 counts it as part of
        /// the DMA request enable; the datasheet names it a FIFO clock select. \[DS\]
        pub const CLKSEL: u32 = bit(31);
        /// Rx randomizer also in the no-CRC area (cache interface, with `RAND_RXEN`). \[DS\]
        pub const NO_CRC_RAND_RXEN: u32 = bit(29);
        /// SPI mode: **0 = mode 3**, 1 = mode 0; change only while disabled. \[DS\]
        pub const SPI_MODE_SELECT: u32 = bit(28);
        /// With `SPI_IO_MODE` 2x/4x: 0 = 2x/4x I/O, 1 = dual/quad output. \[DS\]
        pub const DUAL_QUAD_SELECT: u32 = bit(27);
        /// RX sample point: 0 = 2 spi_clk (SPI_DELAY ≤ 8), 1 = 3 (≤ 15). \[DS\]
        pub const RX_WRITE_SEL: u32 = bit(26);
        /// Cache interface wait: 16 << n cycles. \[DS\]
        pub const TIME_OUT_CTRL: Field = Field::bits(25, 24);
        /// Cache interface: consecutive CRC errors before `CRC_PD`: 2 << n. \[DS\]
        pub const CRC_ERROR_TIME: Field = Field::bits(23, 22);
        /// Cache interface CRC error IRQ enable. \[DS\]
        pub const CRC_IRQ_EN: u32 = bit(21);
        /// Cache interface CRC enable (16-bit CRC). \[DS\]
        pub const CRC_EN: u32 = bit(20);
        /// Read clock delay, n ns (0..15). \[DS\]; 8 in the init word \[HW\].
        pub const SPI_DELAY: Field = Field::bits(19, 16);
        /// Set by the vendor boot stage around scrambled transfers; reserved in
        /// the datasheet. \[code\] system.md §9.1 (BREC)
        pub const UNK_B15: u32 = bit(15);
        /// TX randomizer (scrambler). \[DS\]; used on the BREC write path \[code\].
        pub const RAND_TXEN: u32 = bit(13);
        /// RX randomizer = the **descrambler** for vendor NOR data. \[DS\];
        /// set by the ROM read (ROM `0x2268`) for the NOR dump \[HW\].
        pub const RAND_RXEN: u32 = bit(12);
        /// Data I/O: 0/1 = 1x, 2 = 2x, 3 = 4x. \[DS\]
        pub const SPI_IO_MODE: Field = Field::bits(11, 10);
        /// 3-wire mode (read-only or write-only). \[DS\]; **Disagreement:**
        /// system.md §9.1 lists bit 9 as unknown; the ROM sets it per context
        /// flag (ROM `0x2248`) \[code\].
        pub const SPI_3WIRE: u32 = bit(9);
        /// Bus request (AHB interface). \[DS\]; in the init word \[HW\].
        pub const SPI_REQ: u32 = bit(8);
        /// TX DMA request when ≥ 8 FIFO levels empty. \[DS\]; BREC write path \[code\].
        pub const SPI_TDRQ_EN: u32 = bit(7);
        /// RX DMA request when ≥ 8 FIFO levels full. \[DS\], \[HW\]
        pub const SPI_RDRQ_EN: u32 = bit(6);
        /// TX FIFO enable; the port clears and sets both FIFO enables to empty
        /// the FIFOs ("FIFO reset", \[HW\]). \[DS\]
        pub const SPI_TX_FIFO_EN: u32 = bit(5);
        /// RX FIFO enable. \[DS\], \[HW\]
        pub const SPI_RX_FIFO_EN: u32 = bit(4);
        /// Chip select output level (1 = high = deselected). Reset 1. \[DS\], \[HW\]
        pub const SPI_SS: u32 = bit(3);
        /// MOSI→MISO loopback. \[DS\]
        pub const SPI_LOOP: u32 = bit(2);
        /// Transfer direction, see `WR_*`. \[DS\], \[HW\]
        pub const SPI_WR: Field = Field::bits(1, 0);
        pub const WR_DISABLE: u32 = 0;
        /// Read only: clocks out `BC` bytes.
        pub const WR_READ: u32 = 1;
        pub const WR_WRITE: u32 = 2;
        pub const WR_READ_WRITE: u32 = 3;
        pub const IO_1X: u32 = 0;
        pub const IO_2X: u32 = 2;
        pub const IO_4X: u32 = 3;
    }

    pub mod sta {
        use super::bit;
        /// Ready (AHB interface). \[DS\]
        pub const SPI_READY: u32 = bit(8);
        /// Cache CRC error pending; write 1 to clear. \[DS\]
        pub const CRC_PD: u32 = bit(7);
        /// Clock running or receive count not yet zero. \[DS\]; polled for 0 \[HW\].
        pub const SPI_BUSY: u32 = bit(6);
        pub const SPI_TXFU: u32 = bit(5);
        /// TX FIFO empty (reset 1). \[DS\]; polled for 1 \[HW\].
        pub const SPI_TXEM: u32 = bit(4);
        pub const SPI_RXFU: u32 = bit(3);
        /// RX FIFO empty (reset 1). \[DS\]; polled for 0 before each byte \[HW\].
        pub const SPI_RXEM: u32 = bit(2);
        /// Cache CRC error status; write 1 to clear. \[DS\]
        pub const CRC_ERROR: u32 = bit(1);
    }

    pub mod bc {
        use super::Field;
        /// Bytes still to receive (read-only). \[DS\]
        pub const REMAIN_CNT: Field = Field::bits(31, 16);
        /// Bytes to clock in, read-only mode. \[DS\]; port ≤ `0x2000` \[HW\].
        pub const SPI_BC: Field = Field::bits(15, 0);
    }

    pub mod cache_error_addr {
        use super::Field;
        /// Failing cache read address `[23:5]` (32-byte line). \[DS\]
        pub const CACHE_ERROR_ADDR: Field = Field::bits(23, 5);
    }

    pub mod cache_mode {
        use super::bit;
        /// NOR ready/busy as seen by the auto check: 1 = busy. \[DS\]
        pub const RB: u32 = bit(8);
        /// Start the automatic RB check. \[DS\]
        pub const CHECKRB: u32 = bit(0);
    }

    pub mod checkrb_counter {
        use super::Field;
        pub const RBCOUNTER: Field = Field::bits(11, 0);
    }

    pub mod snor_crm {
        use super::Field;
        pub const CRM: Field = Field::bits(7, 0);
    }

    /// CTL after bring-up as the ROM does it (port `nor_init`): SPI_DELAY 8,
    /// SPI_REQ, both FIFOs on, CS high; mode 3, 1x, idle. \[HW\]
    pub const VENDOR_INIT_CTL: u32 = 0x0008_0138;
    /// OR-ed into CTL before a DMA read (= `CLKSEL | SPI_RDRQ_EN`): ROM
    /// literal at `0x2378`, copied by the port. \[HW\]
    pub const VENDOR_DMA_RX_SET: u32 = 0x8000_0040;
    /// AND-ed into CTL after a DMA read (clears `CLKSEL`, `RAND_RXEN`,
    /// `SPI_RDRQ_EN`, `SPI_WR`): ROM literal at `0x237C`, copied by the port. \[HW\]
    pub const VENDOR_DMA_RX_CLEAR: u32 = 0x7FFF_EFBC;
    /// The bits the port clears and sets again to empty both FIFOs. \[HW\]
    pub const PORT_FIFO_RESET: u32 = ctl::SPI_TX_FIFO_EN | ctl::SPI_RX_FIFO_EN;
}

/// SPI1: general SPI with slave mode, IRQs, 8/32-bit FIFO, QPI and an OPI /
/// DDR-QPI PSRAM interface. Unused on the YM03. \[DS\] §7.2
///
/// The register-list column "KSEG1 Base Address `0xC0110000`" in §7.2.4 is a
/// copy error (that is TWI0); the physical base `0xC00B0000` matches §3.4.
pub mod spi1 {
    use crate::{Field, Reg, bit};

    pub const BASE: usize = 0xC00B_0000;

    /// SPI1_CTL. Reset `0x3000_8008`. \[DS\] §7.2.5.1
    pub const CTL: Reg = Reg::at(BASE + super::CTL_OFFSET);
    /// SPI1_STA. Reset `0x0000_0052`. \[DS\] §7.2.5.2
    pub const STA: Reg = Reg::at(BASE + super::STA_OFFSET);
    /// SPI1_TXDAT: `[7:0]` (8-bit FIFO, 64 deep) or `[31:0]` (32-bit, 16 deep). \[DS\]
    pub const TXDAT: Reg = Reg::at(BASE + super::TXDAT_OFFSET);
    /// SPI1_RXDAT, widths as `TXDAT`. \[DS\] §7.2.5.4
    pub const RXDAT: Reg = Reg::at(BASE + super::RXDAT_OFFSET);
    /// SPI1_BC: fields as [`super::spi0::bc`]. \[DS\] §7.2.5.5
    pub const BC: Reg = Reg::at(BASE + super::BC_OFFSET);
    /// SPI1_DDR_MODE_CTL: OPI/QPI DDR PSRAM timing. Reset 0. \[DS\] §7.2.5.6
    pub const DDR_MODE_CTL: Reg = Reg::at(BASE + 0x18);

    pub mod ctl {
        use super::{Field, bit};
        /// FIFO clock: 0 CPU, 1 DMA. \[DS\]
        pub const CLKSEL: u32 = bit(31);
        /// FIFO width: 0 = 8-bit, 1 = 32-bit. \[DS\]
        pub const FWS: u32 = bit(30);
        /// SPI mode 0..3 (reset 3). \[DS\]
        pub const SPI_MODE_SELECT: Field = Field::bits(29, 28);
        /// With 2x/4x `SPI_IO_MODE`: 0 = 2x/4x I/O, 1 = dual/quad output. \[DS\]
        pub const DUAL_QUAD_SELECT: u32 = bit(27);
        /// RX sample point: 0 = 2 spi_clk (SPI_DELAY ≤ 8), 1 = 3. \[DS\]
        pub const RX_WRITE_SEL: u32 = bit(26);
        /// DMA: 0 = burst-8, 1 = single. \[DS\]
        pub const DMS: u32 = bit(25);
        /// Byte-swap TX words in 32-bit mode. \[DS\]
        pub const TXCEB: u32 = bit(24);
        /// Byte-swap RX words in 32-bit mode. \[DS\]
        pub const RXCEB: u32 = bit(23);
        /// 0 = master, 1 = slave. \[DS\]
        pub const MSS: u32 = bit(22);
        /// Bit order: **0 = MSB first, 1 = LSB first** (despite the name). \[DS\]
        pub const MSB: u32 = bit(21);
        /// RX IRQ level: 0 = not empty, 1 = ≥ 8 entries. \[DS\]
        pub const RILS: u32 = bit(20);
        /// Read clock delay, n ns. \[DS\]
        pub const SPI_DELAY: Field = Field::bits(19, 16);
        /// Bus request (AHB interface), reset 1. \[DS\]
        pub const SPI_REQ: u32 = bit(15);
        /// Full quad (QPI: command, address and data on four lines). \[DS\]
        pub const QPIEN: u32 = bit(14);
        /// Cache interface wait: 0 none, 1/2/3 = 32/64/128 cycles. \[DS\]
        pub const TIME_OUT_CTRL: Field = Field::bits(13, 12);
        /// Data I/O: 0/1 = 1x, 2 = 2x, 3 = 4x. \[DS\]
        pub const SPI_IO_MODE: Field = Field::bits(11, 10);
        /// TX IRQ when ≥ 8 levels empty. \[DS\]
        pub const SPI_TIRQ_EN: u32 = bit(9);
        /// RX IRQ (level per `RILS`). \[DS\]
        pub const SPI_RIRQ_EN: u32 = bit(8);
        /// TX DMA request. \[DS\]
        pub const SPI_TDRQ_EN: u32 = bit(7);
        /// RX DMA request. \[DS\]
        pub const SPI_RDRQ_EN: u32 = bit(6);
        pub const SPI_TX_FIFO_EN: u32 = bit(5);
        pub const SPI_RX_FIFO_EN: u32 = bit(4);
        /// Chip select output level, reset 1. \[DS\]
        pub const SPI_SS: u32 = bit(3);
        pub const SPI_LOOP: u32 = bit(2);
        /// As [`crate::spi::spi0::ctl::SPI_WR`]. \[DS\]
        pub const SPI_WR: Field = Field::bits(1, 0);
    }

    pub mod sta {
        use super::bit;
        /// TX FIFO overflow (write); write 1 to clear. \[DS\]
        pub const TFWO: u32 = bit(11);
        /// RX FIFO overflow (write); write 1 to clear. \[DS\]
        pub const RFWO: u32 = bit(9);
        /// FIFO read underflow; the datasheet describes it as the RX FIFO
        /// despite the name. Write 1 to clear. \[DS\]
        pub const TFRO: u32 = bit(8);
        pub const SPI_RXFU: u32 = bit(7);
        /// RX FIFO empty (reset 1). \[DS\]
        pub const SPI_RXEM: u32 = bit(6);
        pub const SPI_TXFU: u32 = bit(5);
        /// TX FIFO empty (reset 1). \[DS\]
        pub const SPI_TXEM: u32 = bit(4);
        /// TX IRQ pending; write 1 to clear. \[DS\]
        pub const SPI_TIRQ_PD: u32 = bit(3);
        /// RX IRQ pending; write 1 to clear. \[DS\]
        pub const SPI_RIRQ_PD: u32 = bit(2);
        /// Ready (reset 1). \[DS\]
        pub const SPI_READY: u32 = bit(1);
        /// Busy. Note: bit 0 here, bit 6 in SPI0. \[DS\]
        pub const SPI_BUSY: u32 = bit(0);
    }

    pub mod ddr_mode_ctl {
        use super::Field;
        /// Write latency count. \[DS\]
        pub const WLC: Field = Field::bits(19, 16);
        /// Read latency count (variable 3–7, fixed 6–14). \[DS\]
        pub const RLC: Field = Field::bits(15, 12);
        /// Clock-out delay, n × 0.5 ns. \[DS\]
        pub const CLKOUT_DELAY: Field = Field::bits(11, 8);
        /// DQS delay, n × 0.5 ns. \[DS\]
        pub const DQS_DELAY: Field = Field::bits(7, 4);
        /// DDR mode: 0 off, 1 OPI DDR, 2 QPI DDR. \[DS\]
        pub const DMSLT: Field = Field::bits(1, 0);
        pub const DMSLT_OFF: u32 = 0;
        pub const DMSLT_OPI_DDR: u32 = 1;
        pub const DMSLT_QPI_DDR: u32 = 2;
    }
}

/// SPI2: general SPI with slave mode, IRQs and 8/32-bit FIFO; no multi-I/O,
/// no bus request, no `READY` bit. Unused on the YM03. \[DS\] §7.3
pub mod spi2 {
    use crate::{Field, Reg, bit};

    pub const BASE: usize = 0xC00C_0000;

    /// SPI2_CTL. Reset `0x3000_0008`. \[DS\] §7.3.4.1
    pub const CTL: Reg = Reg::at(BASE + super::CTL_OFFSET);
    /// SPI2_STA. Reset `0x0000_0050`. \[DS\] §7.3.4.2
    pub const STA: Reg = Reg::at(BASE + super::STA_OFFSET);
    /// SPI2_TXDAT, widths as SPI1. \[DS\] §7.3.4.3
    pub const TXDAT: Reg = Reg::at(BASE + super::TXDAT_OFFSET);
    /// SPI2_RXDAT. \[DS\] §7.3.4.4
    pub const RXDAT: Reg = Reg::at(BASE + super::RXDAT_OFFSET);
    /// SPI2_BC: fields as [`super::spi0::bc`]. \[DS\] §7.3.4.5
    pub const BC: Reg = Reg::at(BASE + super::BC_OFFSET);

    /// Same positions and meanings as [`super::spi1::ctl`] where present. \[DS\]
    pub mod ctl {
        use super::{Field, bit};
        pub const CLKSEL: u32 = bit(31);
        pub const FWS: u32 = bit(30);
        pub const SPI_MODE_SELECT: Field = Field::bits(29, 28);
        pub const RX_WRITE_SEL: u32 = bit(26);
        pub const DMS: u32 = bit(25);
        pub const TXCEB: u32 = bit(24);
        pub const RXCEB: u32 = bit(23);
        pub const MSS: u32 = bit(22);
        /// 0 = MSB first, 1 = LSB first. \[DS\]
        pub const MSB: u32 = bit(21);
        pub const RILS: u32 = bit(20);
        pub const SPI_DELAY: Field = Field::bits(19, 16);
        pub const SPI_TIRQ_EN: u32 = bit(9);
        pub const SPI_RIRQ_EN: u32 = bit(8);
        pub const SPI_TDRQ_EN: u32 = bit(7);
        pub const SPI_RDRQ_EN: u32 = bit(6);
        pub const SPI_TX_FIFO_EN: u32 = bit(5);
        pub const SPI_RX_FIFO_EN: u32 = bit(4);
        pub const SPI_SS: u32 = bit(3);
        pub const SPI_LOOP: u32 = bit(2);
        pub const SPI_WR: Field = Field::bits(1, 0);
    }

    /// As [`super::spi1::sta`] without `SPI_READY`. \[DS\]
    pub mod sta {
        use super::bit;
        pub const TFWO: u32 = bit(11);
        pub const RFWO: u32 = bit(9);
        pub const TFRO: u32 = bit(8);
        pub const SPI_RXFU: u32 = bit(7);
        pub const SPI_RXEM: u32 = bit(6);
        pub const SPI_TXFU: u32 = bit(5);
        pub const SPI_TXEM: u32 = bit(4);
        pub const SPI_TIRQ_PD: u32 = bit(3);
        pub const SPI_RIRQ_PD: u32 = bit(2);
        pub const SPI_BUSY: u32 = bit(0);
    }
}

/// `CTL` of SPI`n` (the offset is the same in all three blocks).
#[must_use]
pub const fn ctl(n: usize) -> Reg {
    Reg::at(base(n) + CTL_OFFSET)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // atj2157.h and pager-atj2157.c (SPI0 = NOR) [HW]
        assert_eq!(spi0::BASE, 0xc00a_0000);
        assert_eq!(spi0::CTL.addr(), 0xc00a_0000);
        assert_eq!(spi0::STA.addr(), 0xc00a_0004);
        assert_eq!(spi0::TXDAT.addr(), 0xc00a_0008);
        assert_eq!(spi0::RXDAT.addr(), 0xc00a_000c);
        assert_eq!(spi0::BC.addr(), 0xc00a_0010);
    }

    #[test]
    fn datasheet_addresses() {
        assert_eq!(base(0), spi0::BASE);
        assert_eq!(base(1), spi1::BASE);
        assert_eq!(base(2), spi2::BASE);
        assert_eq!(spi1::BASE, 0xC00B_0000);
        assert_eq!(spi2::BASE, 0xC00C_0000);
        assert_eq!(ctl(2), spi2::CTL);
        assert_eq!(spi0::CACHE_ERROR_ADDR.addr(), 0xC00A_0018);
        assert_eq!(spi0::SNOR_CRM.addr(), 0xC00A_0034);
        assert_eq!(spi1::DDR_MODE_CTL.addr(), 0xC00B_0018);
        assert_eq!(spi2::BC.addr(), 0xC00C_0010);
        assert_increasing(&[
            spi0::CTL.addr(),
            spi0::STA.addr(),
            spi0::TXDAT.addr(),
            spi0::RXDAT.addr(),
            spi0::BC.addr(),
            spi0::CACHE_ERROR_ADDR.addr(),
            spi0::CACHE_MODE.addr(),
            spi0::CHECKRB_COUNTER.addr(),
            spi0::SNOR_CRM.addr(),
            spi1::CTL.addr(),
            spi1::STA.addr(),
            spi1::TXDAT.addr(),
            spi1::RXDAT.addr(),
            spi1::BC.addr(),
            spi1::DDR_MODE_CTL.addr(),
            spi2::CTL.addr(),
            spi2::STA.addr(),
            spi2::TXDAT.addr(),
            spi2::RXDAT.addr(),
            spi2::BC.addr(),
        ]);
    }

    #[test]
    fn spi0_fields_disjoint() {
        use spi0::ctl::*;
        assert_disjoint(&[
            CLKSEL,
            NO_CRC_RAND_RXEN,
            SPI_MODE_SELECT,
            DUAL_QUAD_SELECT,
            RX_WRITE_SEL,
            TIME_OUT_CTRL.mask(),
            CRC_ERROR_TIME.mask(),
            CRC_IRQ_EN,
            CRC_EN,
            SPI_DELAY.mask(),
            UNK_B15,
            RAND_TXEN,
            RAND_RXEN,
            SPI_IO_MODE.mask(),
            SPI_3WIRE,
            SPI_REQ,
            SPI_TDRQ_EN,
            SPI_RDRQ_EN,
            SPI_TX_FIFO_EN,
            SPI_RX_FIFO_EN,
            SPI_SS,
            SPI_LOOP,
            SPI_WR.mask(),
        ]);
        use spi0::sta::*;
        assert_disjoint(&[SPI_READY, CRC_PD, SPI_BUSY, SPI_TXFU, SPI_TXEM, SPI_RXFU, SPI_RXEM, CRC_ERROR]);
        assert_disjoint(&[spi0::bc::REMAIN_CNT.mask(), spi0::bc::SPI_BC.mask()]);
        assert_disjoint(&[spi0::cache_mode::RB, spi0::cache_mode::CHECKRB]);
    }

    #[test]
    fn spi1_spi2_fields_disjoint() {
        {
            use spi1::ctl::*;
            assert_disjoint(&[
                CLKSEL,
                FWS,
                SPI_MODE_SELECT.mask(),
                DUAL_QUAD_SELECT,
                RX_WRITE_SEL,
                DMS,
                TXCEB,
                RXCEB,
                MSS,
                MSB,
                RILS,
                SPI_DELAY.mask(),
                SPI_REQ,
                QPIEN,
                TIME_OUT_CTRL.mask(),
                SPI_IO_MODE.mask(),
                SPI_TIRQ_EN,
                SPI_RIRQ_EN,
                SPI_TDRQ_EN,
                SPI_RDRQ_EN,
                SPI_TX_FIFO_EN,
                SPI_RX_FIFO_EN,
                SPI_SS,
                SPI_LOOP,
                SPI_WR.mask(),
            ]);
            use spi1::sta::*;
            assert_disjoint(&[
                TFWO,
                RFWO,
                TFRO,
                SPI_RXFU,
                SPI_RXEM,
                SPI_TXFU,
                SPI_TXEM,
                SPI_TIRQ_PD,
                SPI_RIRQ_PD,
                SPI_READY,
                SPI_BUSY,
            ]);
            use spi1::ddr_mode_ctl::*;
            assert_disjoint(&[WLC.mask(), RLC.mask(), CLKOUT_DELAY.mask(), DQS_DELAY.mask(), DMSLT.mask()]);
        }
        use spi2::ctl::*;
        assert_disjoint(&[
            CLKSEL,
            FWS,
            SPI_MODE_SELECT.mask(),
            RX_WRITE_SEL,
            DMS,
            TXCEB,
            RXCEB,
            MSS,
            MSB,
            RILS,
            SPI_DELAY.mask(),
            SPI_TIRQ_EN,
            SPI_RIRQ_EN,
            SPI_TDRQ_EN,
            SPI_RDRQ_EN,
            SPI_TX_FIFO_EN,
            SPI_RX_FIFO_EN,
            SPI_SS,
            SPI_LOOP,
            SPI_WR.mask(),
        ]);
        use spi2::sta::*;
        assert_disjoint(&[
            TFWO,
            RFWO,
            TFRO,
            SPI_RXFU,
            SPI_RXEM,
            SPI_TXFU,
            SPI_TXEM,
            SPI_TIRQ_PD,
            SPI_RIRQ_PD,
            SPI_BUSY,
        ]);
    }

    /// The SPI0 words the ROM and port use, decoded against the datasheet fields.
    #[test]
    fn spi0_vendor_words_decode() {
        use spi0::ctl::*;
        let init = spi0::VENDOR_INIT_CTL;
        assert_eq!(SPI_DELAY.get(init), 8);
        assert_eq!(init & RX_WRITE_SEL, 0, "2-cycle sample point is valid up to delay 8");
        assert_eq!(init & SPI_MODE_SELECT, 0, "mode 3");
        assert_eq!(SPI_IO_MODE.get(init), IO_1X);
        assert_eq!(SPI_WR.get(init), WR_DISABLE);
        assert_eq!(init, SPI_DELAY.val(8) | SPI_REQ | SPI_TX_FIFO_EN | SPI_RX_FIFO_EN | SPI_SS);
        assert_eq!(spi0::VENDOR_DMA_RX_SET, CLKSEL | SPI_RDRQ_EN);
        assert_eq!(!spi0::VENDOR_DMA_RX_CLEAR, CLKSEL | RAND_RXEN | SPI_RDRQ_EN | SPI_WR.mask());
        assert_eq!(spi0::PORT_FIFO_RESET, 0x30);
        // Port/ROM literals: descrambler 0x1000, CS 8, 3-wire 0x200; status masks 0x10, 0x40, 4.
        assert_eq!(RAND_RXEN, 0x1000);
        assert_eq!(SPI_SS, 8);
        assert_eq!(SPI_3WIRE, 0x200);
        assert_eq!(spi0::sta::SPI_TXEM, 0x10);
        assert_eq!(spi0::sta::SPI_BUSY, 0x40);
        assert_eq!(spi0::sta::SPI_RXEM, 0x04);
    }
}
