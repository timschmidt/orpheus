//! Who made the DAC run dry: underruns charged to a cause (`status`).
//!
//! The PCM writer notices an underrun only at its next half boundary
//! ([`Ledger::missed`]), after whatever kept it from writing has finished.
//! Every operation that runs between two writes and does not feed the DAC
//! (a card refill, a served request, a decoder re-open, …) is noted with
//! its length ([`Ledger::note`]). Halves found missed are charged to the
//! longest operation noted since the last boundary, or to the fallback
//! cause (the decoder) if none was. A boundary with nothing missed forgets
//! the noted operations ([`Ledger::kept_up`]). So the causes add up to the
//! underruns exactly, by construction.
//!
//! No panic paths (used in the firmware's audio task).

/// Underruns by cause; `N` causes, indices chosen by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ledger<const N: usize> {
    by: [u32; N],
    /// The longest operation since the last half boundary: cause, µs.
    worst: Option<(u8, u32)>,
}

impl<const N: usize> Default for Ledger<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Ledger<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self { by: [0; N], worst: None }
    }

    /// An operation of `cause` ran `us` µs without feeding the DAC.
    pub fn note(&mut self, cause: usize, us: u32) {
        if cause < N && self.worst.is_none_or(|(_, w)| us > w) {
            self.worst = Some((cause as u8, us));
        }
    }

    /// The writer found `halves` halves played that it had not written:
    /// charged to the longest operation noted since the last boundary, else
    /// to `fallback`.
    pub fn missed(&mut self, halves: u32, fallback: usize) {
        let cause = self.worst.take().map_or(fallback, |(c, _)| usize::from(c));
        let i = if cause < N { cause } else { N.saturating_sub(1) };
        if let Some(b) = self.by.get_mut(i) {
            *b = b.saturating_add(halves);
        }
    }

    /// A half boundary with nothing missed: the noted operations did no harm.
    pub fn kept_up(&mut self) {
        self.worst = None;
    }

    /// Start counting again (a new track).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    #[must_use]
    pub fn by(&self) -> [u32; N] {
        self.by
    }

    #[must_use]
    pub fn total(&self) -> u32 {
        self.by.iter().fold(0u32, |a, &b| a.saturating_add(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOIN: usize = 0;
    const FILL: usize = 1;
    const LIB: usize = 2;
    const DEC: usize = 3;

    /// A writer with a 2-half ring against a DAC, in 1-ms ticks: operations
    /// block the writer for their length; the writer's half boundaries
    /// check like `PcmOut::push`. The ledger must account for every missed
    /// half and name the operation that caused it.
    ///
    /// Regression: on the player the `ub:` per-cause counters did not add
    /// up to `underruns` (AU-5, 2026-11-24, gapless FLAC run).
    #[test]
    fn every_underrun_is_charged_once_to_its_cause() {
        // (time ms, cause, length ms) of blocking operations; the DAC plays
        // one half per 35 ms; the writer needs 10 ms per half.
        let ops = [(100, FILL, 20), (300, LIB, 120), (600, JOIN, 150), (900, FILL, 30), (1200, LIB, 200)];
        let mut l = Ledger::<4>::new();
        let (mut written, mut played) = (2u32, 0u32);
        let mut underruns = 0u32;
        let mut t = 0u32;
        let mut busy_until = 0u32;
        let mut next_dac = 35u32;
        let mut fill_ms = 0u32;
        let mut by_hand = [0u32; 4];
        let mut last_op: Option<usize> = None;
        while t < 2000 {
            if t >= next_dac {
                played += 1;
                next_dac += 35;
            }
            if let Some(&(_, c, len)) = ops.iter().find(|o| o.0 == t) {
                busy_until = t + len;
                l.note(c, len * 1000);
                last_op = Some(c);
            }
            if t >= busy_until && (written as i32 - played as i32) < 2 {
                fill_ms += 1;
                if fill_ms == 10 {
                    fill_ms = 0;
                    written += 1;
                    if played > written {
                        let lost = played - written;
                        underruns += lost;
                        by_hand[last_op.unwrap_or(DEC)] += lost;
                        l.missed(lost, DEC);
                        written = played + 1;
                    } else {
                        l.kept_up();
                    }
                    last_op = None;
                }
            }
            t += 1;
        }
        assert!(underruns > 0);
        assert_eq!(l.total(), underruns);
        assert_eq!(l.by(), by_hand);
        // The 20-ms refill fits the ring; the 120-ms and 200-ms lookups and
        // the 150-ms join do not.
        assert_eq!(l.by()[FILL], 0, "{:?}", l.by());
        assert!(l.by()[LIB] >= 2 && l.by()[JOIN] >= 1, "{:?}", l.by());
        assert_eq!(l.by()[DEC], 0, "{:?}", l.by());
    }

    #[test]
    fn the_longest_operation_is_charged_and_boundaries_forget() {
        let mut l = Ledger::<4>::new();
        l.note(FILL, 5_000);
        l.note(LIB, 90_000);
        l.note(FILL, 7_000);
        l.missed(2, DEC);
        l.note(LIB, 90_000);
        l.kept_up();
        l.missed(1, DEC);
        l.note(JOIN, 1);
        l.missed(3, DEC);
        l.note(9, 50);
        l.missed(1, DEC);
        assert_eq!(l.by(), [3, 0, 2, 2]);
        assert_eq!(l.total(), 7);
        l.reset();
        assert_eq!(l.total(), 0);
    }
}
