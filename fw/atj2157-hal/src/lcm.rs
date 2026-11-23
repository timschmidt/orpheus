//! LCD controller (8080 CPU interface), `0xC01A0000` (datasheet §9.1;
//! 03-hal-drivers §7). Two ways to move bytes to the panel:
//!
//! * **PIO** (AHB mode): commands and short parameter strings, written
//!   byte by byte to `LCD_DATA` — the way the Rockbox port drives it [HW];
//! * **DMA** (`IN_BUS = 1`): frame-buffer bands streamed by a DMA channel in
//!   mode `0x9600` (destination LCD FIFO, fixed) [code: vendor DRV_LCD].
//!
//! Register constants are local until WP-HAL-0's PAC module lands
//! (TODO(pac): switch to `atj2157_pac::lcm`).

use atj2157_pac::{Reg, cmu, gpio, rmu};

/// CMU_LCDCLK. [DS] §4.3.3.8 (TODO(pac)).
const CMU_LCDCLK: Reg = Reg::at(0xC000_1034);

use crate::dma::{self, Channel};

const BASE: usize = 0xC01A_0000;
const CTL: Reg = Reg::at(BASE);
const DISP_SIZE: Reg = Reg::at(BASE + 0x04);
const LCDC_CLK: Reg = Reg::at(BASE + 0x08);
const EXT_CLK: Reg = Reg::at(BASE + 0x0C);
/// The port writes 0 here before every PIO data byte (named LCM_RS there;
/// DEFAULT_COLOR in the datasheet). Kept until understood. [HW]
const DEFAULT_COLOR: Reg = Reg::at(BASE + 0x10);
const DATA: Reg = Reg::at(BASE + 0x14);
/// Address the DMA writes pixel words to.
pub const DATA_ADDR: u32 = (BASE + 0x14) as u32;

mod ctl {
    pub const EN: u32 = 1 << 0;
    pub const RS: u32 = 1 << 6;
    pub const EMDE: u32 = 1 << 7;
    pub const AHB_CFG_DATA: u32 = 1 << 20;
    pub const RGB565_FORMAT: u32 = 1 << 21;
    pub const STARTT: u32 = 1 << 24;
    pub const IN_BUS: u32 = 1 << 27;
    pub const CESEL: u32 = 1 << 28;
}

/// DMA mode word: memory → LCD FIFO (fixed), 32-bit bursts. [code]
pub const DMA_MODE: u32 = dma::mode::dst(dma::mode::LCD) | dma::mode::DST_FIXED | dma::mode::WIDTH_32;

/// Pins of an 8080 bus: data D0–D7, RS, RD, WR and CS (function 2 [HW]).
pub struct Pins<'a> {
    pub bus: &'a [u8],
    pub cs: u8,
    pub cs_drive: u32,
}

pub struct Lcm {
    dma: Channel,
}

impl Lcm {
    /// Clock, reset, pin functions and PIO mode. `lcd_clk_div` divides
    /// CK24M (`0` = 24 MHz).
    pub fn new(pins: &Pins<'_>, dma: Channel, lcd_clk_div: u32) -> Self {
        // CK24M source, no pre-divider. [DS] §4.3.3.8
        CMU_LCDCLK.write(lcd_clk_div & 0xF);
        cmu::CLKEN0.set_bits(cmu::clken0::LCD | cmu::clken0::DMA);
        rmu::RST0.clear_bits(rmu::rst0::LCD);
        crate::delay::spin_us(10);
        rmu::RST0.set_bits(rmu::rst0::LCD);
        // Fastest strobe timing (1+1 clocks) as the port runs it [HW].
        LCDC_CLK.write(0);
        EXT_CLK.write(0);
        CTL.write(ctl::EN);
        for &p in pins.bus {
            gpio::mfp(usize::from(p)).modify(|v| (v & !0x1F) | 2);
        }
        gpio::mfp(usize::from(pins.cs)).modify(|v| (v & !(0x1F | 0x7000)) | 2 | (pins.cs_drive << 12));
        dma.enable_irq();
        Self { dma }
    }

    /// Send a command byte (RS low).
    pub fn cmd(&mut self, c: u8) {
        CTL.modify(|v| (v & 0xE7FF_FF3F) | ctl::EN);
        DATA.write(u32::from(c));
        crate::delay::spin_us(2);
        // Parameters follow in data mode.
        CTL.modify(|v| (v & 0xE7EF_FF3F) | ctl::RS | ctl::AHB_CFG_DATA);
    }

    /// Send one parameter/data byte (RS high).
    pub fn data(&mut self, d: u8) {
        DEFAULT_COLOR.write(0);
        DATA.write(u32::from(d));
    }

    /// Command with parameters.
    pub fn cmd_with(&mut self, c: u8, params: &[u8]) {
        self.cmd(c);
        for &p in params {
            self.data(p);
        }
    }

    /// Read `out.len()` bytes after a read command (first byte returned by
    /// the panel is a dummy and is included). Experimental (U-LCM-1).
    pub fn read_after(&mut self, c: u8, out: &mut [u8]) {
        self.cmd(c);
        for o in out.iter_mut() {
            *o = DATA.read() as u8;
        }
    }

    /// Stream `len` bytes from `src` (word aligned, `len` multiple of 4) to
    /// the panel by DMA. The panel window must be set and RAMWR issued.
    pub async fn blit(&mut self, src: *const u8, len: u32, w: u32, h: u32) {
        CTL.write(ctl::EN | ctl::RS | ctl::EMDE | ctl::IN_BUS | ctl::RGB565_FORMAT);
        DISP_SIZE.write(((h - 1) << 16) | (w - 1));
        CTL.set_bits(ctl::STARTT);
        self.dma.start(DMA_MODE, src as u32, DATA_ADDR, len);
        self.dma.wait().await;
        self.finish_blit();
    }

    /// Blocking variant (start-up screens, crash screen).
    pub fn blit_blocking(&mut self, src: *const u8, len: u32, w: u32, h: u32) -> bool {
        CTL.write(ctl::EN | ctl::RS | ctl::EMDE | ctl::IN_BUS | ctl::RGB565_FORMAT);
        DISP_SIZE.write(((h - 1) << 16) | (w - 1));
        CTL.set_bits(ctl::STARTT);
        self.dma.start(DMA_MODE, src as u32, DATA_ADDR, len);
        let ok = self.dma.wait_blocking(10_000_000);
        self.finish_blit();
        ok
    }

    fn finish_blit(&mut self) {
        // Let the FIFO drain to the panel before leaving DMA mode.
        crate::delay::spin_us(4);
        CTL.write(ctl::EN);
        let _ = ctl::CESEL;
    }

    /// Raw control word (diagnostics).
    #[must_use]
    pub fn ctl(&self) -> u32 {
        CTL.read()
    }
}
