//! More PCM ahead of the DAC than its DMA ring holds, for a while
//! (a gapless join: the next decoder opens while the old track's last
//! ~200 ms play).
//!
//! The DAC's DMA loops over a ring of two halves. Normally the writer
//! fills a half and waits while both hold unplayed data. In *spill* mode
//! the writer appends frames to a larger buffer instead (free memory, lent
//! for the time), never touching the DMA ring. The half-transfer interrupt
//! moves one whole half from the spill buffer into the half that just
//! finished playing ([`Spill::irq`]). The writer can then decode ahead by
//! the spill buffer's size and stop for that long without the DAC running
//! dry.
//!
//! Halves are counted absolutely: when `played` halves have completed, the
//! DMA is playing half `played`, which lives in ring half `played % 2`.
//! The interrupt can fill half `played + 1` only (the other ring half).
//! `w` is the next half the interrupt fills; a half it could not fill in
//! time (the spill buffer had less than a half) is lost, counted in
//! [`Spill::take_lost`], and filling goes on one half ahead of the DMA.
//!
//! Single core: the interrupt runs to completion between two task
//! instructions, so the task's switches (`enter`, `leave`) see a
//! consistent state. Only atomics: no `unsafe` here; the firmware does the
//! copies through raw pointers. No panic paths (the interrupt is
//! resident).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The spill state shared by the writer task and the DAC interrupt.
pub struct Spill {
    on: AtomicBool,
    /// Frames the writer appended / the interrupt moved (absolute).
    head: AtomicU32,
    tail: AtomicU32,
    /// Next half (absolute) the interrupt fills.
    w: AtomicU32,
    /// Halves the DAC played stale because nothing was there.
    lost: AtomicU32,
    /// Spill buffer length in frames; frames per half.
    cap: AtomicU32,
    half: AtomicU32,
}

/// A copy the interrupt is to make: `n` frames from spill index `from`
/// (wrapping at the buffer's end, `first` frames before the wrap) into
/// ring half `half % 2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub from: u32,
    pub first: u32,
    pub n: u32,
    pub half: u32,
}

/// What the writer continues with when spilling ends ([`Spill::leave`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resume {
    /// The half the writer fills next (absolute; its `written`).
    pub written: u32,
    /// Frames left in the spill buffer (fewer than a half), from spill
    /// index `from` (`first` before the wrap): they go to the start of
    /// half `written`.
    pub from: u32,
    pub first: u32,
    pub n: u32,
}

impl Default for Spill {
    fn default() -> Self {
        Self::new()
    }
}

impl Spill {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            on: AtomicBool::new(false),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            w: AtomicU32::new(0),
            lost: AtomicU32::new(0),
            cap: AtomicU32::new(0),
            half: AtomicU32::new(1),
        }
    }

    #[must_use]
    pub fn active(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// Start spilling. Call when the writer has just completed half
    /// `written - 1` and both ring halves hold unplayed data
    /// (`written == played + 2`): the interrupt fills half `written` when
    /// half `written - 2` has played. `cap`: the buffer's frames (at least
    /// two halves are useful).
    pub fn enter(&self, written: u32, cap: u32, half: u32) {
        self.head.store(0, Ordering::Relaxed);
        self.tail.store(0, Ordering::Relaxed);
        self.lost.store(0, Ordering::Relaxed);
        self.w.store(written, Ordering::Relaxed);
        self.cap.store(cap, Ordering::Relaxed);
        self.half.store(half.max(1), Ordering::Relaxed);
        self.on.store(true, Ordering::Release);
    }

    /// The writer's next slot (spill index), or `None` while the buffer is
    /// full (wait for the interrupt). Write the frame there, then
    /// [`Spill::pushed`].
    #[must_use]
    pub fn slot(&self) -> Option<u32> {
        let cap = self.cap.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        (head.wrapping_sub(tail) < cap).then(|| head % cap.max(1))
    }

    /// One frame was written at [`Spill::slot`].
    pub fn pushed(&self) {
        self.head.fetch_add(1, Ordering::Release);
    }

    /// Frames waiting in the spill buffer.
    #[must_use]
    pub fn queued(&self) -> u32 {
        self.head.load(Ordering::Acquire).wrapping_sub(self.tail.load(Ordering::Acquire))
    }

    /// Frames the DAC has ahead of it in the ring (whole halves, from half
    /// `played` on) plus the spill buffer; the caller subtracts what the
    /// DMA has played of half `played`.
    #[must_use]
    pub fn ahead(&self, played: u32) -> i32 {
        let halves = self.w.load(Ordering::Acquire).wrapping_sub(played) as i32;
        halves * self.half.load(Ordering::Relaxed) as i32 + self.queued() as i32
    }

    /// The interrupt, after `played` halves have completed: the copy to
    /// make now, if any (then call [`Spill::moved`]). A half it cannot
    /// fill in time is counted lost.
    pub fn irq(&self, played: u32) -> Option<Move> {
        if !self.on.load(Ordering::Acquire) {
            return None;
        }
        let mut w = self.w.load(Ordering::Relaxed);
        if (w.wrapping_sub(played) as i32) <= 0 {
            // Halves w..=played are playing (or played) stale.
            self.lost.fetch_add(played.wrapping_sub(w).wrapping_add(1), Ordering::Relaxed);
            w = played.wrapping_add(1);
            self.w.store(w, Ordering::Relaxed);
        }
        let half = self.half.load(Ordering::Relaxed);
        if w != played.wrapping_add(1) || self.queued() < half {
            return None;
        }
        let cap = self.cap.load(Ordering::Relaxed).max(1);
        let from = self.tail.load(Ordering::Relaxed) % cap;
        Some(Move { from, first: half.min(cap - from), n: half, half: w })
    }

    /// The interrupt made the copy [`Spill::irq`] asked for.
    pub fn moved(&self, m: Move) {
        self.tail.fetch_add(m.n, Ordering::Release);
        self.w.store(m.half.wrapping_add(1), Ordering::Release);
    }

    /// Halves lost since the last call.
    pub fn take_lost(&self) -> u32 {
        self.lost.swap(0, Ordering::AcqRel)
    }

    /// Stop spilling (call when fewer than a half is queued, or to drop
    /// it: then `n` is what is left). The writer goes on at
    /// `Resume::written`: if that half is still the one after the playing
    /// one (`written == played + 2`), it waits for the next interrupt as
    /// usual before writing it; if the DAC got ahead, `written` is
    /// `played + 1`.
    pub fn leave(&self, played: u32) -> Resume {
        self.on.store(false, Ordering::Release);
        let mut w = self.w.load(Ordering::Acquire);
        if (w.wrapping_sub(played) as i32) <= 0 {
            self.lost.fetch_add(played.wrapping_sub(w).wrapping_add(1), Ordering::Relaxed);
            w = played.wrapping_add(1);
        }
        let cap = self.cap.load(Ordering::Relaxed).max(1);
        let tail = self.tail.load(Ordering::Acquire);
        let n = self.head.load(Ordering::Acquire).wrapping_sub(tail);
        let from = tail % cap;
        Resume { written: w, from, first: n.min(cap - from), n }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const HALF: u32 = 8;

    /// A DAC playing a two-half ring at one frame per tick, the interrupt
    /// moving spilled halves, and a writer that is told when to stall.
    struct Sim {
        ring: [u32; 2 * HALF as usize],
        spill: Vec<u32>,
        s: Spill,
        played: u32,
        pos: u32,
        heard: Vec<u32>,
        // The writer (normal mode).
        written: u32,
        fill: u32,
        next: u32,
        /// Spilling ended; the rest goes to the ring once its half is free.
        resume: Option<Resume>,
    }

    impl Sim {
        fn new(cap: usize) -> Self {
            Self {
                ring: [0; 2 * HALF as usize],
                spill: std::vec![0; cap],
                s: Spill::new(),
                played: 0,
                pos: 0,
                heard: Vec::new(),
                written: 2,
                fill: 0,
                next: 1,
                resume: None,
            }
        }

        /// One frame played by the DAC; at a half's end the interrupt.
        fn tick(&mut self) {
            let i = ((self.played % 2) * HALF + self.pos) as usize;
            self.heard.push(self.ring[i]);
            self.pos += 1;
            if self.pos == HALF {
                self.pos = 0;
                self.played += 1;
                if let Some(m) = self.s.irq(self.played) {
                    let base = ((m.half % 2) * HALF) as usize;
                    for k in 0..m.n {
                        let src = ((m.from + k) as usize) % self.spill.len();
                        self.ring[base + k as usize] = self.spill[src];
                    }
                    self.s.moved(m);
                }
                self.finish_leave();
            }
        }

        /// The writer's part of leaving, once half `written` is free.
        fn finish_leave(&mut self) {
            let Some(r) = self.resume else { return };
            if r.written.wrapping_sub(self.played) as i32 >= 2 {
                return;
            }
            self.resume = None;
            self.written = r.written;
            let base = ((r.written % 2) * HALF) as usize;
            for k in 0..r.n {
                let src = ((r.from + k) as usize) % self.spill.len();
                self.ring[base + k as usize] = self.spill[src];
            }
            self.fill = r.n;
        }

        /// The writer pushes the next frame if it can (normal or spill).
        fn push(&mut self) -> bool {
            if self.resume.is_some() {
                return false;
            }
            if self.s.active() {
                let Some(i) = self.s.slot() else { return false };
                self.spill[i as usize] = self.next;
                self.s.pushed();
                self.next += 1;
                return true;
            }
            if self.written.wrapping_sub(self.played) as i32 >= 2 {
                return false;
            }
            if (self.written.wrapping_sub(self.played) as i32) < 1 {
                self.written = self.played + 1;
                self.fill = 0;
            }
            let i = ((self.written % 2) * HALF + self.fill) as usize;
            self.ring[i] = self.next;
            self.next += 1;
            self.fill += 1;
            if self.fill == HALF {
                self.fill = 0;
                self.written += 1;
            }
            true
        }

        fn enter(&mut self) {
            assert_eq!((self.fill, self.written), (0, self.played + 2));
            self.s.enter(self.written, self.spill.len() as u32, HALF);
        }

        fn leave(&mut self) {
            self.resume = Some(self.s.leave(self.played));
            self.finish_leave();
        }
    }

    /// Frames heard from the first real one on: consecutive, or the
    /// count of stale ones.
    fn gaps(heard: &[u32]) -> usize {
        let start = heard.iter().position(|&f| f != 0).unwrap_or(heard.len());
        heard[start..].windows(2).filter(|w| w[1] != w[0] + 1).count()
    }

    #[test]
    fn a_stall_longer_than_the_ring_but_within_the_spill_is_seamless() {
        // (spill frames, stall ticks, seamless): the ring holds 2 halves (16
        // frames) and the spill the rest; a stall beyond both is not.
        for (cap, stall, clean) in [
            (6 * HALF as usize, 50u32, true),
            (4 * HALF as usize, 30, true),
            (9 * HALF as usize, 70, true),
            (2 * HALF as usize, 60, false),
        ] {
            let mut sim = Sim::new(cap);
            // Prime the ring as the DAC starts with two full halves.
            for i in 0..2 * HALF as usize {
                sim.ring[i] = i as u32 + 1;
            }
            sim.next = 2 * HALF + 1;
            sim.written = 2;
            let mut entered = false;
            let mut stalled_from = None;
            let mut left = false;
            for t in 0..2000u32 {
                // The writer is 4x faster than the DAC, except while stalled.
                let stalled = stalled_from.is_some_and(|s| t >= s && t < s + stall);
                // Before leaving, the writer lets the spill drain.
                let draining = !left && stalled_from.is_some_and(|s| t > s + stall + 200);
                if !stalled && !draining {
                    for _ in 0..4 {
                        if !entered && t >= 100 && sim.fill == 0 && sim.written == sim.played + 2 {
                            sim.enter();
                            entered = true;
                        }
                        if !sim.push() {
                            break;
                        }
                    }
                }
                // The spill full: the old track ends, the next decoder
                // opens (the stall).
                if entered && stalled_from.is_none() && sim.s.slot().is_none() {
                    stalled_from = Some(t + 1);
                }
                if let Some(s) = stalled_from
                    && !left
                    && t > s + stall + 200
                    && sim.s.queued() < HALF
                {
                    sim.leave();
                    left = true;
                }
                sim.tick();
            }
            assert!(entered && left, "cap {cap} stall {stall}");
            let lost = sim.s.take_lost();
            if clean {
                assert_eq!((gaps(&sim.heard), lost), (0, 0), "cap {cap} stall {stall}");
            } else {
                assert!(gaps(&sim.heard) > 0 && lost > 0, "cap {cap} stall {stall}");
            }
        }
    }

    #[test]
    fn a_stall_beyond_the_spill_is_counted() {
        let mut sim = Sim::new(3 * HALF as usize);
        for i in 0..2 * HALF as usize {
            sim.ring[i] = i as u32 + 1;
        }
        sim.next = 2 * HALF + 1;
        sim.enter();
        // Fill the spill, then stop writing for long.
        while sim.push() {}
        for _ in 0..(10 * HALF) {
            sim.tick();
        }
        let lost = sim.s.take_lost();
        assert!(lost >= 3, "lost {lost}");
        // Writing again: the interrupt fills ahead of the DMA again.
        for _ in 0..(20 * HALF) {
            for _ in 0..4 {
                if !sim.push() {
                    break;
                }
            }
            sim.tick();
        }
        let tail = &sim.heard[sim.heard.len() - 4 * HALF as usize..];
        assert!(tail.windows(2).all(|w| w[1] == w[0] + 1), "{tail:?}");
    }

    #[test]
    fn leaving_with_the_dac_ahead_resyncs() {
        let s = Spill::new();
        s.enter(5, 32, HALF);
        // Nothing spilled; the DAC has completed 6 halves.
        assert_eq!(s.irq(6), None);
        assert_eq!(s.take_lost(), 2);
        let r = s.leave(6);
        assert_eq!((r.written, r.n), (7, 0));
        assert!(!s.active());
        assert_eq!(s.irq(7), None);
    }
}
