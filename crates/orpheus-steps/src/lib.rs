//! The pedometer without hardware (WP SENS-1, 03 §15, 05 §7.7):
//!
//! * [`detector`]: steps from 3-axis accelerometer samples (magnitude,
//!   band-pass, adaptive threshold, cadence window, regularity gate);
//! * [`history`]: today's count and the last seven days, and their file
//!   `/.orpheus/steps.bin` (versioned, checksummed);
//! * [`trace`]: the CSV format of `acc trace` captures
//!   (`/.orpheus/test/acc-<n>.csv`), written by the firmware and read back
//!   by the host tests;
//! * distance and energy for the Steps app ([`distance_m`], [`kcal`]).
//!
//! Units: samples are raw counts of the MiraMEMS DA2xx at ± 2 g, 12 bits
//! right-aligned, **1024 counts per g** (docs/hw/fm-i2c-imu.md §3.3).
//!
//! Recorded traces: any `traces/*.csv` file in this crate with a
//! `# truth=<steps>` header line is replayed by `tests/traces.rs` and must
//! come within ± 5 % of the truth (zero for a trace labelled 0). Captures
//! from the player (`acc trace <secs>`) are added there with the truth the
//! person walking counted.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod detector;
pub mod history;
pub mod trace;

pub use detector::{Detector, Sensitivity};
pub use history::{Day, History};

/// Counts per g at ± 2 g (fm-i2c-imu.md §3.3).
pub const COUNTS_PER_G: i32 = 1024;

/// Metres walked: `steps × stride`.
#[must_use]
pub const fn distance_m(steps: u32, stride_cm: u16) -> u32 {
    (steps as u64 * stride_cm as u64 / 100) as u32
}

/// Energy in kcal for `metres` walked by a person of `weight_kg`:
/// distance (km) × weight (kg) × 1.036 kcal/(kg·km), the walking
/// coefficient of 05 §7.7 (an estimate, not medical data).
#[must_use]
pub const fn kcal(metres: u32, weight_kg: u16) -> u32 {
    // 1.036 kcal/(kg·km) = 1036 / 1e6 kcal per kg·m.
    ((metres as u64 * weight_kg as u64 * 1036 + 500_000) / 1_000_000) as u32
}

/// Days since 2000-01-01 of a calendar date (proleptic Gregorian, years
/// 2000–2179); the history's day key.
#[must_use]
pub const fn day_number(year: u16, month: u8, day: u8) -> u32 {
    const CUM: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let y = if year < 2000 { 0 } else { (year - 2000) as u32 };
    let m = if month < 1 {
        1
    } else if month > 12 {
        12
    } else {
        month
    };
    // Leap years before `year` (2000 is one; 2100 is not).
    let leaps = if y == 0 { 0 } else { (y - 1) / 4 + 1 - (y - 1) / 100 + (y - 1) / 400 };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mut days = y * 365 + leaps + CUM[(m - 1) as usize] as u32 + if day == 0 { 0 } else { day as u32 - 1 };
    if m > 2 && leap {
        days += 1;
    }
    days
}

/// Weekday of a day number, 0 = Monday (2000-01-01 was a Saturday).
#[must_use]
pub const fn weekday(day: u32) -> u8 {
    ((day + 5) % 7) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_and_energy() {
        // 6,482 steps of 72 cm: 4.67 km; 70 kg: 4.667 × 70 × 1.036 = 338 kcal.
        assert_eq!(distance_m(6482, 72), 4667);
        assert_eq!(kcal(4667, 70), 338);
        assert_eq!(kcal(0, 70), 0);
        assert_eq!(distance_m(u32::MAX, 150), (u64::from(u32::MAX) * 150 / 100) as u32);
    }

    #[test]
    fn day_numbers() {
        assert_eq!(day_number(2000, 1, 1), 0);
        assert_eq!(day_number(2000, 3, 1), 60, "2000 is a leap year");
        assert_eq!(day_number(2001, 1, 1), 366);
        assert_eq!(day_number(2026, 11, 24) - day_number(2026, 11, 23), 1);
        assert_eq!(day_number(2101, 3, 1) - day_number(2101, 2, 28), 1);
        assert_eq!(day_number(2100, 3, 1) - day_number(2100, 2, 28), 1, "2100 is not a leap year");
        // 2026-11-24 is a Tuesday.
        assert_eq!(weekday(day_number(2026, 11, 24)), 1);
        assert_eq!(weekday(day_number(2000, 1, 1)), 5);
        // Every day of 2000..2030 follows the one before.
        let mut prev = None;
        for y in 2000..2030u16 {
            for m in 1..=12u8 {
                let n = match m {
                    2 if y % 4 == 0 => 29,
                    2 => 28,
                    4 | 6 | 9 | 11 => 30,
                    _ => 31,
                };
                for d in 1..=n {
                    let k = day_number(y, m, d);
                    if let Some(p) = prev {
                        assert_eq!(k, p + 1, "{y}-{m}-{d}");
                    }
                    prev = Some(k);
                }
            }
        }
    }
}
