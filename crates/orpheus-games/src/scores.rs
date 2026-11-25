//! Best results and statistics per game and per variant (05 §7.13: "per
//! game and per setting combination"), kept in `/.orpheus/games/scores.bin`.
//!
//! A variant is a game's number for the rule settings that change what a
//! score means (Blocks: mode, randomiser, hold, kicks; 4096: board, goal,
//! odds, undo). The table holds [`CAPACITY`] records; when it is full, the
//! least played record makes room. Player names (05 §7.13) are not stored
//! yet.

use crate::save::{self, Reader, Writer};

/// Records kept.
pub const CAPACITY: usize = 24;
/// The save envelope's kind byte of the score table.
pub const KIND: u8 = 0xF0;
pub const VERSION: u8 = 1;
/// Largest encoded table.
pub const MAX_BYTES: usize = save::OVERHEAD + 1 + CAPACITY * 17;

/// One game and variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Record {
    pub game: u8,
    pub variant: u16,
    /// The best result: a score, or (lower is better) a time in ms; 0 =
    /// none yet.
    pub best: u32,
    /// What came with the best result (lines, the largest tile …).
    pub best_detail: u32,
    pub played: u32,
    /// Games won (goal reached, 40 lines done, time played out).
    pub won: u16,
}

/// The result of one finished game.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Outcome {
    pub result: u32,
    pub detail: u32,
    /// The result is a time: lower is better.
    pub lower_is_better: bool,
    /// The result may be a best (a 40-lines game that topped out has no
    /// time).
    pub ranked: bool,
    pub won: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Scores {
    recs: [Record; CAPACITY],
    n: u8,
}

impl Default for Scores {
    fn default() -> Self {
        Scores { recs: [Record::default(); CAPACITY], n: 0 }
    }
}

impl Scores {
    #[must_use]
    pub fn records(&self) -> &[Record] {
        &self.recs[..usize::from(self.n)]
    }

    #[must_use]
    pub fn get(&self, game: u8, variant: u16) -> Option<&Record> {
        self.records().iter().find(|r| r.game == game && r.variant == variant)
    }

    /// Count a finished game; returns true if its result is a new best.
    pub fn record(&mut self, game: u8, variant: u16, o: Outcome) -> bool {
        let i = match self.records().iter().position(|r| r.game == game && r.variant == variant) {
            Some(i) => i,
            None => {
                let i = if usize::from(self.n) < CAPACITY {
                    self.n += 1;
                    usize::from(self.n) - 1
                } else {
                    // The least played record (the first of equals).
                    let mut j = 0;
                    for (k, r) in self.recs.iter().enumerate() {
                        if r.played < self.recs[j].played {
                            j = k;
                        }
                    }
                    j
                };
                self.recs[i] = Record { game, variant, ..Record::default() };
                i
            }
        };
        let r = &mut self.recs[i];
        r.played = r.played.saturating_add(1);
        if o.won {
            r.won = r.won.saturating_add(1);
        }
        let better = o.ranked
            && o.result > 0
            && (r.best == 0 || if o.lower_is_better { o.result < r.best } else { o.result > r.best });
        if better {
            r.best = o.result;
            r.best_detail = o.detail;
        }
        better
    }

    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        save::seal(buf, KIND, VERSION, |w: &mut Writer<'_>| {
            w.u8(self.n);
            for r in self.records() {
                w.u8(r.game);
                w.u16(r.variant);
                w.u32(r.best);
                w.u32(r.best_detail);
                w.u32(r.played);
                w.u16(r.won);
            }
        })
    }

    /// A table back; `None` if damaged or of another version.
    #[must_use]
    pub fn decode(data: &[u8]) -> Option<Scores> {
        let (kind, version, payload) = save::open(data)?;
        if (kind, version) != (KIND, VERSION) {
            return None;
        }
        let mut r = Reader::new(payload);
        let n = r.u8()?;
        if usize::from(n) > CAPACITY {
            return None;
        }
        let mut s = Scores { n, ..Scores::default() };
        for rec in s.recs.iter_mut().take(usize::from(n)) {
            *rec = Record {
                game: r.u8()?,
                variant: r.u16()?,
                best: r.u32()?,
                best_detail: r.u32()?,
                played: r.u32()?,
                won: r.u16()?,
            };
        }
        r.done().then_some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(v: u32) -> Outcome {
        Outcome { result: v, detail: v / 10, ranked: true, ..Outcome::default() }
    }

    #[test]
    fn best_and_counts() {
        let mut s = Scores::default();
        assert!(s.record(1, 0, score(500)));
        assert!(!s.record(1, 0, score(400)));
        assert!(s.record(1, 0, score(900)));
        assert!(s.record(1, 1, score(10)), "another variant");
        let r = s.get(1, 0).unwrap();
        assert_eq!((r.best, r.best_detail, r.played), (900, 90, 3));
        // Times: lower is better; an unranked game only counts.
        let t = |ms, ranked| Outcome { result: ms, lower_is_better: true, ranked, won: ranked, detail: 40 };
        assert!(s.record(1, 2, t(90_000, true)));
        assert!(!s.record(1, 2, t(80_000, false)));
        assert!(s.record(1, 2, t(70_000, true)));
        let r = s.get(1, 2).unwrap();
        assert_eq!((r.best, r.played, r.won), (70_000, 3, 2));
        assert!(!s.record(2, 0, score(0)), "a zero result is no best");
    }

    #[test]
    fn full_table_drops_the_least_played() {
        let mut s = Scores::default();
        for v in 0..CAPACITY as u16 {
            s.record(1, v, score(1));
            if v != 5 {
                s.record(1, v, score(1));
            }
        }
        s.record(2, 0, score(7));
        assert!(s.get(1, 5).is_none());
        assert_eq!(s.get(2, 0).map(|r| r.best), Some(7));
        assert_eq!(s.records().len(), CAPACITY);
    }

    #[test]
    fn round_trip() {
        let mut s = Scores::default();
        for v in 0..CAPACITY as u16 {
            s.record(1 + (v % 2) as u8, v, score(u32::from(v) * 100 + 1));
        }
        let mut buf = [0u8; MAX_BYTES];
        let n = s.encode(&mut buf).unwrap();
        assert_eq!(n, MAX_BYTES);
        assert_eq!(Scores::decode(&buf[..n]), Some(s));
        let e = Scores::default().encode(&mut buf).unwrap();
        assert_eq!(Scores::decode(&buf[..e]), Some(Scores::default()));
        assert_eq!(Scores::decode(&buf[..e - 1]), None);
    }
}
