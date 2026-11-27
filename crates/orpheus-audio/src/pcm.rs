//! The PCM ring's accounting between the writer and the DAC's DMA
//! (`fw/firmware/src/audio.rs` `PcmOut`).
//!
//! The ring is two halves of `half` frames. The DMA runs over both in
//! reload mode; its interrupt counts finished halves (`played`, from 0 at
//! the start). The writer fills the halves in turn: `written` halves are
//! finished, `fill` frames of the next. Half number `n` lives in ring half
//! `n % 2`, so the DMA fetches from half number `played` at frame `at` of
//! it while the writer writes half number `written`.
//!
//! **Headroom** is what the DMA has ahead of it:
//! `(written − played) · half + fill − at`. Below zero the DMA has fetched
//! frames the writer had not written yet: the DAC played the ring's
//! previous lap (64 ms old PCM at 48 kHz) instead. That is an underrun even
//! when the writer catches up before the half ends.
//!
//! Two VER-12 soak findings live here (2026-11-25, `mixed` seed 1, "DAC
//! headroom fell to -13 frames" with no underrun counted):
//!
//! * [`read`]: the count and the position must come from the same half. The
//!   firmware read the DMA's position, then the count; a half interrupt in
//!   between made the reading one half (1536 frames) too low.
//! * [`Book::look`]: underruns were only seen at the writer's half
//!   boundaries, as halves the DMA had *finished* past the writer. A DMA that
//!   overtook the writer inside a half (or finished the writer's half but not
//!   the next one) played stale frames that no counter saw. Every look now
//!   counts the halves the DMA fetched stale data from, once each.

/// The DMA's state at one moment: halves finished and the frame it fetches
/// in the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub played: u32,
    pub at: u32,
}

/// Read the interrupt's half count and the DMA's position so that both
/// belong to the same half: the count before and after the position must
/// agree (retried while a half interrupt lands in between; one does every
/// 32 ms, so the second try agrees).
///
/// The count may still lag the position by the interrupt's latency (a few
/// µs after a crossing): the reading is then one half too *high* for that
/// moment, never too low.
pub fn read(mut played: impl FnMut() -> u32, mut at: impl FnMut() -> u32) -> Reading {
    let mut p = played();
    let mut a = at();
    for _ in 0..3 {
        let q = played();
        if q == p {
            break;
        }
        p = q;
        a = at();
    }
    Reading { played: p, at: a }
}

/// What a [`Book::look`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// Frames the DMA has ahead of it (≤ 0: it is fetching stale frames).
    pub head: i32,
    /// Halves it fetched stale frames from that no earlier look counted.
    pub stale: u32,
}

/// The writer's side of the ring and the underruns already counted.
#[derive(Clone, Copy, Debug)]
pub struct Book {
    half: u32,
    /// Halves finished since the DAC started.
    pub written: u32,
    /// Frames in the half being written.
    pub fill: u32,
    /// The last half number counted as stale.
    stale_to: Option<u32>,
}

impl Book {
    #[must_use]
    pub const fn new(half: u32) -> Self {
        Self { half, written: 0, fill: 0, stale_to: None }
    }

    /// The DAC stopped: a new start counts from 0.
    pub fn reset(&mut self) {
        *self = Self::new(self.half);
    }

    /// One frame written; true when it finished a half.
    pub fn wrote(&mut self) -> bool {
        self.fill += 1;
        if self.fill < self.half {
            return false;
        }
        self.fill = 0;
        self.written = self.written.wrapping_add(1);
        true
    }

    /// Both halves hold frames the DMA has not finished: the writer waits.
    #[must_use]
    pub fn full(&self, played: u32) -> bool {
        // Signed: after an underrun `played` is ahead of `written`, which is
        // room, not a wrapped huge backlog.
        self.written.wrapping_sub(played) as i32 >= 2
    }

    /// Frames the DMA has ahead of it at `r`.
    #[must_use]
    pub fn headroom(&self, r: Reading) -> i32 {
        let halves = self.written.wrapping_sub(r.played) as i32;
        halves.wrapping_mul(self.half as i32).wrapping_add(self.fill as i32).wrapping_sub(r.at as i32)
    }

    /// The headroom at `r`, and the halves newly found stale: from the
    /// writer's half to the DMA's (the one it is in, if it fetched any of it),
    /// less those counted before.
    pub fn look(&mut self, r: Reading) -> Look {
        let head = self.headroom(r);
        if head >= 0 {
            return Look { head, stale: 0 };
        }
        // Negative headroom with `at == 0` means the DMA is past the writer's
        // half and has not fetched from its own yet.
        let last = if r.at == 0 { r.played.wrapping_sub(1) } else { r.played };
        let first = match self.stale_to {
            Some(s) if (s.wrapping_sub(self.written) as i32) >= 0 => s.wrapping_add(1),
            _ => self.written,
        };
        let n = last.wrapping_sub(first).wrapping_add(1) as i32;
        if n <= 0 {
            return Look { head, stale: 0 };
        }
        self.stale_to = Some(last);
        Look { head, stale: n as u32 }
    }

    /// Take back what the writer wrote more than `margin` frames ahead of
    /// the DMA at `r` (TTS-3: speech alone was stopped, the frames queued
    /// for the DAC are not to be heard). Returns the frames taken back (0:
    /// the writer was not that far ahead). The writer goes on from `margin`
    /// frames ahead of the DMA; the caller silences the frames taken back
    /// in the ring (they follow the writer's new place, ring frame
    /// `(written % 2) · half + fill` on).
    pub fn rewind(&mut self, r: Reading, margin: u32) -> u32 {
        let head = self.headroom(r);
        let Ok(margin) = i32::try_from(margin) else { return 0 };
        if head <= margin {
            return 0;
        }
        let back = (head - margin) as u32;
        let mut left = back;
        if left <= self.fill {
            self.fill -= left;
        } else {
            left -= self.fill;
            let halves = left.div_ceil(self.half);
            self.written = self.written.wrapping_sub(halves);
            self.fill = halves * self.half - left;
        }
        back
    }

    /// At a half boundary, after the wait for room: [`Self::look`], then,
    /// when the DMA is past the half the writer starts next, the writer
    /// skips to the half after the DMA's (the next half then waits for a
    /// DMA interrupt, so a decoder that cannot keep up still yields).
    pub fn boundary(&mut self, r: Reading) -> Look {
        let l = self.look(r);
        if (r.played.wrapping_sub(self.written) as i32) > 0 {
            self.written = r.played.wrapping_add(1);
        }
        l
    }
}

/// Where a [`Feeder`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fed {
    /// The whole block went to the sink.
    Done,
    /// The sink finished a half and wants the writer to wait for room
    /// before the next frame ([`Sink::frame`] returned true): call
    /// [`Feeder::run`] again after the wait.
    Wait,
}

/// The writer's side of one decoder block (`fw/firmware/src/audio.rs`
/// `Out`): the DSP over a piece in place, then its frames one by one.
pub trait Sink<S> {
    /// Process a piece (interleaved samples) in place; returns the samples
    /// now valid at its start (fewer after decimation).
    fn piece(&mut self, s: &mut [S]) -> usize;
    /// One frame (`channels` samples). True: a half is finished and the
    /// writer must wait for room before the next frame.
    fn frame(&mut self, f: &[S]) -> bool;
}

/// Feeds one decoder block to the PCM ring in pieces of the scratch
/// buffer's size (AU-PERF): each piece is copied to the scratch (the
/// decoder's buffer may be its state), processed there, and its whole
/// frames are written one by one, a trailing partial frame dropped. The
/// feed stops after a frame that finished a half ([`Fed::Wait`]) and goes
/// on from there on the next call, so the playback loop awaits only at
/// half boundaries and the per-frame work is not async code.
///
/// The scratch must keep its contents between the calls for one block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Feeder {
    /// Samples of the block taken into pieces so far.
    next: usize,
    /// Valid samples in the scratch, and how many of them are written.
    have: usize,
    at: usize,
}

impl Feeder {
    /// At the start of a block.
    #[must_use]
    pub const fn new() -> Self {
        Self { next: 0, have: 0, at: 0 }
    }

    /// Go on feeding `block` (interleaved, `channels` per frame) through
    /// `scratch` into `sink`.
    pub fn run<S: Copy>(
        &mut self,
        block: &[S],
        scratch: &mut [S],
        channels: usize,
        sink: &mut impl Sink<S>,
    ) -> Fed {
        let ch = channels.max(1);
        loop {
            while self.at + ch <= self.have {
                let Some(f) = scratch.get(self.at..self.at + ch) else { break };
                self.at += ch;
                if sink.frame(f) {
                    return Fed::Wait;
                }
            }
            let end = block.len().min(self.next.saturating_add(scratch.len()));
            let Some(part) = block.get(self.next..end).filter(|p| !p.is_empty()) else { return Fed::Done };
            let Some(sc) = scratch.get_mut(..part.len()) else { return Fed::Done };
            sc.copy_from_slice(part);
            self.next = end;
            let n = sink.piece(sc);
            self.have = n.min(sc.len());
            self.at = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::vec;
    use std::vec::Vec;

    use super::*;

    const HALF: u32 = 1536;

    /// A sink that records the frames and the pieces, wants a wait every
    /// `every` frames and decimates by two when `halve`.
    struct Rec {
        frames: Vec<Vec<i16>>,
        pieces: Vec<usize>,
        every: usize,
        n: usize,
        halve: bool,
    }

    impl Sink<i16> for Rec {
        fn piece(&mut self, s: &mut [i16]) -> usize {
            self.pieces.push(s.len());
            for x in s.iter_mut() {
                *x = x.wrapping_add(1);
            }
            if self.halve { s.len() / 2 } else { s.len() }
        }
        fn frame(&mut self, f: &[i16]) -> bool {
            self.frames.push(f.to_vec());
            self.n += 1;
            self.every != 0 && self.n.is_multiple_of(self.every)
        }
    }

    /// The straight loop the feeder replaces (the playback loop before
    /// AU-PERF): pieces through a copy, then their whole frames.
    fn straight(block: &[i16], piece: usize, ch: usize, halve: bool) -> (Vec<Vec<i16>>, Vec<usize>) {
        let mut r = Rec { frames: Vec::new(), pieces: Vec::new(), every: 0, n: 0, halve };
        for part in block.chunks(piece) {
            let mut sc = part.to_vec();
            let n = r.piece(&mut sc);
            for f in sc[..n.min(sc.len())].chunks_exact(ch) {
                r.frame(f);
            }
        }
        (r.frames, r.pieces)
    }

    #[test]
    fn feeder_writes_what_the_straight_loop_wrote_whatever_the_waits() {
        for (len, piece, ch, every, halve) in [
            (2304, 512, 2, 1536, false),
            (2304, 512, 2, 7, false),
            (2304, 512, 2, 1, false),
            (1152, 256, 1, 100, false),
            (1001, 256, 1, 3, false),
            // A trailing partial frame is dropped (odd sample count).
            (1001, 256, 2, 5, false),
            (4096, 512, 2, 333, true),
            (4096, 512, 2, 0, true),
            (0, 512, 2, 4, false),
            (10, 512, 2, 4, false),
        ] {
            let block: Vec<i16> = (0..len).map(|i| (i * 7 % 1000) as i16).collect();
            let (want, want_pieces) = straight(&block, piece, ch, halve);
            let mut scratch = vec![0i16; piece];
            let mut f = Feeder::new();
            let mut r = Rec { frames: Vec::new(), pieces: Vec::new(), every, n: 0, halve };
            let mut waits = 0;
            while f.run(&block, &mut scratch, ch, &mut r) == Fed::Wait {
                waits += 1;
                assert!(waits <= len + 1, "no progress");
            }
            assert_eq!(r.frames, want, "{len} {piece} {ch} {every}");
            assert_eq!(r.pieces, want_pieces);
            assert_eq!(
                waits,
                want.len().checked_div(every).unwrap_or(0),
                "a wait after every `every` frames"
            );
            // Done stays done.
            assert_eq!(f.run(&block, &mut scratch, ch, &mut r), Fed::Done);
            assert_eq!(r.frames.len(), want.len());
        }
    }

    fn at(played: u32, at: u32) -> Reading {
        Reading { played, at }
    }

    #[test]
    fn headroom_counts_the_written_halves_the_fill_and_the_position() {
        let mut b = Book::new(HALF);
        b.written = 7;
        b.fill = 100;
        assert_eq!(b.headroom(at(6, 0)), 1636);
        assert_eq!(b.headroom(at(6, 1000)), 636);
        assert_eq!(b.headroom(at(7, 50)), 50);
        assert_eq!(b.headroom(at(7, 150)), -50);
    }

    #[test]
    fn wrap_around_of_the_counters_is_harmless() {
        let mut b = Book::new(HALF);
        b.written = 1;
        b.fill = 10;
        assert_eq!(b.headroom(at(u32::MAX, 5)), 2 * HALF as i32 + 5);
        assert!(b.full(u32::MAX));
        b.written = u32::MAX;
        assert!(!b.full(u32::MAX));
        let l = b.look(at(0, 40));
        assert_eq!((l.head, l.stale), (-(HALF as i32) - 30, 2));
    }

    #[test]
    fn a_stale_half_is_counted_once_however_often_it_is_seen() {
        let mut b = Book::new(HALF);
        b.written = 10;
        b.fill = 100;
        assert_eq!(b.look(at(10, 150)).stale, 1);
        assert_eq!(b.look(at(10, 300)).stale, 0);
        b.fill = 400;
        // Caught up inside the half: no more.
        assert_eq!(b.look(at(10, 350)), Look { head: 50, stale: 0 });
        // The DMA overtakes again in the same half: already counted.
        b.fill = 500;
        assert_eq!(b.look(at(10, 600)).stale, 0);
        // It moves on into the next half while still ahead: one more.
        assert_eq!(b.look(at(11, 20)).stale, 1);
    }

    #[test]
    fn the_dma_past_the_writers_half_counts_every_half_in_between() {
        let mut b = Book::new(HALF);
        b.written = 4;
        b.fill = 1000;
        // In half 6 at frame 0: halves 4 and 5 stale, nothing of 6 yet.
        assert_eq!(b.look(at(6, 0)).stale, 2);
        assert_eq!(b.look(at(6, 1)).stale, 1);
    }

    #[test]
    fn boundary_skips_past_the_dma_like_the_old_resync() {
        let mut b = Book::new(HALF);
        b.written = 5;
        let l = b.boundary(at(7, 300));
        // Halves 5, 6 and 7 (the one it is in, played stale to the end).
        assert_eq!(l.stale, 3);
        assert_eq!(b.written, 8);
        // The next look starts afresh from half 8.
        assert_eq!(b.look(at(8, 10)).stale, 1);
        // Room: no resync.
        let mut b = Book::new(HALF);
        b.written = 9;
        assert_eq!(b.boundary(at(8, 300)), Look { head: HALF as i32 - 300, stale: 0 });
        assert_eq!(b.written, 9);
    }

    #[test]
    fn read_retries_until_the_count_and_the_position_agree() {
        let mut counts = [4u32, 5, 5].into_iter();
        let mut pos = [1530u32, 3].into_iter();
        let r = read(|| counts.next().unwrap_or(5), || pos.next().unwrap_or(9));
        assert_eq!(r, at(5, 3));
    }

    #[test]
    fn reset_forgets_the_counted_halves() {
        let mut b = Book::new(HALF);
        b.written = 3;
        assert_eq!(b.look(at(3, 10)).stale, 1);
        b.reset();
        assert_eq!((b.written, b.fill), (0, 0));
        b.written = 3;
        assert_eq!(b.look(at(3, 10)).stale, 1);
    }

    #[test]
    fn wrote_finishes_a_half_every_half_frames() {
        let mut b = Book::new(4);
        let ends: std::vec::Vec<bool> = (0..9).map(|_| b.wrote()).collect();
        assert_eq!(ends, [false, false, false, true, false, false, false, true, false]);
        assert_eq!((b.written, b.fill), (2, 1));
    }

    #[test]
    fn rewind_takes_back_what_is_ahead_of_the_dma_and_margin() {
        // Two halves and 100 frames written, the DMA at frame 200 of half 1:
        // 1,436 frames ahead; with a 32-frame margin 1,404 are taken back.
        let mut b = Book::new(HALF);
        b.written = 2;
        b.fill = 100;
        let r = at(1, 200);
        assert_eq!(b.headroom(r), 1436);
        assert_eq!(b.rewind(r, 32), 1404);
        assert_eq!(b.headroom(r), 32);
        assert_eq!((b.written, b.fill), (1, 232));
        // Within the margin: nothing.
        assert_eq!(b.rewind(r, 32), 0);
        assert_eq!(b.rewind(r, 100), 0);
        // Inside the half being written.
        let mut b = Book::new(HALF);
        b.written = 1;
        b.fill = 900;
        assert_eq!(b.rewind(at(1, 100), 48), 752);
        assert_eq!((b.written, b.fill), (1, 148));
        // Across a whole half to its start.
        let mut b = Book::new(HALF);
        b.written = 3;
        b.fill = 0;
        assert_eq!(b.rewind(at(1, HALF - 10), 10), HALF);
        assert_eq!((b.written, b.fill, b.headroom(at(1, HALF - 10))), (2, 0, 10));
        // Behind the DMA (an underrun): nothing to take back.
        let mut b = Book::new(HALF);
        b.written = 1;
        assert_eq!(b.rewind(at(2, 5), 0), 0);
    }
}
