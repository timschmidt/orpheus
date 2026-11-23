//! GPIO and multi-function pin control (GPIO_MFP), `0xC01C0000` (datasheet ch. 10).
//!
//! One control word per pad, `GPIOn_CTL` at `+0x04 + 4·n`: function select, pad
//! options (Schmitt, pulls, drive, double NMOS) and the per-pin external-interrupt
//! settings. Output data, set/reset and input data registers hold 32 pins per bank
//! (bank 2 = GPIO64–77); IRQ pending exists for banks 0–1 only. `WIO1_CTL` is the
//! SVCC-domain wake-up pad (key ladder 1 on LRADC1).
//!
//! Sources: datasheet V1.2 §10.1–10.4 and §1.5 (pin list) \[DS\]; the Rockbox
//! port's header and drivers (MFP words, set/clear/input use) \[HW\]; RE reports
//! docs/hw/system.md §8, §11, fm-i2c-imu.md §1.1, bluetooth.md §2 \[code\].
//!
//! Unknown: `DEBUG_SEL` (`+0x000`) is not in the datasheet register list; what
//! DNMOS ("double NMOS") does (open drain?); `+0x21C` and `+0x400` (toggled by
//! vendor code, not documented); GPIO48/62/63 have no CTL register in §10.3 and no
//! pad in the QFN68 pin list, yet are named as UART0/SWD options by the vendor
//! config.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC01C_0000;

/// Debug/SWD routing word. **Not in the datasheet** (§10.3 starts at `+0x04`).
/// \[code\] system.md §11 (vendor BREC writes it from config byte `MFP_SWD_MAP`).
pub const DEBUG_SEL: Reg = Reg::at(BASE);
pub mod debug_sel {
    use super::{Field, bit};
    /// SWD pad group. \[code\]
    pub const SWD_MAP: Field = Field::bits(1, 0);
    pub const SWD_MAP_GPIO62_63: u32 = 0;
    pub const SWD_MAP_GPIO55_56: u32 = 1;
    pub const SWD_MAP_GPIO16_17: u32 = 2;
    /// SWD enable (cleared in the shipped configuration). \[code\]
    pub const SWD_EN: u32 = bit(4);
}

pub const CTL_OFFSET: usize = 0x04;

/// GPIOn_CTL, `n` = 0–33, 38, 39, 43, 55, 56 (digital, fields in [`mod@ctl`]) and
/// 64–69, 72–75 (analogue-capable, fields in [`ctl_analog`]). \[DS\] §10.3, §10.4.1–49.
/// The address rule holds for every listed pin \[DS\]; port use \[HW\].
#[must_use]
pub const fn ctl(pin: usize) -> Reg {
    Reg::at(BASE + CTL_OFFSET + 4 * pin)
}
/// Alias of [`ctl()`] (the port's `MFP_CTL(pin)`).
#[must_use]
pub const fn mfp(pin: usize) -> Reg {
    ctl(pin)
}

/// Fields of GPIOn_CTL for the digital pads (0–56). \[DS\] §10.4.1–39; identical on every
/// such pin. Reset `0x0000_1000` (PADDRV = 1).
pub mod ctl {
    use super::{Field, bit};
    /// Function select; codes differ per pin (`pinN` modules, §10.4.x lists). \[DS\] \[HW\]
    pub const MFP: Field = Field::bits(4, 0);
    pub const MFP_GPIO: u32 = 0;
    /// Schmitt-trigger input. Set in every vendor peripheral word except SD. \[DS\]
    ///
    /// **Disagreement:** system.md §8 guessed bit 5 = input enable; the datasheet says SMIT.
    pub const SMIT: u32 = bit(5);
    /// GPIO output enable (out data from `GPIO_ODATx`). \[DS\] \[HW\]
    pub const GPIOOUTEN: u32 = bit(6);
    /// GPIO input enable (`GPIO_IDATx` follows the pad). \[DS\]; fm-i2c-imu.md §1.1 \[code\]
    pub const GPIOINEN: u32 = bit(7);
    /// 10 kΩ pull-up (datasheet `10KPUEN`). Set by the vendor on the UART1 RX pad. \[DS\] \[code\]
    pub const GPIO10KPUEN: u32 = bit(8);
    /// 100 kΩ pull-down. \[DS\]
    pub const GPIO100KPDEN: u32 = bit(9);
    /// 50 kΩ pull-up. Set in the vendor SD words (`0x804`). \[DS\] \[HW\]
    ///
    /// **Disagreement:** system.md §8 calls bit 11 unknown.
    pub const GPIO50KPUEN: u32 = bit(11);
    /// Pad drive: 0 = level 1 (weakest) … 7 = level 8. Reset 1. \[DS\] \[HW\]
    pub const PADDRV: Field = Field::bits(14, 12);
    /// "Double NMOS" enable; effect undocumented (open drain?). \[DS\]
    pub const DNMOS: u32 = bit(15);
    /// Interrupt enable: generate a pending bit in `GPIO_PDx` on the trigger event. \[DS\]
    pub const GPIO_INTCEN: u32 = bit(20);
    /// Trigger mode. \[DS\]
    pub const GPIO_TRIG_CTL: Field = Field::bits(23, 21);
    pub const GPIO_TRIG_CTL_RISING: u32 = 0;
    pub const GPIO_TRIG_CTL_FALLING: u32 = 1;
    pub const GPIO_TRIG_CTL_BOTH: u32 = 2;
    pub const GPIO_TRIG_CTL_HIGH: u32 = 3;
    pub const GPIO_TRIG_CTL_LOW: u32 = 4;
    /// 1 = forward the pending event to the INTC (external interrupt, IRQ 36). \[DS\]
    pub const GPIO_INTC_MSK: u32 = bit(25);
    /// All pull resistors.
    pub const PULL_MASK: u32 = GPIO10KPUEN | GPIO100KPDEN | GPIO50KPUEN;
    pub const RESET: u32 = 0x0000_1000;
}

/// Short names used by the HAL (aliases of [`mod@ctl`]).
pub mod mfp_bits {
    use super::{Field, ctl};
    pub const FUNC: Field = ctl::MFP;
    pub const FUNC_GPIO: u32 = ctl::MFP_GPIO;
    pub const SMIT: u32 = ctl::SMIT;
    pub const OUT_EN: u32 = ctl::GPIOOUTEN;
    pub const IN_EN: u32 = ctl::GPIOINEN;
    pub const PULL_UP_10K: u32 = ctl::GPIO10KPUEN;
    pub const PULL_DOWN_100K: u32 = ctl::GPIO100KPDEN;
    pub const PULL_UP_50K: u32 = ctl::GPIO50KPUEN;
    pub const PULL_MASK: u32 = ctl::PULL_MASK;
    pub const DRIVE: Field = ctl::PADDRV;
}

/// Fields of GPIOn_CTL for the analogue-capable pads GPIO64–69, 72–75 (audio pins).
/// No interrupt fields. Reset `0x0000_1010` (analogue, PADDRV = 1). \[DS\] §10.4.40–49
pub mod ctl_analog {
    use super::bit;
    pub use super::ctl::{
        GPIO10KPUEN, GPIO50KPUEN, GPIO100KPDEN, GPIOINEN, GPIOOUTEN, PADDRV, PULL_MASK, SMIT,
    };
    /// 1 = analogue function (MIC/AUX/AOUT/VRO), 0 = digital (GPIO; DMIC on 64/65). Bits
    /// `[3:0]` are marked reserved, yet the GPIO64/65 text lists "0: GPIO, 1: DMIC01". \[DS\]
    pub const AD_SELECT: u32 = bit(4);
    pub const RESET: u32 = 0x0000_1010;
}

/// Number of data banks (bank 2 = GPIO64–77, bits `[13:0]`).
pub const BANKS: usize = 3;
pub const ODAT_OFFSET: usize = 0x200;
pub const BSR_OFFSET: usize = 0x210;
pub const BRR_OFFSET: usize = 0x220;
pub const IDAT_OFFSET: usize = 0x230;
pub const PD_OFFSET: usize = 0x240;

/// GPIO_ODATn: output data latch for pins `32·bank..`, driven where GPIOOUTEN is set.
/// \[DS\] §10.4.50–52; vendor I2C uses it as the level latch \[code\]. Reset 0.
///
/// **Disagreement:** the port's header names it `GPIO_OUTEN`; the datasheet says output
/// data (answers U-GPIO-1; output enable is per pin, [`ctl::GPIOOUTEN`]).
#[must_use]
pub const fn odat(bank: usize) -> Reg {
    Reg::at(BASE + ODAT_OFFSET + 4 * bank)
}
/// GPIO_BSRn: write 1 to set the ODAT bit; self-clearing. \[DS\] §10.4.53–55; \[HW\] (port `GPIO_SET`).
#[must_use]
pub const fn bsr(bank: usize) -> Reg {
    Reg::at(BASE + BSR_OFFSET + 4 * bank)
}
/// GPIO_BRRn: write 1 to clear the ODAT bit; self-clearing. \[DS\] §10.4.56–58; \[HW\] (port `GPIO_CLR`).
#[must_use]
pub const fn brr(bank: usize) -> Reg {
    Reg::at(BASE + BRR_OFFSET + 4 * bank)
}
/// GPIO_IDATn: input data (read-only; needs GPIOINEN). \[DS\] §10.4.59–61; \[HW\] (port `GPIO_IN`).
#[must_use]
pub const fn idat(bank: usize) -> Reg {
    Reg::at(BASE + IDAT_OFFSET + 4 * bank)
}
/// GPIO_PDn, `bank` 0–1 only: interrupt pending per pin, write 1 to clear. \[DS\] §10.4.62–63.
/// Unused by the vendor firmware (system.md §8).
#[must_use]
pub const fn pd(bank: usize) -> Reg {
    Reg::at(BASE + PD_OFFSET + 4 * bank)
}

pub const WIO1_CTL_OFFSET: usize = 0x304;
/// WIO1_CTL: the SVCC-domain wake-up pad (pin 63: WIO1/LRADC1/32KHz_IN); kept in S3/S4.
/// Reset 0. \[DS\] §10.4.64, §1.5; port writes [`VENDOR_WIO1_KEY`] \[HW\].
///
/// **Disagreement:** system.md §8 calls `0xC01C0304` of unknown purpose; 01-hardware §5
/// lists its write under GPIO17, but it configures the ladder-1 pad (LRADC1).
pub const WIO1_CTL: Reg = Reg::at(BASE + WIO1_CTL_OFFSET);
pub mod wio1_ctl {
    use super::{Field, bit};
    /// Function: 0 GPIO, 1 LRADC1, 2 32 kHz clock input. \[DS\] (bit 4 not described)
    pub const MFP: Field = Field::bits(3, 0);
    pub const MFP_GPIO: u32 = 0;
    pub const MFP_LRADC1: u32 = 1;
    pub const MFP_32K_IN: u32 = 2;
    pub const WIOOUTEN: u32 = bit(6);
    /// Must be set for the WIO (wake) function. \[DS\]
    pub const WIOINEN: u32 = bit(7);
    pub const GPIO10KPUEN: u32 = bit(8);
    pub const GPIO100KPDEN: u32 = bit(9);
    pub const GPIO50KPUEN: u32 = bit(11);
    /// Drive level 1–8 as 0–7; reset 0 here.
    pub const PADDRV: Field = Field::bits(14, 12);
    /// Pad data: input level, or output level when WIOOUTEN is set.
    pub const WIODAT: u32 = bit(16);
    pub const WIO_INTCEN: u32 = bit(20);
    /// Trigger mode, encoded as `ctl::GPIO_TRIG_CTL_*`.
    pub const WIO_TRIG_CTL: Field = Field::bits(23, 21);
    /// Interrupt pending, write 1 to clear.
    pub const WIO_INTC_PD: u32 = bit(24);
    pub const WIO_INTC_MSK: u32 = bit(25);
}

/// WIO1_CTL value of the vendor key driver: LRADC1 function, drive 3 (ladder 1). \[HW\]
pub const VENDOR_WIO1_KEY: u32 = 0x3001;
/// WIO1_CTL value written by the vendor MANAGER in one path: GPIO input, 50 kΩ
/// pull-up. \[code\] system.md §8
pub const VENDOR_WIO1_MANAGER: u32 = 0x0880;

/// SPI0 NOR pads 12–15: function 7, SMIT, drive 3. \[HW\] (port pager, boot ROM)
pub const VENDOR_CTL_SPI0: u32 = 0x3027;
/// GPIO17 to LRADC6 (key ladder 2, `PMU+0x64`): function 27, SMIT, drive 3. \[HW\]
pub const VENDOR_CTL_KEY_LRADC6: u32 = 0x303B;
/// SD0 pads: function 4, 50 kΩ pull-up; the port adds PADDRV 5 on CLK, 3 elsewhere. \[HW\]
pub const VENDOR_CTL_SD0: u32 = 0x0804;
/// UART1 pads 38/39: function 10, SMIT, drive 3; the vendor adds GPIO10KPUEN on 39 (RX). \[code\]
pub const VENDOR_CTL_UART1: u32 = 0x302A;
/// UART0 TX pad: function 9, SMIT, drive 3. \[code\] system.md §8
pub const VENDOR_CTL_UART0_TX: u32 = 0x3029;

// Function codes of the pins the YM03 uses (\[DS\] §10.4.x MFP lists; §1.5). Pins used as
// plain GPIO (20 BT enable, 21 LCD reset, 31 BT host-busy) need only `ctl::MFP_GPIO`.
// LCD function (2) on pins 0–8, 10, 11, 43 is \[HW\].

/// GPIO0–7: LCD data D0–D7 (function 2 on each). \[DS\] \[HW\]
pub mod pin0_7 {
    pub const FUNC_LCD_D: u32 = 2;
}
pub mod pin6 {
    /// Alternative TWI1 pads. \[DS\]
    pub const FUNC_TWI1_CLK: u32 = 14;
}
pub mod pin7 {
    pub const FUNC_TWI1_DAT: u32 = 14;
}
pub mod pin8 {
    pub const FUNC_LCD_RS_VSYNC: u32 = 2;
}
pub mod pin10 {
    pub const FUNC_LCD_RDE_LDE: u32 = 2;
}
pub mod pin11 {
    pub const FUNC_LCD_WR_DCLK: u32 = 2;
}
/// GPIO12–15: SPI0 (NOR) on function 7. \[DS\] \[HW\]
pub mod pin12 {
    pub const FUNC_SPI0_MISO: u32 = 7;
}
pub mod pin13 {
    pub const FUNC_SPI0_MOSI: u32 = 7;
}
pub mod pin14 {
    pub const FUNC_SPI0_CLK: u32 = 7;
}
pub mod pin15 {
    pub const FUNC_SPI0_SS: u32 = 7;
}
pub mod pin17 {
    /// Key ladder 2 (`PMU+0x64` = LRADC6_DATA). \[DS\] \[HW\]
    pub const FUNC_LRADC6: u32 = 27;
}
pub mod pin18 {
    /// Stock UART0 TX option. \[DS\]
    pub const FUNC_UART0_TX: u32 = 9;
}
pub mod pin22 {
    /// Backlight. \[DS\] \[HW\]
    pub const FUNC_PWM4: u32 = 16;
}
/// GPIO24/25 (bit-banged I2C on the stock firmware) carry TWI1 on function 14 (U-FM-1). \[DS\]
pub mod pin24 {
    pub const FUNC_TWI1_CLK: u32 = 14;
}
pub mod pin25 {
    pub const FUNC_TWI1_DAT: u32 = 14;
}
/// GPIO27–33: SD0 on function 4. \[DS\] \[HW\]
pub mod pin27 {
    pub const FUNC_SD0_DAT3: u32 = 4;
}
pub mod pin28 {
    pub const FUNC_SD0_DAT2: u32 = 4;
}
pub mod pin29 {
    pub const FUNC_SD0_DAT1: u32 = 4;
}
pub mod pin30 {
    pub const FUNC_SD0_CLK0: u32 = 4;
}
pub mod pin32 {
    pub const FUNC_SD0_CMD: u32 = 4;
}
pub mod pin33 {
    pub const FUNC_SD0_DAT0: u32 = 4;
}
/// GPIO38 = SoC UART1 TX, GPIO39 = RX (function 10). \[DS\]; settles the SoC side of U-BT-1.
pub mod pin38 {
    pub const FUNC_UART1_TX: u32 = 10;
}
pub mod pin39 {
    pub const FUNC_UART1_RX: u32 = 10;
}
pub mod pin43 {
    /// LCD chip select. \[DS\] \[HW\]
    pub const FUNC_LCD_CE0_HSYNC: u32 = 2;
}

/// Pins used on the YM03 (board facts belong to `yoton-bsp`; the numbers
/// are repeated here only for the address arithmetic tests).
pub const PIN_BACKLIGHT: usize = 22;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        assert_eq!(BASE, 0xC01C_0000);
        assert_eq!(DEBUG_SEL.addr(), 0xC01C_0000);
        for pin in [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 17, 21, 22, 27, 28, 29, 30, 32, 33, 43]
        {
            assert_eq!(mfp(pin).addr(), 0xc01c_0004 + 4 * pin);
        }
        assert_eq!(mfp(17).addr(), 0xC01C_0048);
        assert_eq!(mfp(PIN_BACKLIGHT).addr(), 0xC01C_005C);
        assert_eq!(mfp(24).addr(), 0xC01C_0064);
        assert_eq!(mfp(25).addr(), 0xC01C_0068);
        assert_eq!(mfp(38).addr(), 0xC01C_009C);
        assert_eq!(mfp(39).addr(), 0xC01C_00A0);
        assert_eq!(mfp(43).addr(), 0xC01C_00B0);
        for bank in 0..2 {
            assert_eq!(odat(bank).addr(), 0xc01c_0200 + 4 * bank);
            assert_eq!(bsr(bank).addr(), 0xc01c_0210 + 4 * bank);
            assert_eq!(brr(bank).addr(), 0xc01c_0220 + 4 * bank);
            assert_eq!(idat(bank).addr(), 0xc01c_0230 + 4 * bank);
        }
        assert_eq!(WIO1_CTL.addr(), 0xC01C_0304);
    }

    #[test]
    fn datasheet_list() {
        let pins = [
            (0, 0x04),
            (5, 0x18),
            (11, 0x30),
            (19, 0x50),
            (26, 0x6C),
            (33, 0x88),
            (38, 0x9C),
            (39, 0xA0),
            (43, 0xB0),
            (55, 0xE0),
            (56, 0xE4),
            (64, 0x104),
            (69, 0x118),
            (72, 0x124),
            (75, 0x130),
        ];
        for (pin, off) in pins {
            assert_eq!(ctl(pin).addr(), BASE + off, "GPIO{pin}_CTL");
        }
        assert_eq!(odat(2).addr(), 0xC01C_0208);
        assert_eq!(bsr(2).addr(), 0xC01C_0218);
        assert_eq!(brr(2).addr(), 0xC01C_0228);
        assert_eq!(idat(2).addr(), 0xC01C_0238);
        assert_eq!(pd(0).addr(), 0xC01C_0240);
        assert_eq!(pd(1).addr(), 0xC01C_0244);
        assert_increasing(&[
            ctl(0).addr(),
            ctl(56).addr(),
            ctl(75).addr(),
            odat(0).addr(),
            odat(2).addr(),
            bsr(0).addr(),
            bsr(2).addr(),
            brr(0).addr(),
            brr(2).addr(),
            idat(0).addr(),
            idat(2).addr(),
            pd(0).addr(),
            pd(1).addr(),
            WIO1_CTL.addr(),
        ]);
    }

    #[test]
    fn fields_disjoint() {
        use ctl::*;
        assert_disjoint(&[
            MFP.mask(),
            SMIT,
            GPIOOUTEN,
            GPIOINEN,
            GPIO10KPUEN,
            GPIO100KPDEN,
            GPIO50KPUEN,
            PADDRV.mask(),
            DNMOS,
            GPIO_INTCEN,
            GPIO_TRIG_CTL.mask(),
            GPIO_INTC_MSK,
        ]);
        assert_eq!(ctl::RESET, PADDRV.val(1));
        assert_disjoint(&[
            ctl_analog::AD_SELECT,
            ctl_analog::SMIT,
            ctl_analog::GPIOOUTEN,
            ctl_analog::GPIOINEN,
            ctl_analog::PULL_MASK,
            ctl_analog::PADDRV.mask(),
        ]);
        assert_eq!(ctl_analog::RESET, PADDRV.val(1) | ctl_analog::AD_SELECT);
        use wio1_ctl as w;
        assert_disjoint(&[
            w::MFP.mask(),
            w::WIOOUTEN,
            w::WIOINEN,
            w::GPIO10KPUEN,
            w::GPIO100KPDEN,
            w::GPIO50KPUEN,
            w::PADDRV.mask(),
            w::WIODAT,
            w::WIO_INTCEN,
            w::WIO_TRIG_CTL.mask(),
            w::WIO_INTC_PD,
            w::WIO_INTC_MSK,
        ]);
        assert_disjoint(&[debug_sel::SWD_MAP.mask(), debug_sel::SWD_EN]);
    }

    #[test]
    fn vendor_words_decode() {
        use ctl::*;
        let w = |f: u32, drv: u32| MFP.val(f) | PADDRV.val(drv);
        assert_eq!(VENDOR_CTL_SPI0, w(pin12::FUNC_SPI0_MISO, 3) | SMIT);
        assert_eq!(VENDOR_CTL_KEY_LRADC6, w(pin17::FUNC_LRADC6, 3) | SMIT);
        assert_eq!(VENDOR_CTL_SD0, w(pin27::FUNC_SD0_DAT3, 0) | GPIO50KPUEN);
        assert_eq!(VENDOR_CTL_UART1, w(pin38::FUNC_UART1_TX, 3) | SMIT);
        assert_eq!(VENDOR_CTL_UART0_TX, w(pin18::FUNC_UART0_TX, 3) | SMIT);
        assert_eq!(VENDOR_WIO1_KEY, wio1_ctl::MFP.val(wio1_ctl::MFP_LRADC1) | wio1_ctl::PADDRV.val(3));
        assert_eq!(VENDOR_WIO1_MANAGER, wio1_ctl::WIOINEN | wio1_ctl::GPIO50KPUEN);
        // Port backlight clears 0xDF (MFP, OUTEN, INEN; SMIT kept) and sets PWM4; LCD reset clears 0x9F.
        assert_eq!(MFP.mask() | GPIOOUTEN | GPIOINEN, 0xDF);
        assert_eq!(MFP.mask() | GPIOINEN, 0x9F);
        assert_eq!(pin22::FUNC_PWM4, 0x10);
        // Old HAL names still mean the same bits.
        assert_eq!(mfp_bits::OUT_EN, bit(6));
        assert_eq!(mfp_bits::IN_EN, bit(7));
        assert_eq!(mfp_bits::FUNC, Field::bits(4, 0));
        assert_eq!(mfp_bits::DRIVE, Field::bits(14, 12));
        assert_eq!(mfp_bits::PULL_MASK, 0xB00);
    }
}
