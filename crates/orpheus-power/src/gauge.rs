//! Battery gauge from the raw ADC (02-architecture §5.5).
//!
//! Scale [HW, approximate]: mV ≈ raw × 4200 / 900 (the port's figure; the
//! vendor's eFuse trim is not applied: reading it needs a PMU write).
//! Percentage from a typical Li-ion open-circuit curve, smoothed; while
//! charging the reading is high by the charge-current IR drop, so the UI
//! shows "charging" instead of a percentage jump.

/// Millivolts from a raw 10-bit reading.
#[must_use]
pub const fn raw_to_mv(raw: u16) -> u32 {
    (raw as u32 * 4200) / 900
}

/// (mV, percent) points of a typical 1-cell Li-ion discharge curve at low
/// load.
const CURVE: [(u32, u8); 11] = [
    (3300, 0),
    (3500, 3),
    (3600, 8),
    (3680, 15),
    (3740, 25),
    (3790, 40),
    (3840, 52),
    (3900, 63),
    (3980, 75),
    (4080, 88),
    (4170, 100),
];

/// State of charge in percent for an open-circuit-ish voltage.
#[must_use]
pub fn percent(mv: u32) -> u8 {
    if mv <= CURVE[0].0 {
        return 0;
    }
    for w in CURVE.windows(2) {
        let (a, b) = (w[0], w[1]);
        if mv <= b.0 {
            let span = b.0 - a.0;
            let frac = (mv - a.0) * u32::from(b.1 - a.1) / span;
            return a.1 + frac as u8;
        }
    }
    100
}

/// Exponential smoothing of raw readings (1/8 per sample), with a fast
/// start.
#[derive(Clone, Copy, Debug, Default)]
pub struct Smoother {
    acc: u32,
    n: u8,
}

impl Smoother {
    pub fn feed(&mut self, raw: u16) -> u16 {
        let v = u32::from(raw) << 3;
        if self.n < 4 {
            self.n += 1;
            self.acc = if self.n == 1 { v } else { (self.acc + v) / 2 };
        } else {
            self.acc = self.acc - (self.acc >> 3) + (v >> 3);
        }
        (self.acc >> 3) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_and_curve() {
        assert_eq!(raw_to_mv(900), 4200);
        assert_eq!(raw_to_mv(801), 3738);
        assert_eq!(percent(3000), 0);
        assert_eq!(percent(4200), 100);
        assert!(percent(3738) > 20 && percent(3738) < 30);
        let mut last = 0;
        for mv in (3300..4200).step_by(10) {
            let p = percent(mv);
            assert!(p >= last);
            last = p;
        }
    }

    #[test]
    fn smoother_converges() {
        let mut s = Smoother::default();
        assert_eq!(s.feed(800), 800);
        for _ in 0..100 {
            s.feed(820);
        }
        assert!((819..=820).contains(&s.feed(820)));
    }
}
