//! Radio volume: the tuner's own volume (register 05h bits 3:0, 0–15)
//! together with the headphone amplifier gain (`PA_CTL[14:12]`, 0–7), as
//! the stock radio engine pairs them (`FMENGINE.AP` file 0x2D04,
//! fm-i2c-imu.md §2.6, audio.md §4). The FM audio bypasses the DAC, so
//! the DAC's digital volume does not reach it.

/// The vendor's 32 steps: (tuner volume, amplifier gain). VERBATIM
/// (`FMENGINE.AP` 0x110D04); the steps are not monotonic in places (19
/// and 24), as in the stock firmware.
pub const PAIRS: [(u8, u8); 32] = [
    (0, 0),
    (1, 1),
    (1, 4),
    (1, 6),
    (1, 7),
    (2, 6),
    (3, 5),
    (4, 4),
    (4, 5),
    (4, 6),
    (4, 7),
    (5, 6),
    (5, 7),
    (6, 6),
    (6, 7),
    (7, 6),
    (7, 7),
    (8, 6),
    (8, 7),
    (11, 5),
    (9, 7),
    (11, 6),
    (11, 7),
    (12, 6),
    (14, 5),
    (12, 7),
    (13, 6),
    (13, 7),
    (14, 6),
    (14, 7),
    (15, 6),
    (15, 7),
];

/// The amplifier gain music plays at (the vendor's enable wrapper sets 7).
pub const PA_MUSIC: u8 = 7;

/// The player's volume (0–30, the UI's scale) as (tuner volume, amplifier
/// gain): 0 is silence (the tuner is muted too), 30 the vendor's top step.
#[must_use]
pub fn for_ui(v: u8) -> (u8, u8) {
    let v = u32::from(v.min(30));
    let i = usize::try_from((v * 31 + 15) / 30).unwrap_or(31);
    PAIRS.get(i).copied().unwrap_or((15, 7))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_scale_covers_the_table() {
        assert_eq!(for_ui(0), (0, 0));
        assert_eq!(for_ui(30), (15, 7));
        assert_eq!(for_ui(200), (15, 7));
        assert_eq!(for_ui(1), PAIRS[1]);
        assert_eq!(for_ui(15), PAIRS[16]);
        // Every value is within the registers' ranges.
        for (t, p) in PAIRS {
            assert!(t <= 15 && p <= 7);
        }
        // Loudness never drops by more than one table step as the UI
        // volume rises (tuner volume × gain as a rough product).
        let mut last = 0u32;
        for v in 0..=30 {
            let (t, p) = for_ui(v);
            let level = u32::from(t) * u32::from(p);
            assert!(level + 30 >= last, "step {v}");
            last = level;
        }
    }
}
