//! When the playback loop may do card work besides the refill (chores),
//! and how long each piece may take (`fw/firmware/src/audio.rs`
//! `chore_due`, `chores`).
//!
//! A chore is a piece of card work that does not feed the DAC: the next
//! track's open (`prefetch`) and head (`prepare`), one served UI request,
//! one piece of a listing or Up next window, a step of the library search,
//! the gapless track's art, the battery and step logs. While it runs the
//! DAC plays what is in the PCM ring (2 halves of 1,536 frames, ≈ 64 ms at
//! 48 kHz) and nothing refills it.
//!
//! **Budget.** The headroom (frames the DAC has ahead of it, `pcm::Book`)
//! in µs, less a margin for the decoder call that follows the chore
//! ([`margin_us`]: [`MARGIN_US`], or more when the decoder's recent calls
//! took longer), is what a chore may spend ([`budget_us`]).
//!
//! **Rule** ([`decide`]), for the first chore due (the others wait behind
//! it, so the headroom it waits for is not eaten by them):
//!
//! 1. It runs only if the budget covers its *need*: the measured length of
//!    its longest stretch that cannot be split ([`Costs`]: a splittable
//!    chore stops between card operations, so its need is one operation; an
//!    unsplittable one's is its whole length).
//! 2. And only when the DAC has [`Ask::roomy`] frames ahead (1.5 halves),
//!    or the chore has waited [`WAIT_US`]: after the wait it runs at any
//!    headroom that covers its need. (The old rule ran it at *any*
//!    headroom after the wait: soak seed 1, 2026-11-25, chores of 25–55 ms
//!    at −93…−649 frames.)
//! 3. A chore whose need no headroom covered for [`STARVE_US`] (it is
//!    longer than the ring can hold, or nearly: it must be split) runs at
//!    the next roomy moment anyway ([`Verdict::Starved`], logged): the UI
//!    is never left without an answer. A stream whose blocks never leave
//!    the DAC roomy at the moment of the decision (MP3 at 48 kHz on the
//!    player: 1,152-frame blocks against 1,536-frame halves, the headroom
//!    2,283 frames after every block, 21 short of roomy) would hold such a
//!    chore for ever (BOOK-1/2b, 2026-11-27: Books' open never ran while
//!    music played, its need 34 ms); after twice [`STARVE_US`] it runs
//!    at [`NEARLY_ROOMY`] eighths of roomy.
//!
//! **Pieces** ([`Piece`]): a splittable chore asks [`Piece::left`] before
//! each card operation and stops (to resume in its next chore) when the
//! next one might not fit the budget. The largest gap between two asks is
//! the chore's measured need ([`Costs::observe`]); the first ask always
//! says yes, so everything up to the second ask (work before the first
//! ask, such as opening a session, and the first operation) is one gap.
//!
//! **Bounded answers** (the host model in the tests). The decoder needs
//! less than real time (else the too-slow abort ends the track), and while
//! a chore waits nothing else runs from the chore list, so the headroom
//! grows by (1 − ρ) of real time (ρ: the loop's own share) until the ring
//! is full: from empty in 64 ms · ρ / (1 − ρ) at 48 kHz. With a full ring
//! the headroom after a block is at least about one half (32 ms) and, at
//! the block phases that end inside a half, more than 1.5 halves. So a
//! chore whose need fits one half less the margin runs at most `WAIT_US`
//! plus the time to fill the ring after it became first; one that fits 1.5
//! halves at the next roomy block after that; anything else by
//! `STARVE_US`.
//!
//! No panic paths, no floating point (used in the firmware's audio task).

/// The least kept free of chores for what follows one (µs): the decoder
/// call (4–9 ms on the player), ~5 ms more because the chore evicted the
/// decoder's code from the page cache, and a ring refill (3–8 ms). 10 ms
/// let a chore that kept to its budget empty the DAC ring on the player
/// (2026-11-25, `regress_margin_covers_the_reload_after_a_chore`).
pub const MARGIN_US: u32 = 22_000;
/// A chore that has waited this long (µs) for a roomy DAC runs at any
/// headroom that covers its need.
pub const WAIT_US: u32 = 300_000;
/// A chore whose need no headroom covered for this long (µs) runs at the
/// next roomy moment anyway (it cannot fit: it must be split).
pub const STARVE_US: u32 = 2_000_000;
/// Eighths of [`Ask::roomy`] a chore starved for twice [`STARVE_US`] runs
/// at (the stream may never be roomy when it is decided).
pub const NEARLY_ROOMY: i32 = 7;

/// The margin kept for the decoder call after a chore: [`MARGIN_US`], or
/// the longest recent call (`dec_us`) and a quarter more if that is longer
/// (code pages evicted by the UI make calls slower).
#[must_use]
pub fn margin_us(dec_us: u32) -> u32 {
    MARGIN_US.max(dec_us.saturating_add(dec_us / 4))
}

/// The longest recent decoder call (µs; [`margin_us`]): a longer call
/// counts at once, else it decays by 1/256 per call (≈ 6 s of MP3 blocks;
/// faster, the margin fell below the next slow call in the host model).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecPeak(pub u32);

impl DecPeak {
    /// After a call of `us`.
    #[must_use]
    pub fn after(self, us: u32) -> Self {
        DecPeak(us.max(self.0 - self.0 / 256))
    }
}

/// `frames` at `rate` in µs (negative: frames the DAC played stale).
#[must_use]
pub fn frames_us(frames: i32, rate: u32) -> i64 {
    i64::from(frames) * 1_000_000 / i64::from(rate.max(1))
}

/// What a chore may spend at `head` frames of headroom: its length in µs
/// less the margin ([`margin_us`] of `dec_us`), 0 when that is all.
/// `i32::MAX` (the DAC is not running: nothing has a deadline) gives
/// `u32::MAX`.
#[must_use]
pub fn budget_us(head: i32, rate: u32, dec_us: u32) -> u32 {
    if head == i32::MAX {
        return u32::MAX;
    }
    let b = frames_us(head, rate) - i64::from(margin_us(dec_us));
    b.clamp(0, i64::from(u32::MAX - 1)) as u32
}

/// The state a chore is decided in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    /// Frames the DAC has ahead of it (`i32::MAX`: not running).
    pub head: i32,
    /// The DAC's rate.
    pub rate: u32,
    /// Frames ahead that count as roomy (1.5 halves).
    pub roomy: i32,
    /// The chore's need (µs, [`Costs::need`]).
    pub need_us: u32,
    /// How long it has waited (µs).
    pub waited_us: u32,
    /// The longest recent decoder call (µs; [`margin_us`]).
    pub dec_us: u32,
}

/// What [`decide`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not now.
    Wait,
    /// Run one piece of at most `budget_us`.
    Run { budget_us: u32 },
    /// Its need fit no headroom for [`STARVE_US`]: run one piece anyway
    /// (its first card operation goes; it may overrun).
    Starved { budget_us: u32 },
}

impl Verdict {
    /// The budget, when the chore runs.
    #[must_use]
    pub fn budget(self) -> Option<u32> {
        match self {
            Verdict::Wait => None,
            Verdict::Run { budget_us } | Verdict::Starved { budget_us } => Some(budget_us),
        }
    }
}

/// The rule (module docs).
#[must_use]
pub fn decide(a: &Ask) -> Verdict {
    let budget = budget_us(a.head, a.rate, a.dec_us);
    let roomy = a.head >= a.roomy;
    let nearly = a.head >= a.roomy / 8 * NEARLY_ROOMY;
    if budget > 0 && budget >= a.need_us && (roomy || a.waited_us >= WAIT_US) {
        Verdict::Run { budget_us: budget }
    } else if (roomy && a.waited_us >= STARVE_US) || (nearly && a.waited_us >= 2 * STARVE_US) {
        Verdict::Starved { budget_us: budget }
    } else {
        Verdict::Wait
    }
}

/// One chore piece's time: its budget, and the gaps between the asks of a
/// splittable chore ([`Piece::left`] before each card operation). Times are
/// µs of a free-running 32-bit clock (wrapping).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    start: u32,
    budget: u32,
    /// The need the piece was admitted with.
    need: u32,
    last: u32,
    gap: u32,
    asks: u16,
    /// The caller's card-operation count at the start (low 16 bits;
    /// [`Piece::with_io`], [`Piece::worked`]).
    io: u16,
}

/// What a piece took ([`Piece::end`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spent {
    /// µs from start to end.
    pub took: u32,
    /// The longest stretch without an ask (µs): what could not be split.
    pub unit: u32,
    /// Asks answered.
    pub asks: u32,
    /// It took longer than its budget.
    pub over: bool,
    /// Its budget (µs; `u32::MAX`: none).
    pub budget: u32,
}

impl Piece {
    /// No budget (the idle loop, a paused track: nothing plays).
    pub const UNLIMITED: Piece =
        Piece { start: 0, budget: u32::MAX, need: 0, last: 0, gap: 0, asks: 0, io: 0 };

    #[must_use]
    pub const fn new(now: u32, budget_us: u32, need_us: u32) -> Self {
        Piece { start: now, budget: budget_us, need: need_us, last: now, gap: 0, asks: 0, io: 0 }
    }

    /// The piece starts at card-operation count `io` (a free-running
    /// counter; the firmware's `yoton_bsp::card::io_count`).
    #[must_use]
    pub const fn with_io(mut self, io: u32) -> Self {
        self.io = io as u16;
        self
    }

    /// Whether the counter moved since the start ([`Piece::with_io`]): the
    /// piece did card work. A piece that did none is a cheap variant of its
    /// kind and must not lower the kind's need ([`Costs::observe_piece`]).
    #[must_use]
    pub fn worked(&self, io: u32) -> bool {
        io as u16 != self.io
    }

    /// Before a card operation: may it start? The first one always does
    /// (the chore was admitted for it, and something must progress); a later
    /// one when the time spent plus the longest operation known (the need,
    /// or a longer one seen in this piece) fits the budget.
    ///
    /// The gaps: from the start to the second ask is one (nothing in it
    /// could be refused: work done before the first ask, such as opening
    /// a session, and the operation after it); then from ask to ask. A
    /// listing piece that read its first rows before its first ask counted
    /// the session's open (31 ms) and its first row (5 ms) as two gaps,
    /// was admitted at a 36-ms budget and took 37 ms (soak seed 1,
    /// 2026-11-25).
    pub fn left(&mut self, now: u32) -> bool {
        self.asks = self.asks.saturating_add(1);
        let from = if self.asks == 2 { self.start } else { self.last };
        self.gap = self.gap.max(now.wrapping_sub(from));
        self.last = now;
        if self.budget == u32::MAX || self.asks == 1 {
            return true;
        }
        let spent = now.wrapping_sub(self.start);
        spent.saturating_add(self.need.max(self.gap)) <= self.budget
    }

    /// The piece is over.
    #[must_use]
    pub fn end(&self, now: u32) -> Spent {
        let took = now.wrapping_sub(self.start);
        // Fewer than two asks: none of it could be refused.
        let unit = if self.asks < 2 { took } else { self.gap.max(now.wrapping_sub(self.last)) };
        Spent {
            took,
            unit,
            asks: u32::from(self.asks),
            over: self.budget != u32::MAX && took > self.budget,
            budget: self.budget,
        }
    }

    /// The piece's budget (µs; `u32::MAX`: none).
    #[must_use]
    pub fn budget(&self) -> u32 {
        self.budget
    }
}

/// The measured need of `N` chore kinds (µs): the longest unsplittable
/// stretch seen, decaying a quarter of the way towards each shorter one (a
/// one-off slow piece, cold code pages, does not hold the kind back for
/// long; a too-low need shows as an overrun and is raised at once). Until
/// a kind is measured its `seed` (a conservative guess, the caller's)
/// stands for it. Kept in units of [`COST_UNIT_US`] in 16 bits (up to
/// ≈ 1 s, rounded up) and all zeros when new: a firmware static of it is
/// small and costs no initialised RAM (both come off the stack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Costs<const N: usize> {
    /// Units of `COST_UNIT_US`; 0: not measured yet.
    need: [u16; N],
}

/// The resolution of [`Costs`] (µs).
pub const COST_UNIT_US: u32 = 16;

impl<const N: usize> Default for Costs<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Costs<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self { need: [0; N] }
    }

    /// The need of `kind` (µs), `seed` until measured.
    #[must_use]
    pub fn need(&self, kind: usize, seed: u32) -> u32 {
        match self.need.get(kind).copied() {
            Some(0) | None => seed,
            Some(n) => u32::from(n) * COST_UNIT_US,
        }
    }

    /// A piece of `kind` that may have done no card work (`worked`): one
    /// that did is [`Costs::observe`]d; one that did not only raises the
    /// need, never lowers it. A request kind with a variant that touches no
    /// card (`want` while a folder plays: the window is made in later
    /// pieces; `drain` once its file is chosen) otherwise taught the kind a
    /// need of ~0, and its card-reading variant (`want` in a music-library
    /// list) was then admitted at any budget.
    pub fn observe_piece(&mut self, kind: usize, unit: u32, seed: u32, worked: bool) {
        if worked || unit >= self.need(kind, seed) {
            self.observe(kind, unit, seed);
        }
    }

    /// A piece of `kind` (need `seed` until measured) ran; `unit` is its
    /// longest unsplittable stretch.
    pub fn observe(&mut self, kind: usize, unit: u32, seed: u32) {
        let mut n = self.need(kind, seed);
        if unit >= n {
            n = unit;
        } else {
            // Slowly: one long card operation (a 21 ms directory read on
            // the player) must not be forgotten after a few short pieces
            // (2026-11-25: `chore: job:count 22/8 ms` after the need had
            // decayed a quarter per piece).
            n -= (n - unit) / 16;
        }
        if let Some(m) = self.need.get_mut(kind) {
            *m = n.div_ceil(COST_UNIT_US).clamp(1, u32::from(u16::MAX)) as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const HALF: i32 = 1536;
    const ROOMY: i32 = HALF + HALF / 2;

    fn ask(head: i32, need_us: u32, waited_us: u32) -> Ask {
        Ask { head, rate: RATE, roomy: ROOMY, need_us, waited_us, dec_us: 4_000 }
    }

    #[test]
    fn budget_is_headroom_less_margin() {
        // One half at 48 kHz: 32 ms; less the margin.
        assert_eq!(budget_us(HALF, RATE, 0), 32_000 - MARGIN_US);
        assert_eq!(budget_us(ROOMY, RATE, 8_000), 48_000 - MARGIN_US);
        assert_eq!(budget_us(441, 44_100, 0), 0);
        assert_eq!(budget_us(-93, RATE, 0), 0);
        assert_eq!(budget_us(i32::MAX, RATE, 0), u32::MAX);
        assert_eq!(frames_us(-480, RATE), -10_000);
        // Slow decoder calls: the margin grows with them.
        assert_eq!(margin_us(16_000), MARGIN_US.max(20_000));
        assert_eq!(margin_us(24_000), 30_000);
        assert_eq!(budget_us(ROOMY, RATE, 24_000), 18_000);
    }

    #[test]
    fn rule_cases() {
        // Roomy and it fits.
        assert_eq!(decide(&ask(ROOMY, 25_000, 0)), Verdict::Run { budget_us: 48_000 - MARGIN_US });
        // Roomy but it does not fit: waits (until the starvation bound).
        assert_eq!(decide(&ask(ROOMY, 40_000, 0)), Verdict::Wait);
        assert_eq!(
            decide(&ask(ROOMY, 40_000, STARVE_US)),
            Verdict::Starved { budget_us: 48_000 - MARGIN_US }
        );
        // Not roomy, not waited: waits even if it fits.
        assert_eq!(decide(&ask(HALF, 5_000, WAIT_US - 1)), Verdict::Wait);
        // Waited: runs if it fits (the forced path), not otherwise.
        assert_eq!(decide(&ask(HALF, 5_000, WAIT_US)), Verdict::Run { budget_us: 32_000 - MARGIN_US });
        assert_eq!(decide(&ask(HALF, 25_000, WAIT_US)), Verdict::Wait);
        // The soak's forced chores: 25–55 ms at -93 / -265 / -649 frames.
        for head in [-93, -265, -649, 0, 480] {
            assert_eq!(decide(&ask(head, 25_000, 10 * WAIT_US)), Verdict::Wait);
        }
        // Starved only at a roomy moment, never at low headroom.
        assert_eq!(decide(&ask(HALF, 80_000, 10 * STARVE_US)), Verdict::Wait);
        // The DAC is not running: anything goes.
        assert_eq!(decide(&ask(i32::MAX, 500_000, 0)), Verdict::Run { budget_us: u32::MAX });
    }

    #[test]
    fn piece_stops_before_an_operation_that_may_not_fit() {
        let mut p = Piece::new(1_000, 20_000, 4_000);
        // The first operation always goes.
        assert!(p.left(1_000));
        // 3 ms each: at 15 ms spent + 4 ms need ≤ 20 one more goes; at 18 not.
        let mut t = 1_000;
        let mut ran = 1;
        loop {
            t += 3_000;
            if !p.left(t) {
                break;
            }
            ran += 1;
        }
        assert_eq!(ran, 6);
        let s = p.end(t);
        assert_eq!(s.took, 18_000);
        assert_eq!(s.unit, 3_000);
        assert!(!s.over);
        // A longer operation seen raises the estimate for the rest.
        let mut p = Piece::new(0, 20_000, 1_000);
        assert!(p.left(0));
        // 9 spent + 9 (the gap seen) = 18 ≤ 20.
        assert!(p.left(9_000));
        // 12 + 9 = 21 > 20.
        assert!(!p.left(12_000));
        assert_eq!(p.end(12_000).unit, 9_000);
        // An unsplittable chore: no asks, its unit is its length.
        let p = Piece::new(u32::MAX - 1_000, 5_000, 0);
        let s = p.end(9_000);
        assert_eq!((s.took, s.unit, s.over), (10_001, 10_001, true));
        // Unlimited: always.
        let mut p = Piece::UNLIMITED;
        assert!(p.left(5) && p.left(1 << 31) && !p.end(7).over);
    }

    /// The soak's `job:rows 37/36 ms u31`: 31 ms of work before the first
    /// ask (the session's open and the first rows read), 5 ms for the row
    /// after it (the first ask cannot refuse it). Nothing before the
    /// second ask could be refused, so it is one unit of 36 ms, and a
    /// kind with that need is not admitted at less: the old gaps (31, 5)
    /// made it 31 and the piece overran.
    #[test]
    fn regress_work_before_the_first_ask_is_one_unit_with_the_first_operation() {
        let mut p = Piece::new(0, 36_000, 31_000);
        assert!(p.left(31_000));
        assert!(!p.left(36_000));
        let s = p.end(37_000);
        assert_eq!((s.took, s.unit, s.over), (37_000, 36_000, true));
        let mut c = Costs::<1>::new();
        c.observe(0, s.unit, 8_000);
        let need = c.need(0, 8_000);
        assert_eq!(need, 36_000);
        let a = Ask { head: 2_784, rate: RATE, roomy: ROOMY, need_us: need, waited_us: 0, dec_us: 4_000 };
        assert_eq!(budget_us(a.head, RATE, a.dec_us), 36_000);
        assert_eq!(decide(&Ask { head: a.head - 48, ..a }), Verdict::Wait);
        // One ask only (the piece ended after its first operation): all of
        // it was one unit.
        let mut p = Piece::new(0, 36_000, 1_000);
        assert!(p.left(20_000));
        assert_eq!(p.end(30_000).unit, 30_000);
        // Asks from the start: as before, the gaps between them.
        let mut p = Piece::new(0, 36_000, 1_000);
        assert!(p.left(0) && p.left(3_000) && p.left(5_000));
        assert_eq!(p.end(6_000).unit, 3_000);
    }

    /// Regression (2026-11-25): a kind whose first card operation can take
    /// 21 ms was admitted at an 8 ms budget after a handful of short pieces
    /// (`chore: job:count 22/8 ms`). Five short pieces keep most of it.
    #[test]
    fn regress_one_long_operation_is_not_forgotten_after_a_few_short_pieces() {
        let mut c = Costs::<1>::new();
        c.observe(0, 21_000, 8_000);
        for _ in 0..5 {
            c.observe(0, 3_000, 8_000);
        }
        assert!(c.need(0, 8_000) >= 15_000, "{}", c.need(0, 8_000));
    }

    #[test]
    fn costs_rise_at_once_and_decay_slowly() {
        let mut c = Costs::<2>::new();
        // The seed until measured.
        assert_eq!(c.need(0, 10_000), 10_000);
        c.observe(0, 40_000, 10_000);
        assert_eq!(c.need(0, 10_000), 40_000);
        c.observe(0, 8_000, 10_000);
        assert_eq!(c.need(0, 10_000), 38_000);
        for _ in 0..160 {
            c.observe(0, 8_000, 10_000);
        }
        // Integer steps of 1/16 settle within 16 units of the target.
        assert!((8_000..8_300).contains(&c.need(0, 10_000)), "{}", c.need(0, 10_000));
        // Measured below the seed: decays from the seed; rounded up to
        // 16 µs, at most ≈ 1 s.
        c.observe(1, 0, 4_000);
        assert_eq!(c.need(1, 4_000), 3_760);
        c.observe(1, 5_000_000, 4_000);
        assert_eq!(c.need(1, 4_000), 65_535 * 16);
        // Out of range: the seed, nothing stored.
        c.observe(5, 1, 7);
        assert_eq!(c.need(5, 7), 7);
    }

    // ---------------------------------------------------------- the model

    /// The playback loop's own work (µs): a decoder call per block (UI load
    /// makes it slower: pages evicted, the pager's time), a stream refill
    /// every `fill_every` blocks.
    #[derive(Clone, Copy, Debug)]
    struct Load {
        dec: (u32, u32),
        fill: (u32, u32),
        fill_every: u32,
        /// Added to the first decoder call after a chore: the chore's code
        /// evicted the decoder's from the page cache.
        reload: u32,
    }

    /// A chore kind of the model: its card operations' lengths (µs), its
    /// total, and whether it may stop between them.
    #[derive(Clone, Copy, Debug)]
    struct Kind {
        ops: (u32, u32),
        total: u32,
        split: bool,
    }

    /// A chore waiting: kind, arrival (µs), work done (µs).
    #[derive(Clone, Copy, Debug)]
    struct Waiting {
        kind: usize,
        at: u64,
        done: u32,
        /// The kind's cheap variant: no card operation ([`Case::cheap`]).
        cheap: bool,
    }

    /// What a run of the model found.
    #[derive(Debug, Default)]
    struct Found {
        /// DAC underruns during a chore piece.
        chore_underruns: u32,
        /// … and during the playback loop's own work (must stay 0: the
        /// model's decoder keeps up, and the margin covers its call).
        loop_underruns: u32,
        /// A chore piece took longer than the headroom it started with.
        exceeded: u32,
        /// UI answers and background chores done.
        answered: u32,
        /// Longest time from a UI request to its answer (µs).
        worst_latency: u64,
        starved: u32,
        /// Checks with a chore waiting, and those of them roomy.
        checks: u32,
        roomy_checks: u32,
        /// Pieces run after the wait (not roomy).
        forced: u32,
    }

    struct Rng(u32);
    impl Rng {
        fn next(&mut self) -> u32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            x
        }
        fn range(&mut self, lo: u32, hi: u32) -> u32 {
            lo + self.next() % (hi - lo + 1)
        }
    }

    /// The PCM ring and the DAC: `written` frames so far; the DAC starts
    /// after two halves and plays `RATE` frames a second.
    struct Dac {
        t: u64,
        written: u64,
        start: Option<u64>,
    }

    impl Dac {
        fn played(&self) -> u64 {
            self.start.map_or(0, |s| (self.t - s) * u64::from(RATE) / 1_000_000)
        }
        fn head(&self) -> i32 {
            if self.start.is_none() {
                return i32::MAX;
            }
            (self.written as i64 - self.played() as i64) as i32
        }
        /// Work of `us` that does not feed the DAC; true if it ran dry.
        fn busy(&mut self, us: u32) -> bool {
            self.t += u64::from(us);
            let dry = self.start.is_some() && self.written < self.played();
            if dry {
                // The firmware's writer re-syncs to the half after the
                // DMA's (`Book::boundary`).
                let half = HALF as u64;
                self.written = (self.played() / half + 1) * half;
            }
            dry
        }
        /// Write a block, waiting for room like `PcmOut::half`.
        fn push(&mut self, mut frames: u64) {
            let half = HALF as u64;
            while frames > 0 {
                let Some(s) = self.start else {
                    let k = frames.min(2 * half - self.written);
                    self.written += k;
                    frames -= k;
                    if self.written >= 2 * half {
                        self.start = Some(self.t);
                    }
                    continue;
                };
                let room = (self.played() / half + 2) * half - self.written;
                if room == 0 {
                    // Until the DMA finishes its half.
                    let next = (self.played() / half + 1) * half;
                    self.t = s + (next * 1_000_000).div_ceil(u64::from(RATE));
                    continue;
                }
                let k = room.min(frames);
                self.written += k;
                frames -= k;
            }
        }
    }

    /// A model run: the chore kinds, the UI's requests in turn (`ui`, the
    /// next one `think` after an answer), a background chore (kind, every
    /// µs), the loop's load.
    #[derive(Clone, Copy)]
    struct Case<'a> {
        kinds: &'a [Kind],
        ui: &'a [usize],
        think: (u32, u32),
        bg: Option<(usize, u32)>,
        load: Load,
        until_us: u64,
        /// Requests `ui[i]` for which `cheap[i % len]` is true are their
        /// kind's cheap variant: `cheap_us` of work and no card operation
        /// (`want` while a folder plays). Empty: none.
        cheap: &'a [bool],
        cheap_us: u32,
        /// Every piece measured, card work or not (before 2026-11-25).
        measure_all: bool,
    }

    /// The playback loop of `audio::play_block` against the DAC: a decoder
    /// call, the block pushed, a refill every few blocks, then the chore
    /// check with `rule` for the first chore waiting (the others wait behind
    /// it; `waited` counts from when a chore became due with none due
    /// before, or from the last piece run, as `Blk::chore_since`). A
    /// splittable chore asks [`Piece::left`] before each operation. The UI
    /// asks one thing at a time; the background chore (a read-ahead open)
    /// goes first. The costs start at the kinds' longest operation
    /// (unsplittable: the whole chore) and learn from the pieces; the
    /// margin follows the decoder's calls ([`DecPeak`]).
    fn model(rule: &dyn Fn(&Ask) -> Verdict, c: Case<'_>, seed: u32) -> Found {
        let mut dac = Dac { t: 0, written: 0, start: None };
        let mut rng = Rng(seed);
        let mut costs = Costs::<8>::new();
        let seed = |i: usize| c.kinds.get(i).map_or(0, |k: &Kind| if k.split { k.ops.1 } else { k.total });
        let mut f = Found::default();
        let mut back: Option<Waiting> = None;
        let mut next_bg = c.bg.map_or(u64::MAX, |(_, every)| u64::from(every));
        let mut req: Option<Waiting> = None;
        let mut next_ui = 300_000u64;
        let mut ui_n = 0usize;
        let mut since: Option<u64> = None;
        let mut blocks = 0u32;
        let mut dec_peak = 0u32;
        // Card operations so far (the firmware's `card::io_count`).
        let mut ops = 0u32;
        // A chore ran since the last push: an underrun in the decoder call
        // before the next push is the chore's (as the firmware's ledger
        // charges it).
        let mut after_chore = false;
        while dac.t < c.until_us {
            // MP3: 1,152 frames a block (24 ms at 48 kHz).
            let d = rng.range(c.load.dec.0, c.load.dec.1) + if after_chore { c.load.reload } else { 0 };
            dec_peak = DecPeak(dec_peak).after(d).0;
            if dac.busy(d) {
                if after_chore {
                    f.chore_underruns += 1;
                } else {
                    f.loop_underruns += 1;
                }
            }
            after_chore = false;
            dac.push(1152);
            blocks += 1;
            if blocks % c.load.fill_every == 0 && dac.busy(rng.range(c.load.fill.0, c.load.fill.1)) {
                f.loop_underruns += 1;
            }
            if let Some((kind, every)) = c.bg
                && dac.t >= next_bg
                && back.is_none()
            {
                back = Some(Waiting { kind, at: dac.t, done: 0, cheap: false });
                next_bg = dac.t + u64::from(every);
            }
            if req.is_none() && dac.t >= next_ui && !c.ui.is_empty() {
                let cheap = !c.cheap.is_empty() && c.cheap[ui_n % c.cheap.len()];
                req = Some(Waiting { kind: c.ui[ui_n % c.ui.len()], at: dac.t, done: 0, cheap });
                ui_n += 1;
            }
            let (is_bg, w) = match (back.as_mut(), req.as_mut()) {
                (Some(w), _) => (true, w),
                (None, Some(w)) => (false, w),
                (None, None) => {
                    since = None;
                    continue;
                }
            };
            let s = *since.get_or_insert(dac.t);
            let head = dac.head();
            f.checks += 1;
            if head >= ROOMY {
                f.roomy_checks += 1;
            }
            let a = Ask {
                head,
                rate: RATE,
                roomy: ROOMY,
                need_us: costs.need(w.kind, seed(w.kind)),
                waited_us: (dac.t - s).min(u64::from(u32::MAX)) as u32,
                dec_us: dec_peak,
            };
            let v = rule(&a);
            let Some(budget) = v.budget() else { continue };
            match v {
                Verdict::Starved { .. } => f.starved += 1,
                _ if head < ROOMY => f.forced += 1,
                _ => {}
            }
            since = None;
            let k = c.kinds[w.kind];
            let head_us = frames_us(head, RATE);
            let p0 = Piece::new(dac.t as u32, budget, a.need_us).with_io(ops);
            let mut p = p0;
            let mut dry = false;
            if w.cheap {
                // No card operation: the request is answered from RAM.
                dry |= dac.busy(c.cheap_us);
                w.done = k.total;
            }
            while w.done < k.total {
                if k.split && !p.left(dac.t as u32) {
                    break;
                }
                let op = if k.split { rng.range(k.ops.0, k.ops.1) } else { k.total };
                dry |= dac.busy(op);
                ops = ops.wrapping_add(1);
                w.done += op;
            }
            let spent = p.end(dac.t as u32);
            if c.measure_all {
                costs.observe(w.kind, spent.unit, seed(w.kind));
            } else {
                costs.observe_piece(w.kind, spent.unit, seed(w.kind), p.worked(ops));
            }
            if dry {
                f.chore_underruns += 1;
            }
            after_chore = !dry;
            if i64::from(spent.took) > head_us {
                f.exceeded += 1;
            }
            if w.done >= k.total {
                f.answered += 1;
                if is_bg {
                    back = None;
                } else {
                    f.worst_latency = f.worst_latency.max(dac.t - w.at);
                    req = None;
                    next_ui = dac.t + u64::from(rng.range(c.think.0, c.think.1));
                }
            }
        }
        f
    }

    /// The rule before 2026-11-25: roomy, or waited 300 ms at any headroom.
    fn old_rule(a: &Ask) -> Verdict {
        if a.head >= a.roomy || a.waited_us > WAIT_US {
            Verdict::Run { budget_us: budget_us(a.head, a.rate, a.dec_us) }
        } else {
            Verdict::Wait
        }
    }

    /// The soak's chores (seed 1, `browse pictures` with a 256-track
    /// shuffled MP3 queue, 2026-11-25): unsplittable requests of 25 and
    /// 28 ms, listing pieces of 55 ms in card operations of 2–6 ms, the
    /// read-ahead open (9 ms, every 5 s: short tracks).
    const KINDS: [Kind; 4] = [
        Kind { ops: (25_000, 25_000), total: 25_000, split: false },
        Kind { ops: (28_000, 28_000), total: 28_000, split: false },
        Kind { ops: (2_000, 6_000), total: 55_000, split: true },
        Kind { ops: (9_000, 9_000), total: 9_000, split: false },
    ];
    /// The UI's load: decoder calls of 14–19 ms per 24-ms block, refills of
    /// 3–8 ms every other block (≈ 80 % of real time; the loop alone never
    /// runs dry: two blocks take at most 46 of their 48 ms); the UI asks
    /// again 5–60 ms after each answer.
    const SOAK: Case<'static> = Case {
        kinds: &KINDS,
        ui: &[0, 2, 1],
        think: (5_000, 60_000),
        bg: Some((3, 5_000_000)),
        load: Load { dec: (14_000, 19_000), fill: (3_000, 8_000), fill_every: 2, reload: 0 },
        until_us: 120_000_000,
        cheap: &[],
        cheap_us: 0,
        measure_all: false,
    };
    const SEEDS: [u32; 5] = [1, 2, 3, 0x5eed, 0xdead_beef];

    /// The player as measured in the soak (seed 1, `browse pictures`,
    /// 2026-11-25): decoder calls 4–9 ms (`pcm: … dec=6ms`), and the call
    /// after a chore ~5 ms slower (the chore's code evicted the decoder's):
    /// `chore: job:count 29/29 ms` within its budget, then `pcm: head -33`.
    const PLAYER: Case<'static> = Case {
        kinds: &PLAYER_KINDS,
        load: Load { dec: (4_000, 9_000), fill: (3_000, 8_000), fill_every: 2, reload: 5_000 },
        ..SOAK
    };
    /// As [`KINDS`], with the player's listing pieces: single card
    /// operations of up to 21 ms (`chore: job:count 31/28 ms u20`).
    const PLAYER_KINDS: [Kind; 4] = [
        Kind { ops: (25_000, 25_000), total: 25_000, split: false },
        Kind { ops: (28_000, 28_000), total: 28_000, split: false },
        Kind { ops: (5_000, 21_000), total: 55_000, split: true },
        Kind { ops: (9_000, 9_000), total: 9_000, split: false },
    ];

    /// Regression (2026-11-25): with a 10 ms margin a chore that kept to
    /// its budget still emptied the DAC ring on the player, because the
    /// decoder call after it reloads its code. The margin covers a slow
    /// call plus the reload.
    #[test]
    fn regress_margin_covers_the_reload_after_a_chore() {
        for seed in SEEDS {
            let f = model(&decide, PLAYER, seed);
            assert_eq!(f.chore_underruns + f.loop_underruns, 0, "seed {seed}: {f:?}");
            assert_eq!(f.starved, 0, "seed {seed}: {f:?}");
            assert!(f.answered > 300, "seed {seed}: {f:?}");
        }
    }

    /// Soak, 2026-11-25 (`mixed` seed 1, `browse pictures` during a
    /// 256-track shuffled MP3 queue,
    /// `tests/regress/soak/mixed-underrun-seed1-6.soak`): under UI load the
    /// headroom seldom reached 1.5 halves, so chores of 25–55 ms were forced
    /// in after 300 ms of waiting at any headroom (`pcm: head -93 …
    /// ub=serve:25ms`, `-265 … serve:28ms`, `-649 … serve:55ms`), each an
    /// underrun. A forced chore must run only when the headroom covers its
    /// need; a splittable one stops at its budget.
    #[test]
    fn regress_forced_chore_never_exceeds_headroom() {
        for seed in SEEDS {
            let f = model(&decide, SOAK, seed);
            assert_eq!(f.loop_underruns, 0, "seed {seed}: {f:?}");
            assert_eq!(f.chore_underruns, 0, "seed {seed}: {f:?}");
            assert_eq!(f.exceeded, 0, "seed {seed}: {f:?}");
            assert_eq!(f.starved, 0, "seed {seed}: {f:?}");
            // The soak's situation: mostly not roomy, the forced path taken.
            assert!(f.roomy_checks * 2 < f.checks, "seed {seed}: {f:?}");
            assert!(f.forced > 0, "seed {seed}: {f:?}");
            assert!(f.answered > 300, "seed {seed}: {f:?}");
            assert!(f.worst_latency < 2_500_000, "seed {seed}: {f:?}");
        }
    }

    /// The same load under the old rule: the model reproduces the soak's
    /// underruns (so the test above fails on it).
    #[test]
    fn old_rule_underruns_under_the_soak_load() {
        let mut dry = 0;
        for seed in SEEDS {
            let f = model(&old_rule, SOAK, seed);
            assert!(f.chore_underruns >= 1, "seed {seed}: {f:?}");
            dry += f.chore_underruns;
        }
        assert!(dry >= 10, "{dry}");
    }

    /// Bounded answers: from a light load to the loop at ~80 % of real
    /// time, every UI request (all of whose pieces fit 1.5 halves less the
    /// margin) is answered without the starvation escape and without an
    /// underrun, within the bound of the module docs: the background chore
    /// before it, then per piece the wait and the time the ring takes to
    /// fill. Heavier still (≈ 85 %: the margin for 19.5-ms decoder calls is
    /// 24 ms, so the 28-ms request fits only at the best block phases), the
    /// starvation escape answers it, still without an underrun.
    #[test]
    fn every_chore_that_fits_is_answered_in_bounded_time() {
        let fill = (3_000, 8_000);
        let loads = [
            (Load { dec: (2_000, 5_000), fill: (2_000, 5_000), fill_every: 4, reload: 0 }, 600_000),
            (Load { dec: (6_000, 10_000), fill, fill_every: 2, reload: 0 }, 800_000),
            (Load { dec: (10_000, 15_000), fill, fill_every: 2, reload: 0 }, 1_500_000),
            (SOAK.load, 2_500_000),
        ];
        for (load, bound) in loads {
            for seed in SEEDS {
                let f = model(&decide, Case { load, ..SOAK }, seed);
                assert_eq!(f.starved, 0, "{load:?} seed {seed}: {f:?}");
                assert_eq!(f.chore_underruns + f.loop_underruns, 0, "{load:?} seed {seed}: {f:?}");
                assert!(f.answered > 200, "{load:?} seed {seed}: {f:?}");
                assert!(f.worst_latency < bound, "{load:?} seed {seed}: {f:?}");
            }
        }
        for seed in SEEDS {
            let load = Load { dec: (15_000, 19_500), fill, fill_every: 2, reload: 0 };
            let f = model(&decide, Case { load, ..SOAK }, seed);
            assert_eq!(f.chore_underruns + f.loop_underruns, 0, "seed {seed}: {f:?}");
            assert!(f.worst_latency < u64::from(STARVE_US) + 3_000_000, "seed {seed}: {f:?}");
        }
    }

    /// A chore longer than any headroom (unsplittable 80 ms: the ring holds
    /// 64 ms) still gets its answer: by the starvation bound, at a roomy
    /// moment, logged as starved; the others are not held up for longer.
    #[test]
    fn a_chore_that_cannot_fit_is_still_answered() {
        let kinds = [Kind { ops: (80_000, 80_000), total: 80_000, split: false }, KINDS[2]];
        let c = Case {
            kinds: &kinds,
            ui: &[0, 1],
            think: (500_000, 900_000),
            bg: None,
            load: Load { dec: (3_000, 6_000), fill: (2_000, 5_000), fill_every: 2, reload: 0 },
            until_us: 20_000_000,
            ..SOAK
        };
        let f = model(&decide, c, 3);
        // The 80-ms chores starve, the listings between them do not.
        assert!(f.starved > 0 && f.answered + 1 >= 2 * f.starved, "{f:?}");
        assert!(f.worst_latency <= u64::from(STARVE_US) + 200_000, "{f:?}");
        // Each runs with the most headroom the ring gives (< 64 ms): it
        // underruns, once per piece; nothing else does.
        assert_eq!(f.chore_underruns, f.starved, "{f:?}");
        assert_eq!(f.loop_underruns, 0, "{f:?}");
    }

    /// Regression (BOOK-1/2b, 2026-11-27): with MP3 at 48 kHz playing, the
    /// player's headroom after every 1,152-frame block was 2,283 frames
    /// (`status` `due=15/34/2283/5956/0`), never the 2,304 of roomy, so a
    /// chore whose need (34 ms: Books' first, cold steps) no budget at that
    /// headroom covered (25.6 ms) never starved into running: Books froze
    /// while music played ("The book could not be read." over an open that
    /// never ran). After twice the starvation time it runs nearly roomy.
    #[test]
    fn regress_a_chore_starved_for_ever_below_a_roomy_dac() {
        let stuck = |waited_us| ask(2_283, 34_000, waited_us);
        assert_eq!(decide(&stuck(STARVE_US)), Verdict::Wait);
        assert_eq!(decide(&stuck(2 * STARVE_US - 1)), Verdict::Wait);
        let v = decide(&stuck(2 * STARVE_US));
        assert!(matches!(v, Verdict::Starved { budget_us } if budget_us > 20_000), "{v:?}");
        // Not at a low headroom, however long it waited.
        assert_eq!(decide(&ask(HALF, 34_000, 10 * STARVE_US)), Verdict::Wait);
        // A roomy moment still serves it after the first wait.
        assert!(matches!(decide(&ask(ROOMY, 34_000, STARVE_US)), Verdict::Starved { .. }));
    }

    /// Regression (AU-PERF, 2026-11-25): a request kind with a variant that
    /// reads no card (`want` while a folder plays only starts a listing
    /// job; in a music-library list it reads the library's window) learnt
    /// its need from the cheap one: five answers from RAM took the need of
    /// a 25-ms window down to ~6 ms, and the next card-reading one was
    /// admitted at a budget of a few ms (under the soak's load: after the
    /// 300-ms wait, at low headroom) and ran the DAC dry. A piece that did
    /// no card work now raises the need but never lowers it.
    #[test]
    fn regress_a_piece_without_card_work_does_not_lower_the_need() {
        let kinds = [Kind { ops: (25_000, 25_000), total: 25_000, split: false }];
        let c = Case {
            kinds: &kinds,
            ui: &[0],
            think: (5_000, 60_000),
            bg: None,
            cheap: &[true, true, true, true, true, false],
            cheap_us: 300,
            ..SOAK
        };
        // (Measured the old way this ran the ring dry on every seed with the
        // quarter-step decay; the 1/16 decay since also prevents it, so only
        // the fixed behaviour is asserted.)
        for seed in SEEDS {
            let f = model(&decide, c, seed);
            assert_eq!(f.chore_underruns + f.loop_underruns, 0, "seed {seed}: {f:?}");
            assert_eq!(f.exceeded, 0, "seed {seed}: {f:?}");
            assert!(f.answered > 300, "seed {seed}: {f:?}");
        }
        // The rule itself: no card work, shorter than the need: unchanged;
        // longer: raised; card work: measured as before.
        let mut k = Costs::<1>::new();
        k.observe(0, 30_000, 8_000);
        k.observe_piece(0, 300, 8_000, false);
        assert_eq!(k.need(0, 8_000), 30_000);
        k.observe_piece(0, 40_000, 8_000, false);
        assert_eq!(k.need(0, 8_000), 40_000);
        k.observe_piece(0, 8_000, 8_000, true);
        assert_eq!(k.need(0, 8_000), 38_000);
        let p = Piece::new(0, 10_000, 0).with_io(0x1_FFFF);
        assert!(!p.worked(0x1_FFFF) && p.worked(0x2_0000));
        assert_eq!(core::mem::size_of::<Option<Piece>>(), 28);
    }
}
