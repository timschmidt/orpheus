//! Key decoding for the YM03 (docs/plan/03-hal-drivers.md §8, 05 §3).
//!
//! Ten keys: Power on a PMU level bit, nine on two resistor ladders read by
//! the low-rate ADC (10 bit). Nominal values [HW] (01-hardware §5):
//!
//! | Ladder 1 (LRADC1, idle ≈ 742) | Ladder 2 (LRADC6, idle ≈ 838) |
//! |---|---|
//! | Back ≈ 0, Next ≈ 125, Vol+ ≈ 245, Vol− ≈ 370 | Menu ≈ 0, Prev ≈ 140, Down ≈ 280, OK ≈ 421, Up ≈ 561 |
//!
//! Ladder 1 also carries earphone sense: with earphones in and no key the
//! value sits in 385–591 [code] (U-AUD-5), so its thresholds stop below 385.
//! One key per ladder can be seen at a time (the lower resistor wins); Power
//! combines with either.
//!
//! The debouncer reports a key set only after it has been read identically
//! twice in a row (sampling every 10 ms gives 10–20 ms latency).

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

/// The keys, named after their function in the orpheus key grammar (05 §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Key {
    Power = 0,
    Back = 1,
    Menu = 2,
    Ok = 3,
    Prev = 4,
    Next = 5,
    Up = 6,
    Down = 7,
    VolUp = 8,
    VolDown = 9,
}

impl Key {
    pub const ALL: [Key; 10] = [
        Key::Power,
        Key::Back,
        Key::Menu,
        Key::Ok,
        Key::Prev,
        Key::Next,
        Key::Up,
        Key::Down,
        Key::VolUp,
        Key::VolDown,
    ];

    #[must_use]
    pub const fn bit(self) -> u16 {
        1 << self as u8
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Key::Power => "Power",
            Key::Back => "Back",
            Key::Menu => "Menu",
            Key::Ok => "OK",
            Key::Prev => "Prev",
            Key::Next => "Next",
            Key::Up => "Up",
            Key::Down => "Down",
            Key::VolUp => "Vol+",
            Key::VolDown => "Vol-",
        }
    }

    #[must_use]
    pub fn from_index(i: u8) -> Option<Key> {
        Key::ALL.get(usize::from(i)).copied()
    }
}

/// A set of keys held at the same time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys(pub u16);

impl Keys {
    pub const NONE: Keys = Keys(0);

    #[must_use]
    pub const fn contains(self, k: Key) -> bool {
        self.0 & k.bit() != 0
    }

    #[must_use]
    pub const fn with(self, k: Key) -> Keys {
        Keys(self.0 | k.bit())
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Key> {
        Key::ALL.into_iter().filter(move |k| self.contains(*k))
    }
}

/// Upper bounds (exclusive) of each key band on a ladder, midway between
/// neighbouring nominal values.
const LADDER1: [(u16, Key); 4] = [(62, Key::Back), (185, Key::Next), (307, Key::VolUp), (380, Key::VolDown)];
const LADDER2: [(u16, Key); 5] =
    [(70, Key::Menu), (210, Key::Prev), (350, Key::Down), (491, Key::Ok), (700, Key::Up)];

/// Earphone-sense window on ladder 1 with no key pressed [code].
pub const EARPHONE_BAND: core::ops::RangeInclusive<u16> = 385..=591;

fn decode_ladder(v: u16, table: &[(u16, Key)]) -> Option<Key> {
    table.iter().find(|(hi, _)| v < *hi).map(|(_, k)| *k)
}

/// One raw sample of the key hardware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    pub ladder1: u16,
    pub ladder2: u16,
    pub power: bool,
}

/// Keys present in one sample.
#[must_use]
pub fn decode(s: Sample) -> Keys {
    let mut k = Keys::NONE;
    if let Some(x) = decode_ladder(s.ladder1 & 0x3FF, &LADDER1) {
        k = k.with(x);
    }
    if let Some(x) = decode_ladder(s.ladder2 & 0x3FF, &LADDER2) {
        k = k.with(x);
    }
    if s.power {
        k = k.with(Key::Power);
    }
    k
}

/// Earphones are plugged in (ladder 1 in the sense band, no key).
#[must_use]
pub fn earphones_present(s: Sample) -> bool {
    EARPHONE_BAND.contains(&(s.ladder1 & 0x3FF))
}

/// A change of one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub key: Key,
    pub down: bool,
}

/// Two-sample debouncer producing key edges.
#[derive(Clone, Copy, Debug, Default)]
pub struct Debouncer {
    last_raw: Keys,
    stable: Keys,
}

impl Debouncer {
    #[must_use]
    pub const fn new() -> Self {
        Self { last_raw: Keys::NONE, stable: Keys::NONE }
    }

    /// Feed one sample; returns the edges (at most ten) in key order, as a
    /// fixed array and its used length.
    pub fn feed(&mut self, raw: Keys) -> ([Edge; 10], usize) {
        let mut out = [Edge { key: Key::Power, down: false }; 10];
        let mut n = 0;
        if raw == self.last_raw && raw != self.stable {
            let changed = raw.0 ^ self.stable.0;
            for k in Key::ALL {
                if changed & k.bit() != 0 {
                    out[n] = Edge { key: k, down: raw.contains(k) };
                    n += 1;
                }
            }
            self.stable = raw;
        }
        self.last_raw = raw;
        (out, n)
    }

    #[must_use]
    pub fn held(&self) -> Keys {
        self.stable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn s(l1: u16, l2: u16) -> Sample {
        Sample { ladder1: l1, ladder2: l2, power: false }
    }

    #[test]
    fn nominal_values_decode() {
        let cases = [
            (s(742, 838), Keys::NONE),
            (s(0, 838), Keys::NONE.with(Key::Back)),
            (s(125, 838), Keys::NONE.with(Key::Next)),
            (s(245, 838), Keys::NONE.with(Key::VolUp)),
            (s(370, 838), Keys::NONE.with(Key::VolDown)),
            (s(742, 0), Keys::NONE.with(Key::Menu)),
            (s(742, 140), Keys::NONE.with(Key::Prev)),
            (s(742, 280), Keys::NONE.with(Key::Down)),
            (s(742, 421), Keys::NONE.with(Key::Ok)),
            (s(742, 561), Keys::NONE.with(Key::Up)),
        ];
        for (sample, want) in cases {
            assert_eq!(decode(sample), want, "{sample:?}");
        }
    }

    #[test]
    fn tolerance_of_15_percent_of_spacing() {
        // Nominal ± 15 counts still decodes to the same key.
        for (v, k) in [(125u16, Key::Next), (245, Key::VolUp), (370, Key::VolDown)] {
            assert!(decode(s(v - 15, 838)).contains(k));
            assert!(decode(s(v + 9, 838)).contains(k), "{v}");
        }
        for (v, k) in [(140u16, Key::Prev), (280, Key::Down), (421, Key::Ok), (561, Key::Up)] {
            assert!(decode(s(742, v - 15)).contains(k));
            assert!(decode(s(742, v + 15)).contains(k));
        }
    }

    #[test]
    fn earphone_band_is_not_a_key() {
        for v in [385u16, 450, 591] {
            assert!(decode(s(v, 838)).is_empty());
            assert!(earphones_present(s(v, 838)));
        }
        assert!(!earphones_present(s(742, 838)));
    }

    #[test]
    fn power_combines() {
        let k = decode(Sample { ladder1: 742, ladder2: 421, power: true });
        assert_eq!(k.iter().collect::<Vec<_>>(), [Key::Power, Key::Ok]);
    }

    #[test]
    fn debounce_trace() {
        let mut d = Debouncer::new();
        let ok = Keys::NONE.with(Key::Ok);
        // A one-sample glitch is ignored.
        assert_eq!(d.feed(ok).1, 0);
        assert_eq!(d.feed(Keys::NONE).1, 0);
        // Two identical samples make an edge.
        assert_eq!(d.feed(ok).1, 0);
        let (e, n) = d.feed(ok);
        assert_eq!(&e[..n], &[Edge { key: Key::Ok, down: true }]);
        assert_eq!(d.held(), ok);
        // Release.
        assert_eq!(d.feed(Keys::NONE).1, 0);
        let (e, n) = d.feed(Keys::NONE);
        assert_eq!(&e[..n], &[Edge { key: Key::Ok, down: false }]);
    }

    #[test]
    fn roll_over_between_ladders() {
        let mut d = Debouncer::new();
        let a = Keys::NONE.with(Key::Up);
        let b = Keys::NONE.with(Key::Up).with(Key::VolUp);
        d.feed(a);
        d.feed(a);
        d.feed(b);
        let (e, n) = d.feed(b);
        assert_eq!(&e[..n], &[Edge { key: Key::VolUp, down: true }]);
    }
}
