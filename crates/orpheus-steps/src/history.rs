//! Daily totals: today and the six days before it (05 §7.7, SENS-1), and
//! their file `/.orpheus/steps.bin`.
//!
//! Days are keyed by their number since 2000-01-01 ([`crate::day_number`]).
//! While the clock is not valid (U-RTC: it can stop after some resets) the
//! counter keeps counting into the current entry but no day changes, and
//! the firmware does not save (a stopped clock would put the day's steps on
//! 2000-01-01).
//!
//! File format, little-endian, [`FILE_LEN`] bytes:
//!
//! ```text
//! "OSTP" | u8 version (1) | u8 days (7) | u16 0
//!        | 7 × (u32 day number, u32 steps), newest first
//!        | u32 CRC-32 of everything before it
//! ```
//!
//! A newer version, a bad CRC or a short file reads as no history (the
//! counter starts at zero, nothing else breaks).

use orpheus_proto::hash::crc32;

/// Days kept.
pub const DAYS: usize = 7;
/// Current file version.
pub const VERSION: u8 = 1;
/// Bytes of the file.
pub const FILE_LEN: usize = 8 + DAYS * 8 + 4;
/// The file (only `/.orpheus/` is writable; CLAUDE.md rule 5).
pub const FILE: &str = "/.orpheus/steps.bin";

const MAGIC: [u8; 4] = *b"OSTP";
/// Day number of an empty entry.
const NONE: u32 = u32::MAX;

/// One day's total.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Day {
    /// Days since 2000-01-01.
    pub day: u32,
    pub steps: u32,
}

/// The last seven days, newest first (`days[0]` is the current day).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct History {
    days: [Day; DAYS],
    /// Changed since the last save.
    dirty: bool,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub const EMPTY: History = History { days: [Day { day: NONE, steps: 0 }; DAYS], dirty: false };

    #[must_use]
    pub const fn new() -> History {
        History::EMPTY
    }

    /// Make `today` the current day: older entries move down (days with no
    /// steps in between become zero entries), entries from the future (the
    /// clock was set back) are dropped.
    pub fn roll(&mut self, today: u32) {
        let cur = self.days[0].day;
        if cur == today {
            return;
        }
        if cur == NONE {
            // Nothing dated yet: steps counted before the clock was known
            // belong to today.
            self.days[0].day = today;
            self.dirty = true;
            return;
        }
        let mut out = [Day { day: NONE, steps: 0 }; DAYS];
        for (k, slot) in out.iter_mut().enumerate() {
            let want = today.checked_sub(k as u32);
            *slot = match want {
                Some(w) => {
                    let steps = self.days.iter().find(|d| d.day == w).map_or(0, |d| d.steps);
                    Day { day: w, steps }
                }
                None => Day { day: NONE, steps: 0 },
            };
        }
        self.days = out;
        self.dirty = true;
    }

    /// Add steps to the current day.
    pub fn add(&mut self, steps: u32) {
        if steps > 0 {
            self.days[0].steps = self.days[0].steps.saturating_add(steps);
            self.dirty = true;
        }
    }

    /// The current day's count.
    #[must_use]
    pub const fn today(&self) -> u32 {
        self.days[0].steps
    }

    /// The current day's number, if dated.
    #[must_use]
    pub const fn current_day(&self) -> Option<u32> {
        if self.days[0].day == NONE { None } else { Some(self.days[0].day) }
    }

    /// Steps `ago` days before the current day (0 = today); 0 for days
    /// before the history.
    #[must_use]
    pub fn steps_ago(&self, ago: usize) -> u32 {
        self.days.get(ago).map_or(0, |d| d.steps)
    }

    /// Reset today's count (Steps › Reset today).
    pub fn reset_today(&mut self) {
        self.days[0].steps = 0;
        self.dirty = true;
    }

    #[must_use]
    pub const fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Steps counted before the file was read (at mount) are added to the
    /// file's current day.
    pub fn merge_loaded(&mut self, loaded: History) {
        let extra = self.days[0].steps;
        let today = self.days[0].day;
        *self = loaded;
        if today != NONE {
            self.roll(today);
        }
        self.add(extra);
        self.dirty = extra > 0 || self.dirty;
    }

    /// The file's bytes.
    #[must_use]
    pub fn encode(&self) -> [u8; FILE_LEN] {
        let mut b = [0u8; FILE_LEN];
        b[..4].copy_from_slice(&MAGIC);
        b[4] = VERSION;
        b[5] = DAYS as u8;
        for (i, d) in self.days.iter().enumerate() {
            let o = 8 + i * 8;
            b[o..o + 4].copy_from_slice(&d.day.to_le_bytes());
            b[o + 4..o + 8].copy_from_slice(&d.steps.to_le_bytes());
        }
        let crc = crc32(&b[..FILE_LEN - 4]);
        b[FILE_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Read the file; `None` if it is not a valid version-1 file.
    #[must_use]
    pub fn decode(b: &[u8]) -> Option<History> {
        let b = b.get(..FILE_LEN)?;
        if b.get(..4)? != MAGIC || b[4] != VERSION || usize::from(b[5]) != DAYS {
            return None;
        }
        let rd =
            |o: usize| -> Option<u32> { Some(u32::from_le_bytes(*b.get(o..o + 4)?.first_chunk::<4>()?)) };
        if rd(FILE_LEN - 4)? != crc32(b.get(..FILE_LEN - 4)?) {
            return None;
        }
        let mut h = History::new();
        for (i, d) in h.days.iter_mut().enumerate() {
            *d = Day { day: rd(8 + i * 8)?, steps: rd(12 + i * 8)? };
        }
        Some(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day_number;

    #[test]
    fn day_rollover() {
        let mut h = History::new();
        h.add(120);
        assert_eq!(h.current_day(), None);
        let d = day_number(2026, 11, 23);
        h.roll(d);
        assert_eq!((h.current_day(), h.today()), (Some(d), 120), "undated steps belong to today");
        h.add(5000);
        h.roll(d);
        assert_eq!(h.today(), 5120);
        // Midnight.
        h.roll(d + 1);
        assert_eq!((h.today(), h.steps_ago(1)), (0, 5120));
        h.add(10);
        // Three days without the player: zero days in between.
        h.roll(d + 4);
        assert_eq!([h.steps_ago(0), h.steps_ago(1), h.steps_ago(2), h.steps_ago(3)], [0, 0, 0, 10]);
        assert_eq!(h.steps_ago(4), 5120);
        // A week later everything has gone.
        h.roll(d + 20);
        assert!((0..DAYS).all(|k| h.steps_ago(k) == 0));
        assert_eq!(h.steps_ago(99), 0);
    }

    #[test]
    fn clock_set_back() {
        let mut h = History::new();
        let d = day_number(2026, 11, 24);
        h.roll(d);
        h.add(300);
        h.roll(d - 1);
        assert_eq!(h.today(), 0, "the clock went back a day: a new current day");
        h.roll(d);
        assert_eq!(h.today(), 0, "the dropped future day is not restored");
    }

    #[test]
    fn file_roundtrip_and_rejection() {
        let mut h = History::new();
        h.roll(day_number(2026, 11, 20));
        h.add(7000);
        h.roll(day_number(2026, 11, 24));
        h.add(6482);
        let b = h.encode();
        assert_eq!(b.len(), FILE_LEN);
        let back = History::decode(&b).unwrap();
        assert_eq!((back.today(), back.steps_ago(4)), (6482, 7000));
        assert!(!back.dirty());
        // A flipped bit, a newer version, a short file.
        let mut bad = b;
        bad[20] ^= 1;
        assert!(History::decode(&bad).is_none());
        let mut newer = b;
        newer[4] = 2;
        assert!(History::decode(&newer).is_none());
        assert!(History::decode(&b[..FILE_LEN - 1]).is_none());
        assert!(History::decode(b"").is_none());
    }

    #[test]
    fn steps_before_the_file_was_read_are_kept() {
        let d = day_number(2026, 11, 24);
        let mut file = History::new();
        file.roll(d);
        file.add(4000);
        let mut ram = History::new();
        ram.roll(d);
        ram.add(25);
        ram.merge_loaded(History::decode(&file.encode()).unwrap());
        assert_eq!(ram.today(), 4025);
        assert!(ram.dirty());
        // Undated RAM steps go to the file's day.
        let mut ram = History::new();
        ram.add(3);
        ram.merge_loaded(file);
        assert_eq!((ram.today(), ram.current_day()), (4003, Some(d)));
    }
}
