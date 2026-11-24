//! Power-off (03-hal-drivers §10, WP PWR-3a). The vendor sequence, with the
//! vendor's values only (CLAUDE.md rule 3): wake on a long press of Power
//! or USB plug-in (plus the RTC alarm when one is armed), clear the pending
//! wake flags, then drop S3 and S1 500 µs apart.

use atj2157_pac::pmu::{self, onoff, power_ctl, wake_pd, wken};

/// Power the player off. Never returns. Callers flush the card and the
/// settings first; a firmware started by a host test reboots instead
/// (CLAUDE.md rule 4) — that check is the caller's.
pub fn power_off(alarm_wake: bool) -> ! {
    crate::log::line(b"pmu: power off");
    // Long-press threshold 0.5 s for the wake (vendor value).
    pmu::ONOFF_CTL.modify(|v| onoff::PRESS_TIME.set(v, onoff::VENDOR_POWEROFF_PRESS_TIME));
    pmu::WKEN_CTL.write(wken::VENDOR_POWEROFF | if alarm_wake { wken::ALARM } else { 0 });
    pmu::WAKE_PD.write(wake_pd::VENDOR_CLEAR_ALL);
    crate::delay::spin_us(500);
    pmu::POWER_CTL.clear_bits(power_ctl::EN_S3);
    crate::delay::spin_us(500);
    pmu::POWER_CTL.clear_bits(power_ctl::EN_S1);
    // With USB power present the SoC may keep running: fall back to a
    // reboot after a while rather than sit here.
    crate::delay::spin_us(2_000_000);
    crate::wdt::reboot(crate::wdt::Target::Normal)
}
