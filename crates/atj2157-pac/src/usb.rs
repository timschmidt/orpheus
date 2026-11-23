//! USB 2.0 device controller, `0xC0140000` (datasheet §3.4 memory map, §7.6
//! feature list only; NVIC line 12, `irq::USB`).
//!
//! The datasheet has **no register chapter** for this block. Layout: a Faraday
//! FOTG210-style device core at `0x100–0x1FF` plus an Actions DMA engine at
//! `0x300–0x33C` that replaces the FOTG210 DMA/PIO port (all data, SETUP
//! included, moves by DMA). Sources: the Rockbox port's `usb-atj2157.c`
//! (enumerates at high speed and runs mass storage: \[HW\]), `yoton/re/
//! usb_controller.md` (ROM ADFU stack and UDISK.AP: \[code\]) and the FOTG210
//! register layout (\[inferred\]). The sister chip's SDK header describes a
//! different USB core and was not used.
//!
//! Clock gate `CMU_DEVCLKEN0` b15, resets `RMU_RST0` b15/b16 (\[DS\] names
//! USBRESET / USBRESET2), PHY clock word `0xC0001100 = 0x3055` and the
//! bus-master enables at `0xC0040004` belong to the `cmu`, `rmu` and `memctl`
//! modules.
//!
//! Unknown (01-hardware §9 U-USB-1…12): the registers below `0x100` (an
//! FOTG210 keeps its EHCI host/OTG registers there), `0x32C`–`0x33C`, DMA
//! interrupts, short packets ending a multi-packet OUT DMA, EP0 transfers over
//! 64 bytes and zero-length packets, interrupt/isochronous FIFO types.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC014_0000;

/// Written 0 by the ROM and the port, 1 by UDISK.AP; purpose unknown (below
/// the device core). \[code\] usb_controller.md §2; port writes 0 \[HW\].
pub const UNK_03C: Reg = Reg::at(BASE + 0x03C);
/// Value the ROM (and the port) writes to [`UNK_03C`] at init. \[HW\]
pub const VENDOR_UNK_03C: u32 = 0;

/// DMCR, device main control. FOTG210 name; bits 2–6, 9 used by the port \[HW\].
pub const DMCR: Reg = Reg::at(BASE + 0x100);
pub mod dmcr {
    use super::bit;
    /// Remote wake-up. \[inferred\] (FOTG210); never used.
    pub const RMWAKUP: u32 = bit(0);
    /// FOTG210 "half speed" (FPGA test). Set by the ROM, kernel and the port
    /// after the soft reset; UDISK.AP leaves it 0 and works. \[code\]; port \[HW\]
    pub const HALF_SPEED: u32 = bit(1);
    /// Global interrupt enable of the core. \[HW\]
    pub const GLINT_EN: u32 = bit(2);
    /// Go to suspend (set before power-down). \[HW\]
    pub const GOSUSP: u32 = bit(3);
    /// Soft reset; self-clearing (poll until 0). \[HW\]
    pub const SFRST: u32 = bit(4);
    /// Chip enable. \[HW\]
    pub const CHIP_EN: u32 = bit(5);
    /// Read-only: 1 = high speed negotiated (valid after bus reset). \[HW\]
    pub const HS_EN: u32 = bit(6);
    /// Force full speed (no chirp). The port clears it; the kernel's VBUS
    /// detect sets it. \[HW\]
    pub const FORCE_FS: u32 = bit(9);
}

/// DAR, device address. Write the SET_ADDRESS value before CX_DONE; the core
/// applies it after the status stage. \[HW\]
pub const DAR: Reg = Reg::at(BASE + 0x104);
pub mod dar {
    use super::{Field, bit};
    pub const ADDR: Field = Field::bits(6, 0);
    /// Set after SET_CONFIGURATION (non-zero), cleared for configuration 0. \[HW\]
    pub const AFT_CONF: u32 = bit(7);
}

/// DTR, device test register. \[HW\]
pub const DTR: Reg = Reg::at(BASE + 0x108);
pub mod dtr {
    use super::bit;
    /// Clear all FIFOs (at init and at bus reset). \[HW\]
    pub const TST_CLRFF: u32 = bit(0);
}

/// SOFFNR, SOF frame number. The kernel takes a value above 1 as "a host is
/// sending SOFs" (PC rather than charger). \[code\] usb_controller.md §8
pub const SOFFNR: Reg = Reg::at(BASE + 0x10C);
pub mod soffnr {
    use super::Field;
    pub const FNR: Field = Field::bits(10, 0);
}

/// PHYTMSR, PHY test mode select. \[HW\]
pub const PHYTMSR: Reg = Reg::at(BASE + 0x114);
pub mod phytmsr {
    use super::bit;
    /// 1 = soft-detached (D+ pull-up off). Port pulses it 20 ms to attach. \[HW\]
    pub const UNPLUG: u32 = bit(0);
    /// USB test modes: bit = selector (1 J, 2 K, 3 SE0_NAK, 4 PACKET), as
    /// SET_FEATURE(TEST_MODE) sets them. \[code\] ROM; port \[HW\] (not run)
    pub const TST_JSTA: u32 = bit(1);
    pub const TST_KSTA: u32 = bit(2);
    pub const TST_SE0NAK: u32 = bit(3);
    pub const TST_PKT: u32 = bit(4);
}

/// CXFIFO (FOTG210 DCFESR): control-endpoint config and FIFO-empty status.
/// \[HW\] for CX_DONE/CX_STALL.
pub const CXFIFO: Reg = Reg::at(BASE + 0x120);
pub mod cxfifo {
    use super::bit;
    /// Finish the control transfer: the core runs the status stage. \[HW\]
    pub const CX_DONE: u32 = bit(0);
    /// Set after loading the 53-byte test packet. \[code\] ROM
    pub const TST_PKDONE: u32 = bit(1);
    /// Stall EP0; cleared by the core on the next SETUP. \[HW\] (set; clearing \[inferred\])
    pub const CX_STALL: u32 = bit(2);
    /// Clear the CX FIFO. \[inferred\] (FOTG210)
    pub const CX_CLR: u32 = bit(3);
    /// CX FIFO empty. \[inferred\] (FOTG210)
    pub const CX_EMP: u32 = bit(5);
    /// FIFO `k` (0..3) empty. \[inferred\] (FOTG210); see U-USB-7.
    #[must_use]
    pub const fn fifo_empty(k: u8) -> u32 {
        bit(8 + k)
    }
}

/// Idle counter (ms before PHY suspend). \[HW\]
pub const IDLE: Reg = Reg::at(BASE + 0x124);
/// Value written to [`IDLE`] by the ROM, UDISK.AP, kernel and port. \[HW\]
pub const VENDOR_IDLE: u32 = 7;

/// GIMR, group interrupt mask: bit n masks group n (1 = masked). \[HW\]
pub const GIMR: Reg = Reg::at(BASE + 0x130);
/// GIMR0: group-0 (EP0) source mask, bit layout as [`gisr0`]. \[HW\]
pub const GIMR0: Reg = Reg::at(BASE + 0x134);
/// GIMR1: group-1 (FIFO) source mask, bit layout as [`gisr1`]. \[HW\]
pub const GIMR1: Reg = Reg::at(BASE + 0x138);
/// GIMR2: group-2 (bus events) source mask, bit layout as [`gisr2`]. \[HW\]
pub const GIMR2: Reg = Reg::at(BASE + 0x13C);
/// GISR, group interrupt status (RO): pending = `GISR & !GIMR`. \[HW\]
pub const GISR: Reg = Reg::at(BASE + 0x140);
/// GISR0: EP0 sources. \[HW\] for CX_SETUP.
pub const GISR0: Reg = Reg::at(BASE + 0x144);
/// GISR1: FIFO sources, levels that follow the FIFO state (never acked). \[HW\]
pub const GISR1: Reg = Reg::at(BASE + 0x148);
/// GISR2: bus events, **write 1 to clear**. \[HW\]
pub const GISR2: Reg = Reg::at(BASE + 0x14C);

/// Bits of [`GIMR`] and [`GISR`].
pub mod gisr {
    use super::bit;
    /// Group 0: EP0 / control. \[HW\]
    pub const G0: u32 = bit(0);
    /// Group 1: data FIFOs. \[HW\]
    pub const G1: u32 = bit(1);
    /// Group 2: bus events. \[HW\]
    pub const G2: u32 = bit(2);
}

/// Bits of [`GIMR0`] and [`GISR0`].
pub mod gisr0 {
    use super::bit;
    /// 8-byte SETUP waiting; reading it through DMA channel 0 clears it
    /// (the port never acks it and works: U-USB-8 answered in practice). \[HW\]
    pub const CX_SETUP: u32 = bit(0);
    /// \[inferred\] (FOTG210) CX IN / OUT / command end / command fail.
    pub const CX_IN: u32 = bit(1);
    pub const CX_OUT: u32 = bit(2);
    pub const CX_COMEND: u32 = bit(3);
    pub const CX_COMFAIL: u32 = bit(4);
    /// Command abort, W1C (cleared at init). \[HW\]
    pub const CX_COMABT: u32 = bit(5);
}
/// [`GIMR0`] at init: only CX_SETUP unmasked. \[HW\]
pub const VENDOR_GIMR0: u32 = 0x3E;

/// Bits of [`GIMR1`] and [`GISR1`]; `k` = FIFO number 0..3.
pub mod gisr1 {
    use super::bit;
    /// FIFO `k` holds a full OUT packet. \[HW\] (k = 2)
    #[must_use]
    pub const fn fifo_out(k: u8) -> u32 {
        bit(2 * k)
    }
    /// FIFO `k` holds a short OUT packet (e.g. the 31-byte CBW). \[HW\] (k = 2)
    #[must_use]
    pub const fn fifo_spk(k: u8) -> u32 {
        bit(2 * k + 1)
    }
    /// FIFO `k` is empty and wants IN data. \[code\] usb_controller.md §3.1
    #[must_use]
    pub const fn fifo_in(k: u8) -> u32 {
        bit(16 + k)
    }
}
/// [`GIMR1`] at init and bus reset: every FIFO source masked. \[HW\]
pub const VENDOR_GIMR1: u32 = 0x000F_00FF;

/// Bits of [`GIMR2`] and [`GISR2`].
pub mod gisr2 {
    use super::bit;
    /// USB bus reset. \[HW\]
    pub const USBRST: u32 = bit(0);
    /// Suspend. \[HW\]
    pub const SUSP: u32 = bit(1);
    /// Resume. \[HW\]
    pub const RESM: u32 = bit(2);
    /// \[inferred\] (FOTG210): ISO sequence error / abort, zero-length IN sent,
    /// zero-length OUT received, FOTG210 DMA complete / error (U-USB-2, U-USB-3).
    pub const ISO_SEQ_ERR: u32 = bit(3);
    pub const ISO_SEQ_ABORT: u32 = bit(4);
    pub const TX0BYTE: u32 = bit(5);
    pub const RX0BYTE: u32 = bit(6);
    pub const DMA_CMPLT: u32 = bit(7);
    pub const DMA_ERROR: u32 = bit(8);
}
/// [`GIMR2`] at init: only reset, suspend, resume unmasked. \[HW\]
pub const VENDOR_GIMR2: u32 = 0x7F8;
/// Written to [`GISR2`] at init to clear every source. \[HW\]
pub const VENDOR_GISR2_CLEAR: u32 = 0x1FF;

/// INEPMPS: IN endpoint `n` (1..8) max-packet / control. \[HW\] (n = 1)
#[must_use]
pub const fn inepmps(n: usize) -> Reg {
    Reg::at(BASE + 0x160 + 4 * (n - 1))
}
/// OUTEPMPS: OUT endpoint `n` (1..8) max-packet / control. \[HW\] (n = 2)
#[must_use]
pub const fn outepmps(n: usize) -> Reg {
    Reg::at(BASE + 0x180 + 4 * (n - 1))
}
/// Bits of [`inepmps`] and [`outepmps`]. Bus reset writes `RSTG` then 0 to
/// all 16. \[HW\]
pub mod epmps {
    use super::{Field, bit};
    /// Max packet size (512 high speed, 64 full speed for bulk). \[HW\]
    pub const MAXPS: Field = Field::bits(10, 0);
    /// Stall the endpoint. \[HW\]
    pub const STL: u32 = bit(11);
    /// Reset the data toggle: write 1, then 0 (also on un-stall). \[HW\]
    pub const RSTG: u32 = bit(12);
}

/// EPMAP14: FIFO numbers of endpoints 1–4, one byte per endpoint. \[HW\]
pub const EPMAP14: Reg = Reg::at(BASE + 0x1A0);
/// EPMAP58: FIFO numbers of endpoints 5–8. Only reset by the port. \[HW\]
pub const EPMAP58: Reg = Reg::at(BASE + 0x1A4);
/// Map register holding endpoint `ep` (1..8).
#[must_use]
pub const fn epmap(ep: usize) -> Reg {
    if ep <= 4 { EPMAP14 } else { EPMAP58 }
}
pub mod epmap {
    use super::Field;
    /// IN FIFO number of endpoint `ep` (1..8) in its map register. \[HW\]
    #[must_use]
    pub const fn in_fifo(ep: usize) -> Field {
        Field::new((8 * ((ep - 1) % 4)) as u8, 2)
    }
    /// OUT FIFO number of endpoint `ep` (1..8). \[HW\]
    #[must_use]
    pub const fn out_fifo(ep: usize) -> Field {
        Field::new((8 * ((ep - 1) % 4) + 4) as u8, 2)
    }
}
/// Reset value of both EPMAP registers (written back at bus reset). \[HW\]
pub const EPMAP_RESET: u32 = 0x3333_3333;
/// Vendor MSC mapping: `EPMAP14 = (EPMAP14 & !CLEAR) | SET` puts EP1 IN on
/// FIFO 0 and EP2 OUT on FIFO 2. \[HW\]
pub const VENDOR_EPMAP14_CLEAR: u32 = 0x3003;
pub const VENDOR_EPMAP14_SET: u32 = 0x2000;

/// FIFOMAP: endpoint and direction of FIFOs 0–3, one byte per FIFO. \[HW\]
pub const FIFOMAP: Reg = Reg::at(BASE + 0x1A8);
pub mod fifomap {
    use super::Field;
    /// Endpoint number served by FIFO `k` (0..3). \[HW\]
    #[must_use]
    pub const fn epno(k: usize) -> Field {
        Field::new((8 * k) as u8, 4)
    }
    /// Direction of FIFO `k`: 0 OUT, 1 IN \[HW\]; 2 bidirectional \[inferred\].
    #[must_use]
    pub const fn dir(k: usize) -> Field {
        Field::new((8 * k + 4) as u8, 2)
    }
    pub const DIR_OUT: u32 = 0;
    pub const DIR_IN: u32 = 1;
}
/// Reset value of [`FIFOMAP`] (written back at bus reset). \[HW\]
pub const FIFOMAP_RESET: u32 = 0x0F0F_0F0F;
/// Vendor MSC mapping: `FIFOMAP = (FIFOMAP & KEEP) | SET` → FIFO 0 = EP1 IN,
/// FIFO 2 = EP2 OUT. \[HW\]
pub const VENDOR_FIFOMAP_KEEP: u32 = 0xFF00_FF00;
pub const VENDOR_FIFOMAP_SET: u32 = 0x0002_0011;

/// FIFOCF: per-FIFO configuration, one byte per FIFO; 0 at bus reset. \[HW\]
pub const FIFOCF: Reg = Reg::at(BASE + 0x1AC);
pub mod fifocf {
    use super::Field;
    /// Transfer type of FIFO `k`: 2 bulk \[HW\]; 1 isochronous, 3 interrupt
    /// \[inferred\] (USB encoding as in FOTG210; U-USB-11, U-USB-12).
    #[must_use]
    pub const fn typ(k: usize) -> Field {
        Field::new((8 * k) as u8, 2)
    }
    /// Number of blocks − 1 (double buffering takes the next FIFO). \[code\] UDISK
    #[must_use]
    pub const fn blkno(k: usize) -> Field {
        Field::new((8 * k + 2) as u8, 2)
    }
    /// Block size: 1 = 1024 bytes. \[code\]
    #[must_use]
    pub const fn blksz(k: usize) -> Field {
        Field::new((8 * k + 4) as u8, 1)
    }
    /// FIFO enable. \[HW\]
    #[must_use]
    pub const fn en(k: usize) -> Field {
        Field::new((8 * k + 5) as u8, 1)
    }
    pub const TYPE_ISO: u32 = 1;
    pub const TYPE_BULK: u32 = 2;
    pub const TYPE_INT: u32 = 3;
}
/// FIFOCF of the ROM and the port: FIFO 0 and 2 single-buffered bulk. \[HW\]
pub const VENDOR_FIFOCF: u32 = 0x0022_0022;
/// FIFOCF of UDISK.AP: FIFO 0 double-buffered bulk (FIFO 1 its second block),
/// FIFO 2 single bulk. \[code\]
pub const VENDOR_FIFOCF_UDISK: u32 = 0x0022_0626;

/// FIBC: byte count of FIFO `k` (0..3). \[HW\] (k = 2)
#[must_use]
pub const fn fibc(k: usize) -> Reg {
    Reg::at(BASE + 0x1B0 + 4 * k)
}
pub mod fibc {
    use super::{Field, bit};
    /// Bytes waiting in the (OUT) FIFO. \[HW\]
    pub const BC: Field = Field::bits(10, 0);
    /// FIFO reset (drops a packet, e.g. a zero-length one). \[HW\]
    pub const FFRST: u32 = bit(12);
}

/// DMACPSR1, the unused FOTG210 DMA parameter register. Init clears
/// bits 31:30 and 0: `&= VENDOR_DMACPSR1_KEEP`. \[HW\]; name \[inferred\]
pub const DMACPSR1: Reg = Reg::at(BASE + 0x1C8);
pub const VENDOR_DMACPSR1_KEEP: u32 = 0x3FFF_FFFE;

/// Actions DMA channel 0 (EP0 / CX FIFO), control. Layout [`udma`]. \[HW\]
pub const DMA0_CTL: Reg = Reg::at(BASE + 0x300);
/// Channel 0 buffer address (word-aligned). \[HW\]
pub const DMA0_ADDR: Reg = Reg::at(BASE + 0x304);
/// Actions DMA channel for FIFO `k` (0..3), control. Layout [`udma`]. \[HW\] (k = 0, 2)
#[must_use]
pub const fn dma_ctl(k: usize) -> Reg {
    Reg::at(BASE + 0x308 + 8 * k)
}
/// Buffer address for FIFO `k`'s channel; reads back the current address
/// \[inferred\]. \[HW\]
#[must_use]
pub const fn dma_addr(k: usize) -> Reg {
    Reg::at(BASE + 0x30C + 8 * k)
}
/// Bits of [`DMA0_CTL`] and [`dma_ctl`]. Writing 0 aborts the channel. \[HW\]
pub mod udma {
    use super::{Field, bit};
    /// Start; reads 1 while busy. \[HW\]
    pub const START: u32 = bit(0);
    /// 1 = memory → USB (IN), 0 = USB → memory (OUT, SETUP). \[HW\]
    pub const IN: u32 = bit(1);
    /// Length in bytes, up to 65536 (17 bits); may read back the remaining
    /// count \[code\]. The port uses ≤ 32 KiB per start. \[HW\]
    pub const LEN: Field = Field::bits(24, 8);
}

/// DMA_STAT, Actions DMA done/error flags, **W1C**. \[HW\]
pub const DMA_STAT: Reg = Reg::at(BASE + 0x328);
pub mod dma_stat {
    use super::bit;
    /// Channel 0 (EP0) done / error. \[HW\]
    pub const EP0_DONE: u32 = bit(0);
    pub const EP0_ERR: u32 = bit(16);
    /// FIFO `k` channel done. \[HW\]
    #[must_use]
    pub const fn fifo_done(k: u8) -> u32 {
        bit(1 + k)
    }
    /// FIFO `k` channel error / abort. \[HW\]
    #[must_use]
    pub const fn fifo_err(k: u8) -> u32 {
        bit(17 + k)
    }
    /// All five done and error flags (init and bus reset clear these). \[HW\]
    pub const ALL: u32 = 0x001F_001F;
}

/// Written `0x1F001F` at init; probably a mask for the [`DMA_STAT`] sources
/// (1 = masked), U-USB-3. \[HW\] value; meaning \[inferred\]
pub const UNK_32C: Reg = Reg::at(BASE + 0x32C);
pub const VENDOR_UNK_32C: u32 = 0x001F_001F;
/// DMA_EN: write 1 before every channel start, 0 to abort all. \[HW\]
pub const DMA_EN: Reg = Reg::at(BASE + 0x330);
/// Written 0 at init; purpose unknown. \[HW\] value
pub const UNK_334: Reg = Reg::at(BASE + 0x334);
pub const VENDOR_UNK_334: u32 = 0;
/// Written 1 at init; purpose unknown. \[HW\] value
pub const UNK_338: Reg = Reg::at(BASE + 0x338);
pub const VENDOR_UNK_338: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // usb-atj2157.c
        assert_eq!(BASE, 0xC014_0000);
        assert_eq!(UNK_03C.addr(), 0xC014_003C);
        assert_eq!(DMCR.addr(), 0xC014_0100);
        assert_eq!(DAR.addr(), 0xC014_0104);
        assert_eq!(DTR.addr(), 0xC014_0108);
        assert_eq!(SOFFNR.addr(), 0xC014_010C);
        assert_eq!(PHYTMSR.addr(), 0xC014_0114);
        assert_eq!(CXFIFO.addr(), 0xC014_0120);
        assert_eq!(IDLE.addr(), 0xC014_0124);
        assert_eq!(GIMR.addr(), 0xC014_0130);
        assert_eq!(GIMR0.addr(), 0xC014_0134);
        assert_eq!(GIMR1.addr(), 0xC014_0138);
        assert_eq!(GIMR2.addr(), 0xC014_013C);
        assert_eq!(GISR.addr(), 0xC014_0140);
        assert_eq!(GISR0.addr(), 0xC014_0144);
        assert_eq!(GISR1.addr(), 0xC014_0148);
        assert_eq!(GISR2.addr(), 0xC014_014C);
        assert_eq!(inepmps(1).addr(), 0xC014_0160);
        assert_eq!(inepmps(8).addr(), 0xC014_017C);
        assert_eq!(outepmps(1).addr(), 0xC014_0180);
        assert_eq!(outepmps(2).addr(), 0xC014_0184);
        assert_eq!(outepmps(8).addr(), 0xC014_019C);
        assert_eq!(EPMAP14.addr(), 0xC014_01A0);
        assert_eq!(EPMAP58.addr(), 0xC014_01A4);
        assert_eq!(FIFOMAP.addr(), 0xC014_01A8);
        assert_eq!(FIFOCF.addr(), 0xC014_01AC);
        assert_eq!(fibc(0).addr(), 0xC014_01B0);
        assert_eq!(fibc(2).addr(), 0xC014_01B8);
        assert_eq!(DMACPSR1.addr(), 0xC014_01C8);
        assert_eq!(DMA0_CTL.addr(), 0xC014_0300);
        assert_eq!(DMA0_ADDR.addr(), 0xC014_0304);
        assert_eq!(dma_ctl(0).addr(), 0xC014_0308);
        assert_eq!(dma_addr(0).addr(), 0xC014_030C);
        assert_eq!(dma_ctl(2).addr(), 0xC014_0318);
        assert_eq!(dma_addr(2).addr(), 0xC014_031C);
        assert_eq!(dma_ctl(3).addr(), 0xC014_0320);
        assert_eq!(DMA_STAT.addr(), 0xC014_0328);
        assert_eq!(UNK_32C.addr(), 0xC014_032C);
        assert_eq!(DMA_EN.addr(), 0xC014_0330);
        assert_eq!(UNK_334.addr(), 0xC014_0334);
        assert_eq!(UNK_338.addr(), 0xC014_0338);
        assert_eq!(epmap(1), EPMAP14);
        assert_eq!(epmap(5), EPMAP58);
    }

    #[test]
    fn port_values() {
        // Bit positions and words the port writes.
        assert_eq!(dmcr::GLINT_EN | dmcr::GOSUSP | dmcr::SFRST | dmcr::CHIP_EN, 0x3C);
        assert_eq!(dmcr::HS_EN, 0x40);
        assert_eq!(dmcr::FORCE_FS, 0x200);
        assert_eq!(dar::AFT_CONF, 0x80);
        assert_eq!(epmps::STL, 0x800);
        assert_eq!(epmps::RSTG, 0x1000);
        assert_eq!(fibc::FFRST, 0x1000);
        assert_eq!(gisr1::fifo_out(2) | gisr1::fifo_spk(2), 0x30);
        assert_eq!(dma_stat::fifo_done(0), 0x2);
        assert_eq!(dma_stat::fifo_err(2), 1 << 19);
        assert_eq!(dma_stat::EP0_DONE | dma_stat::EP0_ERR, 0x0001_0001);
        // Port's cancel-all clears the four FIFO channels only.
        let fifos = (0..4).fold(0, |a, k| a | dma_stat::fifo_done(k) | dma_stat::fifo_err(k));
        assert_eq!(fifos, 0x001E_001E);
        assert_eq!(fifos | dma_stat::EP0_DONE | dma_stat::EP0_ERR, dma_stat::ALL);
        // EP1 IN -> FIFO 0, EP2 OUT -> FIFO 2.
        let clear = epmap::in_fifo(1).mask() | epmap::out_fifo(2).mask();
        assert_eq!(clear, VENDOR_EPMAP14_CLEAR);
        assert_eq!(epmap::in_fifo(1).val(0) | epmap::out_fifo(2).val(2), VENDOR_EPMAP14_SET);
        let set = fifomap::epno(0).val(1)
            | fifomap::dir(0).val(fifomap::DIR_IN)
            | fifomap::epno(2).val(2)
            | fifomap::dir(2).val(fifomap::DIR_OUT);
        assert_eq!(set, VENDOR_FIFOMAP_SET);
        let one = fifocf::typ(0).val(fifocf::TYPE_BULK) | fifocf::en(0).val(1);
        assert_eq!(one | (one << 16), VENDOR_FIFOCF);
        let dbl = one | fifocf::blkno(0).val(1);
        let second = fifocf::typ(1).val(fifocf::TYPE_BULK) | fifocf::blkno(1).val(1);
        assert_eq!(dbl | second | (one << 16), VENDOR_FIFOCF_UDISK);
        assert_eq!(udma::LEN.val(8) | udma::IN, 0x802);
    }

    #[test]
    fn fields_disjoint() {
        assert_disjoint(&[
            dmcr::RMWAKUP,
            dmcr::HALF_SPEED,
            dmcr::GLINT_EN,
            dmcr::GOSUSP,
            dmcr::SFRST,
            dmcr::CHIP_EN,
            dmcr::HS_EN,
            dmcr::FORCE_FS,
        ]);
        assert_disjoint(&[dar::ADDR.mask(), dar::AFT_CONF]);
        assert_disjoint(&[
            phytmsr::UNPLUG,
            phytmsr::TST_JSTA,
            phytmsr::TST_KSTA,
            phytmsr::TST_SE0NAK,
            phytmsr::TST_PKT,
        ]);
        let mut cx =
            std::vec![cxfifo::CX_DONE, cxfifo::TST_PKDONE, cxfifo::CX_STALL, cxfifo::CX_CLR, cxfifo::CX_EMP];
        cx.extend((0..4).map(cxfifo::fifo_empty));
        assert_disjoint(&cx);
        assert_disjoint(&[gisr::G0, gisr::G1, gisr::G2]);
        assert_disjoint(&[
            gisr0::CX_SETUP,
            gisr0::CX_IN,
            gisr0::CX_OUT,
            gisr0::CX_COMEND,
            gisr0::CX_COMFAIL,
            gisr0::CX_COMABT,
        ]);
        let g1: std::vec::Vec<u32> =
            (0..4).flat_map(|k| [gisr1::fifo_out(k), gisr1::fifo_spk(k), gisr1::fifo_in(k)]).collect();
        assert_disjoint(&g1);
        assert_disjoint(&[
            gisr2::USBRST,
            gisr2::SUSP,
            gisr2::RESM,
            gisr2::ISO_SEQ_ERR,
            gisr2::ISO_SEQ_ABORT,
            gisr2::TX0BYTE,
            gisr2::RX0BYTE,
            gisr2::DMA_CMPLT,
            gisr2::DMA_ERROR,
        ]);
        assert_disjoint(&[epmps::MAXPS.mask(), epmps::STL, epmps::RSTG]);
        let em: std::vec::Vec<u32> =
            (1..=4).flat_map(|ep| [epmap::in_fifo(ep).mask(), epmap::out_fifo(ep).mask()]).collect();
        assert_disjoint(&em);
        let fm: std::vec::Vec<u32> =
            (0..4).flat_map(|k| [fifomap::epno(k).mask(), fifomap::dir(k).mask()]).collect();
        assert_disjoint(&fm);
        let fc: std::vec::Vec<u32> = (0..4)
            .flat_map(|k| {
                [
                    fifocf::typ(k).mask(),
                    fifocf::blkno(k).mask(),
                    fifocf::blksz(k).mask(),
                    fifocf::en(k).mask(),
                ]
            })
            .collect();
        assert_disjoint(&fc);
        assert_disjoint(&[fibc::BC.mask(), fibc::FFRST]);
        assert_disjoint(&[udma::START, udma::IN, udma::LEN.mask()]);
        let ds: std::vec::Vec<u32> = [dma_stat::EP0_DONE, dma_stat::EP0_ERR]
            .into_iter()
            .chain((0..4).flat_map(|k| [dma_stat::fifo_done(k), dma_stat::fifo_err(k)]))
            .collect();
        assert_disjoint(&ds);
        // Group masks mirror the status registers.
        assert_eq!(VENDOR_GIMR0 | gisr0::CX_SETUP, 0x3F);
        assert_eq!(VENDOR_GIMR2 & (gisr2::USBRST | gisr2::SUSP | gisr2::RESM), 0);
    }

    #[test]
    fn register_order() {
        assert_increasing(&[
            UNK_03C.addr(),
            DMCR.addr(),
            DAR.addr(),
            DTR.addr(),
            SOFFNR.addr(),
            PHYTMSR.addr(),
            CXFIFO.addr(),
            IDLE.addr(),
            GIMR.addr(),
            GIMR0.addr(),
            GIMR1.addr(),
            GIMR2.addr(),
            GISR.addr(),
            GISR0.addr(),
            GISR1.addr(),
            GISR2.addr(),
            inepmps(1).addr(),
            inepmps(8).addr(),
            outepmps(1).addr(),
            outepmps(8).addr(),
            EPMAP14.addr(),
            EPMAP58.addr(),
            FIFOMAP.addr(),
            FIFOCF.addr(),
            fibc(0).addr(),
            fibc(3).addr(),
            DMACPSR1.addr(),
            DMA0_CTL.addr(),
            DMA0_ADDR.addr(),
            dma_ctl(0).addr(),
            dma_addr(0).addr(),
            dma_ctl(3).addr(),
            dma_addr(3).addr(),
            DMA_STAT.addr(),
            UNK_32C.addr(),
            DMA_EN.addr(),
            UNK_334.addr(),
            UNK_338.addr(),
        ]);
    }
}
