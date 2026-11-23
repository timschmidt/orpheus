//! SD/MMC host controllers SD0 `0xC0160000` and SD1 `0xC0170000` (datasheet
//! §3.4 memory map, §6.2 feature list only).
//!
//! The datasheet has **no register chapter** for this block. Register names
//! and field positions come from the sister chip's SDK header (`SD_*`, same
//! offsets: \[code\]); what the Rockbox port's `sdmmc-yotonym03.c` uses works
//! on the player (SD0, 4-bit, 24 MHz, DMA channel 4: \[HW\]).
//!
//! Instances: SD0 is the card slot. SD1 = the memory map's "SDIO" block at
//! `0xC0170000` (\[DS\] address; identity with CMU/RMU "SD1" \[inferred\]).
//! Clock `CMU_SD0CLK`/`SD1CLK` (`cmu`), gate and reset `DEVCLKEN0`/`RST0`
//! b2/b3, NVIC 17 "SDC" / 18 "SDIO", DMA device select 5/6 (`dma`).
//!
//! Unknown: meaning of most `CTL` and `STATE` bits beyond the port's use,
//! high-speed timing (U-SD-1), which IRQ and DMA select belong to which
//! instance (U-SD-3).

use crate::{Field, Reg, bit};

/// SD0 (card slot). \[DS\] §3.4 "SD/MMC"; \[HW\]
pub const BASE: usize = 0xC016_0000;
/// SD1. \[DS\] §3.4 "SDIO" block; instance numbering \[inferred\]
pub const SD1_BASE: usize = 0xC017_0000;
pub const COUNT: usize = 2;

/// Base address of instance `n` (0 = SD0, 1 = SD1).
#[must_use]
pub const fn base(n: usize) -> usize {
    BASE + 0x1_0000 * n
}

pub const EN_OFFSET: usize = 0x00;
pub const CTL_OFFSET: usize = 0x04;
pub const STATE_OFFSET: usize = 0x08;
pub const CMD_OFFSET: usize = 0x0C;
pub const ARG_OFFSET: usize = 0x10;
pub const RSPBUF_OFFSET: usize = 0x14;
pub const DAT_OFFSET: usize = 0x28;
pub const BLK_SIZE_OFFSET: usize = 0x2C;
pub const BLK_NUM_OFFSET: usize = 0x30;

/// SD_EN: enable, bus width, DMA select. \[code\] SDK; bits 0, 6, 7 \[HW\].
#[must_use]
pub const fn en(n: usize) -> Reg {
    Reg::at(base(n) + EN_OFFSET)
}
/// SD_CTL: transfer control; writing it with `TS` starts a command. \[code\]
/// SDK; values in [`mod@ctl`] \[HW\].
#[must_use]
pub const fn ctl(n: usize) -> Reg {
    Reg::at(base(n) + CTL_OFFSET)
}
/// SD_STATE: status and error flags of the last command. \[code\] SDK;
/// error mask \[HW\].
#[must_use]
pub const fn state(n: usize) -> Reg {
    Reg::at(base(n) + STATE_OFFSET)
}
/// SD_CMD: command index. \[HW\]
#[must_use]
pub const fn cmd(n: usize) -> Reg {
    Reg::at(base(n) + CMD_OFFSET)
}
/// SD_ARG: 32-bit command argument. \[HW\]
#[must_use]
pub const fn arg(n: usize) -> Reg {
    Reg::at(base(n) + ARG_OFFSET)
}
/// SD_RSPBUFi, i = 0..4: response words (RSPBUF4 holds 8 bits \[code\]).
/// Short (48-bit) response: card status = `RSPBUF1[7:0] : RSPBUF0[31:8]`.
/// R2: most significant word in RSPBUF3, least in RSPBUF0. \[HW\]
#[must_use]
pub const fn rspbuf(n: usize, i: usize) -> Reg {
    Reg::at(base(n) + RSPBUF_OFFSET + 4 * i)
}
/// SD_DAT: data FIFO port; the DMA source/destination (address constant). \[HW\]
#[must_use]
pub const fn dat(n: usize) -> Reg {
    Reg::at(base(n) + DAT_OFFSET)
}
/// SD_BLK_SIZE: block length in bytes, `[9:0]`. \[HW\] (512); width \[code\]
#[must_use]
pub const fn blk_size(n: usize) -> Reg {
    Reg::at(base(n) + BLK_SIZE_OFFSET)
}
/// SD_BLK_NUM: number of blocks, `[15:0]`. \[HW\] (≤ 16); width \[code\]
#[must_use]
pub const fn blk_num(n: usize) -> Reg {
    Reg::at(base(n) + BLK_NUM_OFFSET)
}

/// SD0 registers (the card slot).
pub const EN: Reg = en(0);
pub const CTL: Reg = ctl(0);
pub const STATE: Reg = state(0);
pub const CMD: Reg = cmd(0);
pub const ARG: Reg = arg(0);
pub const DAT: Reg = dat(0);
pub const BLK_SIZE: Reg = blk_size(0);
pub const BLK_NUM: Reg = blk_num(0);

pub mod en {
    use super::{Field, bit};
    /// Data bus width: 0 1-bit, 1 4-bit \[HW\]; 2 8-bit \[inferred\].
    pub const DATAWID: Field = Field::bits(1, 0);
    pub const DATAWID_1BIT: u32 = 0;
    pub const DATAWID_4BIT: u32 = 1;
    pub const DATAWID_8BIT: u32 = 2;
    /// SDIO mode. \[code\] SDK
    pub const SDIOEN: u32 = bit(3);
    /// Data FIFO accessed by DMA (port sets it at power-up and again before
    /// every data command). \[HW\]; name \[code\] SDK
    pub const AHB_DMA_SEL: u32 = bit(6);
    /// Controller enable. \[HW\]; name \[code\] SDK
    pub const EN: u32 = bit(7);
    /// Clock select. \[code\] SDK; meaning unknown
    pub const CLKS: u32 = bit(8);
    /// 1.8 V pad signalling. \[DS\] §6.2 feature; position \[code\] SDK
    pub const S18EN: u32 = bit(12);
}
/// SD_EN at power-up: enable, DMA select, 1-bit bus. \[HW\]
pub const VENDOR_EN_INIT: u32 = 0xC0;

pub mod ctl {
    use super::{Field, bit};
    /// Transfer mode \[HW\] (values below); name \[code\] SDK.
    pub const TM: Field = Field::bits(3, 0);
    pub const TM_NONE: u32 = 0;
    /// Short response with CRC check.
    pub const TM_R1: u32 = 1;
    /// 136-bit response.
    pub const TM_R2: u32 = 2;
    /// Short response without CRC (the port also uses it for R1b).
    pub const TM_R3: u32 = 3;
    pub const TM_READ: u32 = 4;
    pub const TM_WRITE: u32 = 5;
    /// Used once at power-up with `TS`: the 74+ initialisation clocks. \[HW\]
    /// value; meaning \[inferred\]
    pub const TM_INIT_CLOCKS: u32 = 8;
    /// \[code\] SDK name ("CRC7 enable"?); set only in the init word \[HW\].
    pub const C7EN: u32 = bit(5);
    /// \[code\] SDK name; set only in the init word \[HW\].
    pub const LBE: u32 = bit(6);
    /// Transfer start; reads 1 until the command (and data) phase ends. \[HW\]
    pub const TS: u32 = bit(7);
    /// \[code\] SDK; unused.
    pub const TCN: Field = Field::bits(11, 8);
    /// Send continuous clock (SDIO). \[code\] SDK, \[DS\] §6.2 feature.
    /// **Disagreement:** the port header names it `SD_CTL_INITCLK` (unused).
    pub const SCC: u32 = bit(12);
    /// \[code\] SDK; unused.
    pub const CMDLEN: u32 = bit(13);
    /// Output (write) delay chain. \[code\] SDK, \[DS\] §6.2 feature.
    pub const WDELAY: Field = Field::bits(19, 16);
    /// Input (read) latch delay chain. \[code\] SDK, \[DS\] §6.2 feature.
    pub const RDELAY: Field = Field::bits(23, 20);
    /// Data timeout count. \[code\] SDK
    pub const TOUTCNT: Field = Field::bits(30, 24);
    /// Data timeout enable. \[code\] SDK
    pub const TOUTEN: u32 = bit(31);
}
/// Delay chains of every port command: RDELAY 8, WDELAY 15. \[HW\] at ≤ 24 MHz
pub const VENDOR_CTL_TIMING: u32 = 0x008F_0000;
/// Added for data commands: TOUTEN, TOUTCNT 0x3F. \[HW\]
pub const VENDOR_CTL_DATA: u32 = 0xBF00_0000;
/// Power-up initialisation clocks: RDELAY 1, WDELAY 15, TS|LBE|C7EN, TM 8;
/// wait for TS to clear. \[HW\] VERBATIM
pub const VENDOR_CTL_INIT: u32 = 0x001F_00E8;

pub mod state {
    use super::bit;
    /// Response CRC7 error; always set for R3 (no CRC). \[HW\]; name \[code\]
    pub const CRC7ER: u32 = bit(0);
    /// Read-data CRC16 error. \[HW\] ("data CRC"); name \[code\]
    pub const RC16ER: u32 = bit(1);
    /// Write CRC16 error. \[code\] SDK. **Disagreement:** the port counts it as
    /// "other error".
    pub const WC16ER: u32 = bit(2);
    /// \[code\] SDK; meaning unknown.
    pub const CLC: u32 = bit(3);
    /// \[code\] SDK name ("command line no response"?). **Disagreement:** the
    /// port treats it as a CRC error and masks it for R3 with CRC7ER.
    pub const CLNR: u32 = bit(4);
    /// Transfer-end interrupt pending / enable. \[code\] SDK; unused (U-IRQ-1).
    pub const TEI: u32 = bit(5);
    pub const TEIE: u32 = bit(6);
    /// DAT0 line level (low = card busy). \[code\] SDK; the port polls CMD13
    /// instead \[inferred\] alternative for R1b/write busy.
    pub const DAT0S: u32 = bit(7);
    /// \[code\] SDK: SDIO enabled / SDIO interrupt pending, DAT1 and CMD line
    /// levels, memory ready.
    pub const SDIOEN: u32 = bit(8);
    pub const SDIOP: u32 = bit(9);
    pub const DAT1S: u32 = bit(10);
    pub const CMDS: u32 = bit(11);
    pub const MEMRDY: u32 = bit(12);
    /// \[code\] SDK; meaning unknown.
    pub const BAEP: u32 = bit(14);
    /// Timeout. \[HW\]; name \[code\]
    pub const TOUTE: u32 = bit(15);
    /// Error bits the port checks after every command. \[HW\]
    pub const ERR_MASK: u32 = 0x8017;
    /// Removed from [`ERR_MASK`] for responses without CRC (R3). \[HW\]
    pub const NOCRC_IGNORE: u32 = 0x11;
}

pub mod cmd {
    use super::Field;
    /// Command index. Width \[code\] SDK.
    pub const CMD: Field = Field::bits(7, 0);
}
pub mod blk_size {
    use super::Field;
    pub const BC: Field = Field::bits(9, 0);
}
pub mod blk_num {
    use super::Field;
    pub const BN: Field = Field::bits(15, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // atj2157.h
        assert_eq!(BASE, 0xC016_0000);
        assert_eq!(EN.addr(), 0xC016_0000);
        assert_eq!(CTL.addr(), 0xC016_0004);
        assert_eq!(STATE.addr(), 0xC016_0008);
        assert_eq!(CMD.addr(), 0xC016_000C);
        assert_eq!(ARG.addr(), 0xC016_0010);
        assert_eq!(rspbuf(0, 0).addr(), 0xC016_0014);
        assert_eq!(rspbuf(0, 1).addr(), 0xC016_0018);
        assert_eq!(rspbuf(0, 2).addr(), 0xC016_001C);
        assert_eq!(rspbuf(0, 3).addr(), 0xC016_0020);
        assert_eq!(DAT.addr(), 0xC016_0028);
        assert_eq!(BLK_SIZE.addr(), 0xC016_002C);
        assert_eq!(BLK_NUM.addr(), 0xC016_0030);
    }

    #[test]
    fn datasheet_and_sdk_addresses() {
        assert_eq!(base(1), SD1_BASE);
        assert_eq!(en(1).addr(), 0xC017_0000);
        assert_eq!(rspbuf(1, 4).addr(), 0xC017_0024);
        assert_eq!(blk_num(1).addr(), 0xC017_0030);
    }

    #[test]
    fn port_values() {
        assert_eq!(ctl::TS, 1 << 7);
        assert_eq!(ctl::SCC, 1 << 12);
        assert_eq!(ctl::RDELAY.val(8) | ctl::WDELAY.val(15), VENDOR_CTL_TIMING);
        assert_eq!(ctl::TOUTEN | ctl::TOUTCNT.val(0x3F), VENDOR_CTL_DATA);
        let init = ctl::RDELAY.val(1)
            | ctl::WDELAY.val(15)
            | ctl::TS
            | ctl::LBE
            | ctl::C7EN
            | ctl::TM.val(ctl::TM_INIT_CLOCKS);
        assert_eq!(init, VENDOR_CTL_INIT);
        assert_eq!(en::EN | en::AHB_DMA_SEL | en::DATAWID.val(en::DATAWID_1BIT), VENDOR_EN_INIT);
        let err = state::TOUTE | state::CLNR | state::WC16ER | state::RC16ER | state::CRC7ER;
        assert_eq!(err, state::ERR_MASK);
        assert_eq!(state::CRC7ER | state::CLNR, state::NOCRC_IGNORE);
        assert_eq!(ctl::TM.val(ctl::TM_WRITE), 5);
    }

    #[test]
    fn fields_disjoint() {
        assert_disjoint(&[en::DATAWID.mask(), en::SDIOEN, en::AHB_DMA_SEL, en::EN, en::CLKS, en::S18EN]);
        assert_disjoint(&[
            ctl::TM.mask(),
            ctl::C7EN,
            ctl::LBE,
            ctl::TS,
            ctl::TCN.mask(),
            ctl::SCC,
            ctl::CMDLEN,
            ctl::WDELAY.mask(),
            ctl::RDELAY.mask(),
            ctl::TOUTCNT.mask(),
            ctl::TOUTEN,
        ]);
        assert_disjoint(&[
            state::CRC7ER,
            state::RC16ER,
            state::WC16ER,
            state::CLC,
            state::CLNR,
            state::TEI,
            state::TEIE,
            state::DAT0S,
            state::SDIOEN,
            state::SDIOP,
            state::DAT1S,
            state::CMDS,
            state::MEMRDY,
            state::BAEP,
            state::TOUTE,
        ]);
    }

    #[test]
    fn register_order() {
        for n in 0..COUNT {
            assert_increasing(&[
                en(n).addr(),
                ctl(n).addr(),
                state(n).addr(),
                cmd(n).addr(),
                arg(n).addr(),
                rspbuf(n, 0).addr(),
                rspbuf(n, 4).addr(),
                dat(n).addr(),
                blk_size(n).addr(),
                blk_num(n).addr(),
            ]);
        }
    }
}
