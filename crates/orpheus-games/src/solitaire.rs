//! Solitaire (GAME-2): Klondike with one deck.
//!
//! Thirteen piles: the stock, the waste, four foundations and seven
//! tableau columns. The deal puts 1 … 7 cards in the columns, the top one
//! face up, and the other 24 in the stock. Cards go to the foundations by
//! suit from the ace up; on the tableau in descending runs of alternating
//! colour, a king (or a run starting with one) on an empty column. The
//! stock turns one or three cards to the waste; an empty stock takes the
//! waste back (a pass) as often as the setting allows. A column's top card
//! is turned up as soon as it is uncovered.
//!
//! Own rules and numbers: the scoring tables below ([`Scoring`]), the
//! safe auto-move rule, the best-effort deal check ([`Config::winnable`]:
//! randomised greedy play-outs with a bounded effort, no exhaustive
//! search).
//!
//! The cards of all piles live in one array of 52, pile after pile in pile
//! order; `len` gives each pile's length and `down` the face-down cards at
//! the bottom of each column (the stock is all face down, the other piles
//! all face up). A move takes the top `n` cards of a pile onto another
//! (stock and waste turn them over: reversed). Every move is recorded in a
//! short history (the last [`HISTORY`] moves) and taken back exactly.
//!
//! Invariants (tested in `tests/solitaire.rs` and the `solitaire_keys`
//! fuzz target): the 52 cards are always all there, each once; only legal
//! moves change the piles; undo restores the piles, the faces and the
//! counters exactly; a column's top card is face up.

use crate::save::{Reader, Writer};
use crate::{Button, Input, Rng};

/// Save format version.
pub const VERSION: u8 = 1;
/// Moves that can be taken back (older ones are forgotten).
pub const HISTORY: usize = 100;

/// Piles.
pub const STOCK: u8 = 0;
pub const WASTE: u8 = 1;
/// The four foundations: `FOUNDATION .. FOUNDATION + 4`.
pub const FOUNDATION: u8 = 2;
/// The seven columns: `TABLEAU .. TABLEAU + 7`.
pub const TABLEAU: u8 = 6;
pub const PILES: usize = 13;
pub const CARDS: usize = 52;
/// Face-down cards of a fresh deal (0 + 1 + … + 6).
pub const DEALT_DOWN: u32 = 21;

/// A card: `suit * 13 + rank`, rank 0 = ace … 12 = king; suits in the
/// order spades, hearts, clubs, diamonds (odd suits are red).
pub type Card = u8;

#[must_use]
pub const fn rank(c: Card) -> u8 {
    c % 13
}

#[must_use]
pub const fn suit(c: Card) -> u8 {
    c / 13
}

#[must_use]
pub const fn red(c: Card) -> bool {
    suit(c) & 1 == 1
}

#[must_use]
pub const fn is_foundation(p: u8) -> bool {
    p >= FOUNDATION && p < TABLEAU
}

#[must_use]
pub const fn is_tableau(p: u8) -> bool {
    p >= TABLEAU && (p as usize) < PILES
}

/// How a game is scored.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Scoring {
    /// Points: 10 per card on a foundation, 5 per card turned up in the
    /// tableau and per card played from the waste to the tableau, −15 for
    /// a card taken back from a foundation, a pass through the stock −50
    /// (drawing one) or −20 (drawing three); a win adds 2 per second under
    /// 15 minutes. Shown never below 0.
    #[default]
    Standard = 0,
    /// Cumulative ("casino"): a deal costs 52, each card on a foundation
    /// earns 5; the bank is carried from game to game (the app's score
    /// table).
    Casino,
    /// No score; the time of won games is the best.
    None,
}

impl Scoring {
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Scoring> {
        match v {
            0 => Some(Scoring::Standard),
            1 => Some(Scoring::Casino),
            2 => Some(Scoring::None),
            _ => None,
        }
    }
}

/// What a casino deal costs and a card on a foundation earns.
pub const CASINO_DEAL: i32 = 52;
pub const CASINO_CARD: i32 = 5;

/// Rule settings, fixed for a game (the two assists may change at once:
/// [`Solitaire::set_assists`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// Cards turned per draw: 1 or 3.
    pub draw: u8,
    /// Passes through the stock: 0 = unlimited, else 1 or 3.
    pub passes: u8,
    pub scoring: Scoring,
    /// Cards that are safe to play go to the foundations by themselves.
    pub auto_move: bool,
    /// Once the stock and waste are empty and every card is face up, the
    /// game plays the rest to the foundations.
    pub auto_complete: bool,
    /// Deal only games the deal check could win (best effort).
    pub winnable: bool,
}

impl Config {
    /// What suits a first game: draw one, unlimited passes, standard
    /// points, both assists, a winnable deal.
    pub const DEFAULT: Config = Config {
        draw: 1,
        passes: 0,
        scoring: Scoring::Standard,
        auto_move: true,
        auto_complete: true,
        winnable: true,
    };

    #[must_use]
    pub fn sanitised(self) -> Config {
        Config {
            draw: if self.draw == 3 { 3 } else { 1 },
            passes: if matches!(self.passes, 1 | 3) { self.passes } else { 0 },
            ..self
        }
    }

    /// The score table's variant: draw, passes, scoring and the deal
    /// check (the assists do not change what a result means).
    #[must_use]
    pub fn variant(&self) -> u16 {
        let passes = match self.passes {
            1 => 1,
            3 => 2,
            _ => 0,
        };
        u16::from(self.draw == 3) | passes << 1 | (self.scoring as u16) << 3 | u16::from(self.winnable) << 5
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::DEFAULT
    }
}

/// One move as recorded: from, to, how many cards, whether the source
/// column's new top card was turned up, whether the game made it by
/// itself (auto-move, auto-complete: taken back with the move before it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move {
    pub from: u8,
    pub to: u8,
    pub n: u8,
    pub flipped: bool,
    pub auto: bool,
}

impl Move {
    const fn pack(self) -> u16 {
        self.from as u16 & 0xF
            | (self.to as u16 & 0xF) << 4
            | (self.n as u16 & 0x1F) << 8
            | (self.flipped as u16) << 13
            | (self.auto as u16) << 14
    }

    const fn unpack(v: u16) -> Move {
        Move {
            from: (v & 0xF) as u8,
            to: (v >> 4 & 0xF) as u8,
            n: (v >> 8 & 0x1F) as u8,
            flipped: v >> 13 & 1 == 1,
            auto: v >> 14 & 1 == 1,
        }
    }

    fn valid(self) -> bool {
        usize::from(self.from) < PILES && usize::from(self.to) < PILES && self.from != self.to && self.n >= 1
    }
}

/// The cards and their faces: everything the rules look at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Board {
    cards: [Card; CARDS],
    len: [u8; PILES],
    down: [u8; 7],
    /// Times the waste went back to the stock.
    passes: u8,
}

impl Board {
    /// The deal of `seed`: the cards shuffled, 1 … 7 to the columns (the
    /// last of each face up), the rest to the stock.
    #[must_use]
    pub fn deal(seed: u64) -> Board {
        let mut deck = [0u8; CARDS];
        for (i, c) in deck.iter_mut().enumerate() {
            *c = i as u8;
        }
        let mut rng = Rng::new(seed);
        for i in (1..CARDS).rev() {
            let j = rng.below(i as u32 + 1) as usize;
            deck.swap(i, j);
        }
        let mut b = Board { cards: [0; CARDS], len: [0; PILES], down: [0; 7], passes: 0 };
        // Columns from the deck's front, the stock from the rest.
        let mut k = 0;
        let mut cols = [[0u8; 7]; 7];
        for row in 0..7 {
            for c in cols.iter_mut().skip(row) {
                c[row] = deck[k];
                k += 1;
            }
        }
        let mut at = 0;
        for &c in &deck[k..] {
            b.cards[at] = c;
            at += 1;
        }
        b.len[usize::from(STOCK)] = (CARDS - k) as u8;
        for (col, c) in cols.iter().enumerate() {
            for &card in &c[..=col] {
                b.cards[at] = card;
                at += 1;
            }
            b.len[usize::from(TABLEAU) + col] = col as u8 + 1;
            b.down[col] = col as u8;
        }
        b
    }

    /// A board from its piles (bottom card first) and the columns'
    /// face-down counts; `None` unless it is a valid board with all 52
    /// cards (tests, simulator positions).
    #[must_use]
    pub fn from_piles(piles: [&[Card]; PILES], down: [u8; 7], passes: u8) -> Option<Board> {
        let mut b = Board { cards: [0; CARDS], len: [0; PILES], down, passes };
        let mut at = 0;
        for (p, pile) in piles.iter().enumerate() {
            for &c in *pile {
                *b.cards.get_mut(at)? = c;
                at += 1;
            }
            b.len[p] = u8::try_from(pile.len()).ok()?;
        }
        (at == CARDS && b.valid()).then_some(b)
    }

    fn start(&self, p: u8) -> usize {
        self.len[..usize::from(p)].iter().map(|&l| usize::from(l)).sum()
    }

    /// The cards of pile `p`, bottom first.
    #[must_use]
    pub fn pile(&self, p: u8) -> &[Card] {
        let s = self.start(p);
        &self.cards[s..s + usize::from(self.len[usize::from(p)])]
    }

    #[must_use]
    pub fn len(&self, p: u8) -> usize {
        usize::from(self.len[usize::from(p)])
    }

    #[must_use]
    pub fn top(&self, p: u8) -> Option<Card> {
        self.pile(p).last().copied()
    }

    /// Face-down cards at the bottom of pile `p` (all of the stock).
    #[must_use]
    pub fn down(&self, p: u8) -> usize {
        if p == STOCK {
            self.len(STOCK)
        } else if is_tableau(p) {
            usize::from(self.down[usize::from(p - TABLEAU)])
        } else {
            0
        }
    }

    /// Face-up cards on top of pile `p`.
    #[must_use]
    pub fn face_up(&self, p: u8) -> usize {
        self.len(p) - self.down(p)
    }

    #[must_use]
    pub fn passes(&self) -> u8 {
        self.passes
    }

    /// Cards on the foundations.
    #[must_use]
    pub fn on_foundations(&self) -> u32 {
        (FOUNDATION..TABLEAU).map(|p| self.len(p) as u32).sum()
    }

    /// Face-down cards in the columns.
    #[must_use]
    pub fn face_down(&self) -> u32 {
        self.down.iter().map(|&d| u32::from(d)).sum()
    }

    #[must_use]
    pub fn won(&self) -> bool {
        self.on_foundations() == CARDS as u32
    }

    /// Every card that is left is face up and the stock and waste are
    /// empty: the rest plays itself.
    #[must_use]
    pub fn settled(&self) -> bool {
        self.len(STOCK) == 0 && self.len(WASTE) == 0 && self.face_down() == 0 && !self.won()
    }

    /// The top `n` cards of `from` onto `to` (reversed: turned over).
    fn shift(&mut self, from: u8, to: u8, n: usize, reverse: bool) {
        let (a, b) = (usize::from(from), usize::from(to));
        let a_end = self.start(from) + usize::from(self.len[a]);
        let b_end = self.start(to) + usize::from(self.len[b]);
        let at = if from < to {
            self.cards[a_end - n..b_end].rotate_left(n);
            b_end - n
        } else {
            self.cards[b_end..a_end].rotate_right(n);
            b_end
        };
        if reverse {
            self.cards[at..at + n].reverse();
        }
        self.len[a] -= n as u8;
        self.len[b] += n as u8;
    }

    /// `c` may go onto foundation `p`.
    #[must_use]
    pub fn fits_foundation(&self, c: Card, p: u8) -> bool {
        match self.top(p) {
            None => rank(c) == 0,
            Some(t) => suit(t) == suit(c) && rank(c) == rank(t) + 1,
        }
    }

    /// `c` may go onto column `p` (its top is face up when there is one).
    #[must_use]
    pub fn fits_column(&self, c: Card, p: u8) -> bool {
        match self.top(p) {
            None => rank(c) == 12,
            Some(t) => red(t) != red(c) && rank(t) == rank(c) + 1,
        }
    }

    /// Moving the top `n` cards of `from` onto `to` is legal (not the
    /// stock's draw: [`Board::can_draw`]).
    #[must_use]
    pub fn legal(&self, from: u8, to: u8, n: usize) -> bool {
        if from == to || usize::from(from) >= PILES || usize::from(to) >= PILES || n == 0 {
            return false;
        }
        if from == STOCK || to == STOCK || to == WASTE || n > self.face_up(from) {
            return false;
        }
        let pile = self.pile(from);
        let c = pile[pile.len() - n];
        if is_foundation(to) {
            return n == 1 && !is_foundation(from) && self.fits_foundation(c, to);
        }
        // Onto a column: only a column gives several cards, and they are
        // a run (always true of a column's face-up cards; checked anyway).
        if n > 1 && !is_tableau(from) {
            return false;
        }
        let run = &pile[pile.len() - n..];
        let is_run = run.windows(2).all(|w| red(w[0]) != red(w[1]) && rank(w[0]) == rank(w[1]) + 1);
        is_run && self.fits_column(c, to)
    }

    /// The draw is possible: cards in the stock, or the waste may go back.
    #[must_use]
    pub fn can_draw(&self, cfg: &Config) -> bool {
        self.len(STOCK) > 0 || (self.len(WASTE) > 0 && self.recycle_left(cfg))
    }

    /// Another pass may start.
    #[must_use]
    pub fn recycle_left(&self, cfg: &Config) -> bool {
        cfg.passes == 0 || self.passes + 1 < cfg.passes
    }

    /// Apply a legal move; turns the source column's new top card up.
    /// Returns the move as recorded.
    fn apply(&mut self, from: u8, to: u8, n: usize, auto: bool) -> Move {
        self.shift(from, to, n, false);
        let mut flipped = false;
        if is_tableau(from) {
            let i = usize::from(from - TABLEAU);
            if self.down[i] > 0 && usize::from(self.down[i]) == self.len(from) {
                self.down[i] -= 1;
                flipped = true;
            }
        }
        Move { from, to, n: n as u8, flipped, auto }
    }

    /// Turn cards from the stock, or take the waste back. `None` if
    /// neither is possible.
    fn draw(&mut self, cfg: &Config, auto: bool) -> Option<Move> {
        let s = self.len(STOCK);
        if s > 0 {
            let n = s.min(usize::from(cfg.draw));
            self.shift(STOCK, WASTE, n, true);
            return Some(Move { from: STOCK, to: WASTE, n: n as u8, flipped: false, auto });
        }
        let w = self.len(WASTE);
        if w > 0 && self.recycle_left(cfg) {
            self.shift(WASTE, STOCK, w, true);
            self.passes += 1;
            return Some(Move { from: WASTE, to: STOCK, n: w as u8, flipped: false, auto });
        }
        None
    }

    /// Take a move back.
    fn unapply(&mut self, m: Move) {
        match (m.from, m.to) {
            (STOCK, WASTE) => self.shift(WASTE, STOCK, usize::from(m.n), true),
            (WASTE, STOCK) => {
                self.shift(STOCK, WASTE, usize::from(m.n), true);
                self.passes = self.passes.saturating_sub(1);
            }
            _ => {
                if m.flipped && is_tableau(m.from) {
                    self.down[usize::from(m.from - TABLEAU)] += 1;
                }
                self.shift(m.to, m.from, usize::from(m.n), false);
            }
        }
    }

    /// The foundation `c` may go to, if any.
    #[must_use]
    pub fn foundation_for(&self, c: Card) -> Option<u8> {
        (FOUNDATION..TABLEAU).find(|&p| self.fits_foundation(c, p))
    }

    /// `c` can go to the foundations without being needed in the tableau
    /// any more: aces and twos, or a card whose two lower cards of the
    /// other colour are both on the foundations.
    #[must_use]
    pub fn safe(&self, c: Card) -> bool {
        let r = rank(c);
        if r <= 1 {
            return true;
        }
        // Ranks on the foundations per suit (0 = none, 1 = ace …).
        let mut have = [0u8; 4];
        for p in FOUNDATION..TABLEAU {
            if let Some(t) = self.top(p) {
                have[usize::from(suit(t))] = rank(t) + 1;
            }
        }
        (0..4u8).filter(|&s| (s & 1 == 1) != red(c)).all(|s| have[usize::from(s)] >= r)
    }

    /// A safe card on a waste or column top that can go up now.
    fn safe_move(&self) -> Option<(u8, u8)> {
        core::iter::once(WASTE).chain(TABLEAU..PILES as u8).find_map(|p| {
            let c = self.top(p).filter(|_| self.face_up(p) > 0)?;
            let f = self.foundation_for(c)?;
            self.safe(c).then_some((p, f))
        })
    }

    /// Any waste or column top that can go up now.
    fn any_foundation_move(&self) -> Option<(u8, u8)> {
        core::iter::once(WASTE).chain(TABLEAU..PILES as u8).find_map(|p| {
            let c = self.top(p).filter(|_| self.face_up(p) > 0)?;
            Some((p, self.foundation_for(c)?))
        })
    }

    /// The column move of `from` onto `to` that changes something: it
    /// turns a card up, empties a column onto a card, or uncovers a
    /// face-up card that can then go to a foundation. Moving a whole
    /// column to an empty one, or splitting a run for nothing, is not.
    fn useful_column_move(&self, from: u8, to: u8) -> Option<usize> {
        let up = self.face_up(from);
        for n in (1..=up).rev() {
            if !self.legal(from, to, n) {
                continue;
            }
            if n == up {
                let below = self.down(from) > 0;
                let empties_onto_card = self.down(from) == 0 && self.len(to) > 0;
                if below || empties_onto_card {
                    return Some(n);
                }
            } else {
                let pile = self.pile(from);
                let exposed = pile[pile.len() - n - 1];
                if self.foundation_for(exposed).is_some() {
                    return Some(n);
                }
            }
        }
        None
    }

    /// Some move other than the stock's changes something (see
    /// [`Board::useful_column_move`]; a card taken back from a foundation
    /// does not count).
    fn has_useful_move(&self) -> bool {
        if self.any_foundation_move().is_some() {
            return true;
        }
        if let Some(c) = self.top(WASTE)
            && (TABLEAU..PILES as u8).any(|t| self.fits_column(c, t))
        {
            return true;
        }
        (TABLEAU..PILES as u8)
            .any(|f| (TABLEAU..PILES as u8).any(|t| f != t && self.useful_column_move(f, t).is_some()))
    }

    /// No move can change the game any more: none of the piles' cards
    /// can be played and the stock, gone through as often as it may be,
    /// turns up no card that can.
    #[must_use]
    pub fn stuck(&self, cfg: &Config) -> bool {
        if self.won() || self.has_useful_move() {
            return false;
        }
        let mut b = *self;
        // Two whole passes show every card the stock can offer.
        let mut draws = 2 * (CARDS + 2);
        while draws > 0 && b.draw(cfg, false).is_some() {
            draws -= 1;
            if let Some(c) = b.top(WASTE)
                && (b.foundation_for(c).is_some() || (TABLEAU..PILES as u8).any(|t| b.fits_column(c, t)))
            {
                return false;
            }
        }
        true
    }

    /// Every card once, faces and runs as the rules leave them (the
    /// invariants; `false` names a corrupt board).
    #[must_use]
    pub fn valid(&self) -> bool {
        let mut seen = [false; CARDS];
        for &c in &self.cards {
            if usize::from(c) >= CARDS || seen[usize::from(c)] {
                return false;
            }
            seen[usize::from(c)] = true;
        }
        if self.len.iter().map(|&l| usize::from(l)).sum::<usize>() != CARDS {
            return false;
        }
        for p in FOUNDATION..TABLEAU {
            let pile = self.pile(p);
            if pile.len() > 13
                || !pile.iter().enumerate().all(|(i, &c)| rank(c) == i as u8 && suit(c) == suit(pile[0]))
            {
                return false;
            }
        }
        for p in TABLEAU..PILES as u8 {
            let d = self.down(p);
            let pile = self.pile(p);
            if d > pile.len() || (!pile.is_empty() && d == pile.len()) {
                return false;
            }
            let up = &pile[d..];
            if !up.windows(2).all(|w| red(w[0]) != red(w[1]) && rank(w[0]) == rank(w[1]) + 1) {
                return false;
            }
        }
        true
    }
}

// ------------------------------------------------------------ deal check

/// Effort of the deal check: deals tried, play-outs per deal, moves per
/// play-out. Bounded so a new game takes a bounded time on the player.
pub const CHECK_DEALS: u32 = 16;
pub const CHECK_PLAYOUTS: u32 = 4;
pub const CHECK_MOVES: u32 = 400;

/// One randomised greedy play-out from `b`: foundations first, then
/// moves that turn a card up (the column with most face-down cards
/// first), then waste to column, then a draw; stops when won, when a whole
/// pass through the stock changed nothing, or after `max` moves. Pushes
/// the moves into `out` while it has room (tests replay them).
fn playout(mut b: Board, cfg: &Config, rng: &mut Rng, max: u32, mut out: Option<&mut [Move]>) -> (bool, u32) {
    let mut since_progress = 0usize;
    let mut done = 0u32;
    while done < max {
        if b.won() {
            return (true, done);
        }
        // Foundations: anything that goes up (randomly among them).
        let mut cand = [(0u8, 0u8, 0u8); 16];
        let mut nc = 0;
        for p in core::iter::once(WASTE).chain(TABLEAU..PILES as u8) {
            if let Some(c) = b.top(p).filter(|_| b.face_up(p) > 0)
                && let Some(f) = b.foundation_for(c)
                && nc < cand.len()
            {
                cand[nc] = (p, f, 1);
                nc += 1;
            }
        }
        if nc == 0 {
            // Column moves that turn a card up or free a column for a king.
            let mut best = 0usize;
            for f in TABLEAU..PILES as u8 {
                let up = b.face_up(f);
                if up == 0 {
                    continue;
                }
                let d = b.down(f);
                let k = b.pile(f)[b.len(f) - up];
                // A king already at the bottom has nothing to uncover.
                if d == 0 && rank(k) == 12 {
                    continue;
                }
                for t in TABLEAU..PILES as u8 {
                    if f != t && b.legal(f, t, up) && (d > 0 || b.len(t) > 0) {
                        let weight = d + 1;
                        if weight > best {
                            best = weight;
                            nc = 0;
                        }
                        if weight == best && nc < cand.len() {
                            cand[nc] = (f, t, up as u8);
                            nc += 1;
                        }
                    }
                }
            }
        }
        if nc == 0
            && let Some(c) = b.top(WASTE)
        {
            for t in TABLEAU..PILES as u8 {
                if b.fits_column(c, t) && nc < cand.len() {
                    cand[nc] = (WASTE, t, 1);
                    nc += 1;
                }
            }
        }
        let m = if nc > 0 {
            let (f, t, n) = cand[rng.below(nc as u32) as usize];
            since_progress = 0;
            b.apply(f, t, usize::from(n), false)
        } else {
            since_progress += 1;
            // A pass and a bit without any other move: nothing changes.
            if since_progress > b.len(STOCK) + b.len(WASTE) + 2 {
                return (false, done);
            }
            match b.draw(cfg, false) {
                Some(m) => m,
                None => return (false, done),
            }
        };
        if let Some(slot) = out.as_deref_mut().and_then(|o| o.get_mut(done as usize)) {
            *slot = m;
        }
        done += 1;
    }
    (b.won(), done)
}

/// The deal check: some play-out of `b` wins.
#[must_use]
pub fn winnable(b: &Board, cfg: &Config, seed: u64) -> bool {
    let mut rng = Rng::new(seed ^ 0x5011_7A12_E000_0000);
    (0..CHECK_PLAYOUTS).any(|_| playout(*b, cfg, &mut rng, CHECK_MOVES, None).0)
}

/// A winning line for `b` found by the deal check, written into `out`:
/// its length, or `None`.
pub fn solution(b: &Board, cfg: &Config, seed: u64, out: &mut [Move]) -> Option<usize> {
    let mut rng = Rng::new(seed ^ 0x5011_7A12_E000_0000);
    for _ in 0..CHECK_PLAYOUTS {
        let (won, n) = playout(*b, cfg, &mut rng, CHECK_MOVES, Some(out));
        if won {
            return Some(n as usize);
        }
    }
    None
}

/// The deal of a new game: `seed` itself, or with the deal check the first
/// of up to [`CHECK_DEALS`] deals (`seed`, then seeds drawn from it) the
/// check wins. Returns the deal's seed and whether it was checked.
#[must_use]
pub fn choose_deal(cfg: &Config, seed: u64) -> (u64, bool) {
    if !cfg.winnable {
        return (seed, false);
    }
    let mut rng = Rng::new(seed);
    let mut s = seed;
    for _ in 0..CHECK_DEALS {
        if winnable(&Board::deal(s), cfg, s) {
            return (s, true);
        }
        s = rng.next_u64();
    }
    // Best effort: the first deal.
    (seed, false)
}

// ------------------------------------------------------------ the game

/// Logic steps between two cards of the auto-complete.
pub const AUTO_STEPS: u32 = 4;

/// What a key or a step did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SolOut {
    None,
    /// The cursor moved, cards were picked up or put back.
    Cursor,
    /// Cards moved (the board changed).
    Moved,
    /// The cards held cannot go there.
    Refused,
    Undone,
}

/// A game of Solitaire.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Solitaire {
    cfg: Config,
    /// The deal's seed (the deal check's choice).
    seed: u64,
    checked: bool,
    b: Board,
    /// Score counters: cards played from the waste to a column, cards
    /// taken back from a foundation.
    w2t: u16,
    f2t: u16,
    moves: u32,
    steps: u32,
    hist: [u16; HISTORY],
    hist_n: u8,
    /// The cursor: a pile, and on a column how many cards from the top.
    cur: u8,
    depth: u8,
    /// Cards picked up: pile, count (0 = none).
    held_from: u8,
    held_n: u8,
    /// OK went long (undo): its release is not a pick or a place.
    ok_long: bool,
    /// Auto-complete waits after an undo until the next move.
    hold_auto: bool,
    auto_timer: u8,
}

/// The cursor's ring (Prev/Next): the top row left to right, then the
/// columns.
const RING: [u8; PILES] = [STOCK, WASTE, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

/// The top-row pile above column `c` (0–6) and the column below a top-row
/// pile.
#[must_use]
pub const fn above(c: u8) -> u8 {
    match c {
        0 => STOCK,
        1 | 2 => WASTE,
        _ => FOUNDATION + c - 3,
    }
}

#[must_use]
pub const fn below(p: u8) -> u8 {
    match p {
        STOCK => TABLEAU,
        WASTE => TABLEAU + 1,
        _ => TABLEAU + 3 + (p - FOUNDATION),
    }
}

impl Solitaire {
    /// A new game: the deal of `seed` (with the deal check, the first deal
    /// from it the check wins).
    #[must_use]
    pub fn new(cfg: Config, seed: u64) -> Solitaire {
        let cfg = cfg.sanitised();
        let (deal, checked) = choose_deal(&cfg, seed);
        let mut b = Board::deal(deal);
        // Safe cards of the deal go up at once with auto-move (part of the
        // deal: nothing to take back).
        if cfg.auto_move {
            while let Some((p, f)) = b.safe_move() {
                b.apply(p, f, 1, true);
            }
        }
        Solitaire::with_board(cfg, deal, checked, b)
    }

    /// A game on a given board (tests, the simulator).
    #[must_use]
    pub fn with_board(cfg: Config, seed: u64, checked: bool, b: Board) -> Solitaire {
        Solitaire {
            cfg: cfg.sanitised(),
            seed,
            checked,
            b,
            w2t: 0,
            f2t: 0,
            moves: 0,
            steps: 0,
            hist: [0; HISTORY],
            hist_n: 0,
            cur: TABLEAU,
            depth: 1,
            held_from: 0,
            held_n: 0,
            ok_long: false,
            hold_auto: false,
            auto_timer: 0,
        }
    }

    /// A board where every card but the given ones is on the foundations
    /// in order and those stand in the columns (tests, simulator scripts:
    /// an almost finished game). `cols[i]`: cards of column `i`, face up.
    #[must_use]
    pub fn nearly_done(cfg: Config, cols: &[&[Card]]) -> Solitaire {
        let mut b = Board { cards: [0; CARDS], len: [0; PILES], down: [0; 7], passes: 0 };
        let mut left = [false; CARDS];
        for c in cols.iter().flat_map(|c| c.iter()) {
            left[usize::from(*c)] = true;
        }
        let mut at = 0;
        for s in 0..4u8 {
            let mut n = 0;
            for r in 0..13u8 {
                let c = s * 13 + r;
                if left[usize::from(c)] {
                    break;
                }
                b.cards[at] = c;
                at += 1;
                n += 1;
            }
            b.len[usize::from(FOUNDATION + s)] = n;
        }
        // Cards above a gap in a suit go to the columns too.
        let mut extra = [0u8; CARDS];
        let mut ne = 0;
        for s in 0..4u8 {
            let n = b.len[usize::from(FOUNDATION + s)];
            for r in n..13 {
                let c = s * 13 + r;
                if !left[usize::from(c)] {
                    extra[ne] = c;
                    ne += 1;
                }
            }
        }
        for (i, col) in cols.iter().enumerate().take(7) {
            for &c in col.iter() {
                b.cards[at] = c;
                at += 1;
            }
            b.len[usize::from(TABLEAU) + i] = col.len() as u8;
        }
        // Leftovers (none in well-formed scripts) into the stock's place
        // is not possible after the columns: put them on the last column.
        for &c in &extra[..ne] {
            b.cards[at] = c;
            at += 1;
            b.len[PILES - 1] += 1;
        }
        Solitaire::with_board(cfg, 0, false, b)
    }

    #[must_use]
    pub fn config(&self) -> Config {
        self.cfg
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The deal check won this deal.
    #[must_use]
    pub fn checked(&self) -> bool {
        self.checked
    }

    #[must_use]
    pub fn board(&self) -> &Board {
        &self.b
    }

    #[must_use]
    pub fn moves(&self) -> u32 {
        self.moves
    }

    /// Logic steps played (the timer).
    #[must_use]
    pub fn steps(&self) -> u32 {
        self.steps
    }

    #[must_use]
    pub fn won(&self) -> bool {
        self.b.won()
    }

    /// No move can change the game any more.
    #[must_use]
    pub fn stuck(&self) -> bool {
        self.b.stuck(&self.cfg)
    }

    /// The game is over: won, or no move left.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.won() || self.stuck()
    }

    /// The cursor: pile, and the cards it covers on a column.
    #[must_use]
    pub fn cursor(&self) -> (u8, u8) {
        (self.cur, self.depth)
    }

    /// The cards picked up: pile and count.
    #[must_use]
    pub fn held(&self) -> Option<(u8, u8)> {
        (self.held_n > 0).then_some((self.held_from, self.held_n))
    }

    /// Passes through the stock still allowed after this one (`None`:
    /// unlimited).
    #[must_use]
    pub fn passes_left(&self) -> Option<u8> {
        (self.cfg.passes != 0).then(|| self.cfg.passes.saturating_sub(self.b.passes + 1))
    }

    /// Moves that can be taken back now.
    #[must_use]
    pub fn undo_left(&self) -> usize {
        usize::from(self.hist_n)
    }

    /// The recorded moves, oldest first.
    pub fn history(&self) -> impl Iterator<Item = Move> + '_ {
        self.hist[..usize::from(self.hist_n)].iter().map(|&h| Move::unpack(h))
    }

    /// The last recorded move.
    #[must_use]
    pub fn last_move(&self) -> Option<Move> {
        self.hist_n.checked_sub(1).map(|i| Move::unpack(self.hist[usize::from(i)]))
    }

    /// The assists may change during a game.
    pub fn set_assists(&mut self, auto_move: bool, auto_complete: bool) {
        self.cfg.auto_move = auto_move;
        self.cfg.auto_complete = auto_complete;
    }

    /// The score (see [`Scoring`]): standard points (never below 0), or
    /// this game's casino result (−52 + 5 per card up), or 0.
    #[must_use]
    pub fn score(&self) -> i32 {
        let up = self.b.on_foundations() as i32;
        match self.cfg.scoring {
            Scoring::Standard => {
                let turned = (DEALT_DOWN - self.b.face_down().min(DEALT_DOWN)) as i32;
                let pass = if self.cfg.draw == 1 { 50 } else { 20 };
                let mut s = 10 * up + 5 * turned + 5 * i32::from(self.w2t)
                    - 5 * i32::from(self.f2t)
                    - pass * i32::from(self.b.passes);
                if self.won() {
                    let secs = crate::steps_to_ms(self.steps) / 1000;
                    s += 2 * 900i32.saturating_sub(i32::try_from(secs).unwrap_or(i32::MAX)).max(0);
                }
                s.max(0)
            }
            Scoring::Casino => CASINO_CARD * up - CASINO_DEAL,
            Scoring::None => 0,
        }
    }

    fn push(&mut self, m: Move) {
        if usize::from(self.hist_n) == HISTORY {
            self.hist.copy_within(1.., 0);
            self.hist_n -= 1;
        }
        self.hist[usize::from(self.hist_n)] = m.pack();
        self.hist_n += 1;
    }

    fn count(&mut self, m: Move, sign: i8) {
        let d = |v: u16| if sign > 0 { v.saturating_add(1) } else { v.saturating_sub(1) };
        if m.from == WASTE && is_tableau(m.to) {
            self.w2t = d(self.w2t);
        }
        if is_foundation(m.from) && is_tableau(m.to) {
            self.f2t = d(self.f2t);
        }
        if !m.auto {
            self.moves = if sign > 0 { self.moves.saturating_add(1) } else { self.moves.saturating_sub(1) };
        }
    }

    fn record(&mut self, m: Move) {
        self.count(m, 1);
        self.push(m);
    }

    /// Safe cards to the foundations (auto-move), each recorded as made
    /// by the game.
    fn auto_moves(&mut self) {
        if !self.cfg.auto_move {
            return;
        }
        while let Some((p, f)) = self.b.safe_move() {
            let m = self.b.apply(p, f, 1, true);
            self.record(m);
        }
    }

    /// Move the top `n` cards of `from` onto `to` if legal (a player's
    /// move: auto-moves follow).
    pub fn try_move(&mut self, from: u8, to: u8, n: usize) -> bool {
        if !self.b.legal(from, to, n) {
            return false;
        }
        let m = self.b.apply(from, to, n, false);
        self.record(m);
        self.after_move();
        true
    }

    /// The stock: turn cards, or take the waste back.
    pub fn draw(&mut self) -> bool {
        let Some(m) = self.b.draw(&self.cfg, false) else { return false };
        self.record(m);
        self.after_move();
        true
    }

    fn after_move(&mut self) {
        self.hold_auto = false;
        self.auto_moves();
        self.clamp_cursor();
    }

    /// Take back the last move and the moves the game made after it.
    pub fn undo(&mut self) -> bool {
        let mut any = false;
        while let Some(i) = self.hist_n.checked_sub(1) {
            let m = Move::unpack(core::mem::take(&mut self.hist[usize::from(i)]));
            self.hist_n = i;
            self.b.unapply(m);
            self.count(m, -1);
            any = true;
            if !m.auto {
                self.cur = if m.from == STOCK || m.to == STOCK { STOCK } else { m.from };
                break;
            }
        }
        if any {
            self.clear_held();
            self.depth = 1;
            self.hold_auto = true;
            self.clamp_cursor();
        }
        any
    }

    /// Every card that can go to a foundation goes (Menu): the cursor's
    /// card if it can, else all of them; one undo takes them back.
    pub fn to_foundations(&mut self) -> bool {
        let p = if self.held_n == 1 { self.held_from } else { self.cur };
        if self.held_n <= 1
            && let Some(c) = self.b.top(p).filter(|_| self.b.face_up(p) > 0 && !is_foundation(p))
            && let Some(f) = self.b.foundation_for(c)
        {
            self.clear_held();
            return self.try_move(p, f, 1);
        }
        let Some((p, f)) = self.b.any_foundation_move() else { return false };
        self.clear_held();
        let m = self.b.apply(p, f, 1, false);
        self.record(m);
        while let Some((p, f)) = self.b.any_foundation_move() {
            let m = self.b.apply(p, f, 1, true);
            self.record(m);
        }
        self.after_move();
        true
    }

    fn clear_held(&mut self) {
        self.held_from = 0;
        self.held_n = 0;
    }

    fn clamp_cursor(&mut self) {
        let up = self.b.face_up(self.cur);
        let max = if is_tableau(self.cur) { up.max(1) } else { 1 };
        self.depth = self.depth.clamp(1, u8::try_from(max).unwrap_or(1));
    }

    fn ring_step(&mut self, d: i8) {
        let i = RING.iter().position(|&p| p == self.cur).unwrap_or(0) as i8;
        let n = PILES as i8;
        self.cur = RING[((i + d).rem_euclid(n)) as usize];
        self.depth = 1;
    }

    fn pick_or_place(&mut self) -> SolOut {
        if self.held_n > 0 {
            if self.cur == self.held_from {
                self.clear_held();
                return SolOut::Cursor;
            }
            let (from, n) = (self.held_from, usize::from(self.held_n));
            self.clear_held();
            if self.try_move(from, self.cur, n) {
                return SolOut::Moved;
            }
            self.held_from = from;
            self.held_n = n as u8;
            return SolOut::Refused;
        }
        if self.cur == STOCK {
            return if self.draw() { SolOut::Moved } else { SolOut::None };
        }
        let up = self.b.face_up(self.cur);
        if up == 0 {
            return SolOut::None;
        }
        let n = if is_tableau(self.cur) { usize::from(self.depth).min(up) } else { 1 };
        self.held_from = self.cur;
        self.held_n = n as u8;
        SolOut::Cursor
    }

    /// A key. Prev/Next move along the piles, Up/Down choose how many
    /// cards of a column (and change rows at its ends), OK (on release)
    /// picks up and puts down, the stock turns cards, Menu sends cards to
    /// the foundations, OK held takes back a move.
    pub fn input(&mut self, i: Input) -> SolOut {
        match i {
            Input::Press(Button::Ok) => {
                self.ok_long = false;
                SolOut::None
            }
            Input::Long(Button::Ok) => {
                self.ok_long = true;
                if self.undo() { SolOut::Undone } else { SolOut::None }
            }
            Input::Release(Button::Ok) => {
                if core::mem::take(&mut self.ok_long) {
                    return SolOut::None;
                }
                self.pick_or_place()
            }
            Input::Press(Button::Left) => {
                self.ring_step(-1);
                SolOut::Cursor
            }
            Input::Press(Button::Right) => {
                self.ring_step(1);
                SolOut::Cursor
            }
            Input::Press(Button::Up) => {
                if is_tableau(self.cur) {
                    let up = self.b.face_up(self.cur);
                    if self.held_n == 0 && usize::from(self.depth) < up {
                        self.depth += 1;
                    } else {
                        self.cur = above(self.cur - TABLEAU);
                        self.depth = 1;
                    }
                    SolOut::Cursor
                } else {
                    SolOut::None
                }
            }
            Input::Press(Button::Down) => {
                if is_tableau(self.cur) {
                    if self.depth > 1 {
                        self.depth -= 1;
                        return SolOut::Cursor;
                    }
                    SolOut::None
                } else {
                    self.cur = below(self.cur);
                    self.depth = 1;
                    SolOut::Cursor
                }
            }
            Input::Press(Button::Menu) => {
                if self.to_foundations() {
                    SolOut::Moved
                } else {
                    SolOut::None
                }
            }
            _ => SolOut::None,
        }
    }

    /// Put back cards held (Back, a pause).
    pub fn drop_held(&mut self) -> bool {
        let had = self.held_n > 0;
        self.clear_held();
        had
    }

    /// The auto-complete is running.
    #[must_use]
    pub fn completing(&self) -> bool {
        self.cfg.auto_complete && !self.hold_auto && self.b.settled()
    }

    /// One logic step: the timer, and the auto-complete's next card.
    pub fn step(&mut self) -> SolOut {
        if !self.won() {
            self.steps = self.steps.saturating_add(1);
        }
        if !self.completing() {
            self.auto_timer = 0;
            return SolOut::None;
        }
        self.auto_timer += 1;
        if u32::from(self.auto_timer) < AUTO_STEPS {
            return SolOut::None;
        }
        self.auto_timer = 0;
        // The lowest card that can go up first (it looks like a deal).
        let mut best: Option<(u8, u8, u8)> = None;
        for p in TABLEAU..PILES as u8 {
            if let Some(c) = self.b.top(p)
                && let Some(f) = self.b.foundation_for(c)
                && best.is_none_or(|b| rank(c) < b.2)
            {
                best = Some((p, f, rank(c)));
            }
        }
        let Some((p, f, _)) = best else { return SolOut::None };
        self.clear_held();
        let m = self.b.apply(p, f, 1, true);
        self.record(m);
        self.clamp_cursor();
        SolOut::Moved
    }

    /// Everything but the timer, the cursor and the history: what an undo
    /// restores (tests).
    #[must_use]
    pub fn position(&self) -> (Board, u16, u16, u32) {
        (self.b, self.w2t, self.f2t, self.moves)
    }

    pub fn encode(&self, w: &mut Writer<'_>) {
        w.u8(self.cfg.draw);
        w.u8(self.cfg.passes);
        w.u8(self.cfg.scoring as u8);
        w.u8(u8::from(self.cfg.auto_move)
            | u8::from(self.cfg.auto_complete) << 1
            | u8::from(self.cfg.winnable) << 2);
        w.u64(self.seed);
        w.bool(self.checked);
        w.bytes(&self.b.cards);
        w.bytes(&self.b.len);
        w.bytes(&self.b.down);
        w.u8(self.b.passes);
        w.u16(self.w2t);
        w.u16(self.f2t);
        w.u32(self.moves);
        w.u32(self.steps);
        w.u8(self.cur);
        w.u8(self.depth);
        w.bool(self.hold_auto);
        w.u8(self.hist_n);
        for &h in &self.hist[..usize::from(self.hist_n)] {
            w.u16(h);
        }
    }

    /// Read a game written by [`Solitaire::encode`]; `None` for anything
    /// out of range or a board that breaks the rules.
    pub fn decode(r: &mut Reader<'_>) -> Option<Solitaire> {
        let (draw, passes, scoring, flags) = (r.u8()?, r.u8()?, r.u8()?, r.u8()?);
        let cfg = Config {
            draw,
            passes,
            scoring: Scoring::from_u8(scoring)?,
            auto_move: flags & 1 != 0,
            auto_complete: flags & 2 != 0,
            winnable: flags & 4 != 0,
        };
        if cfg.sanitised() != cfg || flags > 7 {
            return None;
        }
        let seed = r.u64()?;
        let checked = r.bool()?;
        let b = Board { cards: r.array()?, len: r.array()?, down: r.array()?, passes: r.u8()? };
        if !b.valid() || (cfg.passes != 0 && b.passes >= cfg.passes) {
            return None;
        }
        let mut s = Solitaire::with_board(cfg, seed, checked, b);
        s.w2t = r.u16()?;
        s.f2t = r.u16()?;
        s.moves = r.u32()?;
        s.steps = r.u32()?;
        s.cur = r.u8()?;
        s.depth = r.u8()?;
        s.hold_auto = r.bool()?;
        let n = r.u8()?;
        if usize::from(s.cur) >= PILES || usize::from(n) > HISTORY {
            return None;
        }
        for k in 0..usize::from(n) {
            let h = r.u16()?;
            if !Move::unpack(h).valid() || Move::unpack(h).pack() != h {
                return None;
            }
            s.hist[k] = h;
        }
        s.hist_n = n;
        s.clamp_cursor();
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn game(seed: u64) -> Solitaire {
        Solitaire::new(Config { winnable: false, ..Config::DEFAULT }, seed)
    }

    #[test]
    fn deal_layout() {
        let b = Board::deal(42);
        assert!(b.valid());
        assert_eq!(b.len(STOCK), 24);
        for c in 0..7u8 {
            assert_eq!(b.len(TABLEAU + c), usize::from(c) + 1);
            assert_eq!(b.face_up(TABLEAU + c), 1);
        }
        assert_eq!(b.face_down(), DEALT_DOWN);
        assert_ne!(Board::deal(42), Board::deal(43));
    }

    #[test]
    fn shift_both_ways_keeps_cards() {
        let mut b = Board::deal(7);
        let before = b;
        b.shift(STOCK, WASTE, 3, true);
        assert_eq!(b.len(WASTE), 3);
        assert_eq!(b.top(WASTE), Some(before.pile(STOCK)[21]));
        b.shift(WASTE, STOCK, 3, true);
        assert_eq!(b, before);
        b.shift(TABLEAU + 6, FOUNDATION, 2, false);
        assert_eq!(b.pile(FOUNDATION), &before.pile(TABLEAU + 6)[5..]);
        b.shift(FOUNDATION, TABLEAU + 6, 2, false);
        assert_eq!(b, before);
    }

    #[test]
    fn rules() {
        let b = Solitaire::nearly_done(Config::DEFAULT, &[&[12], &[25, 11], &[], &[], &[], &[], &[]]).b;
        // Column 0: K♠; column 1: K♥ Q♠ (Q♠ on K♥: alternating).
        assert!(b.valid());
        assert!(b.fits_column(12, TABLEAU + 2), "king to an empty column");
        assert!(!b.fits_column(11, TABLEAU + 2));
        assert!(b.legal(TABLEAU + 1, TABLEAU + 2, 2), "the run moves");
        assert!(!b.legal(TABLEAU + 1, TABLEAU, 1), "Q♠ on K♠: same colour");
        assert!(b.legal(TABLEAU + 1, FOUNDATION, 1), "Q♠ on J♠");
        assert!(!b.legal(TABLEAU, FOUNDATION + 1, 1), "K♠ on hearts");
        assert!(b.safe(11), "Q♠ with both red queens up");
        assert!(!b.safe(25), "K♥ while J♠ is the top of spades");
    }

    #[test]
    fn draw_three_and_passes() {
        let cfg = Config { draw: 3, passes: 3, winnable: false, ..Config::DEFAULT };
        let mut g = Solitaire::new(cfg, 5);
        for _ in 0..8 {
            assert!(g.draw());
        }
        assert_eq!(g.b.len(STOCK), 0);
        assert_eq!(g.passes_left(), Some(2));
        assert!(g.draw(), "first pass back");
        assert_eq!(g.passes_left(), Some(1));
        for _ in 0..8 {
            g.draw();
        }
        assert!(g.draw(), "second pass back");
        assert_eq!(g.passes_left(), Some(0));
        for _ in 0..8 {
            g.draw();
        }
        assert!(!g.draw(), "no third pass back");
        assert!(g.b.valid());
    }

    #[test]
    fn undo_is_exact() {
        let mut g = game(11);
        for _ in 0..30 {
            let before = g.position();
            if g.draw() {
                assert!(g.undo());
                assert_eq!(g.position(), before);
                g.draw();
            }
        }
        let mut g = Solitaire::nearly_done(Config::DEFAULT, &[&[12], &[25, 11], &[], &[], &[], &[], &[]]);
        let before = g.position();
        assert!(g.try_move(TABLEAU + 1, TABLEAU + 2, 2));
        assert!(g.undo());
        assert_eq!(g.position(), before);
    }

    #[test]
    fn auto_complete_wins() {
        let mut g = Solitaire::nearly_done(Config::DEFAULT, &[&[12], &[25, 11], &[38], &[51], &[], &[], &[]]);
        assert!(g.completing());
        for _ in 0..100 {
            g.step();
        }
        assert!(g.won());
        assert!(g.score() > 0);
    }

    #[test]
    fn deal_check_finds_wins_and_lines_replay() {
        let cfg = Config::DEFAULT;
        let mut found = 0;
        for seed in 0..20u64 {
            let b = Board::deal(seed);
            let mut line = [Move { from: 0, to: 0, n: 0, flipped: false, auto: false }; CHECK_MOVES as usize];
            if let Some(n) = solution(&b, &cfg, seed, &mut line) {
                found += 1;
                let mut r = b;
                for m in &line[..n] {
                    let again = if m.from == STOCK || m.to == STOCK {
                        r.draw(&cfg, false).unwrap()
                    } else {
                        assert!(r.legal(m.from, m.to, usize::from(m.n)), "{m:?}");
                        r.apply(m.from, m.to, usize::from(m.n), false)
                    };
                    assert_eq!(again, *m);
                    assert!(r.valid());
                }
                assert!(r.won());
            }
        }
        assert!(found >= 3, "the check wins some deals ({found}/20)");
    }

    #[test]
    fn round_trip() {
        let mut g = game(3);
        g.draw();
        g.draw();
        let mut buf = [0u8; 512];
        let n = crate::save::seal(&mut buf, 3, VERSION, |w| g.encode(w)).unwrap();
        let (_, _, p) = crate::save::open(&buf[..n]).unwrap();
        let mut r = Reader::new(p);
        assert_eq!(Solitaire::decode(&mut r), Some(g));
        assert!(r.done());
    }

    #[test]
    fn variants_differ() {
        let mut seen = std::collections::BTreeSet::new();
        for draw in [1, 3] {
            for passes in [0, 1, 3] {
                for scoring in [Scoring::Standard, Scoring::Casino, Scoring::None] {
                    for winnable in [false, true] {
                        let c = Config { draw, passes, scoring, winnable, ..Config::DEFAULT };
                        assert!(seen.insert(c.variant()));
                    }
                }
            }
        }
    }
}
