//! An index's letter-run table copied into RAM, so that a UI can answer
//! `letter(i)` and letter jumps synchronously (the card is with another
//! task). Four bytes per run; a listing whose table does not fit is left
//! to the caller's fallback.

use crate::{Error, Index, Storage};

/// Up to `N` letter runs of one [`Index`] (`4 × N + 8` bytes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LetterTable<const N: usize> {
    rows: u32,
    n: u16,
    /// `(first row, letter as a BMP code point, 0 = none)`.
    runs: [(u16, u16); N],
}

impl<const N: usize> Default for LetterTable<N> {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl<const N: usize> LetterTable<N> {
    /// No table.
    pub const EMPTY: Self = LetterTable { rows: 0, n: 0, runs: [(0, 0); N] };

    /// Copy the runs of `ix`. `Ok(false)` (and the table left empty) if
    /// they do not fit: more than `N` runs, 65,536 rows or more, or a
    /// letter outside the Basic Multilingual Plane.
    pub fn load<S: Storage>(&mut self, ix: &Index, store: &mut S) -> Result<bool, Error<S::Error>> {
        *self = Self::EMPTY;
        let total = ix.letter_runs();
        if total as usize > N || ix.count() > u32::from(u16::MAX) || N > usize::from(u16::MAX) {
            return Ok(false);
        }
        let mut buf = [(0u32, None); 16];
        let mut done = 0u32;
        while done < total {
            let got = ix.read_letter_runs(store, done, &mut buf)?;
            if got == 0 {
                return Err(Error::Corrupt);
            }
            for &(first, l) in &buf[..got] {
                let (Ok(first), Ok(l)) = (u16::try_from(first), u16::try_from(l.map_or(0, u32::from))) else {
                    *self = Self::EMPTY;
                    return Ok(false);
                };
                self.runs[done as usize] = (first, l);
                done += 1;
            }
        }
        self.rows = ix.count();
        self.n = total as u16;
        Ok(true)
    }

    /// A table is loaded (an empty listing loads none).
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.n > 0
    }

    fn run_of(&self, i: u32) -> Option<usize> {
        if i >= self.rows || self.n == 0 {
            return None;
        }
        let runs = &self.runs[..usize::from(self.n)];
        // Last run starting at or before `i` (the first starts at 0).
        Some(runs.partition_point(|&(f, _)| u32::from(f) <= i).saturating_sub(1))
    }

    fn ch(l: u16) -> Option<char> {
        char::from_u32(u32::from(l)).filter(|&c| c != '\0')
    }

    /// Letter of row `i`, as [`Index::letter`].
    #[must_use]
    pub fn letter(&self, i: u32) -> Option<char> {
        Self::ch(self.runs[self.run_of(i)?].1)
    }

    /// Letter jump from row `from`, as [`Index::seek_letter`].
    #[must_use]
    pub fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        let r = self.run_of(from)?;
        let runs = &self.runs[..usize::from(self.n)];
        let cur = runs[r].1;
        if cur == 0 {
            return None;
        }
        let hit = |&&(_, l): &&(u16, u16)| l != 0 && l != cur;
        let found = if forward { runs[r + 1..].iter().find(hit) } else { runs[..r].iter().rev().find(hit) };
        found.and_then(|&(f, l)| Some((u32::from(f), Self::ch(l)?)))
    }
}
