//! Regressions for the PCM ring's headroom and underrun accounting
//! (`orpheus_audio::pcm`, used by `fw/firmware/src/audio.rs` `PcmOut`).
//!
//! VER-12 soak finding, 2026-11-25 (`mixed` seed 1,
//! `tests/regress/soak/mixed-ring-seed1-7.soak`): "DAC headroom fell to -13
//! frames (budget ≥ 1)" while the track's underrun count stayed 0.

use orpheus_audio::pcm::{Book, Reading, read};

const HALF: u32 = 1536;

/// Bug: `PcmOut::headroom` read the DMA's position first and the half count
/// second. A half interrupt between the two reads paired the old half's
/// position with the new count: one half (1536 frames) too low. With 1500
/// frames in the writer's half and the DMA 23 frames before the crossing
/// the true headroom is 1523 frames and the old reading -13.
#[test]
fn regress_headroom_irq_between_reads() {
    let mut book = Book::new(HALF);
    book.written = 41;
    book.fill = 1500;
    // The DMA is in half 40, 23 frames from its end; the half interrupt
    // lands right after the first position read.
    let played = core::cell::Cell::new(40u32);
    let pos = core::cell::Cell::new(HALF - 23);
    let position = || {
        let p = pos.get();
        if played.get() == 40 {
            // The crossing: the DMA moves on 23 frames, the interrupt runs.
            played.set(41);
            pos.set(0);
        }
        p
    };
    // The old order: position, then count.
    let old = book.headroom(Reading { at: position(), played: played.get() });
    assert_eq!(old, -13, "the reading the soak saw");
    // The same moment through `read`.
    played.set(40);
    pos.set(HALF - 23);
    let r = read(|| played.get(), position);
    assert_eq!(r, Reading { played: 41, at: 0 });
    assert_eq!(book.headroom(r), 1500);
    assert_eq!(book.look(r).stale, 0, "no underrun to count");
}

/// A writer against the DMA, one tick per DAC frame: the writer decodes a
/// block (a stall), then pushes it 8 frames per tick, looking at the DMA
/// before each block like the playback loop; its half boundaries go through
/// [`Book::boundary`]. Every ring frame is stamped with the stream frame
/// written there; the DMA checks the stamp of each frame it fetches.
struct Sim {
    book: Book,
    stamp: Vec<u64>,
    /// DMA frames fetched since the start.
    dma: u64,
    /// Half numbers the DMA fetched a stale frame from.
    stale_halves: Vec<u64>,
    /// Halves the old rule counted (played past written at a boundary).
    old_count: u32,
    new_count: u32,
    head_min: i32,
}

impl Sim {
    fn new() -> Self {
        let mut s = Sim {
            book: Book::new(HALF),
            stamp: vec![u64::MAX; 2 * HALF as usize],
            dma: 0,
            stale_halves: Vec::new(),
            old_count: 0,
            new_count: 0,
            head_min: i32::MAX,
        };
        // Two halves before the DAC starts, like `PcmOut::push`.
        for _ in 0..2 * HALF {
            s.write();
        }
        s.boundary();
        s
    }

    /// A finished half: wait while both halves are unplayed, then account.
    fn boundary(&mut self) {
        while self.book.full(self.reading().played) {
            self.tick();
        }
        let r = self.reading();
        if r.played > self.book.written {
            self.old_count += r.played - self.book.written;
        }
        let l = self.book.boundary(r);
        self.head_min = self.head_min.min(l.head);
        self.new_count += l.stale;
    }

    fn reading(&self) -> Reading {
        Reading { played: (self.dma / u64::from(HALF)) as u32, at: (self.dma % u64::from(HALF)) as u32 }
    }

    fn write(&mut self) -> bool {
        let n = u64::from(self.book.written) * u64::from(HALF) + u64::from(self.book.fill);
        let i = ((self.book.written % 2) * HALF + self.book.fill) as usize;
        self.stamp[i] = n;
        self.book.wrote()
    }

    fn tick(&mut self) {
        let i = (self.dma % (2 * u64::from(HALF))) as usize;
        if self.stamp[i] != self.dma {
            let h = self.dma / u64::from(HALF);
            if self.stale_halves.last() != Some(&h) {
                self.stale_halves.push(h);
            }
        }
        self.dma += 1;
    }

    fn look(&mut self) {
        let l = self.book.look(self.reading());
        self.head_min = self.head_min.min(l.head);
        self.new_count += l.stale;
    }

    /// Run `blocks` blocks of `block` frames; `decode(i)` ticks of decoding
    /// before block `i`.
    fn run(&mut self, blocks: u32, block: u32, decode: impl Fn(u32) -> u64) {
        for b in 0..blocks {
            for _ in 0..decode(b) {
                self.tick();
            }
            self.look();
            let mut left = block;
            while left > 0 {
                for _ in 0..8 {
                    if left == 0 {
                        break;
                    }
                    left -= 1;
                    if self.write() {
                        self.boundary();
                    }
                }
                self.tick();
            }
        }
    }
}

/// Bug: underruns were counted only at the writer's half boundaries, as
/// halves the DMA had finished beyond the writer. A stall that let the DMA
/// overtake the writer inside a half, after which the writer caught up
/// before the half ended, played stale frames with no underrun counted (the
/// soak's -13 frames with `underruns=0`).
#[test]
fn regress_overrun_inside_half_uncounted() {
    let mut s = Sim::new();
    // 1152-frame blocks decoded in 300 ticks; one 3000-tick stall (a card
    // chore of 62 ms at 48 kHz) before block 40.
    s.run(80, 1152, |b| if b == 40 { 3000 } else { 300 });
    assert!(!s.stale_halves.is_empty(), "the stall must make the DMA fetch stale frames");
    assert!(s.head_min < 0, "a look saw it: {}", s.head_min);
    assert_eq!(s.old_count, 0, "the old rule saw nothing");
    assert_eq!(s.new_count as usize, s.stale_halves.len(), "stale halves {:?}", s.stale_halves);
}

/// The accounting matches the stamps for stalls of every length up to a
/// few halves (the old rule's counts only ever came short).
#[test]
fn stale_halves_are_counted_exactly_for_any_stall() {
    for stall in (0..9000).step_by(97) {
        let mut s = Sim::new();
        s.run(60, 1152, |b| if b == 30 { stall } else { 300 });
        assert_eq!(s.new_count as usize, s.stale_halves.len(), "stall {stall}: {:?}", s.stale_halves);
        assert!(s.old_count <= s.new_count, "stall {stall}");
        assert_eq!(s.head_min < 0, !s.stale_halves.is_empty(), "stall {stall}: head {}", s.head_min);
    }
}

/// A decoder slower than the DAC: every half it falls behind is counted,
/// the writer still waits for the DMA (it yields), and the counts match.
#[test]
fn a_decoder_that_cannot_keep_up_is_counted_every_half() {
    let mut s = Sim::new();
    // 1152 frames per 1300 ticks of decoding plus 144 of pushing.
    s.run(40, 1152, |_| 1300);
    assert!(s.stale_halves.len() >= 5);
    assert_eq!(s.new_count as usize, s.stale_halves.len());
}
