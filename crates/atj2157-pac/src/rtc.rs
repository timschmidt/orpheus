//! RTC, watchdog and timers, `0xC0030000` (datasheet §4.5).
//!
//! RTC registers live in the SVCC domain: after writing `CTL`, `DHMSALM`,
//! `DHMS`, `YMD` (and the undocumented `YMDALM`) software writes
//! [`UPDATE_KEY`] to [`REGUPDATA`] and waits for [`UPDATE_DONE`]. Calendar
//! values are binary, not BCD \[DS\]+\[code\].
//!
//! Unknown: whether the calendar keeps running in power-off (U-RTC-1), the
//! watchdog interrupt mode (U-WDT-1), `YMDALM` semantics.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC003_0000;

/// RTC_CTL (SVCC domain). \[DS\] §4.5.3.1. Reset `0x0000_0084`.
pub const CTL: Reg = Reg::at(BASE);
pub mod ctl {
    use super::{Field, bit};
    /// Alarm interrupt pending; write 1 to clear (then the update handshake).
    pub const ALIP: u32 = bit(0);
    /// Alarm interrupt enable (IRQ 6).
    pub const ALIE: u32 = bit(1);
    /// Calendar clock: 1 RTC32K division, 2 HOSC division, 3 RTC32K division
    /// (reset 1).
    pub const CAL_CLK_SEL: Field = Field::bits(3, 2);
    pub const CAL_CLK_RTC32K: u32 = 1;
    pub const CAL_CLK_HOSC: u32 = 2;
    /// Calendar enable ("run"); vendor clears it while writing time/date \[code\].
    pub const CAL_EN: u32 = bit(4);
    /// Read-only: current year is a leap year.
    pub const LEAP: u32 = bit(7);
    /// RTC32K source: 0 LOSC, 1 external 32 kHz.
    pub const RTC32K_SEL: u32 = bit(16);
    /// RTC32K divider enable. Set by the port before using the timers. \[HW\]
    pub const RTC32K_DIV_EN: u32 = bit(17);
    /// HOSC divider enable.
    pub const HOSC_DIV_EN: u32 = bit(18);
}

/// RTC_REGUPDATA: write [`UPDATE_KEY`] after changing SVCC-domain registers,
/// then wait until it reads [`UPDATE_DONE`] (also the reset value). \[DS\]
/// §4.5.3.2, values \[HW\].
pub const REGUPDATA: Reg = Reg::at(BASE + 0x04);
pub const UPDATE_KEY: u32 = 0xA596;
pub const UPDATE_DONE: u32 = 0x5A69;

/// RTC_DHMSALM: alarm hour/minute/second, fields as [`time`]. \[DS\] §4.5.3.3.
pub const DHMSALM: Reg = Reg::at(BASE + 0x08);
/// RTC_DHMS: time of day, fields as [`time`]. \[DS\] §4.5.3.4.
pub const DHMS: Reg = Reg::at(BASE + 0x0C);
/// RTC_YMD: date (year 0..99, month 1..12, day 1..31), fields as [`time`].
/// \[DS\] §4.5.3.5. The vendor stores year − 2000 \[code\].
pub const YMD: Reg = Reg::at(BASE + 0x10);
/// Binary calendar fields. \[DS\] §4.5.3.3–5.
pub mod time {
    use super::Field;
    pub const SEC: Field = Field::bits(5, 0);
    pub const MIN: Field = Field::bits(13, 8);
    pub const HOUR: Field = Field::bits(20, 16);
    pub const DATE: Field = Field::bits(4, 0);
    pub const MON: Field = Field::bits(11, 8);
    pub const YEAR: Field = Field::bits(22, 16);
}
/// RTC_ACCESS: 8 free bits in the SVCC domain. No update handshake, but wait
/// three 32 kHz cycles (≈ 100 µs) after a write. Whether they survive
/// power-off is U-RTC-1. \[DS\] §4.5.3.6
pub const ACCESS: Reg = Reg::at(BASE + 0x14);
pub const ACCESS_BITS: Field = Field::bits(7, 0);
/// Alarm date ("RTC_YMDALM" in a vendor error string). Not in the datasheet.
/// The vendor writes 0 (daily alarm) and checks the read-back. \[code\]
/// system.md §5.1.
pub const YMDALM: Reg = Reg::at(BASE + 0x18);

/// WD_CTL (VDD domain). \[DS\] §4.5.3.7; feed/enable/reboot \[HW\].
pub const WD_CTL: Reg = Reg::at(BASE + 0x1C);
pub mod wd {
    use super::{Field, bit};
    /// Write 1 to restart the count (feed); clears itself.
    pub const CLR: u32 = bit(0);
    /// Period: 0 176 ms, 1 352 ms, 2 703 ms, 3 1.4 s, 4 2.8 s, 5 5.6 s,
    /// 6 11.2 s, 7 10 ms (overflow at 180 counts of a 1 kHz … 16 Hz clock).
    pub const CLKSEL: Field = Field::bits(3, 1);
    pub const EN: u32 = bit(4);
    /// 1: interrupt (IRQ 1) instead of reset on overflow (U-WDT-1).
    pub const SIGS: u32 = bit(5);
    /// Watchdog interrupt pending; write 1 to clear.
    pub const IRQP: u32 = bit(6);
    pub const PERIOD_176MS: u32 = 0;
    pub const PERIOD_1S4: u32 = 3;
    pub const PERIOD_5S6: u32 = 5;
    pub const PERIOD_11S2: u32 = 6;
    pub const PERIOD_10MS: u32 = 7;
    /// Vendor run-time period (MANAGER). \[code\] system.md §5.2.
    pub const VENDOR_RUN_PERIOD: u32 = PERIOD_11S2;
    /// Period the port's `system_reboot` arms. \[HW\]
    pub const PORT_REBOOT_PERIOD: u32 = PERIOD_1S4;
}

/// Timers T0..T3 at `0xC0030100 + 0x20·n`. \[DS\] §4.5.4–4.5.5; T0 \[HW\].
/// Down-counting by default; an interrupt after `VAL + 1` clocks.
pub const TIMER_BASE: usize = BASE + 0x100;
/// Number of timers.
pub const TIMER_COUNT: usize = 4;

/// Tn_CTL. \[DS\] §4.5.5.1/4/7/11.
#[must_use]
pub const fn t_ctl(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n)
}
/// Tn_VAL: reload/compare value. \[DS\] §4.5.5.2.
#[must_use]
pub const fn t_val(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n + 0x04)
}
/// Tn_CNT: current count (read-only). \[DS\] §4.5.5.3.
#[must_use]
pub const fn t_cnt(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n + 0x08)
}
/// Tn_CAP: captured count (T2, T3 only). \[DS\] §4.5.5.10/14.
#[must_use]
pub const fn t_cap(n: usize) -> Reg {
    Reg::at(TIMER_BASE + 0x20 * n + 0x0C)
}
pub mod tctl {
    use super::{Field, bit};
    /// Zero (or compare) reached; write 1 to clear.
    pub const ZIPD: u32 = bit(0);
    pub const ZIEN: u32 = bit(1);
    pub const RELO: u32 = bit(2);
    pub const EN: u32 = bit(5);
    /// T2/T3: capture edge: 0 falling, 1 rising, 2/3 both.
    pub const CAPTURE_SE: Field = Field::bits(7, 6);
    /// T2/T3: capture pending; write 1 to clear.
    pub const CAPTURE_IP: u32 = bit(8);
    /// T2/T3: 0 timer, 1 counter, 2 input capture.
    pub const MODE_SEL: Field = Field::bits(10, 9);
    /// T2/T3: 1 = count up (then compare with `VAL` instead of zero).
    pub const DIR: u32 = bit(11);
    /// T2/T3: read-only level of the input pin.
    pub const LEVEL: u32 = bit(12);
    /// Vendor run value: enable, reload, interrupt. \[HW\] (T0).
    pub const VENDOR_RUN: u32 = EN | RELO | ZIEN;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn addresses() {
        // Port: timer-atj2157.c, system-target.h, pager.
        assert_eq!(CTL.addr(), 0xC003_0000);
        assert_eq!(REGUPDATA.addr(), 0xC003_0004);
        assert_eq!(WD_CTL.addr(), 0xC003_001C);
        assert_eq!(t_ctl(0).addr(), 0xC003_0100);
        assert_eq!(t_val(0).addr(), 0xC003_0104);
        // Datasheet list.
        assert_eq!(t_ctl(1).addr(), 0xC003_0120);
        assert_eq!(t_cap(2).addr(), 0xC003_014C);
        assert_eq!(t_cnt(3).addr(), 0xC003_0168);
        assert_eq!(t_cap(3).addr(), 0xC003_016C);
        assert_increasing(&[
            CTL.addr(),
            REGUPDATA.addr(),
            DHMSALM.addr(),
            DHMS.addr(),
            YMD.addr(),
            ACCESS.addr(),
            YMDALM.addr(),
            WD_CTL.addr(),
        ]);
        assert_eq!(tctl::VENDOR_RUN, 0x26);
    }

    #[test]
    fn disjoint() {
        use ctl::*;
        assert_disjoint(&[
            ALIP,
            ALIE,
            CAL_CLK_SEL.mask(),
            CAL_EN,
            LEAP,
            RTC32K_SEL,
            RTC32K_DIV_EN,
            HOSC_DIV_EN,
        ]);
        assert_disjoint(&[time::SEC.mask(), time::MIN.mask(), time::HOUR.mask()]);
        assert_disjoint(&[time::DATE.mask(), time::MON.mask(), time::YEAR.mask()]);
        assert_disjoint(&[wd::CLR, wd::CLKSEL.mask(), wd::EN, wd::SIGS, wd::IRQP]);
        {
            use tctl::*;
            assert_disjoint(&[
                ZIPD,
                ZIEN,
                RELO,
                tctl::EN,
                CAPTURE_SE.mask(),
                CAPTURE_IP,
                MODE_SEL.mask(),
                DIR,
                LEVEL,
            ]);
        }
        let _ = bit(0);
    }
}
