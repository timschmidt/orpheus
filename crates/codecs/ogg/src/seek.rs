//! Granule bisection over a seekable source, independent of the packet
//! reader that resumes afterwards.

use orpheus_audio::{SeekSource, SourceError};

use crate::{Sync, sync_page};

/// Answer of [`Bisect::step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekStep {
    NeedData(usize),
    /// The last page of the stream with a granule position at or before
    /// the target: `(page offset, granule)`; `None` if the target is before
    /// every page's granule position.
    Done(Option<(u64, u64)>),
}

/// Bisection state: kept by the decoder between `NeedData` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bisect {
    serial: u32,
    target: u64,
    lo: u64,
    hi: u64,
    best: Option<(u64, u64)>,
    probe: u64,
    phase: Phase,
    steps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Scan,
    Linear,
}

/// Below this span the bisection walks pages linearly.
const LINEAR_SPAN: u64 = 48 * 1024;

impl Bisect {
    /// Search the pages of stream `serial` starting in `[start, end)` for
    /// the target granule.
    #[must_use]
    pub fn new(start: u64, end: u64, target: u64, serial: u32) -> Self {
        Self {
            serial,
            target,
            lo: start,
            hi: end.max(start),
            best: None,
            probe: start,
            phase: Phase::Start,
            steps: 0,
        }
    }

    #[must_use]
    pub fn target(&self) -> u64 {
        self.target
    }

    /// Run the bisection until it needs data or is done. Call again after
    /// `NeedData` once the source has the bytes (or is at its end).
    /// Afterwards position the source with `seek` and reset the reader.
    ///
    /// # Errors
    /// The source failed to seek.
    pub fn step(&mut self, src: &mut dyn SeekSource) -> Result<SeekStep, SourceError> {
        let b = self;
        loop {
            if b.phase == Phase::Start {
                b.steps += 1;
                if b.hi <= b.lo || b.hi - b.lo <= LINEAR_SPAN || b.steps > 64 {
                    b.phase = Phase::Linear;
                    b.probe = b.lo;
                } else {
                    b.phase = Phase::Scan;
                    b.probe = b.lo + (b.hi - b.lo) / 2;
                }
                src.seek(b.probe)?;
            }
            let (h, pos) = match sync_page(src) {
                Sync::Need(n) => return Ok(SeekStep::NeedData(n)),
                Sync::End => {
                    if b.phase == Phase::Linear {
                        return Ok(SeekStep::Done(b.best));
                    }
                    b.hi = b.probe;
                    b.phase = Phase::Start;
                    continue;
                }
                Sync::Page(h, _) => (h, src.position()),
            };
            if pos >= b.hi {
                if b.phase == Phase::Linear {
                    return Ok(SeekStep::Done(b.best));
                }
                b.hi = b.probe;
                b.phase = Phase::Start;
                continue;
            }
            if h.serial == b.serial
                && let Some(g) = h.granule_pos()
            {
                if g <= b.target {
                    b.best = Some((pos, g));
                    if b.phase == Phase::Scan {
                        b.lo = pos + h.total() as u64;
                        b.phase = Phase::Start;
                        continue;
                    }
                } else {
                    if b.phase == Phase::Linear {
                        return Ok(SeekStep::Done(b.best));
                    }
                    b.hi = b.probe;
                    b.phase = Phase::Start;
                    continue;
                }
            }
            // Step over this page.
            let total = h.total();
            if src.available() >= total {
                src.consume(total);
            } else {
                let to = pos + total as u64;
                if src.len().is_some_and(|l| to > l) {
                    if b.phase == Phase::Linear {
                        return Ok(SeekStep::Done(b.best));
                    }
                    b.hi = b.probe;
                    b.phase = Phase::Start;
                    continue;
                }
                src.seek(to)?;
            }
        }
    }
}
