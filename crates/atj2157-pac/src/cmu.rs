//! Clock management unit, `0xC0001000` (datasheet §4.3), and the oscillator
//! / PLL block at `0xC0000100` (datasheet §4.2 documents only `HOSC_CTL` and
//! `LOSC_CTL` there; the PLLs are known from vendor code, system.md §1.1).
//!
//! The datasheet register list starts at `CMU_DEVCLKEN0` (`+0x08`); `CMU_SYSCLK`
//! (`+0x00`), the image/DSP/USB-PHY clock registers and everything in the
//! PLL block above `+0x04` are undocumented and come from vendor code and the
//! Rockbox port. Clock-source codes differ per register: each submodule lists
//! its own.
//!
//! Unknown: `SYSCLK` bit 8 and `[25:24]` (U-PWR-2); the SPI0 clock behaviour
//! of the ROM setting `0x101` versus the datasheet (U-PWR-3); the audio-PLL
//! register fields; `+0x30`, `+0xAC`, `+0xB0`, `+0xB8`.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC000_1000;

/// CMU_SYSCLK — core clock source and divider. Not in the datasheet register
/// list. ROM state `0x0300_0002` \[HW\]; field meanings \[code\] system.md §1.2.
pub const SYSCLK: Reg = Reg::at(BASE);
pub mod sysclk {
    use super::{Field, bit};
    /// Core clock source. \[code\]; HOSC and COREPLL \[HW\].
    pub const SRC: Field = Field::bits(1, 0);
    /// Core divider: f = source ÷ 2^n. \[code\] system.md §1.2.
    pub const DIV: Field = Field::bits(5, 4);
    pub const SRC_32K: u32 = 0;
    pub const SRC_HOSC: u32 = 1;
    pub const SRC_COREPLL: u32 = 2;
    pub const SRC_DEVPLL: u32 = 3;
    /// Set by the vendor for every level ≥ 204 MHz; bus ÷2 \[inferred\] (U-PWR-2).
    pub const UNK_B8: u32 = bit(8);
    /// The ROM writes 3, the vendor kernel 0; both run. \[code\] (U-PWR-2).
    pub const UNK_25_24: Field = Field::bits(25, 24);
    /// ROM / port value: COREPLL, ÷1, `[25:24]` = 3. \[HW\]
    pub const ROM_VALUE: u32 = 0x0300_0002;
}

/// CMU_DEVCLKEN0 — device clock gates (1 = clock on). \[DS\] §4.3.3.1.
/// Reset `0x0800_0000` (only LRADC on).
pub const CLKEN0: Reg = Reg::at(BASE + 0x08);
/// CMU_DEVCLKEN1 — audio clock gates. \[DS\] §4.3.3.2. Reset `0x8000_0000`.
pub const CLKEN1: Reg = Reg::at(BASE + 0x0C);

/// Bits of [`CLKEN0`]. \[DS\] §4.3.3.1 unless noted; the port uses DMA, SD0,
/// SPI0, the cache clocks, LCD, USB, TIMER, PWM and LRADC \[HW\].
pub mod clken0 {
    use super::bit;
    /// DMA controller. The vendor also treats it as a common bus gate for
    /// SD, LCD, USB and audio users \[code\] system.md §2.
    pub const DMA: u32 = bit(0);
    pub const NAND: u32 = bit(1);
    pub const SD0: u32 = bit(2);
    pub const SD1: u32 = bit(3);
    pub const SPI0: u32 = bit(6);
    pub const SPI1: u32 = bit(7);
    /// Named `SPTWILKEN` in the datasheet (a typo for SPI2).
    pub const SPI2: u32 = bit(8);
    /// SPI0 cache controller = SRAM2 clock. \[DS\]; crt0 sets it \[HW\].
    pub const SPI0_CACHE: u32 = bit(9);
    /// SPI1 cache controller = SRAM3 clock. \[DS\]; crt0 sets it \[HW\].
    pub const SPI1_CACHE: u32 = bit(10);
    /// Image/video block at `0xC0090000` (datasheet: reserved, R/W). \[code\]
    /// system.md §2 (U-IMG-1).
    pub const IMG: u32 = bit(12);
    pub const LCD: u32 = bit(13);
    /// Security engine. \[DS\]
    pub const SEC: u32 = bit(14);
    pub const USB: u32 = bit(15);
    pub const TWI0: u32 = bit(18);
    pub const TWI1: u32 = bit(19);
    pub const UART0: u32 = bit(20);
    pub const UART1: u32 = bit(21);
    pub const UART2: u32 = bit(22);
    pub const UART3: u32 = bit(23);
    /// All four timers.
    pub const TIMER: u32 = bit(24);
    /// All nine PWM channels.
    pub const PWM: u32 = bit(25);
    /// Reset default 1.
    pub const LRADC: u32 = bit(27);
    /// Value the vendor's charging-screen standby leaves on. \[code\] system.md §3.4.
    pub const VENDOR_STANDBY_MASK: u32 = 0x0B00_A003;
}

/// Bits of [`CLKEN1`]. \[DS\] §4.3.3.2 unless noted.
pub mod clken1 {
    use super::bit;
    pub const ADC01: u32 = bit(0);
    pub const ADC23: u32 = bit(1);
    pub const DAC: u32 = bit(2);
    /// I2S MCLK gates; each also gates the CK24M LRCK detector of its unit.
    pub const I2STX0_MCLK: u32 = bit(3);
    pub const I2STX1_MCLK: u32 = bit(4);
    pub const I2SRX0_MCLK: u32 = bit(5);
    pub const I2SRX1_MCLK: u32 = bit(6);
    pub const I2SRX2_MCLK: u32 = bit(7);
    pub const I2SSRD: u32 = bit(8);
    /// "DAC analog measurement clock"; needed for any analogue output \[HW\].
    pub const DAC_ANALOG: u32 = bit(11);
    /// DSP accelerator (datasheet: reserved, R/W). \[code\] audio.md §7.1.
    pub const DSP: u32 = bit(13);
    /// eFuse controller (datasheet: reserved, R/W, reset 1). \[code\] system.md §5.5.
    pub const EFUSE: u32 = bit(31);
    /// The port's DAC power-up: ADC01 | DAC | DAC_ANALOG. \[HW\]
    pub const PORT_DAC_POWER_UP: u32 = ADC01 | DAC | DAC_ANALOG;
}

/// Field layout shared by CMU_SD0CLK and CMU_SD1CLK. \[DS\] §4.3.3.3–4.
pub mod sdclk {
    use super::{Field, bit};
    /// 0 CK24M, 1 COREPLL, 2 DEVPLL.
    pub const SRC: Field = Field::bits(9, 8);
    pub const SRC_CK24M: u32 = 0;
    pub const SRC_COREPLL: u32 = 1;
    pub const SRC_DEVPLL: u32 = 2;
    /// Post-divider: 0 = ÷1, 1 = ÷128.
    pub const POSTDIV_128: u32 = bit(6);
    /// Divider: ÷(n + 1), n = 0..15.
    pub const DIV: Field = Field::bits(3, 0);
    /// Card identification rate: CK24M ÷128 ÷2 = 93.75 kHz by the datasheet
    /// (the port's comment says ≈ 400 kHz; unmeasured). Works for card
    /// identification \[HW\].
    pub const IDENT_93K75: u32 = 0x41;
    /// CK24M ÷1 = 24 MHz, the port's data rate. \[HW\]
    pub const CK24M_24MHZ: u32 = 0;
    /// Vendor with DEVPLL at 180 MHz: ÷8 = 22.5 MHz. \[code\] system.md §1.5.
    pub const VENDOR_DEVPLL180_22M5: u32 = 0x207;
}
/// CMU_SD0CLK. \[DS\] §4.3.3.3; port \[HW\]. Fields: [`sdclk`].
pub const SD0CLK: Reg = Reg::at(BASE + 0x14);
/// CMU_SD1CLK. \[DS\] §4.3.3.4. Fields: [`sdclk`].
pub const SD1CLK: Reg = Reg::at(BASE + 0x18);

/// Field layout shared by CMU_SPI0CLK, CMU_SPI1CLK and CMU_SPI2CLK
/// (`CMU_SPTWILK`). \[DS\] §4.3.3.5–7.
///
/// **Disagreement:** system.md §1.5 reads the ROM's `0x101` as COREPLL ÷1
/// (from a measured ≈ 46 Mbit/s); the datasheet decodes it as CK24M ÷2
/// (12 MHz). The port's `0x200` = COREPLL ÷1 = 48 MHz fits the datasheet.
/// Settle with system.md T3 (U-PWR-3).
pub mod spiclk {
    use super::Field;
    /// 0 HCLK, 1 CK24M, 2 COREPLL, 3 DEVPLL.
    pub const SRC: Field = Field::bits(9, 8);
    pub const SRC_HCLK: u32 = 0;
    pub const SRC_CK24M: u32 = 1;
    pub const SRC_COREPLL: u32 = 2;
    pub const SRC_DEVPLL: u32 = 3;
    /// Divider code: 0 ÷1, 1 ÷2, n ÷2n for n = 2..13, 14 ÷1.5, 15 ÷2.5.
    pub const DIV: Field = Field::bits(3, 0);
    /// SPI1 only: OPI-mode `SPI1_2X_CLK` divider, same coding (0..7).
    pub const OPI2X_DIV: Field = Field::bits(14, 12);
    /// ROM setting. \[HW\] (works); see the disagreement above.
    pub const ROM_VALUE: u32 = 0x101;
    /// COREPLL ÷1: the port's fast pager setting (48 MHz with COREPLL at 48). \[HW\]
    pub const PORT_COREPLL_DIV1: u32 = 0x200;
    /// DEVPLL ÷2: the port's first choice (needs DEVPLL running). \[code\]
    pub const PORT_DEVPLL_DIV2: u32 = 0x301;
    /// Vendor with DEVPLL 180 MHz: ÷6 = 30 MHz (system.md reads it as ÷3 =
    /// 60 MHz). The vendor picks the smallest d with 90·d > DEVPLL MHz, i.e.
    /// DEVPLL ÷ 2d < 45 MHz, which fits the datasheet coding. \[code\]
    pub const VENDOR_DEVPLL180: u32 = 0x303;
}
/// CMU_SPI0CLK. \[DS\] §4.3.3.5; NOR clock \[HW\]. Fields: [`spiclk`].
pub const SPI0CLK: Reg = Reg::at(BASE + 0x20);
/// CMU_SPI1CLK. \[DS\] §4.3.3.6. Fields: [`spiclk`].
pub const SPI1CLK: Reg = Reg::at(BASE + 0x24);
/// CMU_SPTWILK (SPI2 clock). \[DS\] §4.3.3.7. Fields: [`spiclk`].
pub const SPI2CLK: Reg = Reg::at(BASE + 0x28);

/// Image/video block clock; written by the vendor's image library. Not in the
/// datasheet. \[code\] system.md §2.
pub const IMGCLK: Reg = Reg::at(BASE + 0x30);

/// CMU_LCDCLK. \[DS\] §4.3.3.8; port \[HW\].
pub const LCDCLK: Reg = Reg::at(BASE + 0x34);
pub mod lcdclk {
    use super::{Field, bit};
    /// 0 CK24M, 1 COREPLL, 2 DEVPLL.
    pub const SRC: Field = Field::bits(9, 8);
    pub const SRC_CK24M: u32 = 0;
    pub const SRC_COREPLL: u32 = 1;
    pub const SRC_DEVPLL: u32 = 2;
    /// Pre-divider ÷6.
    pub const PREDIV_6: u32 = bit(4);
    /// Divider ÷(n + 1), n = 0..11.
    pub const DIV: Field = Field::bits(3, 0);
    /// Port: CK24M ÷4 = 6 MHz. \[HW\]
    pub const PORT_VALUE: u32 = 0x003;
    /// Vendor with DEVPLL 180 MHz: ÷6 = 30 MHz. \[code\] system.md §1.5.
    pub const VENDOR_DEVPLL180: u32 = 0x205;
}

/// CMU_TIMERnCLK, n = 0..3. \[DS\] §4.3.3.9–12.
#[must_use]
pub const fn timer_clk(n: usize) -> Reg {
    Reg::at(BASE + 0x40 + 4 * n)
}
/// `[1:0]` of [`timer_clk`]: CK24M.
pub const TIMERCLK_24M: u32 = 0;
/// CK24M ÷ 24 = 1 MHz.
pub const TIMERCLK_1M: u32 = 1;
/// T2/T3 only: external input `TIMERn_EXT`.
pub const TIMERCLK_EXT: u32 = 2;
pub mod timerclk {
    use super::Field;
    pub const SRC: Field = Field::bits(1, 0);
}

/// CMU_PWMnCLK, n = 0..8. \[DS\] §4.3.3.13–21; PWM4 \[HW\].
#[must_use]
pub const fn pwm_clk(n: usize) -> Reg {
    Reg::at(BASE + 0x50 + 4 * n)
}
pub mod pwmclk {
    use super::{Field, bit};
    /// Source: 0 RC32K, 1 CK24M.
    pub const SRC_CK24M: u32 = bit(9);
    /// Divider: n = 0..255 → ÷(n + 1); 256..260 → ÷512, ÷1024 … ÷8192.
    pub const DIV: Field = Field::bits(8, 0);
    /// Port backlight: CK24M ÷16 = 1.5 MHz. \[HW\]
    pub const PORT_BACKLIGHT: u32 = 0x20F;
}

/// CMU_LRADCCLK. \[DS\] §4.3.3.22. Vendor writes 0 \[code\].
pub const LRADCCLK: Reg = Reg::at(BASE + 0x7C);
pub mod lradcclk {
    use super::Field;
    /// 0 CK24M/94 (255 kHz), 1 /46 (522 kHz), 2 /22 (1091 kHz), 3 RC32K.
    pub const SRC: Field = Field::bits(1, 0);
}

/// Field layout shared by CMU_ADC01CLK and CMU_ADC23CLK (`G0`/`G1` are the
/// two ADC clock generators). \[DS\] §4.3.3.23–24.
pub mod adcclk {
    use super::{Field, bit};
    pub const FIRCLK_RVS: u32 = bit(31);
    pub const FIRCLK_DIV2: u32 = bit(29);
    pub const FIRCLK_EN: u32 = bit(28);
    pub const CICCLK_RVS: u32 = bit(27);
    pub const CICCLK_DIV2: u32 = bit(25);
    pub const CICCLK_EN: u32 = bit(24);
    /// DMIC clock source: 0 ADC01 analogue clock, 1 ADC23 analogue clock.
    pub const DMICCLK_SRC: u32 = bit(22);
    pub const DMICCLK_DIV2: u32 = bit(21);
    pub const DMICCLK_EN: u32 = bit(20);
    pub const ANACLK_RVS: u32 = bit(19);
    /// ADC01CLK only: LPADC analogue clock source / enable.
    pub const LPADCANACLK_SRC: u32 = bit(18);
    pub const LPADCANACLK_EN: u32 = bit(16);
    /// ADC01CLK only: ADC analogue clock source / enable.
    pub const ADCANACLK_SRC: u32 = bit(14);
    pub const ADCANACLK_EN: u32 = bit(12);
    /// Over-sample divider: 0 ÷1, 1 ÷2, 2 ÷4, 3 ÷3.
    pub const OVFSDIV: Field = Field::bits(11, 10);
    /// 0 ADCGn_CLK, 1 I2STX0_MCLK, 2 PDMCLK pin.
    pub const CLKSRC: Field = Field::bits(9, 8);
    /// ADCGn_CLK source: 0 AudioPLL0, 1 AudioPLL1, 2 CK24M.
    pub const GCLK_SRC: Field = Field::bits(7, 6);
    pub const GCLK_PREDIV2: u32 = bit(4);
    /// ADCGn divider index: ÷{1, 2, 3, 4, 6, 8, 12, 5}.
    pub const GCLK_DIV: Field = Field::bits(2, 0);
    /// FIR and CIC clocks on, each ÷2. The vendor rate function always sets it.
    /// \[code\] audio.md §1.3; decoded \[DS\].
    pub const VENDOR_FIR_CIC: u32 = 0x3300_0000;
    /// Port value needed for analogue output: FIR/CIC on, AudioPLL1, ÷4. \[HW\]
    pub const PORT_VALUE: u32 = 0x3300_0043;
}
/// CMU_ADC01CLK. \[DS\] §4.3.3.23; port \[HW\]. Fields: [`adcclk`].
pub const ADC01CLK: Reg = Reg::at(BASE + 0x80);
/// CMU_ADC23CLK. \[DS\] §4.3.3.24. Fields: [`adcclk`].
pub const ADC23CLK: Reg = Reg::at(BASE + 0x84);

/// CMU_DACCLK. \[DS\] §4.3.3.25; port \[HW\].
///
/// The port and audio.md §1.3 call bit 6 the "44.1 kHz family" select: per
/// the datasheet it selects AudioPLL1 as the 256fs source, so AudioPLL1 is
/// the 45.1584 MHz PLL and AudioPLL0 the 49.152 MHz one \[DS\]+\[HW\].
pub const DACCLK: Reg = Reg::at(BASE + 0x88);
pub mod dacclk {
    use super::{Field, bit};
    /// PDMTX 128fs clock enable.
    pub const PDMCLK_EN: u32 = bit(11);
    /// SDM 128fs clock enable ("DAC clock on" in audio.md). \[DS\]; port \[HW\].
    pub const SDMCLK_EN: u32 = bit(10);
    /// 128fs source: 0 the 256fs clock, 1 PDMCLK pin.
    pub const CLKSRC_PDM: u32 = bit(8);
    /// 128fs post-divider: 0 ÷2, 1 ÷1. Cleared by vendor and port.
    pub const POSTDIV1: u32 = bit(7);
    /// 256fs source: 0 AudioPLL0 (48 kHz family), 1 AudioPLL1 (44.1 kHz family).
    pub const SRC_PLL1: u32 = bit(6);
    pub const PREDIV2: u32 = bit(4);
    /// 256fs divider index: ÷{1, 2, 3, 4, 6, 8, 12, –}.
    pub const DIV: Field = Field::bits(2, 0);
}

/// Field layout shared by CMU_I2STX0CLK … CMU_I2SRX1CLK. \[DS\] §4.3.3.26–29.
pub mod i2sclk {
    use super::{Field, bit};
    /// I2STX0CLK only: SRD clock source: 0 CK24M, 1 AudioPLL0/2, 2 AudioPLL1/2.
    pub const SRDCLK_SRC: Field = Field::bits(29, 28);
    pub const MCLK_EXT_REV: u32 = bit(12);
    /// MCLK source (per-register coding, see datasheet; 3 = external MCLK pin).
    pub const MCLK_SRC: Field = Field::bits(9, 8);
    /// 0 AudioPLL0, 1 AudioPLL1.
    pub const CLK_SRC_PLL1: u32 = bit(6);
    pub const PREDIV2: u32 = bit(4);
    /// Divider index ÷{1, 2, 3, 4, 6, 8, 12, 24}.
    pub const DIV: Field = Field::bits(2, 0);
}
/// CMU_I2STX0CLK. \[DS\] §4.3.3.26. Fields: [`i2sclk`].
pub const I2STX0CLK: Reg = Reg::at(BASE + 0x8C);
/// CMU_I2STX1CLK. \[DS\] §4.3.3.27.
pub const I2STX1CLK: Reg = Reg::at(BASE + 0x90);
/// CMU_I2SRX0CLK. \[DS\] §4.3.3.28.
pub const I2SRX0CLK: Reg = Reg::at(BASE + 0x94);
/// CMU_I2SRX1CLK. \[DS\] §4.3.3.29.
pub const I2SRX1CLK: Reg = Reg::at(BASE + 0x98);

/// DSP accelerator clock; vendor writes `0x103`. Not in the datasheet.
/// \[code\] audio.md §7.1 (U-DSP-1).
pub const DSPCLK: Reg = Reg::at(BASE + 0xAC);
pub const VENDOR_DSPCLK: u32 = 0x103;
/// Unknown; the ROM writes `0xFFFF_FFFF` at reset. \[code\] system.md §1.5.
pub const UNK_B0: Reg = Reg::at(BASE + 0xB0);
/// Unknown; the ROM writes 0, the image/video libraries use it. \[code\]
pub const UNK_B8: Reg = Reg::at(BASE + 0xB8);

/// USB PHY clock. Not in the datasheet. Value \[HW\] (port, VERBATIM).
pub const USBPHYCLK: Reg = Reg::at(BASE + 0x100);
pub const VENDOR_USBPHYCLK: u32 = 0x3055;

// ---------------------------------------------------------------------------
// Oscillators and PLLs, 0xC0000100.

/// Oscillator / PLL block. `HOSC_CTL`, `LOSC_CTL` \[DS\] §4.2; the rest \[code\]
/// system.md §1.1.
pub const OSC_BASE: usize = 0xC000_0100;

/// HOSC_CTL (VDD domain). \[DS\] §4.2.3.1. The ROM sets `HOSC_EN` first
/// thing; the vendor MANAGER sets `[23:21]` = 2 \[code\].
pub const HOSC_CTL: Reg = Reg::at(OSC_BASE);
pub mod hosc {
    use super::{Field, bit};
    /// HOSCI pad base capacitance: 3 pF × n (reset 4 = 12 pF).
    pub const HOSCI_BC_SEL: Field = Field::bits(31, 29);
    /// HOSCI trim capacitance: 0.1 pF × n.
    pub const HOSCI_TC_SEL: Field = Field::bits(28, 24);
    /// HOSCO pad base capacitance: 3 pF × n (reset 4; the vendor writes 2 \[code\]).
    pub const HOSCO_BC_SEL: Field = Field::bits(23, 21);
    /// HOSCO trim capacitance: 0.2 pF × n.
    pub const HOSCO_TC_SEL: Field = Field::bits(20, 17);
    pub const HOSCO_CMOS_CAP: u32 = bit(16);
    /// Amplitude: 450 + 50·n mV (reset 3).
    pub const AMPLMT: Field = Field::bits(14, 13);
    pub const AMPLMT_EN: u32 = bit(12);
    /// GM select (reset 3).
    pub const HGMC: Field = Field::bits(9, 8);
    /// 1: VDD-domain HOSC after the glitch remover (reset 1).
    pub const VDD_SEL: u32 = bit(5);
    pub const BUF_SEL: u32 = bit(4);
    pub const EN: u32 = bit(0);
}

/// LOSC_CTL (SVCC domain), the 32.768 kHz crystal oscillator. \[DS\] §4.2.3.2.
/// The port's timer start-up sets bit 0 (`0xC0000104 |= 1`) \[HW\].
pub const LOSC_CTL: Reg = Reg::at(OSC_BASE + 0x04);
pub mod losc {
    use super::{Field, bit};
    /// Read-only: LOSC running (U-RTC-1).
    pub const RDY: u32 = bit(24);
    /// LOSCI pad capacitance: 0.8 pF × n (reset 0x19).
    pub const LOSCI_CAP: Field = Field::bits(20, 16);
    /// LOSCO pad capacitance: 0.8 pF × n (reset 0x19).
    pub const LOSCO_CAP: Field = Field::bits(12, 8);
    /// GM select (reset 7).
    pub const LGMC: Field = Field::bits(6, 4);
    pub const EN: u32 = bit(0);
}

/// Layout shared by the two system PLLs: f = 6 MHz × N; written as `N | 0x80`,
/// `0` = off. No lock bit: the vendor waits 100 µs. \[code\] system.md §1.1;
/// COREPLL \[HW\].
pub mod corepll {
    use super::{Field, bit};
    pub const EN: u32 = bit(7);
    /// Multiplier N.
    pub const N: Field = Field::bits(6, 0);
    /// 48 MHz as the ROM and the port set it. \[HW\]
    pub const ROM_48MHZ: u32 = 0x88;
    /// 180 MHz, the vendor's fixed DEVPLL value. \[code\]
    pub const VENDOR_DEVPLL_180MHZ: u32 = 0x9E;
    /// Settling time the vendor waits after a write, in µs. \[code\]
    pub const SETTLE_US: u32 = 100;
}
/// DEVPLL control. Fields: [`corepll`]. \[code\] system.md §1.1.
pub const DEVPLL_CTL: Reg = Reg::at(OSC_BASE + 0x10);
/// COREPLL control: `0x88` = 48 MHz as the ROM sets it. \[HW\]
pub const COREPLL_CTL: Reg = Reg::at(OSC_BASE + 0x14);
/// Audio PLL 0 (by the CMU coding the 49.152 MHz / 48 kHz-family PLL
/// \[inferred\]). `[4:0]`: the port writes `0x0C` then sets bit 4 → `0x1C`. \[HW\]
pub const AUDIOPLL0_CTL: Reg = Reg::at(OSC_BASE + 0x18);
/// Audio PLL 1 (45.1584 MHz / 44.1 kHz family \[inferred\]). `[4:0]`: `0x04`
/// then bit 4 → `0x14`. \[HW\]
pub const AUDIOPLL1_CTL: Reg = Reg::at(OSC_BASE + 0x1C);
pub mod audiopll {
    use super::{Field, bit};
    /// Set last by vendor and port: enable \[inferred\].
    pub const EN: u32 = bit(4);
    /// Frequency/trim select \[inferred\].
    pub const SEL: Field = Field::bits(3, 0);
    pub const PORT_PLL0: u32 = 0x1C;
    pub const PORT_PLL1: u32 = 0x14;
}
/// DEVPLL spread-spectrum control. \[code\] system.md §1.1.
pub const DEVPLL_SSC: Reg = Reg::at(OSC_BASE + 0x20);
/// COREPLL spread-spectrum control. \[code\] system.md §1.1.
pub const COREPLL_SSC: Reg = Reg::at(OSC_BASE + 0x24);
pub mod ssc {
    use super::bit;
    /// Vendor "SSC off": `(reg | 0x20000) & !0x6000`. \[code\]
    pub const VENDOR_OFF_SET: u32 = bit(17);
    pub const VENDOR_OFF_CLEAR: u32 = 0x6000;
    /// Vendor "SSC on" whole-register value. \[code\]
    pub const VENDOR_ON: u32 = 0x2CA6_5F92;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // Rockbox port header and drivers.
        assert_eq!(SYSCLK.addr(), 0xC000_1000);
        assert_eq!(CLKEN0.addr(), 0xC000_1008);
        assert_eq!(CLKEN1.addr(), 0xC000_100C);
        assert_eq!(SD0CLK.addr(), 0xC000_1014);
        assert_eq!(SPI0CLK.addr(), 0xC000_1020);
        assert_eq!(LCDCLK.addr(), 0xC000_1034);
        assert_eq!(pwm_clk(4).addr(), 0xC000_1060);
        assert_eq!(ADC01CLK.addr(), 0xC000_1080);
        assert_eq!(DACCLK.addr(), 0xC000_1088);
        assert_eq!(USBPHYCLK.addr(), 0xC000_1100);
        assert_eq!(LOSC_CTL.addr(), 0xC000_0104);
        assert_eq!(COREPLL_CTL.addr(), 0xC000_0114);
        assert_eq!(AUDIOPLL0_CTL.addr(), 0xC000_0118);
        assert_eq!(AUDIOPLL1_CTL.addr(), 0xC000_011C);
    }

    #[test]
    fn datasheet_list() {
        let regs = [
            CLKEN0,
            CLKEN1,
            SD0CLK,
            SD1CLK,
            SPI0CLK,
            SPI1CLK,
            SPI2CLK,
            LCDCLK,
            timer_clk(0),
            timer_clk(3),
            pwm_clk(0),
            pwm_clk(8),
            LRADCCLK,
            ADC01CLK,
            ADC23CLK,
            DACCLK,
            I2STX0CLK,
            I2STX1CLK,
            I2SRX0CLK,
            I2SRX1CLK,
        ];
        let addrs: [usize; 20] = core::array::from_fn(|i| regs[i].addr());
        assert_increasing(&addrs);
        assert_eq!(timer_clk(3).addr(), BASE + 0x4C);
        assert_eq!(pwm_clk(8).addr(), BASE + 0x70);
        assert_eq!(I2SRX1CLK.addr(), BASE + 0x98);
        assert_eq!(HOSC_CTL.addr(), 0xC000_0100);
    }

    #[test]
    fn fields_disjoint() {
        use clken0::*;
        assert_disjoint(&[
            DMA, NAND, SD0, SD1, SPI0, SPI1, SPI2, SPI0_CACHE, SPI1_CACHE, IMG, LCD, SEC, USB, TWI0, TWI1,
            UART0, UART1, UART2, UART3, TIMER, PWM, LRADC,
        ]);
        {
            use clken1::*;
            assert_disjoint(&[
                ADC01,
                ADC23,
                DAC,
                I2STX0_MCLK,
                I2STX1_MCLK,
                I2SRX0_MCLK,
                I2SRX1_MCLK,
                I2SRX2_MCLK,
                I2SSRD,
                DAC_ANALOG,
                clken1::DSP,
                EFUSE,
            ]);
        }
        assert_disjoint(&[sysclk::SRC.mask(), sysclk::DIV.mask(), sysclk::UNK_B8, sysclk::UNK_25_24.mask()]);
        assert_disjoint(&[sdclk::SRC.mask(), sdclk::POSTDIV_128, sdclk::DIV.mask()]);
        assert_disjoint(&[spiclk::SRC.mask(), spiclk::DIV.mask(), spiclk::OPI2X_DIV.mask()]);
        assert_disjoint(&[lcdclk::SRC.mask(), lcdclk::PREDIV_6, lcdclk::DIV.mask()]);
        assert_disjoint(&[pwmclk::SRC_CK24M, pwmclk::DIV.mask()]);
        {
            use adcclk::*;
            assert_disjoint(&[
                FIRCLK_RVS,
                FIRCLK_DIV2,
                FIRCLK_EN,
                CICCLK_RVS,
                CICCLK_DIV2,
                CICCLK_EN,
                DMICCLK_SRC,
                DMICCLK_DIV2,
                DMICCLK_EN,
                ANACLK_RVS,
                LPADCANACLK_SRC,
                LPADCANACLK_EN,
                ADCANACLK_SRC,
                ADCANACLK_EN,
                OVFSDIV.mask(),
                CLKSRC.mask(),
                GCLK_SRC.mask(),
                GCLK_PREDIV2,
                GCLK_DIV.mask(),
            ]);
        }
        {
            use dacclk::*;
            assert_disjoint(&[PDMCLK_EN, SDMCLK_EN, CLKSRC_PDM, POSTDIV1, SRC_PLL1, PREDIV2, DIV.mask()]);
        }
        {
            use i2sclk::*;
            assert_disjoint(&[
                SRDCLK_SRC.mask(),
                MCLK_EXT_REV,
                MCLK_SRC.mask(),
                CLK_SRC_PLL1,
                PREDIV2,
                DIV.mask(),
            ]);
        }
        {
            use hosc::*;
            assert_disjoint(&[
                HOSCI_BC_SEL.mask(),
                HOSCI_TC_SEL.mask(),
                HOSCO_BC_SEL.mask(),
                HOSCO_TC_SEL.mask(),
                HOSCO_CMOS_CAP,
                AMPLMT.mask(),
                AMPLMT_EN,
                HGMC.mask(),
                VDD_SEL,
                BUF_SEL,
                hosc::EN,
            ]);
        }
        assert_disjoint(&[
            losc::RDY,
            losc::LOSCI_CAP.mask(),
            losc::LOSCO_CAP.mask(),
            losc::LGMC.mask(),
            losc::EN,
        ]);
        assert_disjoint(&[corepll::EN, corepll::N.mask()]);
    }

    #[test]
    fn decoded_values() {
        // The port's values decode as documented.
        assert_eq!(sdclk::SRC.get(sdclk::IDENT_93K75), sdclk::SRC_CK24M);
        assert_eq!(sdclk::DIV.get(sdclk::IDENT_93K75), 1);
        assert_eq!(spiclk::SRC.get(spiclk::PORT_COREPLL_DIV1), spiclk::SRC_COREPLL);
        assert_eq!(spiclk::SRC.get(spiclk::ROM_VALUE), spiclk::SRC_CK24M);
        assert_eq!(adcclk::GCLK_SRC.get(adcclk::PORT_VALUE), 1);
        assert_eq!(adcclk::GCLK_DIV.get(adcclk::PORT_VALUE), 3);
        assert_eq!(
            adcclk::VENDOR_FIR_CIC,
            adcclk::FIRCLK_EN | adcclk::FIRCLK_DIV2 | adcclk::CICCLK_EN | adcclk::CICCLK_DIV2
        );
        assert_eq!(pwmclk::PORT_BACKLIGHT, pwmclk::SRC_CK24M | 15);
        assert_eq!(corepll::N.get(corepll::ROM_48MHZ) * 6, 48);
        assert_eq!(corepll::N.get(corepll::VENDOR_DEVPLL_180MHZ) * 6, 180);
        assert_eq!(sysclk::SRC.get(sysclk::ROM_VALUE), sysclk::SRC_COREPLL);
        let _ = Field::bits(0, 0);
        let _ = bit(0);
    }
}
