//! LCD controller ("LCM"), `0xC01A0000` (datasheet §9.1, "LCDC").
//!
//! Sources: datasheet §9.1 \[DS\]; the Rockbox port `lcd-yotonym03.c` and the payloads
//! `lcdtest.c`/`lcdread.c` \[HW\]; the vendor `DRV_LCD.DRV` (text `0x119700…`, bank 2 init)
//! \[code\]; system.md §1.5, audio.md §6.
//!
//! The YM03 panel (ST7735S class, 128×128) hangs on the CPU interface: 8080 mode,
//! 8-bit bus, CE0. Two ways to feed it:
//!
//! * **AHB mode** (`CTL.IN_BUS = 0`): every write to [`DATA`] starts a bus cycle. Strobe
//!   timing comes from [`EXT_CLK`], counted in **AHB clocks** \[DS\]. The port sends
//!   bytes with `AHB_CFG_DATA = 1` (raw byte, D/C from `CTL.RS`) \[HW\]; the vendor pixel
//!   path sends one RGB565 pixel per 32-bit write with `AHB_CFG_DATA = 0` \[code\].
//! * **DMA mode** (`IN_BUS = 1`, `EMDE = 1`): a DMA channel fills the FIFO behind
//!   [`DATA`]; `STARTT` starts, `IF_PCS.LCDFI` reports the end. Strobe timing comes
//!   from [`LCDC_CLK`], counted in **LCD_CLK** (CMU_LCDCLK) cycles \[DS\].
//!
//! Unknown: what `AHB_CFG_DATA = 0` does to a command byte (`RS = 0`); whether the
//! DMA path sends the high byte first on the 8-bit bus (DS: `MLS`); see U-LCM-1/2.
//! Clock source (CMU_LCDCLK), gate (CLKEN0 b13) and IRQ number (42) live in `cmu`/`irq`.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC01A_0000;

pub const CTL_OFFSET: usize = 0x00;
pub const DISP_SIZE_OFFSET: usize = 0x04;
pub const LCDC_CLK_OFFSET: usize = 0x08;
pub const EXT_CLK_OFFSET: usize = 0x0C;
pub const DEFAULT_COLOR_OFFSET: usize = 0x10;
pub const DATA_OFFSET: usize = 0x14;
pub const IF_PCS_OFFSET: usize = 0x24;
pub const DATA1_OFFSET: usize = 0x40;

/// LCD_CTL. \[DS\] §9.1.3.1. Reset `0x0000_0000`. The port and vendor init write
/// [`VENDOR_CTL_INIT`] \[HW\].
pub const CTL: Reg = Reg::at(BASE + CTL_OFFSET);
pub mod ctl {
    use super::{Field, bit};
    /// Clock gate enable. \[DS\]
    pub const CLK_GAT_EN: u32 = bit(31);
    /// Parity check enable (result in `IF_PCS.PCS`). \[DS\]
    pub const PC_EN: u32 = bit(30);
    /// Swap R and B of the input data. \[DS\]
    pub const RB_SWAP: u32 = bit(29);
    /// Chip select: 0 CE0, 1 CE1. YM03 CS = GPIO43 = `LCD_CE0` (pin list); port clears it
    /// \[HW\]. The vendor exports set/clear helpers (`0x119A00`/`0x119A0E`) \[code\].
    pub const CESEL: u32 = bit(28);
    /// 0 AHB (PIO) transfer mode, 1 DMA transfer mode. \[DS\]; DMA set by vendor \[code\].
    pub const IN_BUS: u32 = bit(27);
    /// TE (tearing effect) detect enable. \[DS\]
    pub const TEFUNCEN: u32 = bit(26);
    /// TE input polarity inversion. \[DS\]
    pub const TEINV: u32 = bit(25);
    /// Start a (DMA) transfer; set after all other settings, cleared by hardware once
    /// synchronised to LCD_CLK. \[DS\]; vendor sets it after starting DMA \[code\].
    pub const STARTT: u32 = bit(24);
    /// Odd/even line colour sequences (RGB IF only). \[DS\]
    pub const ODD_EVEN_L: u32 = bit(23);
    /// RGB IF data source: 0 memory, 1 [`super::DEFAULT_COLOR`]. \[DS\]
    pub const RGB_DR: u32 = bit(22);
    /// RGB565 packing (see [`super::data`]). AHB: 0 = `R[23:19] G[15:10] B[7:3]`, 1 = a
    /// byte-swapped RGB565 halfword in `[15:0]`. DMA, two pixels per word, pixel 0 in
    /// `[15:0]`: 0 = each half byte-swapped, 1 = each half native RGB565 (`R[15:11]
    /// G[10:5] B[4:0]`, a little-endian `u16` frame buffer). \[DS\]; vendor DMA uses 1 \[code\].
    pub const RGB565_FORMAT: u32 = bit(21);
    /// AHB writes are 0 display data (formatted by SDT/RGB565_FORMAT) or 1 configuration
    /// data (`DATA[15:0]` sent raw, "as a command"). The port sends every byte, pixels
    /// included, with 1; D/C is [`RS`] either way \[HW\].
    pub const AHB_CFG_DATA: u32 = bit(20);
    /// Bus width. YM03: 8-bit \[HW\].
    pub const DQBITS: Field = Field::bits(19, 18);
    pub const DQBITS_8: u32 = 0;
    pub const DQBITS_16: u32 = 1;
    pub const DQBITS_24: u32 = 2;
    pub const DQBITS_9: u32 = 3;
    /// Colour order, odd lines (RGB IF, `ODD_EVEN_L = 1`): 0 BGR, 1 RBG, 2 GRB, 3 GBR,
    /// 4 BRG, 5 RGB. \[DS\]
    pub const CC_ODD: Field = Field::bits(16, 14);
    /// Colour order, even lines; encoding as [`CC_ODD`]. \[DS\]
    pub const CC_EVEN: Field = Field::bits(13, 11);
    /// Source data type. 24-bit sources are packed 4 pixels per 3 words. \[DS\]
    /// Vendor: 0 for RGB565 frames, 4 for its YCbCr (special DMA channel) mode \[code\].
    pub const SDT: Field = Field::bits(10, 8);
    pub const SDT_RGB565: u32 = 0;
    pub const SDT_RGB666: u32 = 1;
    pub const SDT_RGB888: u32 = 2;
    pub const SDT_ABGR888: u32 = 3;
    /// YCbCr444 converted to RGB888 by the controller.
    pub const SDT_YCBCR444_TO_RGB: u32 = 4;
    pub const SDT_YCBCR444: u32 = 5;
    /// FIFO-empty DMA request enable; set for DMA transfers. \[DS\]; vendor \[code\].
    pub const EMDE: u32 = bit(7);
    /// Level of the RS (D/CX) pin: 0 command/index, 1 data. \[DS\]; \[HW\] (port).
    pub const RS: u32 = bit(6);
    /// Interface output format: 0 RGB565, 1 RGB666, 2 RGB888. \[DS\]
    pub const DDT: Field = Field::bits(5, 4);
    pub const DDT_RGB565: u32 = 0;
    pub const DDT_RGB666: u32 = 1;
    pub const DDT_RGB888: u32 = 2;
    /// Byte order on an 8-bit bus with DDT = RGB565 (or 9-bit with RGB666): 0 = first
    /// byte R/G (high byte first, what the ST7735S wants), 1 = G/B first. \[DS\]
    pub const MLS: u32 = bit(3);
    /// Interface: 0 i8080, 1 M6800, 2 RGB. YM03: i8080 \[HW\].
    pub const IFSEL: Field = Field::bits(2, 1);
    pub const IFSEL_I8080: u32 = 0;
    pub const IFSEL_M6800: u32 = 1;
    pub const IFSEL_RGB: u32 = 2;
    /// Controller enable. \[DS\]
    pub const EN: u32 = bit(0);
}

/// CTL after init: EN only (i8080, 8-bit, AHB, CE0). \[HW\] port `lcm_init`; vendor
/// DRV_LCD bank 2 `+0x21A` \[code\].
pub const VENDOR_CTL_INIT: u32 = ctl::EN;
/// Port/vendor "command" step: `CTL = (CTL & MASK) | SET`, then write the byte to
/// [`DATA`]. Clears CESEL, IN_BUS, EMDE, RS; leaves AHB_CFG_DATA as it was. \[HW\]
/// `lcm_cmd`; vendor `0x119700` \[code\].
pub const PORT_CMD_MASK: u32 = 0xE7FF_FF3F;
pub const PORT_CMD_SET: u32 = ctl::EN;
/// Port/vendor "data" step (parameters and pixel bytes): clears CESEL, IN_BUS,
/// AHB_CFG_DATA, EMDE, RS, then sets RS and AHB_CFG_DATA (raw byte, D/C = 1). \[HW\]
/// `lcm_data_mode`; vendor `0x119734` \[code\].
pub const PORT_DATA_MASK: u32 = 0xE7EF_FF3F;
pub const PORT_DATA_SET: u32 = ctl::RS | ctl::AHB_CFG_DATA;
/// GRAM read (after command 0x2E; byte reads of [`DATA`], first one dummy, 3 bytes per
/// pixel): clears CESEL, IN_BUS, RGB565_FORMAT, EMDE, RS, then sets RS. \[code\]
/// payload `lcdread.c`, vendor `0x119842`.
pub const PORT_READ_MASK: u32 = 0xE7DF_FF3F;
pub const PORT_READ_SET: u32 = ctl::RS;
/// Vendor PIO pixel path: AHB display data, RGB565_FORMAT = 0, MLS = 0, RS = 1; one
/// 32-bit write per pixel as `R[23:19] G[15:10] B[7:3]` ([`data`]). Run with the core
/// on HOSC. \[code\] vendor `0x119784`.
pub const VENDOR_PIX_MASK: u32 = 0xE7CF_FF37;
pub const VENDOR_PIX_SET: u32 = ctl::RS;
/// Vendor DMA blit (`0x119AEE`): clear DQBITS and AHB_CFG_DATA (`& !0x1C_0000`), set
/// IN_BUS, EMDE, RGB565_FORMAT, EN; clear `IF_PCS.LCDFI`; start DMA channel 1 (mode
/// `0x9600`, destination [`DATA`]); then set STARTT, wait for the DMA, then for LCDFI.
/// SDT/CESEL set up in `0x119A1C` (`& 0xEFFF_FFFE`, same SET). \[code\]
pub const VENDOR_DMA_CLEAR: u32 = 0x001C_0000;
pub const VENDOR_DMA_SET: u32 = 0x0820_0081;

/// DISP_SIZE: frame size minus one (CPU IF: the transfer frame). \[DS\] §9.1.3.2.
/// Reset 0. Not written by the port or the vendor code we have.
pub const DISP_SIZE: Reg = Reg::at(BASE + DISP_SIZE_OFFSET);
pub mod disp_size {
    use super::Field;
    /// Height − 1.
    pub const Y: Field = Field::bits(26, 16);
    /// Width − 1.
    pub const X: Field = Field::bits(10, 0);
}

/// LCDC_CLK: CPU-IF strobe timing for **DMA** transfers, in LCD_CLK cycles (field + 1,
/// 1…64). \[DS\] §9.1.3.3. Reset `0x000F_000F`. Port and vendor write 0 \[HW\]: with the
/// vendor's LCD_CLK of 30 MHz (DEVPLL/6, system.md §1.5) a write cycle is 66.7 ns.
pub const LCDC_CLK: Reg = Reg::at(BASE + LCDC_CLK_OFFSET);
pub mod lcdc_clk {
    use super::Field;
    /// Strobe high time.
    pub const CLKHDU: Field = Field::bits(21, 16);
    /// Second low time (M6800 only).
    pub const CLKL2DU: Field = Field::bits(13, 8);
    /// Strobe low time.
    pub const CLKLDU: Field = Field::bits(5, 0);
}

/// EXT_CLK: CPU-IF strobe timing for **AHB** (PIO) transfers, in AHB_CLK cycles
/// (field + 1). \[DS\] §9.1.3.4. Reset `0x000F_000F`. Port and vendor write 0 \[HW\], i.e. one AHB
/// clock per phase: the likely reason the vendor drops the core to HOSC around PIO.
pub const EXT_CLK: Reg = Reg::at(BASE + EXT_CLK_OFFSET);
pub mod ext_clk {
    use super::Field;
    pub const EXCLKH: Field = Field::bits(21, 16);
    /// Named EXCL2KL in the DS (sic); second low time.
    pub const EXCL2KL: Field = Field::bits(13, 8);
    pub const EXCLKL: Field = Field::bits(5, 0);
}

/// Value the port and vendor write to [`LCDC_CLK`] and [`EXT_CLK`]. \[HW\]
pub const VENDOR_CLK_TIMING: u32 = 0;

/// DEFAULT_COLOR: RGB-IF fill colour (`CTL.RGB_DR`), laid out like [`data`]. \[DS\] §9.1.3.5.
/// **Disagreement:** the port's header calls `+0x10` `LCM_RS` and writes 0 there before
/// every data byte (copied from vendor `0x119756`); per the DS it is this RGB-IF register
/// and the RS pin is `CTL.RS`. The write is most likely a leftover (on the ATJ2127 LCDC,
/// `+0x10` was EXTMEM_DATA) \[inferred\].
pub const DEFAULT_COLOR: Reg = Reg::at(BASE + DEFAULT_COLOR_OFFSET);

/// LCD_DATA: AHB write/read port (a write starts the bus cycle) and DMA FIFO target.
/// \[DS\] §9.1.3.6; byte writes/reads \[HW\]; DMA destination \[code\].
pub const DATA: Reg = Reg::at(BASE + DATA_OFFSET);
/// Layouts of [`DATA`], [`DATA1`] and [`DEFAULT_COLOR`] in AHB display-data mode, by
/// source type. Fields of one layout are disjoint; layouts overlap. \[DS\] §9.1.3.6
pub mod data {
    use super::Field;
    pub const RGB888_R: Field = Field::bits(23, 16);
    pub const RGB888_G: Field = Field::bits(15, 8);
    pub const RGB888_B: Field = Field::bits(7, 0);
    pub const RGB666_R: Field = Field::bits(23, 18);
    pub const RGB666_G: Field = Field::bits(15, 10);
    pub const RGB666_B: Field = Field::bits(7, 2);
    /// `RGB565_FORMAT = 0` (vendor PIO pixel path).
    pub const RGB565_R: Field = Field::bits(23, 19);
    pub const RGB565_G: Field = Field::bits(15, 10);
    pub const RGB565_B: Field = Field::bits(7, 3);
    /// `RGB565_FORMAT = 1`: `[15:0]` = byte-swapped RGB565 (`G[2:0] B R G[5:3]`).
    pub const RGB565_SWAPPED: Field = Field::bits(15, 0);
    /// `AHB_CFG_DATA = 1`: sent as is (bus width per DQBITS).
    pub const CMD: Field = Field::bits(15, 0);
}

/// LCD_TIM0: RGB-IF signal polarities. \[DS\] §9.1.3.7. Reset 0. Unused (CPU IF).
pub const TIM0: Reg = Reg::at(BASE + 0x18);
pub mod tim0 {
    use super::bit;
    pub const VSYNC_INV: u32 = bit(7);
    pub const HSYNC_INV: u32 = bit(6);
    /// 1: data change on the falling DCLK edge.
    pub const DCLK_INV: u32 = bit(5);
    pub const LDE_INV: u32 = bit(4);
}
/// LCD_TIM1: RGB-IF horizontal timing, pixel clocks − 1. \[DS\] §9.1.3.8. Unused.
pub const TIM1: Reg = Reg::at(BASE + 0x1C);
pub mod tim1 {
    use super::Field;
    pub const HSPW: Field = Field::bits(29, 20);
    pub const HFPD: Field = Field::bits(19, 10);
    pub const HBPD: Field = Field::bits(9, 0);
}
/// LCD_TIM2: RGB-IF vertical timing, lines − 1. \[DS\] §9.1.3.9. Unused.
pub const TIM2: Reg = Reg::at(BASE + 0x20);
pub mod tim2 {
    use super::Field;
    pub const VSPW: Field = Field::bits(28, 20);
    pub const VFPD: Field = Field::bits(19, 10);
    pub const VBPD: Field = Field::bits(9, 0);
}

/// LCD_IF_PCS: status, W1C pendings and interrupt enables (IRQ 42). \[DS\] §9.1.3.10.
/// Reset `0x0000_0100`. Vendor polls/clears LCDFI around each DMA blit \[code\]; the
/// interrupt path is untested (system.md T5).
pub const IF_PCS: Reg = Reg::at(BASE + IF_PCS_OFFSET);
pub mod if_pcs {
    use super::{Field, bit};
    /// Transfer finished; W1C. \[DS\]; \[code\] vendor `0x119BB2`.
    pub const LCDFI: u32 = bit(31);
    /// Frame finished (RGB IF); W1C.
    pub const FRAME_END: u32 = bit(30);
    /// Vertical blanking pending (RGB IF); W1C.
    pub const VBI: u32 = bit(29);
    /// FIFO underflow (RGB IF); W1C, also resets the FIFO.
    pub const UNDER_F: u32 = bit(17);
    pub const VBI_INTEN: u32 = bit(15);
    pub const LCDFI_INTEN: u32 = bit(14);
    pub const FRAME_END_INTEN: u32 = bit(13);
    pub const UNDER_F_INTEN: u32 = bit(12);
    /// FIFO empty (read only).
    pub const FIFOET: u32 = bit(10);
    /// TE input active (read only, reset 1).
    pub const TE_STATUS: u32 = bit(8);
    /// Parity check sum of the bus data (CPU IF, `CTL.PC_EN`).
    pub const PCS: Field = Field::bits(7, 0);
    /// All W1C pending bits.
    pub const PENDING: u32 = LCDFI | FRAME_END | VBI | UNDER_F;
}

/// LCD_IMG_XPOS: image window in the screen (RGB IF), pixel − 1. \[DS\] §9.1.3.11.
pub const IMG_XPOS: Reg = Reg::at(BASE + 0x28);
/// LCD_IMG_YPOS: as [`IMG_XPOS`] for lines. \[DS\] §9.1.3.12.
pub const IMG_YPOS: Reg = Reg::at(BASE + 0x2C);
/// Fields of [`IMG_XPOS`] and [`IMG_YPOS`].
pub mod img_pos {
    use super::Field;
    pub const START: Field = Field::bits(26, 16);
    pub const END: Field = Field::bits(10, 0);
}

/// GAMMA_IDX: gamma table pointer and enables. Fill [`PATH_GAMMA_RAM`] first, then set
/// GAMMA_EN. \[DS\] §9.1.3.13.
pub const GAMMA_IDX: Reg = Reg::at(BASE + 0x30);
pub mod gamma_idx {
    use super::{Field, bit};
    pub const GAMMA_EN: u32 = bit(16);
    /// 0 gamma only, 1 gamma and dither.
    pub const GTD_EN: u32 = bit(8);
    /// Word index 0x00…0xBF (R at 0x00, G at 0x40, B at 0x80; 4 entries per word).
    pub const RAM_IDX: Field = Field::bits(7, 0);
}
/// PATH_GAMMA_RAM: gamma table data port, four 8-bit entries per word. \[DS\] §9.1.3.14.
pub const PATH_GAMMA_RAM: Reg = Reg::at(BASE + 0x34);
/// PATH_DITHER: ordered dither. \[DS\] §9.1.3.15. Reset 0.
pub const PATH_DITHER: Reg = Reg::at(BASE + 0x38);
pub mod path_dither {
    use super::{Field, bit};
    pub const START_YPOS: Field = Field::bits(11, 9);
    pub const START_XPOS: Field = Field::bits(8, 6);
    pub const POS_EN: u32 = bit(5);
    /// 0: 4×4, 1: 8×8 matrix.
    pub const STRENGTH: u32 = bit(4);
    pub const ENABLE: u32 = bit(0);
}

/// LCD_DATA1: first pixel of a pair for RGB666/888 over a 16-bit AHB bus; write it
/// before [`DATA`] (which starts the transfer). Layouts as [`data`]. \[DS\] §9.1.3.16.
pub const DATA1: Reg = Reg::at(BASE + DATA1_OFFSET);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // atj2157.h: LCM_CTL, LCM_RS, LCM_DATA; lcd-yotonym03.c: +0x08, +0x0C.
        assert_eq!(BASE, 0xC01A_0000);
        assert_eq!(CTL.addr(), 0xC01A_0000);
        assert_eq!(LCDC_CLK.addr(), 0xC01A_0008);
        assert_eq!(EXT_CLK.addr(), 0xC01A_000C);
        assert_eq!(DEFAULT_COLOR.addr(), 0xC01A_0010);
        assert_eq!(DATA.addr(), 0xC01A_0014);
        // Vendor DRV_LCD: IF_PCS (+0x24) polled for LCDFI.
        assert_eq!(IF_PCS.addr(), 0xC01A_0024);
    }

    #[test]
    fn datasheet_addresses() {
        assert_eq!(DISP_SIZE.addr(), 0xC01A_0004);
        assert_eq!(TIM2.addr(), 0xC01A_0020);
        assert_eq!(IMG_YPOS.addr(), 0xC01A_002C);
        assert_eq!(PATH_DITHER.addr(), 0xC01A_0038);
        assert_eq!(DATA1.addr(), 0xC01A_0040);
        assert_increasing(&[
            CTL.addr(),
            DISP_SIZE.addr(),
            LCDC_CLK.addr(),
            EXT_CLK.addr(),
            DEFAULT_COLOR.addr(),
            DATA.addr(),
            TIM0.addr(),
            TIM1.addr(),
            TIM2.addr(),
            IF_PCS.addr(),
            IMG_XPOS.addr(),
            IMG_YPOS.addr(),
            GAMMA_IDX.addr(),
            PATH_GAMMA_RAM.addr(),
            PATH_DITHER.addr(),
            DATA1.addr(),
        ]);
    }

    #[test]
    fn ctl_fields_disjoint() {
        use ctl::*;
        assert_disjoint(&[
            CLK_GAT_EN,
            PC_EN,
            RB_SWAP,
            CESEL,
            IN_BUS,
            TEFUNCEN,
            TEINV,
            STARTT,
            ODD_EVEN_L,
            RGB_DR,
            RGB565_FORMAT,
            AHB_CFG_DATA,
            DQBITS.mask(),
            CC_ODD.mask(),
            CC_EVEN.mask(),
            SDT.mask(),
            EMDE,
            RS,
            DDT.mask(),
            MLS,
            IFSEL.mask(),
            EN,
        ]);
    }

    #[test]
    fn other_fields_disjoint() {
        assert_disjoint(&[disp_size::Y.mask(), disp_size::X.mask()]);
        assert_disjoint(&[lcdc_clk::CLKHDU.mask(), lcdc_clk::CLKL2DU.mask(), lcdc_clk::CLKLDU.mask()]);
        assert_disjoint(&[ext_clk::EXCLKH.mask(), ext_clk::EXCL2KL.mask(), ext_clk::EXCLKL.mask()]);
        assert_disjoint(&[data::RGB888_R.mask(), data::RGB888_G.mask(), data::RGB888_B.mask()]);
        assert_disjoint(&[data::RGB666_R.mask(), data::RGB666_G.mask(), data::RGB666_B.mask()]);
        assert_disjoint(&[data::RGB565_R.mask(), data::RGB565_G.mask(), data::RGB565_B.mask()]);
        assert_disjoint(&[tim0::VSYNC_INV, tim0::HSYNC_INV, tim0::DCLK_INV, tim0::LDE_INV]);
        assert_disjoint(&[tim1::HSPW.mask(), tim1::HFPD.mask(), tim1::HBPD.mask()]);
        assert_disjoint(&[tim2::VSPW.mask(), tim2::VFPD.mask(), tim2::VBPD.mask()]);
        assert_disjoint(&[
            if_pcs::LCDFI,
            if_pcs::FRAME_END,
            if_pcs::VBI,
            if_pcs::UNDER_F,
            if_pcs::VBI_INTEN,
            if_pcs::LCDFI_INTEN,
            if_pcs::FRAME_END_INTEN,
            if_pcs::UNDER_F_INTEN,
            if_pcs::FIFOET,
            if_pcs::TE_STATUS,
            if_pcs::PCS.mask(),
        ]);
        assert_disjoint(&[img_pos::START.mask(), img_pos::END.mask()]);
        assert_disjoint(&[gamma_idx::GAMMA_EN, gamma_idx::GTD_EN, gamma_idx::RAM_IDX.mask()]);
        assert_disjoint(&[
            path_dither::START_YPOS.mask(),
            path_dither::START_XPOS.mask(),
            path_dither::POS_EN,
            path_dither::STRENGTH,
            path_dither::ENABLE,
        ]);
    }

    /// The literal masks the port and vendor use, decoded against the DS fields.
    #[test]
    fn port_values_decode() {
        use ctl::*;
        assert_eq!(VENDOR_CTL_INIT, 0x0000_0001);
        assert_eq!(!PORT_CMD_MASK, CESEL | IN_BUS | EMDE | RS);
        assert_eq!(!PORT_DATA_MASK, CESEL | IN_BUS | AHB_CFG_DATA | EMDE | RS);
        assert_eq!(PORT_DATA_SET, 0x0010_0040);
        assert_eq!(!PORT_READ_MASK, CESEL | IN_BUS | RGB565_FORMAT | EMDE | RS);
        assert_eq!(!VENDOR_PIX_MASK, CESEL | IN_BUS | RGB565_FORMAT | AHB_CFG_DATA | EMDE | RS | MLS);
        assert_eq!(VENDOR_DMA_CLEAR, AHB_CFG_DATA | DQBITS.mask());
        assert_eq!(VENDOR_DMA_SET, IN_BUS | RGB565_FORMAT | EMDE | EN);
        // Vendor 0x119A1C: CTL & 0xEFFF_FFFE (CESEL and EN cleared, EN set again).
        assert_eq!(!0xEFFF_FFFEu32, CESEL | EN);
        // Vendor pixel word for 0xF81F (magenta): R = 0x1F, G = 0, B = 0x1F.
        let w = data::RGB565_R.val(0x1F) | data::RGB565_G.val(0) | data::RGB565_B.val(0x1F);
        assert_eq!(w, 0x00F8_00F8);
        assert_eq!(if_pcs::PENDING, 0xE002_0000);
    }
}
