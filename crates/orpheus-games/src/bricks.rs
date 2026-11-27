//! Bricks (GAME-6): a paddle, a ball and a wall of bricks.
//!
//! An original game in the brick-breaking genre: own levels, power-ups,
//! rules and look. The field is 128 × 116 px; bricks of 10 × 5 px stand in
//! up to ten rows of twelve from y = 12; the paddle runs along y = 106;
//! the ball is a 3 × 3 square. Walls on the left, the right and the top;
//! a ball that falls past the bottom is lost.
//!
//! **Physics.** Integers only (positions and speeds in 1/256 px, a sine
//! table in 1/4096): the same game on every target, no floating point and
//! so no question of fused multiply-adds. A ball moves at the game's speed
//! in one of seven directions on each side of the vertical (12° … 60°
//! from it). Each 60 Hz step is cut into sub-steps of at most one pixel on
//! each axis, as many as the speed needs, and each sub-step moves x, then
//! y. A move that would make the ball overlap a wall, a brick or the
//! paddle is not made: the ball turns on that axis instead (and hits the
//! bricks it would have entered). As every obstacle is thicker than a
//! sub-step, the ball can never pass through one, at any speed; and it is
//! never inside one after a step (`check`).
//!
//! The paddle turns the ball by where it is met: the centre sends it
//! nearly straight up, the ends at 60° to the side it came from. The
//! paddle stops where it would push into a ball beside it. The ball
//! speeds up by 1/24 of the base speed every four seconds of play, up to
//! 1.5 × the level's speed; each level starts 1/40 faster than the last
//! (ten levels at most). A ball that bounces only between walls and
//! unbreakable bricks for 15 s is nudged to the next angle.
//!
//! **Bricks**: one, two or three hits (10 points per hit it took when
//! new, paid when it breaks), unbreakable. Clearing every breakable brick
//! scores 100 and moves on to the next level. **Levels**: eight hand-made
//! ones (symmetric, own designs), then (level set "mixed") generated ones
//! from the game's seed and the level: a symmetric pattern (random, rows,
//! chequer, pyramid, blocks) of rows, density and hardness rising with the
//! level, unbreakable bricks from level 4; every breakable brick
//! reachable (a breadth-first search over the cells; walls that close a
//! pocket become three-hit bricks). "Hand-made" goes round the eight
//! again with one hit more per brick; "generated" generates every level.
//!
//! **Power-ups** (setting): one breakable brick in seven carries a capsule
//! that falls when it breaks (three at most at a time); the paddle
//! catches it: wider (+8 px, up to 40, until a life is lost or the level
//! ends), slower (the speed × ¾), split (up to three balls), catch (for
//! 20 s the paddle holds a ball it meets; OK or 2.5 s lets it go, aimed by
//! where it sits), and an extra life (nine at most).
//!
//! **Keys**: Prev/Next (or Down/Up) move the paddle, faster while held
//! (paddle speed: the setting); OK (or Menu) launches the ball.

use crate::rng::Rng;
use crate::save::{Reader, Writer};
use crate::{Button, Input};

pub const VERSION: u8 = 1;

/// Fixed point: 1/256 px.
pub const ONE: i32 = 256;
/// The field, px.
pub const FIELD_W: i32 = 128;
pub const FIELD_H: i32 = 116;
/// Brick grid.
pub const COLS: usize = 12;
pub const ROWS: usize = 10;
pub const CELLS: usize = COLS * ROWS;
pub const BRICK_W: i32 = 10;
pub const BRICK_H: i32 = 5;
/// The grid's top-left corner in the field, px.
pub const BRICKS_X: i32 = 4;
pub const BRICKS_Y: i32 = 12;
/// The ball's side, px.
pub const BALL: i32 = 3;
/// The paddle's top and height, px.
pub const PADDLE_Y: i32 = 106;
pub const PADDLE_H: i32 = 3;
/// Paddle widths of the setting (small, medium, large), px.
pub const PADDLE_WIDTHS: [u8; 3] = [16, 22, 28];
/// The wider power-up adds this much, up to [`PADDLE_MAX`].
pub const WIDE_STEP: u8 = 8;
pub const PADDLE_MAX: u8 = 40;
pub const MAX_BALLS: usize = 3;
pub const MAX_CAPS: usize = 3;
/// A capsule, px; it falls 0.75 px a step.
pub const CAP_W: i32 = 8;
pub const CAP_H: i32 = 5;
pub const CAP_FALL: i32 = 192;
/// Ball speeds of the setting (slow, normal, fast), 1/256 px a step:
/// 75, 98 and 120 px/s.
pub const BASE_SPEEDS: [i32; 3] = [320, 416, 512];
/// No ball is ever faster (5 px a step, 300 px/s) or slower.
pub const MAX_SPEED: i32 = 5 * ONE;
pub const MIN_SPEED: i32 = ONE;
/// Play steps between speed-ups.
pub const SPEEDUP_STEPS: u16 = 240;
/// Steps the "level cleared" banner stays.
pub const CLEAR_STEPS: u16 = 90;
/// Steps the catch power-up lasts, and a caught ball waits at most.
pub const CATCH_STEPS: u16 = 1200;
pub const HOLD_STEPS: u16 = 150;
pub const MAX_LIVES: u8 = 9;
/// Points per hit a brick took when new, and per level cleared.
pub const BRICK_POINTS: u32 = 10;
pub const CLEAR_POINTS: u32 = 100;
/// Steps without touching the paddle or a breakable brick before a
/// bounce nudges the angle.
pub const STALL_STEPS: u16 = 900;
/// One breakable brick in this many (per mille: 1/7) carries a capsule.
const CARRY_PER_MILLE: u16 = 143;

/// The ball's directions: degrees from the vertical, and their sine and
/// cosine in 1/4096 (rounded; checked against `f64` in the tests).
pub const ANGLES: [u8; 7] = [12, 20, 28, 36, 44, 52, 60];
pub const SIN: [i32; 7] = [852, 1401, 1923, 2408, 2845, 3228, 3547];
pub const COS: [i32; 7] = [4006, 3849, 3617, 3314, 2946, 2522, 2048];
/// The direction a new ball is served in.
const SERVE_ANGLE: u8 = 3;

/// Paddle speed of the setting (low, medium, high): at the press and
/// after [`PADDLE_RAMP`] steps held, 1/256 px a step.
pub const PADDLE_START: [i32; 3] = [192, 288, 384];
pub const PADDLE_TOP: [i32; 3] = [512, 768, 1152];
pub const PADDLE_RAMP: i32 = 15;

/// A brick cell: 0 empty; bits 0–2 the hits left (1–3) or [`SOLID`];
/// bits 4–5 the hits it took when new; bit 7 it carries a capsule.
pub const EMPTY: u8 = 0;
pub const SOLID: u8 = 7;
pub const CARRY: u8 = 0x80;

/// A new brick of `hits` (1–3).
#[must_use]
pub const fn brick(hits: u8) -> u8 {
    hits | hits << 4
}

/// Hits left (1–3), [`SOLID`], or 0.
#[must_use]
pub const fn hits_left(c: u8) -> u8 {
    c & 7
}

/// Hits the brick took when new (its points and colour).
#[must_use]
pub const fn strength(c: u8) -> u8 {
    (c >> 4) & 3
}

#[must_use]
pub const fn breakable(c: u8) -> bool {
    matches!(c & 7, 1..=3)
}

/// A cell the rules can make: empty, unbreakable, or a brick with no more
/// hits left than it took when new (and no stray bits).
#[must_use]
pub const fn valid_cell(c: u8) -> bool {
    c == EMPTY
        || c == SOLID
        || (breakable(c) && c & 0x48 == 0 && strength(c) >= 1 && hits_left(c) <= strength(c))
}

/// The power-ups.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Power {
    Wide = 1,
    Slow,
    Split,
    Catch,
    Life,
}

impl Power {
    pub const ALL: [Power; 5] = [Power::Wide, Power::Slow, Power::Split, Power::Catch, Power::Life];

    #[must_use]
    pub fn from_u8(v: u8) -> Option<Power> {
        Power::ALL.get(usize::from(v).wrapping_sub(1)).copied()
    }
}

/// Which levels are played.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum LevelSet {
    /// The eight hand-made levels, then again with harder bricks.
    HandMade = 0,
    /// Every level generated from the seed.
    Generated,
    /// The hand-made levels, then generated ones.
    #[default]
    Mixed,
}

impl LevelSet {
    #[must_use]
    pub fn from_u8(v: u8) -> Option<LevelSet> {
        [LevelSet::HandMade, LevelSet::Generated, LevelSet::Mixed].get(usize::from(v)).copied()
    }
}

/// Rule settings, fixed for a game but the paddle speed (handling:
/// [`Bricks::set_handling`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// 1 ..= 10.
    pub start_level: u8,
    /// Ball speed 0–2 ([`BASE_SPEEDS`]).
    pub speed: u8,
    /// Paddle size 0–2 ([`PADDLE_WIDTHS`]).
    pub paddle: u8,
    /// Lives at the start, 1 ..= [`MAX_LIVES`].
    pub lives: u8,
    pub powerups: bool,
    pub set: LevelSet,
    /// Paddle speed 0–2 ([`PADDLE_START`], [`PADDLE_TOP`]).
    pub sens: u8,
}

impl Config {
    pub const DEFAULT: Config = Config {
        start_level: 1,
        speed: 1,
        paddle: 1,
        lives: 3,
        powerups: true,
        set: LevelSet::Mixed,
        sens: 1,
    };

    #[must_use]
    pub fn sanitised(self) -> Config {
        Config {
            start_level: self.start_level.clamp(1, 10),
            speed: self.speed.min(2),
            paddle: self.paddle.min(2),
            lives: self.lives.clamp(1, MAX_LIVES),
            sens: self.sens.min(2),
            ..self
        }
    }

    /// The score table's variant: start level, speed, paddle, lives,
    /// power-ups, level set (the paddle speed is handling).
    #[must_use]
    pub fn variant(&self) -> u16 {
        u16::from(self.start_level - 1)
            | u16::from(self.speed) << 4
            | u16::from(self.paddle) << 6
            | u16::from(self.lives) << 8
            | u16::from(self.powerups) << 12
            | (self.set as u16) << 13
    }

    fn base_width(&self) -> u8 {
        PADDLE_WIDTHS[usize::from(self.paddle.min(2))]
    }

    fn base_speed(&self) -> i32 {
        BASE_SPEEDS[usize::from(self.speed.min(2))]
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::DEFAULT
    }
}

/// A ball: its top-left corner in 1/256 px.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Ball {
    pub x: i32,
    pub y: i32,
    /// Index into [`ANGLES`].
    pub ang: u8,
    flags: u8,
    /// Steps a caught ball has waited.
    hold: u16,
}

const LIVE: u8 = 1;
const LEFT: u8 = 2;
const UP: u8 = 4;
const STUCK: u8 = 8;

impl Ball {
    #[must_use]
    pub fn live(&self) -> bool {
        self.flags & LIVE != 0
    }

    /// Held by the paddle (served or caught).
    #[must_use]
    pub fn stuck(&self) -> bool {
        self.flags & STUCK != 0
    }

    #[must_use]
    pub fn left(&self) -> bool {
        self.flags & LEFT != 0
    }

    #[must_use]
    pub fn up(&self) -> bool {
        self.flags & UP != 0
    }

    /// Its top-left pixel.
    #[must_use]
    pub fn px(&self) -> (i16, i16) {
        (px16(self.x), px16(self.y))
    }
}

fn px16(v: i32) -> i16 {
    i16::try_from(v >> 8).unwrap_or(i16::MAX)
}

/// A falling capsule (`kind` 0: none).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Capsule {
    /// Left edge, px.
    pub x: u8,
    /// Top, 1/256 px.
    pub y: i32,
    pub kind: u8,
}

impl Capsule {
    #[must_use]
    pub fn power(&self) -> Option<Power> {
        Power::from_u8(self.kind)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Phase {
    /// A ball waits on the paddle for OK.
    Serve = 0,
    Play,
    /// The level is cleared; the next one comes after [`CLEAR_STEPS`].
    Cleared,
    /// No lives left.
    Over,
}

/// What a step or an input did (sounds, tests).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StepOut {
    /// Bricks broken, and hits on bricks that stood.
    pub broke: u8,
    pub hits: u8,
    pub caught: Option<Power>,
    /// The last ball was lost (a life gone).
    pub lost: bool,
    pub cleared: bool,
    /// The game ended now.
    pub ended: bool,
}

/// A game of Bricks.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bricks {
    cfg: Config,
    seed: u64,
    rng: Rng,
    cells: [u8; CELLS],
    balls: [Ball; MAX_BALLS],
    caps: [Capsule; MAX_CAPS],
    /// The paddle's left edge (1/256 px), width and the width it grows to.
    paddle_x: i32,
    paddle_w: u8,
    wide: u8,
    /// Paddle keys: buttons held, direction (−1, 0, 1), steps held.
    held: u8,
    dir: i8,
    run: u16,
    phase: Phase,
    level: u16,
    lives: u8,
    /// Breakable bricks left.
    left: u8,
    /// Ball speed, 1/256 px a step.
    speed: u16,
    rally: u16,
    /// Steps left: the cleared banner; the catch power-up.
    timer: u16,
    catch: u16,
    stall: u16,
    score: u32,
    /// Bricks broken by their strength (1, 2, 3 hits).
    broken: [u32; 3],
    cleared: u16,
    steps: u32,
}

/// The eight hand-made levels: the left half of each row (mirrored onto
/// the right); `1`–`3` hits, `#` unbreakable.
const HAND: [[&[u8; 6]; ROWS]; 8] = [
    // Shelves.
    [
        b"......", b"......", b"111111", b"111111", b"111111", b"111111", b"......", b"......", b"......",
        b"......",
    ],
    // Funnel.
    [
        b"1.....", b"11....", b"111...", b"2111..", b"22111.", b"222111", b"......", b"......", b"......",
        b"......",
    ],
    // Pillars.
    [
        b"......", b"2.2.2.", b"1.1.1.", b"1.1.1.", b"1.1.1.", b"1.1.1.", b"3.3.3.", b"......", b"......",
        b"......",
    ],
    // Diamond.
    [
        b".....2", b"....21", b"...211", b"..2111", b".21111", b"..2111", b"...211", b"....21", b".....2",
        b"......",
    ],
    // Gates.
    [
        b"......", b"222222", b"1.1.1.", b"#1.1.1", b"1.1.1.", b"222222", b"......", b"##.1..", b"......",
        b"......",
    ],
    // Frame.
    [
        b"......", b".#2222", b".21111", b".21331", b".21331", b".21111", b".#2222", b"......", b"......",
        b"......",
    ],
    // Waves.
    [
        b"1.....", b"21....", b"321...", b".321..", b"..321.", b"...321", b"....32", b".....3", b"......",
        b"......",
    ],
    // Keep.
    [
        b"......", b"3#33#3", b"222222", b"111111", b"#.##.#", b"111111", b"......", b"......", b"......",
        b"......",
    ],
];

/// Hand-made levels.
pub const HAND_LEVELS: u16 = HAND.len() as u16;

fn set_mirrored(cells: &mut [u8; CELLS], row: usize, col: usize, v: u8) {
    cells[row * COLS + col] = v;
    cells[row * COLS + COLS - 1 - col] = v;
}

/// Hand-made level `level` (1-based; past the eighth, round again with a
/// hit more per brick, three at most).
fn hand(level: u16) -> [u8; CELLS] {
    let k = level.max(1) - 1;
    let layout = &HAND[usize::from(k % HAND_LEVELS)];
    let extra = u8::try_from(k / HAND_LEVELS).unwrap_or(2);
    let mut cells = [EMPTY; CELLS];
    for (row, line) in layout.iter().enumerate() {
        for (col, &ch) in line.iter().enumerate() {
            let v = match ch {
                b'1'..=b'3' => brick((ch - b'0').saturating_add(extra).min(3)),
                b'#' => SOLID,
                _ => continue,
            };
            set_mirrored(&mut cells, row, col, v);
        }
    }
    cells
}

/// Generated level `level` from `seed`: a symmetric pattern, rows, density
/// and hardness rising with the level; every breakable brick reachable.
fn generate(level: u16, seed: u64) -> [u8; CELLS] {
    let mut r = Rng::new(seed ^ u64::from(level).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let d = u32::from(level.min(30));
    let top = r.below(2) as usize;
    let rows = (4 + d as usize / 3).min(ROWS - top);
    let pattern = r.below(5);
    let density = u16::try_from((450 + 15 * d).min(850)).unwrap_or(850);
    let p2 = u16::try_from((50 + 30 * d).min(500)).unwrap_or(500);
    let p3 = u16::try_from((15 * d).min(300)).unwrap_or(300);
    let solid = if d < 4 { 0 } else { u16::try_from((10 * (d - 3)).min(80)).unwrap_or(80) };
    let mut cells = [EMPTY; CELLS];
    for row in 0..rows {
        for col in 0..COLS / 2 {
            let on = match pattern {
                0 => r.chance(density),
                1 => row % 2 == 0 || r.chance(density / 4),
                2 => (row + col) % 2 == 0,
                3 => col + row >= COLS / 2 - 1,
                _ => (row / 2 + col / 2) % 2 == 0,
            };
            if !on {
                continue;
            }
            let v = if row + 1 < rows && r.chance(solid) {
                SOLID
            } else {
                brick((1 + u8::from(r.chance(p2)) + u8::from(r.chance(p3))).min(3))
            };
            set_mirrored(&mut cells, top + row, col, v);
        }
    }
    open_pockets(&mut cells);
    if cells.iter().filter(|&&c| breakable(c)).count() < 6 {
        let row = top + rows - 1;
        for col in 0..COLS / 2 {
            if cells[row * COLS + col] == EMPTY {
                set_mirrored(&mut cells, row, col, brick(1));
            }
        }
    }
    cells
}

/// The cells a ball can reach (breaking what it meets): a breadth-first
/// search from the grid's edges (the field around the grid is open)
/// through every cell that is not unbreakable. A cell is larger than the
/// ball on both axes, so neighbouring open cells always let it through.
#[must_use]
pub fn reachable(cells: &[u8; CELLS]) -> [bool; CELLS] {
    let mut seen = [false; CELLS];
    let mut queue = [0u8; CELLS];
    let (mut head, mut tail) = (0, 0);
    for i in 0..CELLS {
        let (r, c) = (i / COLS, i % COLS);
        if (r == 0 || r == ROWS - 1 || c == 0 || c == COLS - 1) && cells[i] != SOLID {
            seen[i] = true;
            queue[tail] = i as u8;
            tail += 1;
        }
    }
    while head < tail {
        let i = usize::from(queue[head]);
        head += 1;
        let (r, c) = (i / COLS, i % COLS);
        let mut visit = |j: usize| {
            if !seen[j] && cells[j] != SOLID {
                seen[j] = true;
                queue[tail] = j as u8;
                tail += 1;
            }
        };
        if r > 0 {
            visit(i - COLS);
        }
        if r + 1 < ROWS {
            visit(i + COLS);
        }
        if c > 0 {
            visit(i - 1);
        }
        if c + 1 < COLS {
            visit(i + 1);
        }
    }
    seen
}

/// Unbreakable bricks that close off a pocket become three-hit bricks,
/// until every open cell can be reached.
fn open_pockets(cells: &mut [u8; CELLS]) {
    loop {
        let seen = reachable(cells);
        let before = *cells;
        let mut changed = false;
        for i in 0..CELLS {
            if seen[i] || before[i] == SOLID {
                continue;
            }
            let (r, c) = (i / COLS, i % COLS);
            for (ok, j) in [
                (r > 0, i.wrapping_sub(COLS)),
                (r + 1 < ROWS, i + COLS),
                (c > 0, i.wrapping_sub(1)),
                (c + 1 < COLS, i + 1),
            ] {
                if ok && cells[j] == SOLID {
                    cells[j] = brick(3);
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
    }
}

/// The bricks of `level` (1-based) of a level set and seed, without
/// capsules.
#[must_use]
pub fn layout(set: LevelSet, level: u16, seed: u64) -> [u8; CELLS] {
    match set {
        LevelSet::HandMade => hand(level),
        LevelSet::Generated => generate(level, seed),
        LevelSet::Mixed if level <= HAND_LEVELS => hand(level),
        LevelSet::Mixed => generate(level, seed),
    }
}

/// The cells a rectangle (1/256 px, half-open) overlaps, as a range of
/// rows and columns (empty when it misses the grid).
fn cell_span(x0: i32, y0: i32, x1: i32, y1: i32) -> Option<(usize, usize, usize, usize)> {
    let (gx0, gx1) = (x0 - BRICKS_X * ONE, x1 - BRICKS_X * ONE);
    let (gy0, gy1) = (y0 - BRICKS_Y * ONE, y1 - BRICKS_Y * ONE);
    let (gw, gh) = (COLS as i32 * BRICK_W * ONE, ROWS as i32 * BRICK_H * ONE);
    if gx1 <= 0 || gy1 <= 0 || gx0 >= gw || gy0 >= gh {
        return None;
    }
    let c0 = gx0.max(0) / (BRICK_W * ONE);
    let c1 = (gx1 - 1).min(gw - 1) / (BRICK_W * ONE);
    let r0 = gy0.max(0) / (BRICK_H * ONE);
    let r1 = (gy1 - 1).min(gh - 1) / (BRICK_H * ONE);
    Some((r0 as usize, r1 as usize, c0 as usize, c1 as usize))
}

impl Bricks {
    #[must_use]
    pub fn new(cfg: Config, seed: u64) -> Bricks {
        let cfg = cfg.sanitised();
        let w = cfg.base_width();
        let mut g = Bricks {
            cfg,
            seed,
            rng: Rng::new(seed),
            cells: [EMPTY; CELLS],
            balls: [Ball::default(); MAX_BALLS],
            caps: [Capsule::default(); MAX_CAPS],
            paddle_x: (FIELD_W - i32::from(w)) / 2 * ONE,
            paddle_w: w,
            wide: w,
            held: 0,
            dir: 0,
            run: 0,
            phase: Phase::Serve,
            level: u16::from(cfg.start_level),
            lives: cfg.lives,
            left: 0,
            speed: 0,
            rally: 0,
            timer: 0,
            catch: 0,
            stall: 0,
            score: 0,
            broken: [0; 3],
            cleared: 0,
            steps: 0,
        };
        g.load_level();
        g
    }

    /// A game whose first level is `cells` (tests and the simulator).
    #[must_use]
    pub fn with_cells(cfg: Config, seed: u64, cells: &[u8; CELLS]) -> Bricks {
        let mut g = Bricks::new(cfg, seed);
        g.set_cells(cells);
        g
    }

    /// The bricks become `cells` (invalid cells dropped); the ball is
    /// served again.
    pub fn set_cells(&mut self, cells: &[u8; CELLS]) {
        for (dst, &c) in self.cells.iter_mut().zip(cells) {
            *dst = if valid_cell(c) { c } else { EMPTY };
        }
        self.left = self.count_left();
        if self.left == 0 {
            self.cells[CELLS - COLS / 2] = brick(1);
            self.left = 1;
        }
        self.serve();
    }

    fn count_left(&self) -> u8 {
        u8::try_from(self.cells.iter().filter(|&&c| breakable(c)).count()).unwrap_or(u8::MAX)
    }

    /// The speed a level starts at.
    fn level_speed(&self) -> i32 {
        let base = self.cfg.base_speed();
        (base + base * i32::from(self.level.saturating_sub(1).min(10)) / 40).min(MAX_SPEED)
    }

    /// The fastest the ball gets in this level by itself.
    fn speed_cap(&self) -> i32 {
        (self.level_speed() * 3 / 2).min(MAX_SPEED)
    }

    fn load_level(&mut self) {
        self.cells = layout(self.cfg.set, self.level, self.seed);
        if self.cfg.powerups {
            for c in &mut self.cells {
                if breakable(*c) && self.rng.chance(CARRY_PER_MILLE) {
                    *c |= CARRY;
                }
            }
        }
        self.left = self.count_left();
        let w = self.cfg.base_width();
        self.wide = w;
        self.set_width(w);
        self.serve();
    }

    /// The paddle `w` wide, about its centre, inside the walls.
    fn set_width(&mut self, w: u8) {
        let centre = self.paddle_x + i32::from(self.paddle_w) * ONE / 2;
        self.paddle_w = w;
        let x = centre - i32::from(w) * ONE / 2;
        self.paddle_x = x.clamp(0, (FIELD_W - i32::from(w)) * ONE);
    }

    /// A new ball on the paddle; capsules gone, the paddle back to its
    /// size, the level's speed.
    fn serve(&mut self) {
        self.balls = [Ball::default(); MAX_BALLS];
        self.caps = [Capsule::default(); MAX_CAPS];
        let w = self.cfg.base_width();
        self.wide = w;
        self.set_width(w);
        self.balls[0] = Ball {
            x: self.paddle_x + (i32::from(w) - BALL) * ONE / 2,
            y: (PADDLE_Y - BALL) * ONE,
            ang: SERVE_ANGLE,
            flags: LIVE | STUCK | UP,
            hold: 0,
        };
        self.phase = Phase::Serve;
        self.catch = 0;
        self.stall = 0;
        self.rally = 0;
        self.speed = u16::try_from(self.level_speed()).unwrap_or(0);
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

    #[must_use]
    pub fn cells(&self) -> &[u8; CELLS] {
        &self.cells
    }

    #[must_use]
    pub fn cell(&self, row: usize, col: usize) -> u8 {
        self.cells.get(row * COLS + col).copied().unwrap_or(EMPTY)
    }

    #[must_use]
    pub fn balls(&self) -> &[Ball; MAX_BALLS] {
        &self.balls
    }

    #[must_use]
    pub fn capsules(&self) -> &[Capsule; MAX_CAPS] {
        &self.caps
    }

    /// The paddle's left edge (1/256 px) and width (px).
    #[must_use]
    pub fn paddle(&self) -> (i32, u8) {
        (self.paddle_x, self.paddle_w)
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    #[must_use]
    pub fn level(&self) -> u16 {
        self.level
    }

    #[must_use]
    pub fn lives(&self) -> u8 {
        self.lives
    }

    #[must_use]
    pub fn score(&self) -> u32 {
        self.score
    }

    /// Breakable bricks left in the level.
    #[must_use]
    pub fn bricks_left(&self) -> u8 {
        self.left
    }

    /// Bricks broken by strength (1, 2, 3 hits).
    #[must_use]
    pub fn broken(&self) -> [u32; 3] {
        self.broken
    }

    /// Levels cleared.
    #[must_use]
    pub fn cleared(&self) -> u16 {
        self.cleared
    }

    #[must_use]
    pub fn speed(&self) -> i32 {
        i32::from(self.speed)
    }

    /// Steps of the catch power-up left.
    #[must_use]
    pub fn catching(&self) -> u16 {
        self.catch
    }

    #[must_use]
    pub fn steps(&self) -> u32 {
        self.steps
    }

    #[must_use]
    pub fn ended(&self) -> bool {
        self.phase == Phase::Over
    }

    // ------------------------------------------------------------ input

    const LEFT_KEYS: u8 = Button::Left.bit() | Button::Down.bit();
    const RIGHT_KEYS: u8 = Button::Right.bit() | Button::Up.bit();

    /// A key edge: Prev/Down move left, Next/Up right (the last pressed
    /// wins while both are held), OK or Menu launches.
    pub fn input(&mut self, i: Input) -> StepOut {
        match i {
            Input::Press(b) => {
                self.held |= b.bit();
                match b {
                    Button::Left | Button::Down => {
                        self.dir = -1;
                        self.run = 0;
                    }
                    Button::Right | Button::Up => {
                        self.dir = 1;
                        self.run = 0;
                    }
                    Button::Ok | Button::Menu => self.launch(),
                }
            }
            Input::Release(b) => {
                self.held &= !b.bit();
                let (l, r) = (self.held & Self::LEFT_KEYS != 0, self.held & Self::RIGHT_KEYS != 0);
                let dir = match (self.dir, l, r) {
                    (-1, false, true) | (1, _, true) => 1,
                    (1, true, false) | (-1, true, _) => -1,
                    _ => 0,
                };
                if dir != self.dir {
                    self.dir = dir;
                    self.run = 0;
                }
            }
            Input::Long(_) => {}
        }
        StepOut::default()
    }

    /// Forget held keys (a pause).
    pub fn release_all(&mut self) {
        self.held = 0;
        self.dir = 0;
        self.run = 0;
    }

    /// Take a new paddle speed (settings changed during a pause).
    pub fn set_handling(&mut self, sens: u8) {
        self.cfg.sens = sens.min(2);
    }

    /// OK: the served ball goes; caught balls go.
    fn launch(&mut self) {
        match self.phase {
            Phase::Serve => {
                self.phase = Phase::Play;
                for i in 0..MAX_BALLS {
                    if self.balls[i].stuck() {
                        let b = &mut self.balls[i];
                        b.flags = (b.flags & !(STUCK | LEFT)) | UP | if self.dir < 0 { LEFT } else { 0 };
                        b.ang = SERVE_ANGLE;
                    }
                }
            }
            Phase::Play => self.release_caught(),
            Phase::Cleared | Phase::Over => {}
        }
    }

    fn release_caught(&mut self) {
        for i in 0..MAX_BALLS {
            if self.balls[i].live() && self.balls[i].stuck() {
                self.aim(i);
                self.balls[i].flags &= !STUCK;
                self.balls[i].hold = 0;
            }
        }
    }

    /// Up and away from the paddle, at the angle of where the ball meets
    /// it: centre nearly straight, the ends at 60°.
    fn aim(&mut self, i: usize) {
        let w = i32::from(self.paddle_w);
        let b = &mut self.balls[i];
        let off = b.x + BALL * ONE / 2 - (self.paddle_x + w * ONE / 2);
        let half = (w + BALL) * ONE / 2;
        b.ang = u8::try_from((off.abs() * 7 / half).min(6)).unwrap_or(6);
        b.flags |= UP;
        if off < 0 {
            b.flags |= LEFT;
        } else if off > 0 {
            b.flags &= !LEFT;
        }
    }

    // ------------------------------------------------------------ steps

    /// One fixed step of `1 / STEP_HZ` s.
    pub fn step(&mut self) -> StepOut {
        let mut out = StepOut::default();
        match self.phase {
            Phase::Over => return out,
            Phase::Cleared => {
                self.steps = self.steps.saturating_add(1);
                self.timer = self.timer.saturating_sub(1);
                if self.timer == 0 {
                    self.level = self.level.saturating_add(1);
                    self.load_level();
                }
                return out;
            }
            Phase::Serve | Phase::Play => {}
        }
        self.steps = self.steps.saturating_add(1);
        self.move_paddle();
        self.grow_paddle();
        if self.phase == Phase::Serve {
            return out;
        }
        self.rally = self.rally.wrapping_add(1);
        if self.rally % SPEEDUP_STEPS == 0 {
            let up = self.cfg.base_speed() / 24;
            let s = (i32::from(self.speed) + up).min(self.speed_cap().max(i32::from(self.speed)));
            self.speed = u16::try_from(s).unwrap_or(self.speed);
        }
        self.catch = self.catch.saturating_sub(1);
        self.stall = self.stall.saturating_add(1);
        for i in 0..MAX_BALLS {
            self.move_ball(i, &mut out);
        }
        self.move_caps(&mut out);
        if self.left == 0 {
            self.phase = Phase::Cleared;
            self.timer = CLEAR_STEPS;
            self.cleared = self.cleared.saturating_add(1);
            self.score = self.score.saturating_add(CLEAR_POINTS);
            out.cleared = true;
        } else if !self.balls.iter().any(Ball::live) {
            out.lost = true;
            self.lives = self.lives.saturating_sub(1);
            if self.lives == 0 {
                self.phase = Phase::Over;
                self.caps = [Capsule::default(); MAX_CAPS];
                out.ended = true;
            } else {
                self.serve();
            }
        }
        out
    }

    /// A free ball in the paddle's band (beside it, never in it).
    fn in_band(b: &Ball) -> bool {
        b.live() && !b.stuck() && b.y < (PADDLE_Y + PADDLE_H) * ONE && b.y + BALL * ONE > PADDLE_Y * ONE
    }

    fn move_paddle(&mut self) {
        if self.dir == 0 {
            self.run = 0;
            return;
        }
        let s = usize::from(self.cfg.sens.min(2));
        let (a, top) = (PADDLE_START[s], PADDLE_TOP[s]);
        let v = (a + (top - a) * i32::from(self.run) / PADDLE_RAMP).min(top);
        self.run = self.run.saturating_add(1);
        let w = i32::from(self.paddle_w) * ONE;
        let mut d = (i32::from(self.dir) * v).min(FIELD_W * ONE - w - self.paddle_x).max(-self.paddle_x);
        for b in &self.balls {
            if !Self::in_band(b) {
                continue;
            }
            if d > 0 && b.x >= self.paddle_x + w {
                d = d.min(b.x - (self.paddle_x + w));
            } else if d < 0 && b.x + BALL * ONE <= self.paddle_x {
                d = d.max(b.x + BALL * ONE - self.paddle_x);
            }
        }
        self.paddle_x += d;
        for b in &mut self.balls {
            if b.live() && b.stuck() {
                b.x += d;
            }
        }
    }

    /// The wider power-up: a pixel a step, on the right if free, else on
    /// the left (never into a wall or a ball beside the paddle).
    fn grow_paddle(&mut self) {
        if self.paddle_w >= self.wide {
            return;
        }
        let right = self.paddle_x + i32::from(self.paddle_w) * ONE;
        let blocked = |x0: i32, x1: i32| {
            self.balls.iter().any(|b| Self::in_band(b) && b.x < x1 && b.x + BALL * ONE > x0)
        };
        if right + ONE <= FIELD_W * ONE && !blocked(right, right + ONE) {
            self.paddle_w += 1;
        } else if self.paddle_x >= ONE && !blocked(self.paddle_x - ONE, self.paddle_x) {
            self.paddle_x -= ONE;
            self.paddle_w += 1;
        }
    }

    fn overlaps_paddle(&self, x: i32, y: i32) -> bool {
        x < self.paddle_x + i32::from(self.paddle_w) * ONE
            && x + BALL * ONE > self.paddle_x
            && y < (PADDLE_Y + PADDLE_H) * ONE
            && y + BALL * ONE > PADDLE_Y * ONE
    }

    /// Whether a ball at (x, y) would overlap a brick.
    fn in_brick(&self, x: i32, y: i32) -> bool {
        let Some((r0, r1, c0, c1)) = cell_span(x, y, x + BALL * ONE, y + BALL * ONE) else { return false };
        (r0..=r1).any(|r| (c0..=c1).any(|c| self.cells[r * COLS + c] != EMPTY))
    }

    fn move_ball(&mut self, i: usize, out: &mut StepOut) {
        let b = self.balls[i];
        if !b.live() {
            return;
        }
        if b.stuck() {
            if self.phase == Phase::Play {
                self.balls[i].hold += 1;
                if self.balls[i].hold >= HOLD_STEPS {
                    self.aim(i);
                    self.balls[i].flags &= !STUCK;
                    self.balls[i].hold = 0;
                }
            }
            return;
        }
        let a = usize::from(b.ang.min(6));
        let s = i32::from(self.speed);
        let (ax, ay) = ((s * SIN[a]) >> 12, (s * COS[a]) >> 12);
        // Sub-steps of at most a pixel on each axis.
        let n = ((ax.max(ay) + ONE - 1) / ONE).max(1);
        for k in 0..n {
            let mx = ax * (k + 1) / n - ax * k / n;
            let my = ay * (k + 1) / n - ay * k / n;
            if mx > 0 {
                self.axis(i, mx, true, out);
            }
            if !self.balls[i].live() || self.balls[i].stuck() {
                break;
            }
            if my > 0 {
                self.axis(i, my, false, out);
            }
            if !self.balls[i].live() || self.balls[i].stuck() {
                break;
            }
        }
        if self.balls[i].y >= FIELD_H * ONE {
            self.balls[i] = Ball::default();
        }
    }

    /// Move ball `i` by `m` (1/256 px, at most a pixel) along one axis in
    /// its direction, or turn it there.
    fn axis(&mut self, i: usize, m: i32, horizontal: bool, out: &mut StepOut) {
        let b = self.balls[i];
        let (nx, ny) = if horizontal {
            (if b.left() { b.x - m } else { b.x + m }, b.y)
        } else {
            (b.x, if b.up() { b.y - m } else { b.y + m })
        };
        let flip = if horizontal { LEFT } else { UP };
        if nx < 0 || nx + BALL * ONE > FIELD_W * ONE || ny < 0 {
            self.balls[i].flags ^= flip;
            self.nudge(i);
            return;
        }
        if let Some((r0, r1, c0, c1)) = cell_span(nx, ny, nx + BALL * ONE, ny + BALL * ONE) {
            let mut hit = false;
            let mut solid_only = true;
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let cell = r * COLS + c;
                    if self.cells[cell] != EMPTY {
                        hit = true;
                        solid_only &= self.cells[cell] == SOLID;
                        self.hit(cell, out);
                    }
                }
            }
            if hit {
                self.balls[i].flags ^= flip;
                if solid_only {
                    self.nudge(i);
                }
                return;
            }
        }
        if self.overlaps_paddle(nx, ny) {
            if horizontal || b.up() {
                self.balls[i].flags ^= flip;
                return;
            }
            self.stall = 0;
            self.aim(i);
            if self.catch > 0 {
                let w = i32::from(self.paddle_w);
                let ball = &mut self.balls[i];
                ball.flags |= STUCK;
                ball.hold = 0;
                ball.y = (PADDLE_Y - BALL) * ONE;
                ball.x = ball.x.clamp(self.paddle_x, self.paddle_x + (w - BALL) * ONE);
            }
            return;
        }
        self.balls[i].x = nx;
        self.balls[i].y = ny;
    }

    /// A ball that has met only walls and unbreakable bricks for a while
    /// turns to the next angle at its next such bounce.
    fn nudge(&mut self, i: usize) {
        if self.stall >= STALL_STEPS {
            self.stall = 0;
            let b = &mut self.balls[i];
            b.ang = if b.ang >= 5 { 1 } else { b.ang + 1 };
        }
    }

    fn hit(&mut self, cell: usize, out: &mut StepOut) {
        let v = self.cells[cell];
        if !breakable(v) {
            return;
        }
        self.stall = 0;
        if hits_left(v) > 1 {
            self.cells[cell] = v - 1;
            out.hits = out.hits.saturating_add(1);
            return;
        }
        let s = strength(v).clamp(1, 3);
        self.cells[cell] = EMPTY;
        self.left = self.left.saturating_sub(1);
        self.broken[usize::from(s - 1)] += 1;
        self.score = self.score.saturating_add(BRICK_POINTS * u32::from(s));
        out.broke = out.broke.saturating_add(1);
        if v & CARRY != 0
            && self.cfg.powerups
            && let Some(slot) = self.caps.iter_mut().find(|c| c.kind == 0)
        {
            let (r, c) = ((cell / COLS) as i32, (cell % COLS) as i32);
            *slot = Capsule {
                x: u8::try_from(BRICKS_X + c * BRICK_W + 1).unwrap_or(0),
                y: (BRICKS_Y + r * BRICK_H) * ONE,
                kind: u8::try_from(self.rng.below(Power::ALL.len() as u32)).unwrap_or(0) + 1,
            };
        }
    }

    fn move_caps(&mut self, out: &mut StepOut) {
        let (px, pw) = (self.paddle_x, i32::from(self.paddle_w) * ONE);
        for k in 0..MAX_CAPS {
            let c = &mut self.caps[k];
            if c.kind == 0 {
                continue;
            }
            c.y += CAP_FALL;
            let x = i32::from(c.x) * ONE;
            let on_paddle = x < px + pw
                && x + CAP_W * ONE > px
                && c.y < (PADDLE_Y + PADDLE_H) * ONE
                && c.y + CAP_H * ONE > PADDLE_Y * ONE;
            if on_paddle {
                let p = c.power();
                c.kind = 0;
                if let Some(p) = p {
                    out.caught = Some(p);
                    self.apply(p);
                }
            } else if c.y >= FIELD_H * ONE {
                c.kind = 0;
            }
        }
    }

    fn apply(&mut self, p: Power) {
        match p {
            Power::Wide => self.wide = self.wide.saturating_add(WIDE_STEP).min(PADDLE_MAX),
            Power::Slow => {
                let s = (i32::from(self.speed) * 3 / 4).max(MIN_SPEED);
                self.speed = u16::try_from(s).unwrap_or(self.speed);
                self.rally = 0;
            }
            Power::Split => {
                self.release_caught();
                let Some(src) = self.balls.iter().copied().find(Ball::live) else { return };
                let mut turn = 2u8;
                for b in &mut self.balls {
                    if !b.live() {
                        *b = Ball {
                            ang: if src.ang >= 4 { src.ang - turn } else { src.ang + turn },
                            flags: src.flags ^ LEFT,
                            ..src
                        };
                        turn = 1;
                    }
                }
            }
            Power::Catch => self.catch = CATCH_STEPS,
            Power::Life => self.lives = self.lives.saturating_add(1).min(MAX_LIVES),
        }
    }

    // ------------------------------------------------------------ tests and the simulator

    /// Ball `i` free at (x, y) (1/256 px) in direction `ang`, playing
    /// (ball 0: the others gone). False, and nothing changed, if it would
    /// overlap something there.
    pub fn place_ball(&mut self, i: usize, x: i32, y: i32, ang: u8, left: bool, up: bool) -> bool {
        if i >= MAX_BALLS
            || x < 0
            || x + BALL * ONE > FIELD_W * ONE
            || !(0..FIELD_H * ONE).contains(&y)
            || ang > 6
            || self.in_brick(x, y)
            || self.overlaps_paddle(x, y)
            || matches!(self.phase, Phase::Over | Phase::Cleared)
        {
            return false;
        }
        if i == 0 {
            self.balls = [Ball::default(); MAX_BALLS];
        }
        let flags = LIVE | if left { LEFT } else { 0 } | if up { UP } else { 0 };
        self.balls[i] = Ball { x, y, ang, flags, hold: 0 };
        self.phase = Phase::Play;
        true
    }

    /// The ball speed (1/256 px a step), within [`MIN_SPEED`]..=[`MAX_SPEED`].
    pub fn set_speed(&mut self, s: i32) {
        self.speed = u16::try_from(s.clamp(MIN_SPEED, MAX_SPEED)).unwrap_or(0);
    }

    /// The paddle's left edge (1/256 px), if no ball is in the way.
    pub fn set_paddle(&mut self, x: i32) -> bool {
        let x = x.clamp(0, (FIELD_W - i32::from(self.paddle_w)) * ONE);
        let old = self.paddle_x;
        self.paddle_x = x;
        if self.balls.iter().any(|b| b.live() && !b.stuck() && self.overlaps_paddle(b.x, b.y)) {
            self.paddle_x = old;
            return false;
        }
        for b in &mut self.balls {
            if b.live() && b.stuck() {
                b.x += x - old;
            }
        }
        true
    }

    /// A capsule of `p` at (x px, y px).
    pub fn drop_capsule(&mut self, p: Power, x: u8, y: i32) {
        if let Some(slot) = self.caps.iter_mut().find(|c| c.kind == 0) {
            *slot = Capsule {
                x: x.min(u8::try_from(FIELD_W - CAP_W).unwrap_or(0)),
                y: y.clamp(0, FIELD_H - 1) * ONE,
                kind: p as u8,
            };
        }
    }

    /// The level's breakable bricks gone (unscored): cleared now.
    pub fn clear_now(&mut self) {
        if !matches!(self.phase, Phase::Serve | Phase::Play) {
            return;
        }
        for c in &mut self.cells {
            if breakable(*c) {
                *c = EMPTY;
            }
        }
        self.left = 0;
        self.phase = Phase::Cleared;
        self.timer = CLEAR_STEPS;
        self.cleared = self.cleared.saturating_add(1);
        self.score = self.score.saturating_add(CLEAR_POINTS);
    }

    /// Go to level `n` now (served).
    pub fn goto_level(&mut self, n: u16) {
        if self.phase != Phase::Over {
            self.level = n.max(1);
            self.load_level();
        }
    }

    /// Lives left (1 ..= 9).
    pub fn set_lives(&mut self, n: u8) {
        if self.phase != Phase::Over {
            self.lives = n.clamp(1, MAX_LIVES);
        }
    }

    // ------------------------------------------------------------ invariants

    /// The invariants, for tests and fuzzing: `Err` names the first one
    /// broken.
    pub fn check(&self) -> Result<(), &'static str> {
        // Values come from save files (`decode`): compared without sums
        // that can overflow (VER-3, 2026-11-27).
        let (px, pw) = (self.paddle_x, i32::from(self.paddle_w));
        if px < 0 || px > FIELD_W * ONE - pw * ONE {
            return Err("paddle outside the walls");
        }
        let base = self.cfg.base_width();
        if self.paddle_w < base || self.paddle_w > self.wide || self.wide > PADDLE_MAX {
            return Err("paddle width");
        }
        let mut live = 0;
        let mut stuck = 0;
        for b in &self.balls {
            if !b.live() {
                continue;
            }
            live += 1;
            if b.ang > 6 {
                return Err("ball angle");
            }
            if b.x < 0 || b.x > FIELD_W * ONE - BALL * ONE || b.y < 0 {
                return Err("ball inside a wall");
            }
            if b.y >= FIELD_H * ONE {
                return Err("lost ball still in play");
            }
            if self.in_brick(b.x, b.y) {
                return Err("ball inside a brick");
            }
            if self.overlaps_paddle(b.x, b.y) {
                return Err("ball inside the paddle");
            }
            if b.stuck() {
                stuck += 1;
                if b.y != (PADDLE_Y - BALL) * ONE || b.x < px || b.x + BALL * ONE > px + pw * ONE {
                    return Err("held ball off the paddle");
                }
            }
        }
        if !self.cells.iter().all(|&c| valid_cell(c)) {
            return Err("bad brick");
        }
        if self.left != self.count_left() {
            return Err("bricks left miscounted");
        }
        let points = self
            .broken
            .iter()
            .zip(1u64..)
            .map(|(&n, s)| u64::from(n) * s * u64::from(BRICK_POINTS))
            .sum::<u64>()
            + u64::from(self.cleared) * u64::from(CLEAR_POINTS);
        if u64::from(self.score) != points {
            return Err("score does not follow the bricks broken");
        }
        if self.lives > MAX_LIVES {
            return Err("lives");
        }
        if !(MIN_SPEED..=MAX_SPEED).contains(&i32::from(self.speed)) {
            return Err("ball speed");
        }
        match self.phase {
            Phase::Serve if stuck == 0 || stuck != live => return Err("serve without a ball on the paddle"),
            Phase::Play if live == 0 => return Err("play without a ball"),
            Phase::Serve | Phase::Play if self.left == 0 => return Err("a cleared level still played"),
            Phase::Cleared if self.left != 0 => return Err("cleared with bricks left"),
            Phase::Over if self.lives != 0 => return Err("over with lives left"),
            _ => {}
        }
        for c in &self.caps {
            if c.kind != 0
                && (c.power().is_none()
                    || i32::from(c.x) + CAP_W > FIELD_W
                    || !(0..FIELD_H * ONE).contains(&c.y))
            {
                return Err("bad capsule");
            }
        }
        if self.cfg.sanitised() != self.cfg {
            return Err("config");
        }
        Ok(())
    }

    // ------------------------------------------------------------ files

    pub fn encode(&self, w: &mut Writer<'_>) {
        let c = &self.cfg;
        for v in [c.start_level, c.speed, c.paddle, c.lives, u8::from(c.powerups), c.set as u8, c.sens] {
            w.u8(v);
        }
        w.u64(self.seed);
        w.u64(self.rng.state());
        w.bytes(&self.cells);
        for b in &self.balls {
            w.u32(b.x as u32);
            w.u32(b.y as u32);
            w.u8(b.ang);
            w.u8(b.flags);
            w.u16(b.hold);
        }
        for c in &self.caps {
            w.u8(c.x);
            w.u32(c.y as u32);
            w.u8(c.kind);
        }
        w.u32(self.paddle_x as u32);
        w.u8(self.paddle_w);
        w.u8(self.wide);
        w.u8(self.held);
        w.i8(self.dir);
        w.u16(self.run);
        w.u8(self.phase as u8);
        w.u16(self.level);
        w.u8(self.lives);
        w.u16(self.speed);
        w.u16(self.rally);
        w.u16(self.timer);
        w.u16(self.catch);
        w.u16(self.stall);
        w.u32(self.score);
        for n in self.broken {
            w.u32(n);
        }
        w.u16(self.cleared);
        w.u32(self.steps);
    }

    /// Read a game written by [`Bricks::encode`]; `None` for anything out
    /// of range or a state the rules cannot reach.
    pub fn decode(r: &mut Reader<'_>) -> Option<Bricks> {
        let cfg = Config {
            start_level: r.u8()?,
            speed: r.u8()?,
            paddle: r.u8()?,
            lives: r.u8()?,
            powerups: r.bool()?,
            set: LevelSet::from_u8(r.u8()?)?,
            sens: r.u8()?,
        };
        if cfg.sanitised() != cfg {
            return None;
        }
        let seed = r.u64()?;
        let rng = Rng::new(r.u64()?);
        let cells: [u8; CELLS] = r.array()?;
        let mut balls = [Ball::default(); MAX_BALLS];
        for b in &mut balls {
            *b =
                Ball { x: r.u32()? as i32, y: r.u32()? as i32, ang: r.u8()?, flags: r.u8()?, hold: r.u16()? };
            if b.flags >> 4 != 0 || b.hold > HOLD_STEPS || (!b.live() && *b != Ball::default()) {
                return None;
            }
        }
        let mut caps = [Capsule::default(); MAX_CAPS];
        for c in &mut caps {
            *c = Capsule { x: r.u8()?, y: r.u32()? as i32, kind: r.u8()? };
        }
        let g = Bricks {
            cfg,
            seed,
            rng,
            cells,
            balls,
            caps,
            paddle_x: r.u32()? as i32,
            paddle_w: r.u8()?,
            wide: r.u8()?,
            held: r.u8()?,
            dir: r.i8()?,
            run: r.u16()?,
            phase: match r.u8()? {
                0 => Phase::Serve,
                1 => Phase::Play,
                2 => Phase::Cleared,
                3 => Phase::Over,
                _ => return None,
            },
            level: r.u16()?,
            lives: r.u8()?,
            speed: r.u16()?,
            rally: r.u16()?,
            timer: r.u16()?,
            catch: r.u16()?,
            stall: r.u16()?,
            score: r.u32()?,
            broken: [r.u32()?, r.u32()?, r.u32()?],
            cleared: r.u16()?,
            steps: r.u32()?,
            left: 0,
        };
        let mut g = g;
        g.left = g.count_left();
        let valid = (-1..=1).contains(&g.dir)
            && g.held >> Button::ALL.len() == 0
            && g.level >= 1
            && g.timer <= CLEAR_STEPS
            && g.catch <= CATCH_STEPS
            && (g.phase == Phase::Cleared) == (g.timer > 0)
            && g.check().is_ok();
        valid.then_some(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_levels_are_reachable_and_symmetric() {
        for set in [LevelSet::HandMade, LevelSet::Generated, LevelSet::Mixed] {
            for level in 1..=60u16 {
                for seed in [0u64, 1, 0xDEAD_BEEF] {
                    let cells = layout(set, level, seed);
                    let seen = reachable(&cells);
                    let n = cells.iter().filter(|&&c| breakable(c)).count();
                    assert!(n >= 6, "{set:?} level {level}: {n} bricks");
                    for i in 0..CELLS {
                        assert!(
                            !breakable(cells[i]) || seen[i],
                            "{set:?} level {level} cell {i} out of reach"
                        );
                        let (r, c) = (i / COLS, i % COLS);
                        assert_eq!(cells[i], cells[r * COLS + COLS - 1 - c], "symmetric");
                    }
                }
            }
        }
        // The hand-made set goes round harder.
        let a = layout(LevelSet::HandMade, 1, 0);
        let b = layout(LevelSet::HandMade, 1 + HAND_LEVELS, 0);
        assert!(
            a.iter().zip(&b).all(|(&x, &y)| x == y || (breakable(x) && hits_left(y) == hits_left(x) + 1))
        );
    }

    #[test]
    fn pockets_are_opened() {
        let mut cells = [EMPTY; CELLS];
        // A ring of unbreakable bricks around one brick.
        for (r, c) in [(3, 4), (3, 5), (3, 6), (4, 4), (4, 6), (5, 4), (5, 5), (5, 6)] {
            cells[r * COLS + c] = SOLID;
        }
        cells[4 * COLS + 5] = brick(1);
        assert!(!reachable(&cells)[4 * COLS + 5]);
        open_pockets(&mut cells);
        assert!(reachable(&cells)[4 * COLS + 5]);
        assert_eq!(cells.iter().filter(|&&c| c == SOLID).count(), 4, "only the walls of the pocket's cell");
    }
}
