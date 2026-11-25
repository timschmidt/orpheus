//! Bodies of the orpheus-games fuzz targets (07 §3.4): key sequences into
//! each game with the invariants of 05 §7.13 asserted after every input
//! and step. Shared by the cargo-fuzz targets, the crate's corpus replay
//! (`tests/fuzz_corpus.rs`) and its property tests.
//!
//! Input layout. `tiles_keys`: size, goal, odds, undo (one byte each), a
//! 64-bit seed, then one input byte per key ([`Input::from_byte`]; other
//! bytes are skipped). `blocks_keys`: mode, start level, flags (bit 0 pure
//! random, bit 1 no hold, bit 2 no kicks), lock delay, auto-shift delay,
//! repeat (one byte each), a 64-bit seed, then per byte either an input
//! (bit 7 clear) or `(b & 0x7F) + 1` logic steps (bit 7 set).

#![allow(clippy::missing_panics_doc)]

use orpheus_games::blocks::{self, Blocks, CLEAR_POINTS, COMBO_POINTS, H, HIDDEN, ROWS, W, level_for};
use orpheus_games::tiles::{self, Dir, Tiles, TilesOut};
use orpheus_games::{Input, SAVE_MAX, Session};

/// The rule choices the app offers (`orpheus-apps` games settings).
pub const GOALS: [u8; 4] = [11, 12, 13, 0];
pub const FOURS: [u16; 4] = [0, 100, 250, 500];
pub const LOCKS: [u16; 4] = [250, 500, 750, 1000];
pub const DAS: [u16; 5] = [100, 133, 167, 200, 267];
pub const ARR: [u16; 5] = [0, 17, 33, 50, 83];

fn seed(d: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    for (dst, src) in b.iter_mut().zip(d) {
        *dst = *src;
    }
    u64::from_le_bytes(b)
}

fn pick<T: Copy>(table: &[T], b: u8) -> T {
    table[usize::from(b) % table.len()]
}

/// The 4096 configuration of an input's first four bytes.
#[must_use]
pub fn tiles_config(d: &[u8]) -> tiles::Config {
    let b = |i: usize| d.get(i).copied().unwrap_or(0);
    tiles::Config {
        size: 4 + b(0) % 3,
        goal: pick(&GOALS, b(1)),
        four_per_mille: pick(&FOURS, b(2)),
        undo: b(3) % 4,
    }
}

/// An independent slide of one line towards index 0 (compress, merge
/// neighbours once from the front, compress): the reference the game's
/// slide is checked against.
fn reference_line(line: &[u8]) -> ([u8; tiles::MAX_N], u32) {
    let mut v: [u8; tiles::MAX_N] = [0; tiles::MAX_N];
    let tiles: Vec<u8> = line.iter().copied().filter(|&e| e != 0).collect();
    let mut out = Vec::new();
    let mut gained = 0u32;
    let mut i = 0;
    while i < tiles.len() {
        if i + 1 < tiles.len() && tiles[i] == tiles[i + 1] {
            out.push(tiles[i] + 1);
            gained += 1 << (tiles[i] + 1);
            i += 2;
        } else {
            out.push(tiles[i]);
            i += 1;
        }
    }
    v[..out.len()].copy_from_slice(&out);
    (v, gained)
}

/// The board after a slide by the reference, and the score gained.
#[must_use]
pub fn reference_slide(t: &Tiles, dir: Dir) -> (Vec<u8>, u32) {
    let n = t.n();
    let mut out = vec![0u8; n * n];
    let mut gained = 0;
    for line in 0..n {
        let pos = |k: usize| match dir {
            Dir::Left => line * n + k,
            Dir::Right => line * n + (n - 1 - k),
            Dir::Up => k * n + line,
            Dir::Down => (n - 1 - k) * n + line,
        };
        let cells: Vec<u8> = (0..n).map(|k| t.cells()[pos(k)]).collect();
        let (moved, g) = reference_line(&cells);
        gained += g;
        for k in 0..n {
            out[pos(k)] = moved[k];
        }
    }
    (out, gained)
}

fn value(e: u8) -> u64 {
    if e == 0 { 0 } else { 1 << e }
}

/// Play 4096 with the input's keys; every invariant asserted. Returns the
/// final game.
pub fn tiles_run(data: &[u8]) -> Tiles {
    let cfg = tiles_config(data);
    let mut t = Tiles::new(cfg, seed(data.get(4..12).unwrap_or(&[])));
    let mut history: Vec<Tiles> = Vec::new();
    let mut buf = [0u8; SAVE_MAX];
    for (k, &b) in data.iter().skip(12).enumerate() {
        let Some(i) = Input::from_byte(b) else { continue };
        let before = t.clone();
        let refs: Vec<(Vec<u8>, u32)> = Dir::ALL.iter().map(|&d| reference_slide(&before, d)).collect();
        match t.input(i) {
            TilesOut::Moved(m) => {
                let d = Dir::ALL.iter().position(|&d| d == m.dir).unwrap();
                let (want, gained) = &refs[d];
                // The slide is the reference's, plus exactly one new tile.
                let (cell, e) = m.spawn.expect("a changing slide adds a tile");
                let mut got = t.cells().to_vec();
                assert_eq!(got[usize::from(cell)], e);
                assert_eq!(want[usize::from(cell)], 0, "the new tile is on an empty cell");
                got[usize::from(cell)] = 0;
                assert_eq!(&got, want, "slide {:?}", m.dir);
                assert_eq!(m.gained, *gained);
                assert!(e == 1 || e == 2);
                // The sum grows by the new tile only.
                assert_eq!(t.sum(), before.sum() + value(e));
                assert_eq!(t.score(), before.score() + m.gained);
                // Every tile merges at most once: a target takes one or two
                // tiles, two only if one of them merged.
                let mut into = [0u8; tiles::CELLS];
                for p in m.paths() {
                    into[usize::from(p.to)] += 1;
                }
                assert!(into.iter().all(|&c| c <= 2));
                for p in m.paths().iter().filter(|p| p.merged) {
                    assert_eq!(into[usize::from(p.to)], 2);
                }
                let merged = m.paths().iter().filter(|p| p.merged).count();
                assert_eq!(merged, usize::from(m.merges));
                let tiles_before = before.cells().iter().filter(|&&e| e != 0).count();
                let tiles_after = t.cells().iter().filter(|&&e| e != 0).count();
                assert_eq!(tiles_after, tiles_before - merged + 1);
                if cfg.undo > 0 {
                    history.push(before);
                    if history.len() > usize::from(cfg.undo) {
                        history.remove(0);
                    }
                }
            }
            TilesOut::Undone => {
                let back = history.pop().expect("undo only with a slide to take back");
                assert_eq!(t.cells(), back.cells());
                assert_eq!(t.score(), back.score());
                assert_eq!(t.phase(), back.phase());
            }
            TilesOut::KeptGoing => assert_eq!(t.phase(), tiles::Phase::Playing),
            TilesOut::None => {
                // Nothing changed — a slide that changes nothing adds no tile.
                assert_eq!(t.cells(), before.cells());
                assert_eq!(t.score(), before.score());
                if let Input::Press(b) = i
                    && let Some(d) = dir_of(b)
                    && before.phase() != tiles::Phase::Over
                {
                    assert_eq!(
                        &refs[d].0,
                        &before.cells().to_vec(),
                        "a slide that changes the board happens"
                    );
                }
            }
        }
        // Over exactly when no slide changes the board.
        let stuck = Dir::ALL.iter().all(|&d| reference_slide(&t, d).0 == t.cells());
        assert_eq!(t.over(), stuck);
        assert_eq!(t.over(), !t.can_move());
        if k % 16 == 15 {
            let n = Session::Tiles(t.clone()).encode(&mut buf).expect("fits");
            assert_eq!(Session::decode(&buf[..n], None), Some(Session::Tiles(t.clone())));
        }
    }
    t
}

fn dir_of(b: orpheus_games::Button) -> Option<usize> {
    use orpheus_games::Button;
    match b {
        Button::Up => Some(0),
        Button::Down => Some(1),
        Button::Left => Some(2),
        Button::Right => Some(3),
        _ => None,
    }
}

/// The fuzz target: the same keys twice give the same game.
pub fn tiles_keys(data: &[u8]) {
    let a = tiles_run(data);
    assert_eq!(a, tiles_run(data), "deterministic");
}

/// The Blocks configuration of an input's first six bytes.
#[must_use]
pub fn blocks_config(d: &[u8]) -> blocks::Config {
    let b = |i: usize| d.get(i).copied().unwrap_or(0);
    blocks::Config {
        mode: blocks::Mode::from_u8(b(0) % 3).unwrap_or_default(),
        start_level: 1 + b(1) % 15,
        bag: b(2) & 1 == 0,
        hold: b(2) & 2 == 0,
        kicks: b(2) & 4 == 0,
        lock_ms: pick(&LOCKS, b(3)),
        das_ms: pick(&DAS, b(4)),
        arr_ms: pick(&ARR, b(5)),
    }
}

/// The reference for Blocks' line and score counts: the locked pieces
/// placed into a plain occupancy grid, full rows removed, points counted
/// from the rules' tables with its own level and combo.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reference {
    pub rows: Vec<[bool; W]>,
    pub lines: u32,
    pub clear_points: u32,
    combo: u32,
    start: u8,
}

impl Reference {
    #[must_use]
    pub fn new(b: &Blocks) -> Reference {
        let rows = (0..ROWS).map(|y| core::array::from_fn(|x| b.cell(x, y) != 0)).collect();
        Reference { rows, lines: b.lines(), clear_points: 0, combo: 0, start: b.config().start_level }
    }

    /// A piece locked at these cells; returns the rows it cleared.
    pub fn lock(&mut self, cells: [(i8, i8); 4]) -> u8 {
        let level = u32::from(level_for(self.start, self.lines));
        for (x, y) in cells {
            let cell = &mut self.rows[usize::try_from(y).unwrap()][usize::try_from(x).unwrap()];
            assert!(!*cell, "locked onto an occupied cell");
            *cell = true;
        }
        let before = self.rows.len();
        self.rows.retain(|r| !r.iter().all(|&c| c));
        let n = before - self.rows.len();
        while self.rows.len() < ROWS {
            self.rows.insert(0, [false; W]);
        }
        if n > 0 {
            self.clear_points += CLEAR_POINTS[n] * level + self.combo * COMBO_POINTS * level;
            self.combo += 1;
            self.lines += n as u32;
        } else {
            self.combo = 0;
        }
        n as u8
    }

    /// The game's well matches (when no row is flashing).
    #[must_use]
    pub fn matches(&self, b: &Blocks) -> bool {
        (0..ROWS).all(|y| (0..W).all(|x| (b.cell(x, y) != 0) == self.rows[y][x]))
    }
}

/// Play Blocks with the input; every invariant asserted after each input
/// and step, the counts against [`Reference`]. Returns the final game.
pub fn blocks_run(data: &[u8]) -> Blocks {
    let cfg = blocks_config(data);
    let mut b = Blocks::new(cfg, seed(data.get(6..14).unwrap_or(&[])));
    let mut r = Reference::new(&b);
    let mut buf = [0u8; SAVE_MAX];
    let check = |b: &Blocks, r: &mut Reference, out: blocks::StepOut| {
        if let Err(e) = b.check() {
            panic!("invariant: {e}");
        }
        if let Some(l) = out.lock {
            assert_eq!(r.lock(l.at.cells()), l.cleared, "cleared rows");
        }
        assert_eq!(b.lines(), r.lines, "lines");
        assert_eq!(b.score(), b.drop_points() + r.clear_points, "score");
        if b.clearing().1 == 0 {
            assert!(r.matches(b), "the well is the reference's");
        }
        // The visible well is H rows below HIDDEN.
        assert_eq!(ROWS, H + HIDDEN);
    };
    for (k, &byte) in data.iter().skip(14).enumerate() {
        if byte & 0x80 != 0 {
            for _ in 0..=(byte & 0x7F) {
                let out = b.step();
                check(&b, &mut r, out);
            }
        } else if let Some(i) = Input::from_byte(byte) {
            let out = b.input(i);
            check(&b, &mut r, out);
        }
        if k % 32 == 31 {
            let n = Session::Blocks(b.clone()).encode(&mut buf).expect("fits");
            assert_eq!(Session::decode(&buf[..n], None), Some(Session::Blocks(b.clone())));
        }
    }
    b
}

/// The fuzz target: the same keys twice give the same game.
pub fn blocks_keys(data: &[u8]) {
    let a = blocks_run(data);
    assert_eq!(a, blocks_run(data), "deterministic");
}
