//! The backlight's PWM4 duty (U-PWM-3, 01-hardware §9): the stock
//! firmware's twelve levels and nothing else.
//!
//! The stock firmware maps a level 0…11 to `DUTY` 2, 3, 4, 5, 6, 7, 8, 9,
//! 10, 11, 13, 15 of `DUTYMAX` 16 (a 12-byte table in every AP with the
//! common UI code); the user range is 1–11, the default 6 (duty 8, 50 %)
//! and the dim state after the backlight timer is level 0 (duty 2). It
//! never drives 1, 12, 14 or 16. [`Duty`] can hold only a value of that
//! table, so the backlight driver (`yoton_bsp::display::Backlight`) can
//! never be asked for another.

/// `DUTYMAX` of PWM4: the period in duty steps (`CMU_PWM4CLK` 0x20F:
/// CK24M ÷ 16, 93.75 kHz).
pub const DUTY_MAX: u16 = 16;

/// The stock level → duty table (U-PWM-3), level 0 … 11.
pub const LEVEL_DUTY: [u8; 12] = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15];

/// The lowest level Settings › Display › Brightness offers (stock
/// `SETTING_AP_ID_LIGHTNESS = 6[1~11,1]`).
pub const MIN_LEVEL: i16 = 1;
/// The highest level (duty 15, the most the stock firmware drives).
pub const MAX_LEVEL: i16 = 11;
/// The stock default level (duty 8).
pub const DEFAULT_LEVEL: i16 = 6;

/// A PWM4 duty the stock firmware uses: always one of [`LEVEL_DUTY`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Duty(u8);

impl Duty {
    /// The stock default (level 6, duty 8).
    pub const DEFAULT: Duty = Duty::of_level(DEFAULT_LEVEL);
    /// The stock dim state (level 0, duty 2).
    pub const DIM: Duty = Duty::of_level(0);
    /// The most the stock firmware drives (level 11, duty 15).
    pub const MAX: Duty = Duty::of_level(MAX_LEVEL);

    /// The duty of stock level `level`; a level outside 0…11 is clamped
    /// to the nearer end (never a duty outside the table, never off).
    #[must_use]
    pub const fn of_level(level: i16) -> Duty {
        let i = if level < 0 {
            0
        } else if level > 11 {
            11
        } else {
            level as usize
        };
        Duty(LEVEL_DUTY[i])
    }

    /// The `DUTY[15:0]` value (of [`DUTY_MAX`]).
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// The alert flash (05 §7.12, UI-5d): bright and dim every 500 ms. The
/// bright phase is the stock maximum, 15: the stock firmware never drives
/// 16 (100 %), and Orpheus does not either until a user check says it is
/// safe (U-PWM-3).
pub const FLASH_BRIGHT: Duty = Duty::MAX;
/// The flash's dim phase (level 1, duty 3).
pub const FLASH_DIM: Duty = Duty::of_level(1);

/// What the backlight shows: `None` off (panel dark: the pad driven low),
/// else the duty. `level` is Settings › Display › Brightness; `lit`
/// whether the panel is lit (screen on, or the screen saver on it);
/// `flash` the alert's phase while one rings (`Some(true)` bright).
#[must_use]
pub const fn light(level: i16, lit: bool, flash: Option<bool>) -> Option<Duty> {
    if !lit {
        return None;
    }
    Some(match flash {
        Some(true) => FLASH_BRIGHT,
        Some(false) => FLASH_DIM,
        None => Duty::of_level(level),
    })
}

/// The alert flash's phase at `now_ms` (bright in the first 500 ms of
/// every second).
#[must_use]
pub const fn flash_phase(now_ms: u64) -> bool {
    (now_ms / 500) % 2 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every possible stored or computed level gives a duty of the stock
    /// table (U-PWM-3): none outside it, ever.
    #[test]
    fn every_level_maps_into_the_stock_table() {
        for level in i16::MIN..=i16::MAX {
            let d = Duty::of_level(level).get();
            assert!(LEVEL_DUTY.contains(&d), "level {level} → duty {d}");
            assert!(u16::from(d) < DUTY_MAX, "level {level}: below DUTYMAX");
            for lit in [false, true] {
                for flash in [None, Some(false), Some(true)] {
                    if let Some(d) = light(level, lit, flash) {
                        assert!(LEVEL_DUTY.contains(&d.get()), "level {level} {lit} {flash:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn stock_levels_and_ends() {
        for (l, d) in LEVEL_DUTY.iter().enumerate() {
            assert_eq!(Duty::of_level(l as i16).get(), *d);
        }
        assert_eq!(Duty::DEFAULT.get(), 8, "stock default level 6");
        assert_eq!(Duty::DIM.get(), 2);
        assert_eq!(Duty::MAX.get(), 15);
        assert_eq!(Duty::of_level(-1), Duty::DIM, "below: the lowest");
        assert_eq!(Duty::of_level(12), Duty::MAX, "above: the highest, never 0 or 16");
        // The user range is 1–11.
        assert_eq!(Duty::of_level(MIN_LEVEL).get(), 3);
        assert_eq!(Duty::of_level(MAX_LEVEL).get(), 15);
    }

    #[test]
    fn light_follows_the_panel_and_the_alert() {
        assert_eq!(light(6, false, None), None, "dark panel: off");
        assert_eq!(light(6, false, Some(true)), None, "an alert on a dark panel: off");
        assert_eq!(light(6, true, None), Some(Duty::DEFAULT));
        assert_eq!(light(1, true, Some(true)), Some(FLASH_BRIGHT), "the flash ignores the level");
        assert_eq!(light(11, true, Some(false)), Some(FLASH_DIM));
        assert!(flash_phase(0) && flash_phase(499) && !flash_phase(500) && flash_phase(1000));
    }

    /// Bug (2026-11-26, U-PWM-3): the alert flash drove PWM4 duty 16
    /// (100 %) in its bright phase — a value the stock firmware never
    /// writes (its maximum is 15), so the backlight ran outside anything
    /// the board is known to take. The flash now uses the stock maximum.
    #[test]
    fn regress_alert_flash_never_drives_16() {
        for t in (0..4000).step_by(50) {
            let d = light(MAX_LEVEL, true, Some(flash_phase(t))).unwrap().get();
            assert!(d <= 15 && LEVEL_DUTY.contains(&d), "{t} ms: duty {d}");
        }
        assert_eq!(FLASH_BRIGHT.get(), 15);
        assert_eq!(FLASH_DIM.get(), 3, "the flash's dim phase as before");
    }
}
