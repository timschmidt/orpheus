//! RTC drift correction (06-connectivity §5.3).
//!
//! The RTC runs uncorrected. At each time synchronisation the firmware
//! records a [`SyncPoint`] (raw RTC seconds and true UTC). From two of them
//! [`measure`] derives the drift rate; [`corrected_utc`] applies it when the
//! clock is read. Rates are in milliseconds per day (positive = the RTC
//! runs fast).

/// Seconds per day.
const DAY: i64 = 86_400;
/// Shortest interval between two synchronisations that yields a rate. A
/// one-second reading error over six hours is about 4 s/day.
pub const MIN_INTERVAL_S: u64 = 6 * 3600;
/// Rates beyond this are treated as a clock fault, not drift (±10 min/day,
/// about 7000 ppm; a 32 kHz crystal is within ±100 ppm, ≈ 9 s/day).
pub const MAX_RATE_MS_PER_DAY: i32 = 600_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncPoint {
    /// RTC reading (seconds) at the synchronisation, before it was set.
    pub rtc: u64,
    /// True UTC (seconds) at the same moment.
    pub utc: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriftError {
    /// Too little time between the two synchronisations.
    TooShort,
    /// Time went backwards or the rate is implausible (RTC stopped or was
    /// set by something else in between).
    Implausible,
}

/// Drift rate in ms/day from the previous synchronisation `prev` (after
/// which the RTC was set to `prev.utc`, or simply ran on from `prev.rtc`)
/// and the current one `now`.
pub fn measure(prev: SyncPoint, now: SyncPoint) -> Result<i32, DriftError> {
    let true_elapsed = now.utc.checked_sub(prev.utc).ok_or(DriftError::Implausible)?;
    let rtc_elapsed = now.rtc.checked_sub(prev.rtc).ok_or(DriftError::Implausible)?;
    if true_elapsed < MIN_INTERVAL_S {
        return Err(DriftError::TooShort);
    }
    let err_s = i128::from(rtc_elapsed) - i128::from(true_elapsed);
    let rate = err_s * 1000 * i128::from(DAY) / i128::from(true_elapsed);
    if rate.unsigned_abs() > MAX_RATE_MS_PER_DAY as u128 {
        return Err(DriftError::Implausible);
    }
    Ok(rate as i32)
}

/// UTC now from a raw RTC reading, the last synchronisation and the rate.
#[must_use]
pub fn corrected_utc(rtc_now: u64, last: SyncPoint, rate_ms_per_day: i32) -> u64 {
    let elapsed = i128::from(rtc_now.saturating_sub(last.rtc));
    // elapsed_rtc = elapsed_true * (1 + r), r = rate / (1000 * DAY).
    let denom = 1000 * i128::from(DAY) + i128::from(rate_ms_per_day);
    let true_elapsed = elapsed * 1000 * i128::from(DAY) / denom.max(1);
    (i128::from(last.utc) + true_elapsed).max(0) as u64
}

/// Worst-case error of the clock `elapsed_s` after a synchronisation, in
/// seconds, assuming the rate is known to ±`rate_uncertainty_ms_per_day`.
/// The UI warns when this exceeds a few seconds (TOTP windows are 30 s).
#[must_use]
pub fn uncertainty_s(elapsed_s: u64, rate_uncertainty_ms_per_day: u32) -> u64 {
    (u128::from(elapsed_s) * u128::from(rate_uncertainty_ms_per_day) / (1000 * DAY as u128)) as u64 + 1
}

/// Plausibility of a UTC value: not before the firmware was built and not
/// after 2100. An RTC that lost power typically restarts near zero.
#[must_use]
pub const fn plausible(utc: u64, build_utc: u64) -> bool {
    utc >= build_utc && utc < 4_102_444_800
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_and_correct() {
        // RTC gains 9 s in 3 days: +3000 ms/day.
        let prev = SyncPoint { rtc: 1_700_000_000, utc: 1_700_000_000 };
        let now = SyncPoint { rtc: 1_700_000_000 + 3 * 86_400 + 9, utc: 1_700_000_000 + 3 * 86_400 };
        let r = measure(prev, now).unwrap();
        assert_eq!(r, 3000);
        // Ten days after the next sync the raw RTC is 30 s ahead.
        let last = SyncPoint { rtc: 1_800_000_000, utc: 1_800_000_000 };
        let raw = last.rtc + 10 * 86_400 + 30;
        assert_eq!(corrected_utc(raw, last, r), last.utc + 10 * 86_400);
        // Slow clock.
        let now = SyncPoint { rtc: 1_700_000_000 + 86_400 - 2, utc: 1_700_000_000 + 86_400 };
        assert_eq!(measure(prev, now).unwrap(), -2000);
        assert_eq!(corrected_utc(prev.rtc + 86_400 - 2, prev, -2000), prev.utc + 86_400);
        // RTC was not reset at the previous sync (rtc != utc there).
        let prev = SyncPoint { rtc: 100, utc: 1_700_000_000 };
        let now = SyncPoint { rtc: 100 + 86_400 + 1, utc: 1_700_000_000 + 86_400 };
        assert_eq!(measure(prev, now).unwrap(), 1000);
    }

    #[test]
    fn rejects() {
        let p = SyncPoint { rtc: 1000, utc: 1_700_000_000 };
        assert_eq!(measure(p, SyncPoint { rtc: 2000, utc: 1_700_001_000 }), Err(DriftError::TooShort));
        assert_eq!(measure(p, SyncPoint { rtc: 10, utc: 1_800_000_000 }), Err(DriftError::Implausible));
        assert_eq!(
            measure(p, SyncPoint { rtc: 1000 + 86_400 * 2, utc: 1_700_000_000 + 86_400 }),
            Err(DriftError::Implausible)
        );
        assert!(!plausible(5, 1_790_000_000));
        assert!(plausible(1_800_000_000, 1_790_000_000));
        assert_eq!(uncertainty_s(30 * 86_400, 1000), 31);
    }
}
