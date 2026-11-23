//! PWM, 9 channels at `0xC0190000 + 0x100·n` (datasheet §9.2).
//!
//! Sources: datasheet §9.2 \[DS\]; the Rockbox port `backlight-yotonym03.c` and
//! `atj2157.h` (`PWM_CTL/PERIOD/DUTY(n)`) \[HW\]; vendor `DRV_LCD.DRV` bank 1 (backlight)
//! \[code\].
//!
//! Modes: fixed (period = DUTYMAX PWM clocks, high/active for DUTY of them), breath
//! (duty ramps 1/C…C/C, hold H, ramp down, hold L) and programmable (duties streamed
//! by DMA through [`FIFODAT`]; one channel at a time, chosen in [`DMACTL`]).
//! The YM03 backlight is channel 4 on GPIO22 \[HW\]. The PWM clock is `CMU_PWMnCLK`
//! (module `cmu`); the port's `0x20F` = 24 MHz / 16 = 1.5 MHz, so DUTYMAX 16 gives a
//! 93.75 kHz backlight PWM \[DS\] §4.3.3.17 + \[HW\] values.
//!
//! Unknown: whether a DUTY write while running takes effect at the next period (the
//! vendor stops the channel around it, the port does not): U-PWM-1. The PWM DMA request
//! line (01-hardware: selector 0x15) is \[code\].

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC019_0000;
/// Number of channels. \[DS\] §9.2.1
pub const COUNT: usize = 9;
/// Stride between channels. \[DS\] §9.2.6; \[HW\] (port).
pub const STRIDE: usize = 0x100;

pub const CTL_OFFSET: usize = 0x00;
pub const C_OFFSET: usize = 0x04;
pub const Q_OFFSET: usize = 0x08;
pub const H_OFFSET: usize = 0x0C;
pub const L_OFFSET: usize = 0x10;
pub const DUTYMAX_OFFSET: usize = 0x14;
pub const DUTY_OFFSET: usize = 0x18;

/// Base of channel `n` (0…8).
#[must_use]
pub const fn base(n: usize) -> usize {
    BASE + STRIDE * n
}

/// PWMx_CTL. \[DS\] §9.2.6.1. Reset 0. Port: `(CTL & !3) | 0x1C` \[HW\].
#[must_use]
pub const fn ctl(n: usize) -> Reg {
    Reg::at(base(n) + CTL_OFFSET)
}
pub mod ctl {
    use super::{Field, bit};
    /// 1 run; 0 stops at the end of the current period. \[DS\]
    pub const CH_START: u32 = bit(4);
    /// Channel enable; 0 stops at once, output then at the inactive level. \[DS\]
    pub const CH_EN: u32 = bit(3);
    /// 1 active high, 0 active low (fixed and breath modes). \[DS\]
    pub const POL_SEL: u32 = bit(2);
    pub const MODE_SEL: Field = Field::bits(1, 0);
    pub const MODE_FIXED: u32 = 0;
    pub const MODE_BREATH: u32 = 1;
    pub const MODE_PROGRAMMABLE: u32 = 2;
}

/// PWMx_C: breath-mode duty steps (duty runs 1/C…C/C). \[DS\] §9.2.6.2. Reset `0x20`.
#[must_use]
pub const fn c(n: usize) -> Reg {
    Reg::at(base(n) + C_OFFSET)
}
pub mod c {
    use super::Field;
    pub const C: Field = Field::bits(7, 0);
}

/// PWMx_Q: breath ramp times, T = Q·C·C·t (t = PWM clock period). \[DS\] §9.2.6.3. Reset 0.
#[must_use]
pub const fn q(n: usize) -> Reg {
    Reg::at(base(n) + Q_OFFSET)
}
pub mod q {
    use super::Field;
    /// Fall time factor.
    pub const QD: Field = Field::bits(23, 16);
    /// Rise time factor.
    pub const QU: Field = Field::bits(7, 0);
}

/// PWMx_H: breath full-on time H·C·t, `[15:0]`. \[DS\] §9.2.6.4. Reset 0.
#[must_use]
pub const fn h(n: usize) -> Reg {
    Reg::at(base(n) + H_OFFSET)
}
/// PWMx_L: breath off time L·C·t, `[15:0]`. \[DS\] §9.2.6.5. Reset 0.
#[must_use]
pub const fn l(n: usize) -> Reg {
    Reg::at(base(n) + L_OFFSET)
}
/// Value field of [`h`], [`l`], [`dutymax`], [`duty`] and [`FIFODAT`].
pub const VALUE: Field = Field::bits(15, 0);

/// PWMx_DUTYMAX: period in PWM clocks, ≥ 1, `[15:0]`. \[DS\] §9.2.6.6. Reset 1.
/// **Name:** the port's header calls it `PWM_PERIOD` \[HW\].
#[must_use]
pub const fn dutymax(n: usize) -> Reg {
    Reg::at(base(n) + DUTYMAX_OFFSET)
}
/// PWMx_DUTY: active clocks per period (duty = DUTY / DUTYMAX), fixed mode, `[15:0]`.
/// \[DS\] §9.2.6.7. Reset 0. \[HW\] (port backlight level 1…16).
#[must_use]
pub const fn duty(n: usize) -> Reg {
    Reg::at(base(n) + DUTY_OFFSET)
}

/// PWM_DMACTL: programmable-mode DMA. \[DS\] §9.2.6.8. Reset 0. (The DS bit table says
/// "31:1 reserved" and then defines `[7:4]`; reserved is `[31:8]`.)
pub const DMACTL: Reg = Reg::at(BASE + 0xF00);
pub mod dmactl {
    use super::{Field, bit};
    /// Channel fed by the DMA FIFO (0…8). DS name PWM_FIFO_CLK_SEL.
    pub const FIFO_CLK_SEL: Field = Field::bits(7, 4);
    pub const START: u32 = bit(0);
}
/// PWM_FIFODAT: next duty in programmable mode, `[15:0]`, ≤ DUTYMAX (DS marks it
/// read-only; it is the DMA target \[inferred\]). \[DS\] §9.2.6.9.
pub const FIFODAT: Reg = Reg::at(BASE + 0xF04);
/// PWM_FIFOSTA. \[DS\] §9.2.6.10. Reset `0x14` (LEVEL 2, EMPTY).
pub const FIFOSTA: Reg = Reg::at(BASE + 0xF08);
pub mod fifosta {
    use super::{Field, bit};
    pub const LEVEL: Field = Field::bits(4, 3);
    pub const EMPTY: u32 = bit(2);
    pub const FULL: u32 = bit(1);
    /// FIFO error; W1C.
    pub const ERROR: u32 = bit(0);
}

/// Backlight channel (GPIO22 function PWM4). \[HW\]
pub const BACKLIGHT_CHANNEL: usize = 4;
/// Backlight CTL bits: `(CTL & !MODE_SEL) | 0x1C` = run, enable, active high, fixed
/// mode. \[HW\] port `backlight_hw_on`; vendor bank 1 `+0x116` \[code\].
pub const PORT_BACKLIGHT_CTL: u32 = 0x1C;
/// Backlight DUTYMAX; the port's levels 1…16 are written to DUTY directly (default 10).
/// \[HW\]; same value in vendor bank 1 `+0xDA` \[code\].
pub const PORT_BACKLIGHT_DUTYMAX: u32 = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // atj2157.h: PWM_CTL(n) 0xc0190000 + 0x100n, PWM_PERIOD(n) +0x14, PWM_DUTY(n) +0x18.
        assert_eq!(BASE, 0xC019_0000);
        assert_eq!(ctl(0).addr(), 0xC019_0000);
        assert_eq!(ctl(BACKLIGHT_CHANNEL).addr(), 0xC019_0400);
        assert_eq!(dutymax(BACKLIGHT_CHANNEL).addr(), 0xC019_0414);
        assert_eq!(duty(BACKLIGHT_CHANNEL).addr(), 0xC019_0418);
    }

    #[test]
    fn datasheet_addresses() {
        assert_eq!(c(1).addr(), 0xC019_0104);
        assert_eq!(l(8).addr(), 0xC019_0810);
        assert_eq!(base(COUNT - 1), 0xC019_0800);
        assert!(base(COUNT - 1) + DUTY_OFFSET < DMACTL.addr());
        assert_eq!(DMACTL.addr(), 0xC019_0F00);
        assert_eq!(FIFOSTA.addr(), 0xC019_0F08);
        assert_increasing(&[
            ctl(2).addr(),
            c(2).addr(),
            q(2).addr(),
            h(2).addr(),
            l(2).addr(),
            dutymax(2).addr(),
            duty(2).addr(),
            DMACTL.addr(),
            FIFODAT.addr(),
            FIFOSTA.addr(),
        ]);
    }

    #[test]
    fn fields_disjoint() {
        assert_disjoint(&[ctl::CH_START, ctl::CH_EN, ctl::POL_SEL, ctl::MODE_SEL.mask()]);
        assert_disjoint(&[q::QD.mask(), q::QU.mask()]);
        assert_disjoint(&[dmactl::FIFO_CLK_SEL.mask(), dmactl::START]);
        assert_disjoint(&[fifosta::LEVEL.mask(), fifosta::EMPTY, fifosta::FULL, fifosta::ERROR]);
        assert_eq!(c::C.mask(), 0xFF);
        assert_eq!(VALUE.mask(), 0xFFFF);
    }

    #[test]
    fn port_values_decode() {
        assert_eq!(PORT_BACKLIGHT_CTL, ctl::CH_START | ctl::CH_EN | ctl::POL_SEL);
        assert_eq!(ctl::MODE_SEL.get(PORT_BACKLIGHT_CTL), ctl::MODE_FIXED);
        assert_eq!(ctl::MODE_SEL.mask(), 3);
        assert_eq!(fifosta::LEVEL.val(2) | fifosta::EMPTY, 0x14);
    }
}
