//! Real-time clock (03-hal-drivers §11; system.md §5.1). Binary calendar in
//! the SVCC domain; every write is followed by the `0xA596 → 0x5A69`
//! update handshake. Time is UTC; local time is the UI's business.

use atj2157_pac::rtc;

use crate::time::rtc_update_handshake;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    #[must_use]
    pub fn valid(&self) -> bool {
        (2000..=2127).contains(&self.year)
            && (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
    }
}

/// Whether the calendar is running.
#[must_use]
pub fn running() -> bool {
    rtc::CTL.read() & rtc::ctl::CAL_EN != 0
}

/// Read the time (twice until two reads agree, so a carry between the
/// registers is never seen).
#[must_use]
pub fn now() -> DateTime {
    let rd = || (rtc::DHMS.read(), rtc::YMD.read());
    let mut a = rd();
    for _ in 0..3 {
        let b = rd();
        if a == b {
            break;
        }
        a = b;
    }
    let (t, d) = a;
    DateTime {
        year: 2000 + rtc::time::YEAR.get(d) as u16,
        month: rtc::time::MON.get(d) as u8,
        day: rtc::time::DATE.get(d) as u8,
        hour: rtc::time::HOUR.get(t) as u8,
        minute: rtc::time::MIN.get(t) as u8,
        second: rtc::time::SEC.get(t) as u8,
    }
}

/// Set the time and start the calendar from the 32 kHz crystal (vendor
/// sequence: stop, write, start, each followed by the handshake).
pub fn set(t: DateTime) -> bool {
    if !t.valid() {
        return false;
    }
    rtc::CTL.clear_bits(rtc::ctl::CAL_EN);
    rtc_update_handshake();
    rtc::DHMS.write(
        rtc::time::HOUR.val(u32::from(t.hour))
            | rtc::time::MIN.val(u32::from(t.minute))
            | rtc::time::SEC.val(u32::from(t.second)),
    );
    rtc_update_handshake();
    rtc::YMD.write(
        rtc::time::YEAR.val(u32::from(t.year - 2000))
            | rtc::time::MON.val(u32::from(t.month))
            | rtc::time::DATE.val(u32::from(t.day)),
    );
    rtc_update_handshake();
    // RTC32K from the crystal (LOSC), calendar clock = RTC32K division.
    rtc::CTL.modify(|v| (v & !rtc::ctl::RTC32K_SEL) | rtc::ctl::RTC32K_DIV_EN);
    rtc::CTL.modify(|v| rtc::ctl::CAL_CLK_SEL.set(v, 1) | rtc::ctl::CAL_EN);
    rtc_update_handshake()
}

/// Daily alarm at h:m:s (vendor writes the alarm date register as 0).
pub fn set_alarm(hour: u8, minute: u8, second: u8) {
    rtc::DHMSALM.write(
        rtc::time::HOUR.val(u32::from(hour))
            | rtc::time::MIN.val(u32::from(minute))
            | rtc::time::SEC.val(u32::from(second)),
    );
    rtc_update_handshake();
    atj2157_pac::Reg::at(rtc::BASE + 0x18).write(0);
    rtc_update_handshake();
    rtc::CTL.set_bits(rtc::ctl::ALIP | rtc::ctl::ALIE | rtc::ctl::CAL_EN);
    rtc_update_handshake();
}

pub fn clear_alarm() {
    rtc::CTL.modify(|v| (v & !rtc::ctl::ALIE) | rtc::ctl::ALIP);
    rtc_update_handshake();
}

/// Seconds since 2000-01-01 00:00:00 UTC (for TOTP and logs).
#[must_use]
pub fn epoch2000(t: DateTime) -> u32 {
    const CUM: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let y = u32::from(t.year.saturating_sub(2000));
    let leap_days = y.div_ceil(4); // leap years before `year` (2000 is leap)
    let mut days =
        y * 365 + leap_days + u32::from(CUM[usize::from(t.month.clamp(1, 12) - 1)]) + u32::from(t.day.max(1))
            - 1;
    if t.month > 2 && t.year.is_multiple_of(4) {
        days += 1;
    }
    days * 86_400 + u32::from(t.hour) * 3600 + u32::from(t.minute) * 60 + u32::from(t.second)
}
