//! Blocks (GAME-1): the falling-tetromino puzzle.
//!
//! A well 10 cells wide and 20 high (two more rows above it, hidden, where
//! pieces appear). Pieces of four cells fall one row per gravity period;
//! the player shifts them sideways, rotates them, drops them faster (soft
//! drop) or at once (hard drop), and may keep one aside (hold). A piece that
//! rests on the stack or the floor locks after the lock delay; full rows
//! disappear after a short flash, and the rows above move down.
//!
//! The rules here are this project's own: the rotation is a plain quarter
//! turn of the piece's box (3 × 3, 4 × 4 for the long piece; the square
//! does not turn), with a short list of kicks (one cell left, right, up,
//! and two for the long piece) tried when the turned piece does not fit;
//! the gravity and score tables are below. Pieces come from a shuffled bag
//! of all seven, or each at random.
//!
//! Modes: endless (until the stack reaches the top), 40 lines (as fast as
//! possible) and a two-minute score attack.
//!
//! Invariants (`Blocks::check`, tested in `tests/blocks.rs` and the
//! `blocks_keys` fuzz target): the falling piece never overlaps the stack
//! and never leaves the well; no full row stays in the well; the level
//! follows the lines; line and score counts match an independent replay of
//! the locked pieces.

use crate::save::{Reader, Writer};
use crate::{Button, Input, Rng, ms_to_steps};

/// Well width and visible height in cells.
pub const W: usize = 10;
pub const H: usize = 20;
/// Rows above the visible well where pieces appear.
pub const HIDDEN: usize = 2;
pub const ROWS: usize = H + HIDDEN;
/// Save format version.
pub const VERSION: u8 = 1;
pub const MAX_LEVEL: u8 = 20;
/// Lines of the 40-lines mode.
pub const LINES_GOAL: u32 = 40;
/// Length of the two-minute mode in steps.
pub const ATTACK_STEPS: u32 = 120 * crate::STEP_HZ;
/// Steps a cleared row flashes before it goes.
pub const CLEAR_STEPS: u8 = 12;
/// Soft drop: at least one row every this many steps (30 rows/s).
const SOFT_STEPS: u16 = 2;
/// Shifts and turns of a resting piece that restart its lock delay.
const MAX_RESETS: u8 = 15;
/// Queue capacity: three previews and a fresh bag.
const QCAP: usize = 14;

/// Steps per row of gravity for levels 1 ..= 20 (own table: about 0.8 s
/// per row at level 1, one row per step at 20).
const GRAVITY: [u8; MAX_LEVEL as usize] =
    [48, 42, 37, 32, 28, 24, 20, 17, 14, 11, 9, 8, 7, 6, 5, 4, 3, 3, 2, 1];
/// Points for clearing 0–4 rows with one piece, times the level.
pub const CLEAR_POINTS: [u32; 5] = [0, 100, 250, 450, 800];
/// Points per consecutive clearing piece after the first, times the level.
pub const COMBO_POINTS: u32 = 50;
/// Points per row of soft drop and of hard drop.
pub const SOFT_POINTS: u32 = 1;
pub const HARD_POINTS: u32 = 2;

/// Rotation kicks (x, y; y grows downwards), tried in order. The first
/// is no kick.
const KICKS: [(i8, i8); 6] = [(0, 0), (-1, 0), (1, 0), (0, -1), (-1, -1), (1, -1)];
const KICKS_LONG: [(i8, i8); 8] = [(0, 0), (-1, 0), (1, 0), (0, -1), (-1, -1), (1, -1), (-2, 0), (2, 0)];

/// The seven pieces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Piece {
    I = 0,
    O,
    T,
    S,
    Z,
    J,
    L,
}

impl Piece {
    pub const ALL: [Piece; 7] = [Piece::I, Piece::O, Piece::T, Piece::S, Piece::Z, Piece::J, Piece::L];

    #[must_use]
    pub fn from_u8(v: u8) -> Option<Piece> {
        Piece::ALL.get(usize::from(v)).copied()
    }

    /// Side of the box the piece turns in.
    #[must_use]
    pub const fn box_size(self) -> i8 {
        match self {
            Piece::I => 4,
            Piece::O => 2,
            _ => 3,
        }
    }

    const fn base(self) -> [(i8, i8); 4] {
        match self {
            Piece::I => [(0, 1), (1, 1), (2, 1), (3, 1)],
            Piece::O => [(0, 0), (1, 0), (0, 1), (1, 1)],
            Piece::T => [(1, 0), (0, 1), (1, 1), (2, 1)],
            Piece::S => [(1, 0), (2, 0), (0, 1), (1, 1)],
            Piece::Z => [(0, 0), (1, 0), (1, 1), (2, 1)],
            Piece::J => [(0, 0), (0, 1), (1, 1), (2, 1)],
            Piece::L => [(2, 0), (0, 1), (1, 1), (2, 1)],
        }
    }

    /// The four cells in its box after `rot` quarter turns clockwise.
    #[must_use]
    pub fn cells(self, rot: u8) -> [(i8, i8); 4] {
        let n = self.box_size();
        let mut c = self.base();
        for _ in 0..rot % 4 {
            c = c.map(|(x, y)| (n - 1 - y, x));
        }
        c
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Mode {
    #[default]
    Endless = 0,
    /// Clear 40 lines; the time counts.
    Lines40,
    /// Score as much as possible in two minutes.
    Attack,
}

impl Mode {
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Mode> {
        [Mode::Endless, Mode::Lines40, Mode::Attack].get(usize::from(v)).copied()
    }
}

/// Rule and handling settings, fixed for a game (handling can be changed
/// on resume: [`Blocks::set_handling`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    pub mode: Mode,
    /// 1 ..= 15.
    pub start_level: u8,
    /// Bag of seven (each piece once per seven) or each piece at random.
    pub bag: bool,
    pub hold: bool,
    pub lock_ms: u16,
    /// Auto-shift delay and repeat interval (0 = straight to the wall).
    pub das_ms: u16,
    pub arr_ms: u16,
    pub kicks: bool,
}

impl Config {
    /// What suits a first game.
    pub const DEFAULT: Config = Config {
        mode: Mode::Endless,
        start_level: 1,
        bag: true,
        hold: true,
        lock_ms: 500,
        das_ms: 167,
        arr_ms: 33,
        kicks: true,
    };

    #[must_use]
    pub fn sanitised(self) -> Config {
        Config {
            start_level: self.start_level.clamp(1, 15),
            lock_ms: self.lock_ms.clamp(100, 2000),
            das_ms: self.das_ms.clamp(17, 1000),
            arr_ms: self.arr_ms.min(500),
            ..self
        }
    }

    /// The score table's variant: mode, randomiser, hold, kicks.
    #[must_use]
    pub fn variant(&self) -> u16 {
        self.mode as u16 | u16::from(self.bag) << 2 | u16::from(self.hold) << 3 | u16::from(self.kicks) << 4
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::DEFAULT
    }
}

/// The falling piece: its box's top-left cell in well rows (0 = the top
/// hidden row).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Active {
    pub piece: Piece,
    pub rot: u8,
    pub x: i8,
    pub y: i8,
}

impl Active {
    /// Its cells in well coordinates.
    #[must_use]
    pub fn cells(&self) -> [(i8, i8); 4] {
        self.piece.cells(self.rot).map(|(cx, cy)| (self.x + cx, self.y + cy))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Playing,
    /// The stack reached the top.
    Over,
    /// The mode's goal was met (40 lines, two minutes played).
    Done,
}

/// A piece locked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Lock {
    pub at: Active,
    pub cleared: u8,
    pub hard: bool,
}

/// What an input or a step did that the app may want to know (sound,
/// score records).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StepOut {
    pub lock: Option<Lock>,
    /// The game ended (over or done) now.
    pub ended: bool,
}

/// A game of Blocks.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Blocks {
    cfg: Config,
    /// Row-major, row 0 = top hidden row; 0 empty, else piece + 1.
    well: [[u8; W]; ROWS],
    active: Option<Active>,
    hold: Option<Piece>,
    hold_used: bool,
    queue: [u8; QCAP],
    qlen: u8,
    rng: Rng,
    seed: u64,
    level: u8,
    lines: u32,
    score: u32,
    /// The part of `score` from soft and hard drops.
    drop_points: u32,
    combo: u8,
    /// Pieces locked, and pieces brought into play (spawns and holds).
    pieces: u32,
    spawned: u32,
    steps: u32,
    fall: u16,
    lock: u16,
    resets: u8,
    lowest: i8,
    clear_mask: u32,
    clear_timer: u8,
    /// Buttons held (input edges seen).
    held: u8,
    /// Horizontal direction being auto-shifted (−1, 0, 1) and steps since
    /// its press.
    shift: i8,
    das: u16,
    /// `spawned` when Down went down: holding it hard-drops that piece
    /// only.
    down_piece: u32,
    phase: Phase,
}

const NO_PIECE: u32 = u32::MAX;

impl Blocks {
    #[must_use]
    pub fn new(cfg: Config, seed: u64) -> Blocks {
        let cfg = cfg.sanitised();
        let mut b = Blocks {
            cfg,
            well: [[0; W]; ROWS],
            active: None,
            hold: None,
            hold_used: false,
            queue: [0; QCAP],
            qlen: 0,
            rng: Rng::new(seed),
            seed,
            level: cfg.start_level,
            lines: 0,
            score: 0,
            drop_points: 0,
            combo: 0,
            pieces: 0,
            spawned: 0,
            steps: 0,
            fall: 0,
            lock: 0,
            resets: 0,
            lowest: 0,
            clear_mask: 0,
            clear_timer: 0,
            held: 0,
            shift: 0,
            das: 0,
            down_piece: NO_PIECE,
            phase: Phase::Playing,
        };
        b.spawn_next();
        b
    }

    /// A game whose stack starts as `rows` (bottom row last; each cell 0
    /// or a piece number + 1): tests and the simulator.
    #[must_use]
    pub fn with_stack(cfg: Config, seed: u64, rows: &[[u8; W]]) -> Blocks {
        let mut b = Blocks::new(cfg, seed);
        let start = ROWS.saturating_sub(rows.len());
        for (dst, src) in b.well[start..].iter_mut().zip(rows) {
            *dst = src.map(|c| c.min(7));
        }
        b
    }

    // ------------------------------------------------------------ queries

    #[must_use]
    pub fn config(&self) -> Config {
        self.cfg
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Well cell at column `x`, row `y` (0 = top hidden row): 0 or the
    /// piece number + 1. Outside the well: 0.
    #[must_use]
    pub fn cell(&self, x: usize, y: usize) -> u8 {
        self.well.get(y).and_then(|r| r.get(x)).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn active(&self) -> Option<Active> {
        self.active
    }

    /// Rows the falling piece would drop in a hard drop.
    #[must_use]
    pub fn drop_distance(&self) -> i8 {
        let Some(a) = self.active else { return 0 };
        let mut d = 0;
        while self.fits(a.piece, a.rot, a.x, a.y + d + 1) {
            d += 1;
        }
        d
    }

    /// Where the falling piece would land (the ghost).
    #[must_use]
    pub fn ghost(&self) -> Option<Active> {
        self.active.map(|a| Active { y: a.y + self.drop_distance(), ..a })
    }

    #[must_use]
    pub fn held_piece(&self) -> Option<Piece> {
        self.hold
    }

    /// Hold can be used for the falling piece.
    #[must_use]
    pub fn hold_ready(&self) -> bool {
        self.cfg.hold && !self.hold_used
    }

    /// The `k`-th next piece (0 = the next one).
    #[must_use]
    pub fn next(&self, k: usize) -> Option<Piece> {
        if k < usize::from(self.qlen) { Piece::from_u8(self.queue[k]) } else { None }
    }

    #[must_use]
    pub fn level(&self) -> u8 {
        self.level
    }

    #[must_use]
    pub fn lines(&self) -> u32 {
        self.lines
    }

    #[must_use]
    pub fn score(&self) -> u32 {
        self.score
    }

    /// The part of the score from soft and hard drops (the rest is from
    /// cleared rows).
    #[must_use]
    pub fn drop_points(&self) -> u32 {
        self.drop_points
    }

    /// The combo count: pieces in a row that cleared rows.
    #[must_use]
    pub fn combo(&self) -> u8 {
        self.combo
    }

    #[must_use]
    pub fn pieces(&self) -> u32 {
        self.pieces
    }

    /// Steps played.
    #[must_use]
    pub fn steps(&self) -> u32 {
        self.steps
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    #[must_use]
    pub fn ended(&self) -> bool {
        self.phase != Phase::Playing
    }

    /// Rows flashing before they go (bit per well row) while non-zero.
    #[must_use]
    pub fn clearing(&self) -> (u32, u8) {
        (self.clear_mask, self.clear_timer)
    }

    /// A number that changes whenever anything drawn changes (the screen
    /// redraws only then).
    #[must_use]
    pub fn view_hash(&self) -> u32 {
        let mut h = 0x811C_9DC5u32;
        let mut mix = |b: u8| h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        for row in &self.well {
            for &c in row {
                mix(c);
            }
        }
        if let Some(a) = self.active {
            for v in [a.piece as u8, a.rot, a.x as u8, a.y as u8] {
                mix(v);
            }
        }
        mix(self.hold.map_or(0xFF, |p| p as u8));
        mix(u8::from(self.hold_used));
        for &q in &self.queue[..usize::from(self.qlen).min(3)] {
            mix(q);
        }
        for b in self.score.to_le_bytes().into_iter().chain(self.lines.to_le_bytes()) {
            mix(b);
        }
        mix(self.level);
        mix(self.clear_timer);
        mix(self.phase as u8);
        // The clock of the timed modes, in whole seconds.
        for b in (self.steps / crate::STEP_HZ).to_le_bytes() {
            mix(b);
        }
        h
    }

    // ------------------------------------------------------------ rules

    fn fits(&self, piece: Piece, rot: u8, x: i8, y: i8) -> bool {
        piece.cells(rot).iter().all(|&(cx, cy)| {
            let (wx, wy) = (x + cx, y + cy);
            (0..W as i8).contains(&wx)
                && (0..ROWS as i8).contains(&wy)
                && self.well[wy as usize][wx as usize] == 0
        })
    }

    fn resting(&self) -> bool {
        self.active.is_some_and(|a| !self.fits(a.piece, a.rot, a.x, a.y + 1))
    }

    fn gravity_steps(&self) -> u16 {
        let g = u16::from(GRAVITY[usize::from(self.level.clamp(1, MAX_LEVEL) - 1)]);
        if self.held & Button::Down.bit() != 0 { g.min(SOFT_STEPS) } else { g }
    }

    fn fill_queue(&mut self) {
        while self.qlen < 4 {
            if self.cfg.bag {
                let mut bag = [0u8, 1, 2, 3, 4, 5, 6];
                for i in (1..bag.len()).rev() {
                    let j = self.rng.below(i as u32 + 1) as usize;
                    bag.swap(i, j);
                }
                for p in bag {
                    self.queue[usize::from(self.qlen)] = p;
                    self.qlen += 1;
                }
            } else {
                self.queue[usize::from(self.qlen)] = self.rng.below(7) as u8;
                self.qlen += 1;
            }
        }
    }

    fn spawn_next(&mut self) {
        self.fill_queue();
        let p = Piece::from_u8(self.queue[0]).unwrap_or(Piece::I);
        self.queue.copy_within(1..usize::from(self.qlen), 0);
        self.qlen -= 1;
        // Nothing stale past the end (saved games compare equal).
        self.queue[usize::from(self.qlen)] = 0;
        self.fill_queue();
        self.spawn(p);
    }

    fn spawn(&mut self, piece: Piece) {
        let x = if piece.box_size() == 2 { 4 } else { 3 };
        self.fall = 0;
        self.lock = 0;
        self.resets = 0;
        self.spawned = self.spawned.wrapping_add(1);
        for y in [1, 0] {
            if self.fits(piece, 0, x, y) {
                self.active = Some(Active { piece, rot: 0, x, y });
                self.lowest = y;
                return;
            }
        }
        // No room: the stack reached the top.
        self.active = None;
        self.phase = Phase::Over;
    }

    /// A shift or turn of a resting piece restarts the lock delay (a
    /// limited number of times per row reached).
    fn moved(&mut self) {
        if self.resting() && self.resets < MAX_RESETS {
            self.lock = 0;
            self.resets += 1;
        }
    }

    fn try_shift(&mut self, dx: i8) -> bool {
        let Some(a) = self.active else { return false };
        if !self.fits(a.piece, a.rot, a.x + dx, a.y) {
            return false;
        }
        self.active = Some(Active { x: a.x + dx, ..a });
        self.moved();
        true
    }

    fn rotate(&mut self, turn: u8) -> bool {
        let Some(a) = self.active else { return false };
        if a.piece == Piece::O {
            return false;
        }
        let rot = (a.rot + turn) % 4;
        let kicks: &[(i8, i8)] = match (self.cfg.kicks, a.piece) {
            (false, _) => &KICKS[..1],
            (true, Piece::I) => &KICKS_LONG,
            (true, _) => &KICKS,
        };
        for &(dx, dy) in kicks {
            if self.fits(a.piece, rot, a.x + dx, a.y + dy) {
                self.active = Some(Active { rot, x: a.x + dx, y: a.y + dy, ..a });
                self.moved();
                return true;
            }
        }
        false
    }

    fn hold_swap(&mut self) {
        let Some(a) = self.active else { return };
        if !self.hold_ready() {
            return;
        }
        match self.hold.replace(a.piece) {
            Some(h) => self.spawn(h),
            None => self.spawn_next(),
        }
        self.hold_used = true;
    }

    fn add_drop(&mut self, points: u32) {
        self.score = self.score.saturating_add(points);
        self.drop_points = self.drop_points.saturating_add(points);
    }

    fn hard_drop(&mut self) -> StepOut {
        let Some(a) = self.active else { return StepOut::default() };
        let d = self.drop_distance();
        self.active = Some(Active { y: a.y + d, ..a });
        self.add_drop(HARD_POINTS * d as u32);
        self.lock_piece(true)
    }

    fn lock_piece(&mut self, hard: bool) -> StepOut {
        let mut out = StepOut::default();
        let Some(a) = self.active.take() else { return out };
        let mut above = true;
        for (x, y) in a.cells() {
            if let Some(c) = self.well.get_mut(y as usize).and_then(|r| r.get_mut(x as usize)) {
                *c = a.piece as u8 + 1;
            }
            above &= (y as usize) < HIDDEN;
        }
        self.pieces = self.pieces.saturating_add(1);
        self.hold_used = false;
        let mut mask = 0u32;
        for (y, row) in self.well.iter().enumerate() {
            if row.iter().all(|&c| c != 0) {
                mask |= 1 << y;
            }
        }
        let n = mask.count_ones() as u8;
        if n > 0 {
            let lv = u32::from(self.level);
            self.lines = self.lines.saturating_add(u32::from(n));
            let combo = u32::from(self.combo) * COMBO_POINTS * lv;
            self.score = self.score.saturating_add(CLEAR_POINTS[usize::from(n.min(4))] * lv + combo);
            self.combo = self.combo.saturating_add(1);
            self.level = level_for(self.cfg.start_level, self.lines);
            self.clear_mask = mask;
            self.clear_timer = CLEAR_STEPS;
        } else {
            self.combo = 0;
        }
        out.lock = Some(Lock { at: a, cleared: n, hard });
        if above && n == 0 {
            // Locked wholly above the visible well.
            self.phase = Phase::Over;
        } else if self.cfg.mode == Mode::Lines40 && self.lines >= LINES_GOAL {
            self.phase = Phase::Done;
        } else if n == 0 {
            self.spawn_next();
        }
        out.ended = self.ended();
        out
    }

    fn collapse(&mut self) {
        let mut dst = ROWS;
        for src in (0..ROWS).rev() {
            if self.clear_mask & (1 << src) == 0 {
                dst -= 1;
                self.well[dst] = self.well[src];
            }
        }
        for row in &mut self.well[..dst] {
            *row = [0; W];
        }
        self.clear_mask = 0;
    }

    /// One fixed step of `1 / STEP_HZ` s.
    pub fn step(&mut self) -> StepOut {
        let mut out = StepOut::default();
        if self.phase != Phase::Playing {
            return out;
        }
        self.steps = self.steps.saturating_add(1);
        if self.cfg.mode == Mode::Attack && self.steps >= ATTACK_STEPS {
            self.phase = Phase::Done;
            out.ended = true;
            return out;
        }
        if self.clear_timer > 0 {
            self.clear_timer -= 1;
            if self.clear_timer == 0 {
                self.collapse();
                self.spawn_next();
                out.ended = self.ended();
            }
            return out;
        }
        if self.active.is_none() {
            return out;
        }
        // Auto-shift: after the delay, every interval (or to the wall).
        if self.shift != 0 {
            self.das = self.das.saturating_add(1);
            let delay = ms_to_steps(u32::from(self.cfg.das_ms)).max(1);
            let every = ms_to_steps(u32::from(self.cfg.arr_ms));
            let since = u32::from(self.das);
            if since >= delay {
                if every == 0 {
                    while self.try_shift(self.shift) {}
                } else if (since - delay) % every == 0 {
                    self.try_shift(self.shift);
                }
            }
        }
        // Gravity (soft drop while Down is held).
        self.fall += 1;
        if self.fall >= self.gravity_steps() {
            self.fall = 0;
            if let Some(a) = self.active
                && self.fits(a.piece, a.rot, a.x, a.y + 1)
            {
                self.active = Some(Active { y: a.y + 1, ..a });
                if self.held & Button::Down.bit() != 0 {
                    self.add_drop(SOFT_POINTS);
                }
                if a.y + 1 > self.lowest {
                    self.lowest = a.y + 1;
                    self.resets = 0;
                }
                self.lock = 0;
            }
        }
        // Lock delay.
        if self.resting() {
            self.lock += 1;
            if u32::from(self.lock) >= ms_to_steps(u32::from(self.cfg.lock_ms)).max(1) {
                return self.lock_piece(false);
            }
        } else {
            self.lock = 0;
        }
        out
    }

    /// A key edge. Prev/Next (Left/Right) shift, Up turns clockwise, OK
    /// anticlockwise, Down soft-drops while held and hard-drops when held
    /// long, Menu holds.
    pub fn input(&mut self, i: Input) -> StepOut {
        match i {
            Input::Press(b) => {
                self.held |= b.bit();
                if self.phase != Phase::Playing {
                    return StepOut::default();
                }
                match b {
                    Button::Left | Button::Right => {
                        let dx = if b == Button::Left { -1 } else { 1 };
                        self.shift = dx;
                        self.das = 0;
                        self.try_shift(dx);
                    }
                    Button::Up => {
                        self.rotate(1);
                    }
                    Button::Ok => {
                        self.rotate(3);
                    }
                    Button::Down => self.down_piece = self.spawned,
                    Button::Menu => self.hold_swap(),
                }
            }
            Input::Release(b) => {
                self.held &= !b.bit();
                match b {
                    Button::Left | Button::Right => {
                        let (this, other, odx) =
                            if b == Button::Left { (-1, Button::Right, 1) } else { (1, Button::Left, -1) };
                        if self.shift == this {
                            if self.held & other.bit() != 0 {
                                self.shift = odx;
                                self.das = 0;
                            } else {
                                self.shift = 0;
                            }
                        }
                    }
                    Button::Down => self.down_piece = NO_PIECE,
                    _ => {}
                }
            }
            Input::Long(Button::Down)
                if self.phase == Phase::Playing
                    && self.down_piece == self.spawned
                    && self.active.is_some() =>
            {
                self.down_piece = NO_PIECE;
                return self.hard_drop();
            }
            Input::Long(_) => {}
        }
        StepOut::default()
    }

    /// Forget held keys (the game was paused; nothing is held on resume).
    pub fn release_all(&mut self) {
        self.held = 0;
        self.shift = 0;
        self.das = 0;
        self.down_piece = NO_PIECE;
    }

    /// Take new auto-shift and repeat timings (settings changed during a
    /// pause). Lock delay and the rules stay as the game started.
    pub fn set_handling(&mut self, das_ms: u16, arr_ms: u16) {
        self.cfg = Config { das_ms, arr_ms, ..self.cfg }.sanitised();
    }

    /// The invariants, for tests and fuzzing: `Err` names the first one
    /// broken.
    pub fn check(&self) -> Result<(), &'static str> {
        if let Some(a) = self.active {
            for (x, y) in a.cells() {
                if !(0..W as i8).contains(&x) || !(0..ROWS as i8).contains(&y) {
                    return Err("piece outside the well");
                }
                if self.well[y as usize][x as usize] != 0 {
                    return Err("piece overlaps the stack");
                }
            }
        }
        for (y, row) in self.well.iter().enumerate() {
            if row.iter().any(|&c| c > 7) {
                return Err("bad cell");
            }
            if row.iter().all(|&c| c != 0) && (self.clear_timer == 0 || self.clear_mask & (1 << y) == 0) {
                return Err("full row left in the well");
            }
        }
        if self.level != level_for(self.cfg.start_level, self.lines) {
            return Err("level does not follow the lines");
        }
        if self.phase == Phase::Playing {
            if self.qlen < 3 {
                return Err("fewer than three pieces queued");
            }
            if self.active.is_none() && self.clear_timer == 0 {
                return Err("no piece in play");
            }
        }
        if self.cfg.mode == Mode::Lines40 && self.lines >= LINES_GOAL && self.phase == Phase::Playing {
            return Err("40 lines played on");
        }
        Ok(())
    }

    // ------------------------------------------------------------ files

    pub fn encode(&self, w: &mut Writer<'_>) {
        let c = &self.cfg;
        w.u8(c.mode as u8);
        w.u8(c.start_level);
        w.bool(c.bag);
        w.bool(c.hold);
        w.u16(c.lock_ms);
        w.u16(c.das_ms);
        w.u16(c.arr_ms);
        w.bool(c.kicks);
        w.u64(self.seed);
        w.u64(self.rng.state());
        for row in &self.well {
            w.bytes(row);
        }
        match self.active {
            Some(a) => {
                w.u8(a.piece as u8);
                w.u8(a.rot);
                w.i8(a.x);
                w.i8(a.y);
            }
            None => w.bytes(&[0xFF; 4]),
        }
        w.u8(self.hold.map_or(0xFF, |p| p as u8));
        w.bool(self.hold_used);
        w.u8(self.qlen);
        w.bytes(&self.queue[..usize::from(self.qlen)]);
        w.u8(self.level);
        w.u32(self.lines);
        w.u32(self.score);
        w.u32(self.drop_points);
        w.u8(self.combo);
        w.u32(self.pieces);
        w.u32(self.spawned);
        w.u32(self.steps);
        w.u16(self.fall);
        w.u16(self.lock);
        w.u8(self.resets);
        w.i8(self.lowest);
        w.u32(self.clear_mask);
        w.u8(self.clear_timer);
        w.u8(self.held);
        w.i8(self.shift);
        w.u16(self.das);
        w.u32(self.down_piece);
        w.u8(self.phase as u8);
    }

    /// Read a game written by [`Blocks::encode`]; `None` for anything out
    /// of range or a state the rules cannot reach.
    pub fn decode(r: &mut Reader<'_>) -> Option<Blocks> {
        let cfg = Config {
            mode: Mode::from_u8(r.u8()?)?,
            start_level: r.u8()?,
            bag: r.bool()?,
            hold: r.bool()?,
            lock_ms: r.u16()?,
            das_ms: r.u16()?,
            arr_ms: r.u16()?,
            kicks: r.bool()?,
        };
        if cfg.sanitised() != cfg {
            return None;
        }
        let seed = r.u64()?;
        let rng = Rng::new(r.u64()?);
        let mut well = [[0u8; W]; ROWS];
        for row in &mut well {
            *row = r.array()?;
        }
        let a = r.array::<4>()?;
        let active = if a == [0xFF; 4] {
            None
        } else {
            Some(Active { piece: Piece::from_u8(a[0])?, rot: a[1], x: a[2] as i8, y: a[3] as i8 })
        };
        if active.is_some_and(|a| a.rot > 3) {
            return None;
        }
        let hold = match r.u8()? {
            0xFF => None,
            p => Some(Piece::from_u8(p)?),
        };
        let hold_used = r.bool()?;
        let qlen = r.u8()?;
        if usize::from(qlen) > QCAP {
            return None;
        }
        let mut queue = [0u8; QCAP];
        queue[..usize::from(qlen)].copy_from_slice(r.bytes(usize::from(qlen))?);
        let b = Blocks {
            cfg,
            well,
            active,
            hold,
            hold_used,
            queue,
            qlen,
            rng,
            seed,
            level: r.u8()?,
            lines: r.u32()?,
            score: r.u32()?,
            drop_points: r.u32()?,
            combo: r.u8()?,
            pieces: r.u32()?,
            spawned: r.u32()?,
            steps: r.u32()?,
            fall: r.u16()?,
            lock: r.u16()?,
            resets: r.u8()?,
            lowest: r.i8()?,
            clear_mask: r.u32()?,
            clear_timer: r.u8()?,
            held: r.u8()?,
            shift: r.i8()?,
            das: r.u16()?,
            down_piece: r.u32()?,
            phase: match r.u8()? {
                0 => Phase::Playing,
                1 => Phase::Over,
                2 => Phase::Done,
                _ => return None,
            },
        };
        let valid = queue[..usize::from(qlen)].iter().all(|&p| p < 7)
            && (-1..=1).contains(&b.shift)
            && b.clear_timer <= CLEAR_STEPS
            && (b.clear_timer == 0 || b.active.is_none())
            && b.clear_mask >> ROWS == 0
            && b.held >> Button::ALL.len() == 0
            && b.drop_points <= b.score
            && b.check().is_ok();
        valid.then_some(b)
    }
}

/// The level after `lines` lines from `start`: one up per ten lines.
#[must_use]
pub fn level_for(start: u8, lines: u32) -> u8 {
    let up = u8::try_from(lines / 10).unwrap_or(MAX_LEVEL);
    start.saturating_add(up).min(MAX_LEVEL)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn game() -> Blocks {
        Blocks::new(Config::DEFAULT, 1)
    }

    #[test]
    fn pieces_turn_in_their_box() {
        for p in Piece::ALL {
            for r in 0..4 {
                let n = p.box_size();
                assert!(p.cells(r).iter().all(|&(x, y)| (0..n).contains(&x) && (0..n).contains(&y)));
                assert_eq!(p.cells(r + 4), p.cells(r));
            }
        }
        // T points up, then right after a clockwise turn.
        let mut t = Piece::T.cells(1);
        t.sort_unstable();
        assert_eq!(t, [(1, 0), (1, 1), (1, 2), (2, 1)]);
    }

    #[test]
    fn spawns_visible_with_three_next() {
        let b = game();
        let a = b.active().unwrap();
        assert!(a.cells().iter().any(|&(_, y)| y as usize >= HIDDEN), "partly visible at once");
        assert!((0..3).all(|k| b.next(k).is_some()));
        assert!(b.check().is_ok());
    }

    #[test]
    fn bag_gives_each_piece_once_per_seven() {
        let mut b = Blocks::new(Config::DEFAULT, 77);
        let mut seen = std::vec::Vec::new();
        for _ in 0..14 {
            seen.push(b.active().unwrap().piece as u8);
            b.input(Input::Press(Button::Down));
            b.input(Input::Long(Button::Down));
            b.input(Input::Release(Button::Down));
            while b.active().is_none() && !b.ended() {
                b.step();
            }
            // Keep the stack low: clear the well between pieces.
            b.well = [[0; W]; ROWS];
        }
        for bag in seen.chunks(7) {
            let mut s = bag.to_vec();
            s.sort_unstable();
            assert_eq!(s, [0, 1, 2, 3, 4, 5, 6]);
        }
    }

    #[test]
    fn hard_drop_scores_two_per_row_and_locks() {
        let mut b = game();
        let d = b.drop_distance();
        assert!(d > 10);
        b.input(Input::Press(Button::Down));
        let out = b.input(Input::Long(Button::Down));
        assert!(out.lock.is_some_and(|l| l.hard));
        assert_eq!(b.score(), 2 * d as u32);
        assert_eq!(b.pieces(), 1);
        // The next piece is not hard-dropped by the same hold.
        assert!(b.input(Input::Long(Button::Down)).lock.is_none());
    }

    #[test]
    fn lock_delay_and_wall() {
        let mut b = game();
        // Shift left to the wall.
        b.input(Input::Press(Button::Left));
        for _ in 0..60 {
            b.step();
        }
        let a = b.active().unwrap();
        assert_eq!(a.cells().iter().map(|c| c.0).min(), Some(0));
        b.input(Input::Release(Button::Left));
        // Falls and locks by itself.
        let mut locked = false;
        for _ in 0..60 * 60 {
            if b.step().lock.is_some() {
                locked = true;
                break;
            }
        }
        assert!(locked);
        assert!(b.check().is_ok());
    }

    #[test]
    fn clearing_rows_scores_and_collapses() {
        // Bottom row full but for column 9, a long piece standing there.
        let mut row = [1u8; W];
        row[9] = 0;
        let mut b = Blocks::with_stack(Config { bag: false, ..Config::DEFAULT }, 3, &[row, row, row, row]);
        b.active = Some(Active { piece: Piece::I, rot: 1, x: 7, y: 0 });
        assert!(b.check().is_ok());
        b.input(Input::Press(Button::Down));
        let out = b.input(Input::Long(Button::Down));
        assert_eq!(out.lock.map(|l| l.cleared), Some(4));
        assert_eq!(b.lines(), 4);
        assert_eq!(b.clearing().1, CLEAR_STEPS);
        for _ in 0..CLEAR_STEPS {
            b.step();
        }
        assert!((0..ROWS).all(|y| (0..W).all(|x| b.cell(x, y) == 0)), "all four rows went");
        assert!(b.active().is_some());
        let d = 18; // rows dropped by the long piece
        assert_eq!(b.score(), CLEAR_POINTS[4] + HARD_POINTS * d);
    }

    #[test]
    fn hold_swaps_once_per_piece() {
        let mut b = game();
        let first = b.active().unwrap().piece;
        let next = b.next(0).unwrap();
        b.input(Input::Press(Button::Menu));
        assert_eq!(b.held_piece(), Some(first));
        assert_eq!(b.active().unwrap().piece, next);
        b.input(Input::Press(Button::Menu));
        assert_eq!(b.active().unwrap().piece, next, "used for this piece");
        let off = Blocks::new(Config { hold: false, ..Config::DEFAULT }, 1);
        let mut o = off.clone();
        o.input(Input::Press(Button::Menu));
        assert_eq!((o.active(), o.held_piece()), (off.active(), None));
    }

    #[test]
    fn kicks_off_turns_only_in_place() {
        let mut b = Blocks::new(Config { kicks: false, ..Config::DEFAULT }, 5);
        b.active = Some(Active { piece: Piece::I, rot: 1, x: -2, y: 5 });
        assert!(b.check().is_ok(), "standing at the left wall");
        assert!(!b.rotate(1), "would leave the well, no kick");
        b.cfg.kicks = true;
        assert!(b.rotate(1));
        assert!(b.check().is_ok());
    }

    #[test]
    fn modes_end() {
        let mut b = Blocks::new(Config { mode: Mode::Attack, ..Config::DEFAULT }, 9);
        let mut ended = false;
        for i in 0..ATTACK_STEPS {
            ended |= b.step().ended;
            if i % 500 == 0 && b.active().is_some() {
                // Keep the stack from reaching the top.
                b.well = [[0; W]; ROWS];
            }
        }
        assert!(ended && b.phase() == Phase::Done);
        assert_eq!(b.steps(), ATTACK_STEPS);
    }

    #[test]
    fn forty_lines_ends_at_forty() {
        let mut b = Blocks::new(Config { mode: Mode::Lines40, ..Config::DEFAULT }, 3);
        let mut row = [1u8; W];
        row[0] = 0;
        for k in 0..10 {
            // Four rows open in column 0 and a long piece standing over it.
            b.well[ROWS - 4..].copy_from_slice(&[row; 4]);
            b.active = Some(Active { piece: Piece::I, rot: 1, x: -2, y: 0 });
            b.input(Input::Press(Button::Down));
            let out = b.input(Input::Long(Button::Down));
            b.input(Input::Release(Button::Down));
            assert_eq!(out.lock.map(|l| l.cleared), Some(4));
            assert_eq!(out.ended, k == 9);
            for _ in 0..CLEAR_STEPS {
                b.step();
            }
            assert!(b.check().is_ok());
        }
        assert_eq!((b.lines(), b.phase()), (40, Phase::Done));
        assert_eq!(b.level(), 5);
    }

    #[test]
    fn levels_and_tables() {
        assert_eq!(level_for(1, 0), 1);
        assert_eq!(level_for(1, 9), 1);
        assert_eq!(level_for(1, 10), 2);
        assert_eq!(level_for(15, 1000), MAX_LEVEL);
        assert!(GRAVITY.windows(2).all(|w| w[0] >= w[1]));
        let mut seen = std::collections::BTreeSet::new();
        for mode in [Mode::Endless, Mode::Lines40, Mode::Attack] {
            for bag in [false, true] {
                for hold in [false, true] {
                    for kicks in [false, true] {
                        let c = Config { mode, bag, hold, kicks, ..Config::DEFAULT };
                        assert!(seen.insert(c.variant()));
                    }
                }
            }
        }
    }
}
