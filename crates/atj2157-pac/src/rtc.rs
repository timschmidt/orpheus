//! RTC, watchdog and timers, `0xC0030000` (datasheet §4.5).

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC003_0000;

/// RTC_CTL (SVCC domain). \[DS\] §4.5.3.1.
pub const CTL: Reg = Reg::at(BASE);
pub mod ctl {
    use super::{Field, bit};
    pub const ALIP: u32 = bit(0);
    pub const ALIE: u32 = bit(1);
    pub const CAL_CLK_SEL: Field = Field::bits(3, 2);
    pub const CAL_EN: u32 = bit(4);
    pub const LEAP: u32 = bit(7);
    pub const RTC32K_SEL: u32 = bit(16);
    /// Set by the port before using the timers. \[HW\]
    pub const RTC32K_DIV_EN: u32 = bit(17);
    pub const HOSC_DIV_EN: u32 = bit(18);
}

/// RTC_REGUPDATA: write [`UPDATE_KEY`] after changing SVCC-domain registers,
/// then wait until it reads [`UPDATE_DONE`]. \[DS\] §4.5.3.2, values \[HW\].
pub const REGUPDATA: Reg = Reg::at(BASE + 0x04);
pub const UPDATE_KEY: u32 = 0xA596;
pub const UPDATE_DONE: u32 = 0x5A69;

pub const DHMSALM: Reg = Reg::at(BASE + 0x08);
pub const DHMS: Reg = Reg::at(BASE + 0x0C);
pub const YMD: Reg = Reg::at(BASE + 0x10);
pub mod time {
    use super::Field;
    pub const SEC: Field = Field::bits(5, 0);
    pub const MIN: Field = Field::bits(13, 8);
    pub const HOUR: Field = Field::bits(20, 16);
    pub const DATE: Field = Field::bits(4, 0);
    pub const MON: Field = Field::bits(11, 8);
    pub const YEAR: Field = Field::bits(22, 16);
}
/// RTC_ACCESS: 8 free bits in the SVCC domain (survive power-off while the
/// battery holds). \[DS\] §4.5.3.6
pub const ACCESS: Reg = Reg::at(BASE + 0x14);

/// WD_CTL (VDD domain). \[DS\] §4.5.3.7; feed/enable \[HW\].
pub const WD_CTL: Reg = Reg::at(BASE + 0x1C);
pub mod wd {
    use super::{Field, bit};
    /// Write 1 to restart the count (feed).
    pub const CLR: u32 = bit(0);
    /// Period: 0 176 ms, 1 352 ms, 2 703 ms, 3 1.4 s, 4 2.8 s, 5 5.6 s,
    /// 6 11.2 s, 7 10 ms.
    pub const CLKSEL: Field = Field::bits(3, 1);
    pub const EN: u32 = bit(4);
    /// 1: interrupt (IRQ 1) instead of reset on overflow.
    pub const SIGS: u32 = bit(5);
    pub const IRQP: u32 = bit(6);
    pub const PERIOD_10MS: u32 = 7;
    pub const PERIOD_5S6: u32 = 5;
    pub const PERIOD_11S2: u32 = 6;
}

/// Timers T0..T3 at `0xC0030100 + 0x20·n`. \[DS\] §4.5.4–4.5.5; T0 \[HW\].
/// Down-counting in normal mode; an interrupt after `VAL + 1` clocks.
pub const TIMER_BASE: usize = BASE + 0x100;

#[must_use]
pub const fn t_ctl(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n)
}
#[must_use]
pub const fn t_val(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n + 0x04)
}
#[must_use]
pub const fn t_cnt(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n + 0x08)
}
/// Capture value (T2, T3 only).
#[must_use]
pub const fn t_cap(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n + 0x0C)
}
pub mod tctl {
    use super::{Field, bit};
    /// Zero-reached interrupt pending; write 1 to clear.
    pub const ZIPD: u32 = bit(0);
    pub const ZIEN: u32 = bit(1);
    pub const RELO: u32 = bit(2);
    pub const EN: u32 = bit(5);
    /// T2/T3: capture pending.
    pub const CAPTURE_IP: u32 = bit(8);
    /// T2/T3: 0 timer, 1 counter, 2 input capture.
    pub const MODE_SEL: Field = Field::bits(10, 9);
    /// T2/T3: 1 = count up.
    pub const DIR: u32 = bit(11);
}
