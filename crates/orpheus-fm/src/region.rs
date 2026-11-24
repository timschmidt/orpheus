//! Regions (05 §7.3: Europe, United States, World, Japan, Wide), the
//! tuner's hardware bands and the channel arithmetic.
//!
//! The tuner counts channels from a band base in steps of the hardware
//! spacing (register 03h `CHAN`, `BAND`, `SPACE`). Every region tunes with
//! 100 kHz hardware spacing except World (50 kHz); the region's own step
//! (what Prev/Next move by) is a multiple of it. The United States step is
//! 200 kHz on odd tenths (87.9 … 107.9), which 100 kHz spacing reaches.
//!
//! Wide (65–108 MHz) needs two hardware bands: `BAND = 11` covers 65–76 MHz
//! (register 07h bit 9 is set in every vendor init table, which selects
//! 65 MHz rather than 50 MHz as the base) and `BAND = 10` 76–108 MHz. The
//! plan's "64–108" is cut to the 65 MHz the chip can tune.
//!
//! De-emphasis follows the region (03 §15): 75 µs in the United States,
//! 50 µs elsewhere (the stock firmware leaves 75 µs everywhere).

/// A hardware band: register 03h bits 3:2 and its channel range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HwBand {
    /// The `BAND` field value (0..=3).
    pub bits: u16,
    pub base_khz: u32,
    pub top_khz: u32,
}

/// `BAND = 00`: 87–108 MHz.
pub const BAND_87: HwBand = HwBand { bits: 0, base_khz: 87_000, top_khz: 108_000 };
/// `BAND = 10`: 76–108 MHz ("world wide").
pub const BAND_76: HwBand = HwBand { bits: 2, base_khz: 76_000, top_khz: 108_000 };
/// `BAND = 11` with 07h bit 9 set: 65–76 MHz.
pub const BAND_65: HwBand = HwBand { bits: 3, base_khz: 65_000, top_khz: 76_000 };

/// The fourth harmonic of the tuner's 24 MHz reference lands at 96.0 MHz:
/// the stock firmware never stops a seek or scan at 95.9–96.1 MHz
/// (fm-i2c-imu.md §2.4).
#[must_use]
pub const fn is_spur(khz: u32) -> bool {
    khz >= 95_900 && khz <= 96_100
}

/// The Region setting (Settings › Radio, store key `0x0701`), in the
/// setting's choice order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Region {
    #[default]
    Europe,
    UnitedStates,
    World,
    Japan,
    Wide,
}

impl Region {
    pub const ALL: [Region; 5] =
        [Region::Europe, Region::UnitedStates, Region::World, Region::Japan, Region::Wide];

    /// From the setting's value (anything unknown is Europe).
    #[must_use]
    pub fn from_index(i: i16) -> Region {
        usize::try_from(i).ok().and_then(|i| Region::ALL.get(i).copied()).unwrap_or_default()
    }

    #[must_use]
    pub const fn low_khz(self) -> u32 {
        match self {
            Region::Europe | Region::World => 87_500,
            Region::UnitedStates => 87_900,
            Region::Japan => 76_000,
            Region::Wide => 65_000,
        }
    }

    #[must_use]
    pub const fn high_khz(self) -> u32 {
        match self {
            Region::UnitedStates => 107_900,
            Region::Japan => 95_000,
            Region::Europe | Region::World | Region::Wide => 108_000,
        }
    }

    /// What Prev/Next move by.
    #[must_use]
    pub const fn step_khz(self) -> u32 {
        match self {
            Region::UnitedStates => 200,
            Region::World => 50,
            Region::Europe | Region::Japan | Region::Wide => 100,
        }
    }

    /// The tuner's channel spacing.
    #[must_use]
    pub const fn spacing_khz(self) -> u32 {
        if matches!(self, Region::World) { 50 } else { 100 }
    }

    /// Register 03h `SPACE` (bits 1:0): 00 = 100 kHz, 10 = 50 kHz.
    #[must_use]
    pub const fn space_bits(self) -> u16 {
        if matches!(self, Region::World) { 2 } else { 0 }
    }

    /// 50 µs de-emphasis (register 04h `DE` = 1); 75 µs in the Americas.
    #[must_use]
    pub const fn deemphasis_50us(self) -> bool {
        !matches!(self, Region::UnitedStates)
    }

    /// The hardware band `khz` is tuned in.
    #[must_use]
    pub const fn band(self, khz: u32) -> HwBand {
        match self {
            Region::Wide if khz < 76_000 => BAND_65,
            Region::Wide | Region::Japan => BAND_76,
            Region::Europe | Region::UnitedStates | Region::World => BAND_87,
        }
    }

    #[must_use]
    pub const fn contains(self, khz: u32) -> bool {
        khz >= self.low_khz() && khz <= self.high_khz()
    }

    /// The nearest channel of this region (inside its limits, on its step).
    #[must_use]
    pub const fn snap(self, khz: u32) -> u32 {
        let lo = self.low_khz();
        let hi = self.high_khz();
        let k = if khz < lo {
            lo
        } else if khz > hi {
            hi
        } else {
            khz
        };
        let step = self.step_khz();
        let n = (k - lo + step / 2) / step;
        let v = lo + n * step;
        if v > hi { v - step } else { v }
    }

    /// One step up (`up`) or down from `khz`, wrapping at the limits.
    #[must_use]
    pub const fn step(self, khz: u32, up: bool) -> u32 {
        let k = self.snap(khz);
        if up {
            if k + self.step_khz() > self.high_khz() { self.low_khz() } else { k + self.step_khz() }
        } else if k < self.low_khz() + self.step_khz() {
            self.snap(self.high_khz())
        } else {
            k - self.step_khz()
        }
    }

    /// The tuner's channel number of `khz` (in its hardware band).
    #[must_use]
    pub const fn channel(self, khz: u32) -> u16 {
        let b = self.band(khz);
        let k = if khz < b.base_khz { b.base_khz } else { khz };
        let c = (k - b.base_khz) / self.spacing_khz();
        if c > 0x3FF { 0x3FF } else { c as u16 }
    }

    /// The frequency of channel `chan` in band `band`.
    #[must_use]
    pub const fn khz_of(self, band: HwBand, chan: u16) -> u32 {
        band.base_khz + chan as u32 * self.spacing_khz()
    }

    /// Hardware seeks stop at the band's edge; this is where the next one
    /// starts: the other band of Wide, else the far end (wrap).
    #[must_use]
    pub const fn segment_after(self, khz: u32, up: bool) -> u32 {
        match (self, up) {
            (Region::Wide, true) if khz < 76_000 => 76_000,
            (Region::Wide, false) if khz >= 76_000 => 75_900,
            (_, true) => self.low_khz(),
            (_, false) => self.snap(self.high_khz()),
        }
    }

    /// Band segments a full seek passes through (Wide has two).
    #[must_use]
    pub const fn segments(self) -> u8 {
        if matches!(self, Region::Wide) { 2 } else { 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_steps_and_wrap() {
        let r = Region::Europe;
        assert_eq!(r.snap(87_000), 87_500);
        assert_eq!(r.snap(101_930), 101_900);
        assert_eq!(r.snap(120_000), 108_000);
        assert_eq!(r.step(108_000, true), 87_500, "wraps up");
        assert_eq!(r.step(87_500, false), 108_000, "wraps down");
        assert_eq!(r.step(101_900, true), 102_000);
        let us = Region::UnitedStates;
        assert_eq!(us.snap(88_000), 88_100, "US channels are odd tenths");
        assert_eq!(us.snap(87_990), 87_900);
        assert_eq!(us.snap(88_200), 88_300);
        assert_eq!(us.step(107_900, true), 87_900);
        assert_eq!(us.step(87_900, false), 107_900);
        assert_eq!(Region::World.step(87_500, true), 87_550);
        let jp = Region::Japan;
        assert_eq!(jp.snap(100_000), 95_000);
        assert_eq!(jp.step(95_000, true), 76_000);
        assert!(!jp.contains(98_000));
        assert_eq!(Region::Wide.step(108_000, true), 65_000);
    }

    #[test]
    fn channels_round_trip_in_every_band() {
        for r in Region::ALL {
            let mut k = r.low_khz();
            while k <= r.high_khz() {
                let b = r.band(k);
                let c = r.channel(k);
                assert_eq!(r.khz_of(b, c), k, "{r:?} {k}");
                assert!(k >= b.base_khz && k <= b.top_khz);
                k += r.step_khz();
            }
        }
        assert_eq!(Region::Europe.channel(87_000), 0);
        assert_eq!(Region::Europe.channel(98_000), 110, "the vendor table's 03h = 0x1B90 is 98.0 MHz");
        assert_eq!(Region::World.channel(87_550), 11);
        assert_eq!(Region::Wide.band(75_900), BAND_65);
        assert_eq!(Region::Wide.band(76_000), BAND_76);
    }

    #[test]
    fn de_emphasis_and_segments() {
        assert!(!Region::UnitedStates.deemphasis_50us());
        assert!(Region::Europe.deemphasis_50us() && Region::Japan.deemphasis_50us());
        assert_eq!(Region::Wide.segment_after(70_000, true), 76_000);
        assert_eq!(Region::Wide.segment_after(90_000, true), 65_000);
        assert_eq!(Region::Wide.segment_after(90_000, false), 75_900);
        assert_eq!(Region::Wide.segment_after(70_000, false), 108_000);
        assert_eq!(Region::UnitedStates.segment_after(100_000, false), 107_900);
        assert_eq!(Region::from_index(3), Region::Japan);
        assert_eq!(Region::from_index(9), Region::Europe);
        assert!(is_spur(96_000) && !is_spur(96_200));
    }
}
