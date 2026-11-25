//! 4096 (GAME-3): sliding number tiles.
//!
//! A square board of 4 × 4, 5 × 5 or 6 × 6 cells. A slide moves every tile
//! as far as it goes towards one edge; two equal tiles that meet become one
//! of twice the value, at most once per tile per slide, nearest the edge
//! first. A slide that changes the board adds one new tile (a 2, or a 4
//! with the set chance) on a random empty cell. Reaching the goal tile
//! wins (the game may go on); the game is over when no slide changes the
//! board. Undo takes back up to the set number of slides, the new tile
//! with them (the generator is restored too, so the same slide brings the
//! same tile back).
//!
//! Tiles are stored as exponents: 1 = 2, 2 = 4, … 12 = 4096; 0 is empty.
//!
//! Invariants (tested in `tests/tiles.rs` and the `tiles_keys` fuzz
//! target): the tile sum grows by exactly the new tile in each slide;
//! every tile merges at most once per slide; the game is over exactly when
//! no slide would change the board.

use crate::save::{Reader, Writer};
use crate::{Button, Input, Rng};

/// Largest board side.
pub const MAX_N: usize = 6;
pub const CELLS: usize = MAX_N * MAX_N;
/// Undo steps at most.
pub const MAX_UNDO: usize = 3;
/// Largest exponent a tile may have in a save file (2^40, beyond any
/// reachable 6 × 6 tile).
pub const MAX_EXP: u8 = 40;
/// Save format version.
pub const VERSION: u8 = 1;

/// Rule settings, fixed for a game.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// Board side, 4 ..= 6.
    pub size: u8,
    /// Goal tile as an exponent (11 = 2048, 12 = 4096, 13 = 8192); 0 =
    /// endless.
    pub goal: u8,
    /// Chance that a new tile is a 4, per mille.
    pub four_per_mille: u16,
    /// Slides that can be taken back, 0 ..= 3.
    pub undo: u8,
}

impl Config {
    /// What suits a first game: 4 × 4, goal 4096, one 4 in ten, one undo.
    pub const DEFAULT: Config = Config { size: 4, goal: 12, four_per_mille: 100, undo: 1 };

    /// Values out of range replaced by the defaults.
    #[must_use]
    pub fn sanitised(self) -> Config {
        let d = Config::DEFAULT;
        Config {
            size: if (4..=6).contains(&self.size) { self.size } else { d.size },
            goal: if self.goal == 0 || (3..=20).contains(&self.goal) { self.goal } else { d.goal },
            four_per_mille: self.four_per_mille.min(1000),
            undo: self.undo.min(MAX_UNDO as u8),
        }
    }

    /// The score table's variant: one number per rule combination.
    #[must_use]
    pub fn variant(&self) -> u16 {
        u16::from(self.size - 4)
            | u16::from(self.goal & 0x1F) << 2
            | (self.four_per_mille.min(1000) / 50) << 7
            | u16::from(self.undo & 3) << 12
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::DEFAULT
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Up, Dir::Down, Dir::Left, Dir::Right];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Playing,
    /// The goal tile was made; waiting for "keep going".
    Won,
    /// No slide changes the board.
    Over,
}

/// One tile's way in a slide (for the animation and the invariant tests).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Path {
    pub from: u8,
    pub to: u8,
    /// Its exponent before the slide.
    pub exp: u8,
    /// It merged into the tile that got to `to` first.
    pub merged: bool,
}

/// What a slide did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Moved {
    pub dir: Dir,
    pub paths: [Path; CELLS],
    pub n: u8,
    /// The new tile: cell and exponent.
    pub spawn: Option<(u8, u8)>,
    /// Score gained (the values of the merged tiles).
    pub gained: u32,
    pub merges: u8,
}

impl Moved {
    #[must_use]
    pub fn paths(&self) -> &[Path] {
        &self.paths[..usize::from(self.n)]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Snap {
    cells: [u8; CELLS],
    score: u32,
    rng: Rng,
    moves: u32,
    phase: Phase,
    kept_going: bool,
}

/// A game of 4096.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tiles {
    cfg: Config,
    cells: [u8; CELLS],
    score: u32,
    rng: Rng,
    seed: u64,
    moves: u32,
    phase: Phase,
    /// The player chose to go on after the goal.
    kept_going: bool,
    undo: [Snap; MAX_UNDO],
    undo_n: u8,
}

impl Snap {
    const EMPTY: Snap = Snap {
        cells: [0; CELLS],
        score: 0,
        rng: Rng::new(0),
        moves: 0,
        phase: Phase::Playing,
        kept_going: false,
    };
}

fn pow2(e: u8) -> u64 {
    if e == 0 { 0 } else { 1u64 << e.min(63) }
}

impl Tiles {
    /// A new game: the empty board and two tiles.
    #[must_use]
    pub fn new(cfg: Config, seed: u64) -> Tiles {
        let mut t = Tiles::from_cells(cfg, seed, &[]);
        t.spawn();
        t.spawn();
        t
    }

    /// A game on a given board (row-major exponents, missing cells empty),
    /// no tiles added: tests and the simulator's `set:tiles=`.
    #[must_use]
    pub fn from_cells(cfg: Config, seed: u64, cells: &[u8]) -> Tiles {
        let cfg = cfg.sanitised();
        let mut t = Tiles {
            cfg,
            cells: [0; CELLS],
            score: 0,
            rng: Rng::new(seed),
            seed,
            moves: 0,
            phase: Phase::Playing,
            kept_going: false,
            undo: [Snap::EMPTY; MAX_UNDO],
            undo_n: 0,
        };
        let n = t.n();
        for (i, &e) in cells.iter().enumerate().take(n * n) {
            t.cells[i] = e.min(MAX_EXP);
        }
        t.update_phase();
        t
    }

    #[must_use]
    pub fn config(&self) -> Config {
        self.cfg
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Board side.
    #[must_use]
    pub fn n(&self) -> usize {
        usize::from(self.cfg.size)
    }

    /// Exponent at column `x`, row `y` (0 empty).
    #[must_use]
    pub fn at(&self, x: usize, y: usize) -> u8 {
        if x < self.n() && y < self.n() { self.cells[y * self.n() + x] } else { 0 }
    }

    /// The board, row-major, `n × n` cells.
    #[must_use]
    pub fn cells(&self) -> &[u8] {
        &self.cells[..self.n() * self.n()]
    }

    #[must_use]
    pub fn score(&self) -> u32 {
        self.score
    }

    #[must_use]
    pub fn moves(&self) -> u32 {
        self.moves
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    #[must_use]
    pub fn over(&self) -> bool {
        self.phase == Phase::Over
    }

    /// The largest tile's exponent.
    #[must_use]
    pub fn best_tile(&self) -> u8 {
        self.cells().iter().copied().max().unwrap_or(0)
    }

    /// Sum of all tile values.
    #[must_use]
    pub fn sum(&self) -> u64 {
        self.cells().iter().map(|&e| pow2(e)).sum()
    }

    /// Slides that can be taken back now.
    #[must_use]
    pub fn undo_left(&self) -> u8 {
        self.undo_n
    }

    /// The goal was reached in this game.
    #[must_use]
    pub fn goal_reached(&self) -> bool {
        self.cfg.goal != 0 && self.best_tile() >= self.cfg.goal
    }

    /// Cell index of the `k`-th cell of line `line` counted from the edge
    /// the tiles move towards.
    fn cell(&self, dir: Dir, line: usize, k: usize) -> usize {
        let n = self.n();
        let (x, y) = match dir {
            Dir::Left => (k, line),
            Dir::Right => (n - 1 - k, line),
            Dir::Up => (line, k),
            Dir::Down => (line, n - 1 - k),
        };
        y * n + x
    }

    /// The board after sliding towards `dir`, without a new tile: the
    /// cells and what happened. Does not change `self`.
    #[must_use]
    pub fn preview(&self, dir: Dir) -> ([u8; CELLS], Moved) {
        let n = self.n();
        let mut out = [0u8; CELLS];
        let mut m = Moved { dir, paths: [Path::default(); CELLS], n: 0, spawn: None, gained: 0, merges: 0 };
        for line in 0..n {
            // The tile last placed in this line, if it may still merge.
            let mut open: Option<(usize, u8)> = None;
            let mut next = 0;
            for k in 0..n {
                let from = self.cell(dir, line, k);
                let e = self.cells[from];
                if e == 0 {
                    continue;
                }
                let (to, merged) = match open {
                    Some((slot, oe)) if oe == e => {
                        open = None;
                        let to = self.cell(dir, line, slot);
                        out[to] = e + 1;
                        m.gained = m.gained.saturating_add(u32::try_from(pow2(e + 1)).unwrap_or(u32::MAX));
                        m.merges += 1;
                        (to, true)
                    }
                    _ => {
                        let to = self.cell(dir, line, next);
                        out[to] = e;
                        open = Some((next, e));
                        next += 1;
                        (to, false)
                    }
                };
                if let Some(p) = m.paths.get_mut(usize::from(m.n)) {
                    *p = Path { from: from as u8, to: to as u8, exp: e, merged };
                    m.n += 1;
                }
            }
        }
        (out, m)
    }

    /// Whether sliding towards `dir` would change the board.
    #[must_use]
    pub fn can_slide(&self, dir: Dir) -> bool {
        self.preview(dir).0[..self.n() * self.n()] != *self.cells()
    }

    /// Whether any slide would change the board.
    #[must_use]
    pub fn can_move(&self) -> bool {
        let n = self.n();
        for y in 0..n {
            for x in 0..n {
                let e = self.at(x, y);
                if e == 0 || (x + 1 < n && self.at(x + 1, y) == e) || (y + 1 < n && self.at(x, y + 1) == e) {
                    return true;
                }
            }
        }
        false
    }

    fn snap(&self) -> Snap {
        Snap {
            cells: self.cells,
            score: self.score,
            rng: self.rng,
            moves: self.moves,
            phase: self.phase,
            kept_going: self.kept_going,
        }
    }

    /// Slide towards `dir`. `None` (nothing changes, no tile is added) if
    /// the slide would not move or merge anything, or the game is over.
    pub fn slide(&mut self, dir: Dir) -> Option<Moved> {
        if self.phase == Phase::Over {
            return None;
        }
        let (cells, mut m) = self.preview(dir);
        if cells[..self.n() * self.n()] == *self.cells() {
            return None;
        }
        if self.cfg.undo > 0 {
            let keep = usize::from(self.cfg.undo);
            if usize::from(self.undo_n) == keep {
                self.undo.copy_within(1..keep, 0);
                self.undo_n -= 1;
            }
            self.undo[usize::from(self.undo_n)] = self.snap();
            self.undo_n += 1;
        }
        if self.phase == Phase::Won {
            // Sliding on is keeping going.
            self.kept_going = true;
            self.phase = Phase::Playing;
        }
        self.cells = cells;
        self.score = self.score.saturating_add(m.gained);
        self.moves = self.moves.saturating_add(1);
        m.spawn = self.spawn();
        self.update_phase();
        Some(m)
    }

    /// Take the last slide back. False if there is none.
    pub fn undo(&mut self) -> bool {
        let Some(n) = self.undo_n.checked_sub(1) else { return false };
        let s = core::mem::replace(&mut self.undo[usize::from(n)], Snap::EMPTY);
        self.undo_n = n;
        self.cells = s.cells;
        self.score = s.score;
        self.rng = s.rng;
        self.moves = s.moves;
        self.phase = s.phase;
        self.kept_going = s.kept_going;
        true
    }

    /// After the goal: go on playing.
    pub fn keep_going(&mut self) {
        if self.phase == Phase::Won {
            self.kept_going = true;
            self.phase = Phase::Playing;
        }
    }

    fn update_phase(&mut self) {
        if !self.can_move() {
            self.phase = Phase::Over;
        } else if self.goal_reached() && !self.kept_going && self.phase == Phase::Playing {
            self.phase = Phase::Won;
        }
    }

    /// A new tile on a random empty cell: (cell, exponent).
    fn spawn(&mut self) -> Option<(u8, u8)> {
        let n = self.n() * self.n();
        let empty = self.cells[..n].iter().filter(|&&e| e == 0).count();
        if empty == 0 {
            return None;
        }
        let k = self.rng.below(u32::try_from(empty).unwrap_or(1)) as usize;
        let e = if self.rng.chance(self.cfg.four_per_mille) { 2 } else { 1 };
        let i = self.cells[..n].iter().enumerate().filter(|(_, e)| **e == 0).nth(k).map(|(i, _)| i)?;
        self.cells[i] = e;
        Some((i as u8, e))
    }

    /// A key: the four directions slide, OK takes back a slide (or keeps
    /// going after the goal). Returns the slide if one happened.
    pub fn input(&mut self, i: Input) -> TilesOut {
        let Input::Press(b) = i else { return TilesOut::None };
        let dir = match b {
            Button::Up => Dir::Up,
            Button::Down => Dir::Down,
            Button::Left => Dir::Left,
            Button::Right => Dir::Right,
            Button::Ok if self.phase == Phase::Won => {
                self.keep_going();
                return TilesOut::KeptGoing;
            }
            Button::Ok => return if self.undo() { TilesOut::Undone } else { TilesOut::None },
            Button::Menu => return TilesOut::None,
        };
        match self.slide(dir) {
            Some(m) => TilesOut::Moved(m),
            None => TilesOut::None,
        }
    }

    pub fn encode(&self, w: &mut Writer<'_>) {
        w.u8(self.cfg.size);
        w.u8(self.cfg.goal);
        w.u16(self.cfg.four_per_mille);
        w.u8(self.cfg.undo);
        w.u64(self.seed);
        let snap = |w: &mut Writer<'_>, s: &Snap| {
            w.bytes(&s.cells);
            w.u32(s.score);
            w.u64(s.rng.state());
            w.u32(s.moves);
            w.u8(s.phase as u8);
            w.bool(s.kept_going);
        };
        snap(w, &self.snap());
        w.u8(self.undo_n);
        for s in &self.undo[..usize::from(self.undo_n)] {
            snap(w, s);
        }
    }

    /// Read a game written by [`Tiles::encode`]; `None` for any value out
    /// of range.
    pub fn decode(r: &mut Reader<'_>) -> Option<Tiles> {
        let cfg = Config { size: r.u8()?, goal: r.u8()?, four_per_mille: r.u16()?, undo: r.u8()? };
        if cfg.sanitised() != cfg {
            return None;
        }
        let seed = r.u64()?;
        let n = usize::from(cfg.size);
        let snap = |r: &mut Reader<'_>| -> Option<Snap> {
            let cells: [u8; CELLS] = r.array()?;
            let board_ok = cells.iter().enumerate().all(|(i, &e)| e <= MAX_EXP && (i < n * n || e == 0));
            let s = Snap {
                cells,
                score: r.u32()?,
                rng: Rng::new(r.u64()?),
                moves: r.u32()?,
                phase: match r.u8()? {
                    0 => Phase::Playing,
                    1 => Phase::Won,
                    2 => Phase::Over,
                    _ => return None,
                },
                kept_going: r.bool()?,
            };
            board_ok.then_some(s)
        };
        let cur = snap(r)?;
        let undo_n = r.u8()?;
        if undo_n > cfg.undo {
            return None;
        }
        let mut t = Tiles::from_cells(cfg, seed, &[]);
        for k in 0..usize::from(undo_n) {
            t.undo[k] = snap(r)?;
        }
        t.undo_n = undo_n;
        t.cells = cur.cells;
        t.score = cur.score;
        t.rng = cur.rng;
        t.moves = cur.moves;
        t.phase = cur.phase;
        t.kept_going = cur.kept_going;
        // The phase must be the one the board implies.
        let over = !t.can_move();
        if over != (t.phase == Phase::Over) {
            return None;
        }
        Some(t)
    }
}

/// What a key did in 4096.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TilesOut {
    None,
    Moved(Moved),
    Undone,
    KeptGoing,
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn board(cells: &[u8]) -> Tiles {
        Tiles::from_cells(Config { undo: 3, ..Config::DEFAULT }, 7, cells)
    }

    #[test]
    fn merges_nearest_the_edge_first_and_once() {
        // 2 2 2 2 → 4 4 . . ; 4 4 8 . → 8 8 . .
        let t = board(&[1, 1, 1, 1, 2, 2, 3, 0, 1, 0, 0, 1, 1, 2, 1, 2]);
        let (c, m) = t.preview(Dir::Left);
        assert_eq!(&c[..16], &[2, 2, 0, 0, 3, 3, 0, 0, 2, 0, 0, 0, 1, 2, 1, 2]);
        assert_eq!(m.merges, 4);
        assert_eq!(m.gained, 4 + 4 + 8 + 4);
        let (c, _) = t.preview(Dir::Right);
        assert_eq!(&c[..16], &[0, 0, 2, 2, 0, 0, 3, 3, 0, 0, 0, 2, 1, 2, 1, 2]);
        // A merged tile does not merge again in the same slide: 4 2 2 → 4 4.
        let t = board(&[2, 1, 1, 0]);
        assert_eq!(&t.preview(Dir::Left).0[..4], &[2, 2, 0, 0]);
    }

    #[test]
    fn slide_adds_one_tile_and_undo_restores_everything() {
        let mut t = board(&[1, 1, 0, 0]);
        let before = t.clone();
        let m = t.slide(Dir::Left).unwrap();
        assert!(m.spawn.is_some());
        assert_eq!(t.sum(), 4 + pow2(m.spawn.unwrap().1));
        assert_eq!(t.score(), 4);
        assert!(t.undo());
        assert_eq!(t.cells(), before.cells());
        assert_eq!(t.score(), 0);
        let again = t.slide(Dir::Left).unwrap();
        assert_eq!(again.spawn, m.spawn, "the generator came back too");
        assert!(t.undo());
        assert!(!t.undo(), "one slide taken back, none left");
        assert!(t.slide(Dir::Up).is_some() || t.slide(Dir::Down).is_some());
    }

    #[test]
    fn no_change_no_tile() {
        let mut t = board(&[1, 0, 0, 0]);
        assert!(t.slide(Dir::Left).is_none());
        assert!(t.slide(Dir::Up).is_none());
        assert_eq!(t.sum(), 2);
        assert_eq!(t.moves(), 0);
    }

    #[test]
    fn over_exactly_when_stuck_and_goal() {
        let stuck = [1, 2, 1, 2, 2, 1, 2, 1, 1, 2, 1, 2, 2, 1, 2, 1];
        let t = board(&stuck);
        assert!(t.over());
        assert!(Dir::ALL.iter().all(|&d| !t.can_slide(d)));
        let mut almost = stuck;
        almost[15] = 0;
        let t = board(&almost);
        assert!(!t.over());
        let cfg = Config { goal: 3, ..Config::DEFAULT };
        let mut t = Tiles::from_cells(cfg, 1, &[2, 2, 0, 0]);
        t.slide(Dir::Left).unwrap();
        assert_eq!(t.phase(), Phase::Won);
        assert_eq!(t.input(Input::Press(Button::Ok)), TilesOut::KeptGoing);
        assert_eq!(t.phase(), Phase::Playing);
        let endless = Tiles::from_cells(Config { goal: 0, ..Config::DEFAULT }, 1, &[20]);
        assert_eq!(endless.phase(), Phase::Playing);
    }

    #[test]
    fn sizes_and_variants() {
        for size in 4..=6u8 {
            let t = Tiles::new(Config { size, ..Config::DEFAULT }, 3);
            assert_eq!(t.cells().len(), usize::from(size * size));
            assert_eq!(t.cells().iter().filter(|&&e| e != 0).count(), 2);
        }
        let mut seen = std::collections::BTreeSet::new();
        for size in 4..=6 {
            for goal in [0, 11, 12, 13] {
                for four in [0, 100, 250, 500] {
                    for undo in 0..=3 {
                        assert!(seen.insert(Config { size, goal, four_per_mille: four, undo }.variant()));
                    }
                }
            }
        }
        assert_eq!(Config { size: 9, goal: 1, four_per_mille: 2000, undo: 7 }.sanitised().size, 4);
    }
}
