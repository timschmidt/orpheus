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

/// The operation [`Ledger::note_as`] was given no detail for.
pub const NO_DETAIL: u8 = u8::MAX;

/// Underruns by cause; `N` causes (at most 255), indices chosen by the
/// user. Each noted operation may carry a *detail* byte, also the user's
/// (the firmware: which chore or request it was), that the candidate keeps
/// so a log line can name the exact path ([`Ledger::candidate`]); the
/// counts are by cause only. 24 bytes for `N = 8` (counts saturate at
/// 65,535 per track), all zeros when new (a firmware static of it is in
/// `.bss`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ledger<const N: usize> {
    by: [u16; N],
    /// The longest operation since the last half boundary: µs, cause + 1
    /// (0: none), detail.
    worst_us: u32,
    worst_cause: u8,
    worst_detail: u8,
}

impl<const N: usize> Default for Ledger<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Ledger<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self { by: [0; N], worst_us: 0, worst_cause: 0, worst_detail: 0 }
    }

    /// An operation of `cause` ran `us` µs without feeding the DAC.
    pub fn note(&mut self, cause: usize, us: u32) {
        self.note_as(cause, NO_DETAIL, us);
    }

    /// As [`Ledger::note`], naming the operation more precisely (`detail`,
    /// the caller's code; kept with the candidate, not counted).
    pub fn note_as(&mut self, cause: usize, detail: u8, us: u32) {
        if cause < N && cause < usize::from(u8::MAX) && (self.worst_cause == 0 || us > self.worst_us) {
            self.worst_us = us;
            self.worst_cause = cause as u8 + 1;
            self.worst_detail = detail;
        }
    }

    /// The writer found `halves` halves played that it had not written:
    /// charged to the longest operation noted since the last boundary, else
    /// to `fallback`.
    pub fn missed(&mut self, halves: u32, fallback: usize) {
        let cause = self.candidate().map_or(fallback, |(c, _, _)| c);
        self.kept_up();
        let i = if cause < N { cause } else { N.saturating_sub(1) };
        if let Some(b) = self.by.get_mut(i) {
            *b = b.saturating_add(u16::try_from(halves).unwrap_or(u16::MAX));
        }
    }

    /// A half boundary with nothing missed: the noted operations did no
    /// harm. Also when the DAC starts: what ran while it was stopped had no
    /// deadline and cannot be the cause of a later underrun.
    pub fn kept_up(&mut self) {
        self.worst_cause = 0;
        self.worst_detail = 0;
        self.worst_us = 0;
    }

    /// The longest operation noted since the last half boundary (cause,
    /// µs): what kept the writer away when it finds itself behind.
    #[must_use]
    pub fn worst(&self) -> Option<(usize, u32)> {
        self.candidate().map(|(c, _, us)| (c, us))
    }

    /// [`Ledger::worst`] with its detail ([`NO_DETAIL`]: none given).
    #[must_use]
    pub fn candidate(&self) -> Option<(usize, u8, u32)> {
        let c = usize::from(self.worst_cause).checked_sub(1)?;
        Some((c, self.worst_detail, self.worst_us))
    }

    /// Start counting again (a new track).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    #[must_use]
    pub fn by(&self) -> [u32; N] {
        self.by.map(u32::from)
    }

    #[must_use]
    pub fn total(&self) -> u32 {
        self.by.iter().fold(0u32, |a, &b| a.saturating_add(u32::from(b)))
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

    /// The detail goes with the candidate it was noted with; a DAC start
    /// (`kept_up`) forgets what ran while the DAC was stopped (soak seed 1,
    /// 2026-11-25: `ub=serve:55ms` named only the cause, not the path).
    #[test]
    fn regress_the_candidate_keeps_its_detail() {
        let mut l = Ledger::<4>::new();
        assert_eq!(l.candidate(), None);
        l.note_as(LIB, 7, 30_000);
        l.note_as(FILL, 9, 20_000);
        assert_eq!(l.candidate(), Some((LIB, 7, 30_000)));
        l.note_as(FILL, 9, 55_000);
        assert_eq!(l.candidate(), Some((FILL, 9, 55_000)));
        assert_eq!(l.worst(), Some((FILL, 55_000)));
        l.missed(2, DEC);
        assert_eq!(l.candidate(), None);
        assert_eq!(l.by(), [0, 2, 0, 0]);
        l.note(JOIN, 3);
        assert_eq!(l.candidate(), Some((JOIN, NO_DETAIL, 3)));
        l.kept_up();
        l.missed(1, DEC);
        assert_eq!(l.by(), [0, 2, 0, 1]);
        // Counts saturate.
        l.missed(u32::MAX, DEC);
        assert_eq!(l.by()[DEC], 65_535);
        assert_eq!(core::mem::size_of::<Ledger<8>>(), 24);
        assert_eq!(Ledger::<8>::new(), Ledger { by: [0; 8], worst_us: 0, worst_cause: 0, worst_detail: 0 });
    }
}
