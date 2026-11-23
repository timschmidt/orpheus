//! DMA controller, `0xC0070000` (datasheet ch. 5, §5.1–§5.3).
//!
//! Ten ordinary channels at `0xC0070100 + 0x100·n` (n = 0..9) with one common
//! layout, plus the special LCD/YUV channel "SDMA" at `0xC0071000`. The
//! datasheet descriptions of DMA0CTL…DMA9CTL are word-for-word identical, so
//! the fields are described once ([`ctl`](mod@ctl)). Per §5.1 only one channel moves
//! data at any moment (the channels share one engine).
//!
//! Sources: datasheet §5 \[DS\]; the Rockbox port (`atj2157.h`, `pager-atj2157.c`,
//! `sdmmc-yotonym03.c`, `pcm-yotonym03.c`) and `payloads/dmabeep.c` \[HW\];
//! audio.md §6 and system.md §6/§10 \[code\]; FINDINGS §7 (SRAM2/3 DMA aliases) \[HW\].
//!
//! Unknown: what "DMA0-7 can be triggered on selected events" (§5.1) means; whether
//! RC counts live while a channel runs; selector codes `0x11` and `0x1D` (printed
//! as "-"); how the SDMA interrupt is enabled (DMAIE has no SDMA bits).

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC007_0000;

/// Number of ordinary channels.
pub const COUNT: usize = 10;

/// DMAIP: interrupt pending, write 1 to clear. \[DS\] §5.3.1.1, reset `0`.
/// The port clears `bit n | bit n+16` together \[HW\].
pub const IP: Reg = Reg::at(BASE);
/// DMAIE: interrupt enable, same layout as [`IP`] for channels 0..9 (no SDMA
/// bits). \[DS\] §5.3.1.2, reset `0`; bit 5 used by the port \[HW\].
pub const IE: Reg = Reg::at(BASE + 0x04);

/// Bits of [`IP`] and [`IE`].
pub mod ip {
    use super::{Field, bit};
    /// DMAnTCIP / DMAnTCIE: transmission complete, channel n at bit n. \[DS\] §5.3.1.1–2
    pub const TC: Field = Field::bits(9, 0);
    /// DMAnHFIP / DMAnHFIE: half transmission, channel n at bit n + 16. \[DS\] §5.3.1.1–2
    /// **Disagreement:** audio.md §6.1 / system.md §6 guessed "second event,
    /// half-complete \[inferred\]"; `payloads/dmatest.c` labels bit 5 "half" and
    /// bit 21 "full", the reverse of the datasheet (no recorded result).
    pub const HF: Field = Field::bits(25, 16);
    /// SDMATCIP: special channel complete (pending register only). \[DS\];
    /// cleared by the vendor LCD driver with `0x0400_0400` \[code\].
    pub const SDMATCIP: u32 = bit(10);
    /// SDMAHFIP: special channel half transmission (pending register only). \[DS\]
    pub const SDMAHFIP: u32 = bit(26);

    /// Transmission-complete bit of channel `n`.
    #[must_use]
    pub const fn tc(n: usize) -> u32 {
        1 << n
    }
    /// Half-transmission bit of channel `n`.
    #[must_use]
    pub const fn hf(n: usize) -> u32 {
        1 << (n + 16)
    }
    /// Both bits of channel `n` (what the port writes to clear a channel). \[HW\]
    #[must_use]
    pub const fn both(n: usize) -> u32 {
        tc(n) | hf(n)
    }
}

/// Offset of channel 0 from [`BASE`]; channels are `0x100` apart. \[DS\] §5.2
pub const CH_OFFSET: usize = 0x100;
pub const CH_STRIDE: usize = 0x100;
pub const CTL_OFFSET: usize = 0x00;
pub const START_OFFSET: usize = 0x04;
pub const SADDR0_OFFSET: usize = 0x08;
pub const SADDR1_OFFSET: usize = 0x0C;
pub const DADDR0_OFFSET: usize = 0x10;
pub const DADDR1_OFFSET: usize = 0x14;
pub const BC_OFFSET: usize = 0x18;
pub const RC_OFFSET: usize = 0x1C;

/// Base of channel `n` (0..[`COUNT`]). \[DS\] §5.2; \[HW\] for n = 0, 4, 5.
#[must_use]
pub const fn base(n: usize) -> usize {
    BASE + CH_OFFSET + CH_STRIDE * n
}

/// DMAnCTL, the mode word (fields in [`ctl`](mod@ctl)). \[DS\] §5.3.1.3, reset `0`.
/// Port name `DMA_MODE(n)` \[HW\].
#[must_use]
pub const fn ctl(n: usize) -> Reg {
    Reg::at(base(n) + CTL_OFFSET)
}
/// DMAnSTART (fields in [`start`](mod@start)). \[DS\] §5.3.1.4, reset `0`. Port name `DMA_CTL(n)` \[HW\].
#[must_use]
pub const fn start(n: usize) -> Reg {
    Reg::at(base(n) + START_OFFSET)
}
/// DMAnSADDR0: source address (low bits ignored per width). \[DS\] §5.3.1.5, reset `0`. \[HW\]
#[must_use]
pub const fn saddr0(n: usize) -> Reg {
    Reg::at(base(n) + SADDR0_OFFSET)
}
/// DMAnSADDR1: second source address (right-channel buffer when
/// [`ctl::AUDIOTYPE`] = separated). \[DS\] §5.3.1.6; use \[code\] audio.md §6.1.
#[must_use]
pub const fn saddr1(n: usize) -> Reg {
    Reg::at(base(n) + SADDR1_OFFSET)
}
/// DMAnDADDR0: destination address. \[DS\] §5.3.1.7, reset `0`. \[HW\]
#[must_use]
pub const fn daddr0(n: usize) -> Reg {
    Reg::at(base(n) + DADDR0_OFFSET)
}
/// DMAnDADDR1: second destination address (planar capture). \[DS\] §5.3.1.8;
/// use \[code\] audio.md §6.1.
#[must_use]
pub const fn daddr1(n: usize) -> Reg {
    Reg::at(base(n) + DADDR1_OFFSET)
}
/// DMAnBC: transfer length in bytes ([`bc::DMABYTECOUNTER`]). \[DS\] §5.3.1.9,
/// reset `0`. Port name `DMA_LEN(n)` \[HW\].
#[must_use]
pub const fn bc(n: usize) -> Reg {
    Reg::at(base(n) + BC_OFFSET)
}
/// DMAnRC: remaining byte count, read-only. \[DS\] §5.3.1.10, reset `0`. The
/// vendor audio driver reads it to report buffered data \[code\] audio.md §6.1.
#[must_use]
pub const fn rc(n: usize) -> Reg {
    Reg::at(base(n) + RC_OFFSET)
}

/// DMAnCTL fields, identical for n = 0..9. \[DS\] §5.3.1.3 (and .11, .19, … .75).
pub mod ctl {
    use super::{Field, bit};

    /// SRCSL: source device (`SRC_*` / `SEL_MEMORY`).
    pub const SRCSL: Field = Field::bits(4, 0);
    /// SAM: source address constant (0 = increment). Set for every FIFO source
    /// in the vendor words \[code\].
    pub const SAM: u32 = bit(7);
    /// DSTSL: destination device (`DST_*` / `SEL_MEMORY`).
    pub const DSTSL: Field = Field::bits(12, 8);
    /// DAM: destination address constant (0 = increment).
    pub const DAM: u32 = bit(15);
    /// AUDIOTYPE: 1 = left/right in two buffers (SADDR1 / DADDR1), 0 = interleaved.
    /// For DMA-ADC, -DAC, -I2S and -ASRC transfers.
    pub const AUDIOTYPE: u32 = bit(16);
    /// TRM: 1 = single transfers, 0 = burst of 8 (burst needs FIFO thresholds of
    /// 8 bytes, \[DS\] §7.4.5.1 RDIC/TDIC). UART1 TX word sets it \[code\].
    pub const TRM: u32 = bit(17);
    /// reload: when a transfer completes, reload the channel registers and start
    /// again. Set in every vendor audio word \[code\]; the port's DAC word has it \[HW\].
    pub const RELOAD: u32 = bit(18);
    /// TWS: transfer width (`TWS_*`). **Disagreement:** the DAC/ADC/I2S chapters
    /// point at "DMAx_CTL\[14:13\]" for the width (reserved here; stale reference).
    pub const TWS: Field = Field::bits(21, 20);
    pub const TWS_32BIT: u32 = 0;
    pub const TWS_16BIT: u32 = 1;
    pub const TWS_8BIT: u32 = 2;

    /// Memory (either side).
    pub const SEL_MEMORY: u32 = 0x00;

    // SRCSL values (receive FIFOs). \[DS\] §5.3.1.3
    pub const SRC_UART0_RX: u32 = 0x01;
    /// \[code\] vendor Bluetooth RX word.
    pub const SRC_UART1_RX: u32 = 0x02;
    pub const SRC_UART2_RX: u32 = 0x03;
    pub const SRC_UART3_RX: u32 = 0x04;
    /// "SDIO FIFO" per the datasheet. **Disagreement:** the port drives the SD card
    /// controller at `0xC0160000` (memory map "SD/MMC") with this code \[HW\].
    pub const SRC_SDIO: u32 = 0x05;
    /// "SD/MMC FIFO" per the datasheet; unused by the port (see [`SRC_SDIO`]).
    pub const SRC_SD_MMC: u32 = 0x06;
    /// \[HW\] pager and ROM NOR reads.
    pub const SRC_SPI0_RX: u32 = 0x08;
    pub const SRC_SPI1_RX: u32 = 0x09;
    pub const SRC_SPI2_RX: u32 = 0x0A;
    /// \[code\] vendor capture words.
    pub const SRC_ADC01: u32 = 0x0B;
    /// Printed "ADC23 FIFO (I2S RX0)".
    pub const SRC_ADC23: u32 = 0x0C;
    pub const SRC_I2S_RX0: u32 = 0x0D;
    pub const SRC_I2S_RX1: u32 = 0x0E;
    pub const SRC_TWI0_RX: u32 = 0x13;
    pub const SRC_TWI1_RX: u32 = 0x14;
    /// \[code\] ROM NAND read word.
    pub const SRC_NAND: u32 = 0x18;
    /// Printed as "-" in the datasheet; the vendor uses it for the DSP
    /// accelerator at `0xC0200000` \[code\] audio.md §6.2.
    pub const SRC_UNK_1D: u32 = 0x1D;

    // DSTSL values (transmit FIFOs). \[DS\] §5.3.1.3
    pub const DST_UART0_TX: u32 = 0x01;
    pub const DST_UART1_TX: u32 = 0x02;
    pub const DST_UART2_TX: u32 = 0x03;
    pub const DST_UART3_TX: u32 = 0x04;
    /// See [`SRC_SDIO`]: the port's SD write code \[HW\].
    pub const DST_SDIO: u32 = 0x05;
    pub const DST_SD_MMC: u32 = 0x06;
    pub const DST_SPI0_TX: u32 = 0x08;
    pub const DST_SPI1_TX: u32 = 0x09;
    pub const DST_SPI2_TX: u32 = 0x0A;
    /// "DAC(PDMTX) FIFO0" \[DS\]; the port's audio path \[HW\].
    pub const DST_DAC_FIFO0: u32 = 0x0B;
    pub const DST_DAC_FIFO1: u32 = 0x0C;
    pub const DST_I2S_TX0: u32 = 0x0D;
    pub const DST_I2S_TX1: u32 = 0x0E;
    pub const DST_TWI0_TX: u32 = 0x13;
    pub const DST_TWI1_TX: u32 = 0x14;
    pub const DST_PWM: u32 = 0x15;
    /// \[code\] vendor LCD blit word.
    pub const DST_LCD: u32 = 0x16;
    pub const DST_NAND: u32 = 0x18;
    /// See [`SRC_UNK_1D`] (DSP accelerator input) \[code\].
    pub const DST_UNK_1D: u32 = 0x1D;
}

/// DMAnSTART fields. \[DS\] §5.3.1.4 (titled "Interrupt Pending Register" in error).
pub mod start {
    use super::bit;
    /// DMASTART: a 0→1 edge loads addresses, length and mode and starts; clears
    /// itself on completion or error; write 0 to abort. Polled as busy \[HW\].
    pub const DMASTART: u32 = bit(0);
}

/// DMAnBC / SDMABC fields. \[DS\] §5.3.1.9
pub mod bc {
    use super::Field;
    /// DMABYTECOUNTER: transfer length in bytes ("frame length").
    pub const DMABYTECOUNTER: Field = Field::bits(17, 0);
    /// Largest length one transfer can carry.
    pub const MAX: u32 = 0x3_FFFF;
}

/// DMAnRC / SDMARC fields. \[DS\] §5.3.1.10
pub mod rc {
    use super::Field;
    /// DMARemainCounter (read-only).
    pub const DMAREMAINCOUNTER: Field = Field::bits(17, 0);
}

/// Mode word from a source selector, a destination selector and flag bits
/// (`SAM`, `DAM`, `AUDIOTYPE`, `TRM`, `RELOAD`, `WIDTH_*`).
#[must_use]
pub const fn mode(src: u32, dst: u32, flags: u32) -> u32 {
    ctl::SRCSL.val(src) | ctl::DSTSL.val(dst) | flags
}
/// [`ctl::TWS`] placed for [`mode`].
pub const WIDTH_32: u32 = ctl::TWS_32BIT << 20;
pub const WIDTH_16: u32 = ctl::TWS_16BIT << 20;
pub const WIDTH_8: u32 = ctl::TWS_8BIT << 20;

/// SPI0 RX FIFO → memory, 8-bit, burst 8. ROM NOR read and the port's pager
/// (channel 0) \[HW\].
pub const VENDOR_MODE_SPI0_RX: u32 = 0x0020_0088;
/// SD FIFO → memory, 32-bit (`DMA_MODE_SD0_READ`, channel 4) \[HW\].
pub const VENDOR_MODE_SD_READ: u32 = 0x0000_0085;
/// Memory → SD FIFO, 32-bit (`DMA_MODE_SD0_WRITE`) \[HW\].
pub const VENDOR_MODE_SD_WRITE: u32 = 0x0000_8500;
/// NAND → memory (ROM 0x16B6) \[code\].
pub const VENDOR_MODE_NAND_READ: u32 = 0x0000_0098;
/// Memory → DAC FIFO0, 16-bit interleaved, reload. Port channel 5 \[HW\]
/// (the port restarts each block and does not rely on the reload).
pub const VENDOR_MODE_DAC_16: u32 = 0x0014_8B00;
/// As [`VENDOR_MODE_DAC_16`] with two planar buffers \[code\] audio.md §6.2.
pub const VENDOR_MODE_DAC_16_PLANAR: u32 = 0x0015_8B00;
/// Memory → DAC FIFO0, 32-bit words, reload \[code\].
pub const VENDOR_MODE_DAC_32: u32 = 0x0004_8B00;
/// ADC01 FIFO → memory, 16-bit interleaved, reload \[code\].
pub const VENDOR_MODE_ADC_16: u32 = 0x0014_008B;
/// As [`VENDOR_MODE_ADC_16`], planar \[code\].
pub const VENDOR_MODE_ADC_16_PLANAR: u32 = 0x0015_008B;
/// Memory → LCD FIFO, 32-bit (vendor `DRV_LCD`, channel 1) \[code\].
pub const VENDOR_MODE_LCD: u32 = 0x0000_9600;
/// UART1 RX → memory, 8-bit (vendor Bluetooth, channel 8) \[code\]. No reload bit:
/// the "circular" buffer of 01-hardware §6 is software polling, not hardware.
pub const VENDOR_MODE_UART1_RX: u32 = 0x0020_0082;
/// Memory → UART1 TX, 8-bit, single (vendor Bluetooth, channel 9) \[code\].
pub const VENDOR_MODE_UART1_TX: u32 = 0x0022_8200;
/// Memory → DSP accelerator (selector `0x1D`, 32-bit) \[code\].
pub const VENDOR_MODE_DSP_IN: u32 = 0x0000_1D00;
/// DSP accelerator → memory \[code\].
pub const VENDOR_MODE_DSP_OUT: u32 = 0x0000_001D;

/// SRAM2 as the CPU sees it (32 KiB, aliased over 1 MiB) \[HW\] FINDINGS §7.
/// Not in the datasheet memory map (§3.4 lists RAM0–6 and "HW RAM" only).
pub const SRAM2_CPU_BASE: u32 = 0x0020_0000;
/// SRAM2 as the DMA engine addresses it \[HW\] (port `atj_dma_addr`).
pub const SRAM2_DMA_BASE: u32 = 0x1000_0000;
/// SRAM3 as the CPU sees it \[HW\].
pub const SRAM3_CPU_BASE: u32 = 0x0030_0000;
/// SRAM3 as the DMA engine addresses it \[HW\].
pub const SRAM3_DMA_BASE: u32 = 0x1800_0000;
/// Size of SRAM2 and of SRAM3. SRAM0/SRAM1 use their CPU addresses for DMA \[HW\].
pub const SRAM23_SIZE: u32 = 0x8000;

/// Special channel "SDMA" for LCD output (RGB or one-buffer YUV with up to three
/// source planes). \[DS\] §5.2; used by the vendor LCD driver next to channel 1 \[code\].
pub const SDMA_BASE: usize = BASE + 0x1000;
/// SDMACTL. \[DS\] §5.3.1.83, reset `0`.
pub const SDMA_CTL: Reg = Reg::at(SDMA_BASE);
/// SDMASTART, same bit as [`start::DMASTART`]. \[DS\] §5.3.1.84
pub const SDMA_START: Reg = Reg::at(SDMA_BASE + 0x04);
/// SDMASADDR0 (Y / RGB plane). \[DS\] §5.3.1.85
pub const SDMA_SADDR0: Reg = Reg::at(SDMA_BASE + 0x08);
/// SDMASADDR1. \[DS\] §5.3.1.86; its printed offset `0x10c` is a typo for `0x100c` (§5.2).
pub const SDMA_SADDR1: Reg = Reg::at(SDMA_BASE + 0x0C);
/// SDMASADDR2. \[DS\] §5.3.1.87
pub const SDMA_SADDR2: Reg = Reg::at(SDMA_BASE + 0x10);
/// SDMALCDCFG (fields in [`sdma_lcdcfg`]). \[DS\] §5.3.1.88; vendor writes
/// width·4 into both halves \[code\] audio.md §6.1.
pub const SDMA_LCDCFG: Reg = Reg::at(SDMA_BASE + 0x14);
/// SDMABC, as [`bc`](mod@bc). \[DS\] §5.3.1.89
pub const SDMA_BC: Reg = Reg::at(SDMA_BASE + 0x18);
/// SDMARC, as [`rc`](mod@rc). \[DS\] §5.3.1.90
pub const SDMA_RC: Reg = Reg::at(SDMA_BASE + 0x1C);

/// SDMACTL fields. \[DS\] §5.3.1.83 (no selector, width or address-mode fields).
pub mod sdma_ctl {
    use super::{Field, bit};
    /// reload, as [`super::ctl::RELOAD`].
    pub const RELOAD: u32 = bit(18);
    /// LCDMODE: 0 RGB, 2 YUV444 one buffer, 3 YUV420 one buffer (1 reserved).
    /// Vendor values 0, `0x20000`, `0x30000` \[code\] audio.md §6.1.
    pub const LCDMODE: Field = Field::bits(17, 16);
    pub const LCDMODE_RGB: u32 = 0;
    pub const LCDMODE_YUV444: u32 = 2;
    pub const LCDMODE_YUV420: u32 = 3;
}

/// SDMALCDCFG fields. \[DS\] §5.3.1.88 (reserved range printed `31:11`, read `31:27`).
pub mod sdma_lcdcfg {
    use super::Field;
    /// IMAGWIDTH: width of the image buffer.
    pub const IMAGWIDTH: Field = Field::bits(26, 16);
    /// LCDWIDTH: width of the LCD buffer.
    pub const LCDWIDTH: Field = Field::bits(10, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // atj2157.h: DMA_IRQPD, DMA_IRQEN, DMA_MODE/CTL/SRC/DST/LEN(n).
        assert_eq!(IP.addr(), 0xc007_0000);
        assert_eq!(IE.addr(), 0xc007_0004);
        for n in 0..COUNT {
            assert_eq!(ctl(n).addr(), 0xc007_0100 + 0x100 * n);
            assert_eq!(start(n).addr(), 0xc007_0104 + 0x100 * n);
            assert_eq!(saddr0(n).addr(), 0xc007_0108 + 0x100 * n);
            assert_eq!(daddr0(n).addr(), 0xc007_0110 + 0x100 * n);
            assert_eq!(bc(n).addr(), 0xc007_0118 + 0x100 * n);
        }
        // pager-atj2157.c DMA0(o); dmabeep.c channel 5 literals.
        assert_eq!(base(0), 0xc007_0100);
        assert_eq!(ctl(5).addr(), 0xc007_0600);
        assert_eq!(start(5).addr(), 0xc007_0604);
        assert_eq!(saddr0(5).addr(), 0xc007_0608);
        assert_eq!(daddr0(5).addr(), 0xc007_0610);
        assert_eq!(bc(5).addr(), 0xc007_0618);
        // pcm/sdmmc: pending masks for channels 5 and 4.
        assert_eq!(ip::both(5), (1 << 5) | (1 << 21));
        assert_eq!(ip::both(4), (1 << 4) | (1 << 20));
        // atj2157.h mode words.
        assert_eq!(VENDOR_MODE_SD_READ, 0x85);
        assert_eq!(VENDOR_MODE_SD_WRITE, 0x8500);
        assert_eq!(VENDOR_MODE_DAC_16, 0x0014_8b00);
        assert_eq!(VENDOR_MODE_SPI0_RX, 0x0020_0088);
        // Special channel and its pending bits as the vendor LCD driver uses them.
        assert_eq!(SDMA_BASE, 0xc007_1000);
        assert_eq!(ip::SDMATCIP | ip::SDMAHFIP, 0x0400_0400);
        assert_eq!(ip::both(1), 0x0002_0002);
        // SRAM aliases (system-target.h atj_dma_addr).
        assert_eq!(SRAM2_DMA_BASE, 0x1000_0000);
        assert_eq!(SRAM3_DMA_BASE, 0x1800_0000);
    }

    #[test]
    fn datasheet_addresses() {
        assert_eq!(saddr1(6).addr(), 0xC007_070C);
        assert_eq!(daddr1(3).addr(), 0xC007_0414);
        assert_eq!(rc(9).addr(), 0xC007_0A1C);
        assert_eq!(ctl(9).addr(), 0xC007_0A00);
        assert_eq!(SDMA_SADDR1.addr(), 0xC007_100C);
        assert_eq!(SDMA_LCDCFG.addr(), 0xC007_1014);
        assert_eq!(SDMA_RC.addr(), 0xC007_101C);
        let mut list = [0usize; 2 + COUNT * 8 + 8];
        list[0] = IP.addr();
        list[1] = IE.addr();
        let mut i = 2;
        for n in 0..COUNT {
            for r in [ctl(n), start(n), saddr0(n), saddr1(n), daddr0(n), daddr1(n), bc(n), rc(n)] {
                list[i] = r.addr();
                i += 1;
            }
        }
        for r in [SDMA_CTL, SDMA_START, SDMA_SADDR0, SDMA_SADDR1, SDMA_SADDR2, SDMA_LCDCFG, SDMA_BC, SDMA_RC]
        {
            list[i] = r.addr();
            i += 1;
        }
        assert_eq!(i, list.len());
        assert_increasing(&list);
    }

    const CTL_FIELDS: [u32; 8] = [
        ctl::SRCSL.mask(),
        ctl::SAM,
        ctl::DSTSL.mask(),
        ctl::DAM,
        ctl::AUDIOTYPE,
        ctl::TRM,
        ctl::RELOAD,
        ctl::TWS.mask(),
    ];

    #[test]
    fn fields_disjoint() {
        assert_disjoint(&CTL_FIELDS);
        assert_disjoint(&[ip::TC.mask(), ip::SDMATCIP, ip::HF.mask(), ip::SDMAHFIP]);
        assert_disjoint(&[sdma_ctl::LCDMODE.mask(), sdma_ctl::RELOAD]);
        assert_disjoint(&[sdma_lcdcfg::LCDWIDTH.mask(), sdma_lcdcfg::IMAGWIDTH.mask()]);
        assert_eq!(ip::TC.mask(), 0x3FF);
        assert_eq!(ip::HF.mask(), 0x3FF << 16);
        assert_eq!(ip::hf(9), bit(25));
        assert_eq!(bc::DMABYTECOUNTER.mask(), bc::MAX);
        assert_eq!(rc::DMAREMAINCOUNTER.mask(), bc::MAX);
        assert_eq!(sdma_ctl::RELOAD, ctl::RELOAD);
    }

    #[test]
    fn vendor_modes_decode() {
        use ctl::*;
        let defined = CTL_FIELDS.iter().fold(0, |a, m| a | m);
        let modes = [
            (VENDOR_MODE_SPI0_RX, mode(SRC_SPI0_RX, SEL_MEMORY, SAM | WIDTH_8)),
            (VENDOR_MODE_SD_READ, mode(SRC_SDIO, SEL_MEMORY, SAM | WIDTH_32)),
            (VENDOR_MODE_SD_WRITE, mode(SEL_MEMORY, DST_SDIO, DAM | WIDTH_32)),
            (VENDOR_MODE_NAND_READ, mode(SRC_NAND, SEL_MEMORY, SAM)),
            (VENDOR_MODE_DAC_16, mode(SEL_MEMORY, DST_DAC_FIFO0, DAM | RELOAD | WIDTH_16)),
            (VENDOR_MODE_DAC_16_PLANAR, mode(SEL_MEMORY, DST_DAC_FIFO0, DAM | RELOAD | AUDIOTYPE | WIDTH_16)),
            (VENDOR_MODE_DAC_32, mode(SEL_MEMORY, DST_DAC_FIFO0, DAM | RELOAD | WIDTH_32)),
            (VENDOR_MODE_ADC_16, mode(SRC_ADC01, SEL_MEMORY, SAM | RELOAD | WIDTH_16)),
            (VENDOR_MODE_ADC_16_PLANAR, mode(SRC_ADC01, SEL_MEMORY, SAM | RELOAD | AUDIOTYPE | WIDTH_16)),
            (VENDOR_MODE_LCD, mode(SEL_MEMORY, DST_LCD, DAM | WIDTH_32)),
            (VENDOR_MODE_UART1_RX, mode(SRC_UART1_RX, SEL_MEMORY, SAM | WIDTH_8)),
            (VENDOR_MODE_UART1_TX, mode(SEL_MEMORY, DST_UART1_TX, DAM | TRM | WIDTH_8)),
            (VENDOR_MODE_DSP_IN, mode(SEL_MEMORY, DST_UNK_1D, 0)),
            (VENDOR_MODE_DSP_OUT, mode(SRC_UNK_1D, SEL_MEMORY, 0)),
        ];
        for (word, built) in modes {
            assert_eq!(word, built, "{word:#x}");
            assert_eq!(word & !defined, 0, "{word:#x} sets reserved bits");
            assert_ne!(TWS.get(word), 3, "{word:#x} uses the reserved width");
        }
        assert_eq!(TWS.get(VENDOR_MODE_DAC_16), TWS_16BIT);
        assert_eq!(TWS.val(TWS_8BIT), WIDTH_8);
    }

    #[test]
    fn selectors_fit() {
        use ctl::*;
        for s in [
            SRC_UART0_RX,
            SRC_UART3_RX,
            SRC_SD_MMC,
            SRC_SPI2_RX,
            SRC_ADC23,
            SRC_I2S_RX1,
            SRC_TWI0_RX,
            SRC_TWI1_RX,
            SRC_UNK_1D,
            DST_UART0_TX,
            DST_UART2_TX,
            DST_UART3_TX,
            DST_SD_MMC,
            DST_SPI0_TX,
            DST_SPI1_TX,
            DST_SPI2_TX,
            DST_DAC_FIFO1,
            DST_I2S_TX0,
            DST_I2S_TX1,
            DST_TWI0_TX,
            DST_TWI1_TX,
            DST_PWM,
            DST_NAND,
        ] {
            assert_eq!(SRCSL.val(s) >> SRCSL.shift, s);
        }
        assert_eq!(SRC_UART2_RX, 3);
        assert_eq!(SRC_SPI1_RX, 9);
        assert_eq!(SRC_I2S_RX0, 0x0D);
    }
}
