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
use orpheus_games::solitaire;
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

// ------------------------------------------------------------ Solitaire

/// The Solitaire configuration of an input's first byte: bit 0 draw
/// three, bits 1–2 passes (unlimited, one, three), bits 3–4 scoring, bit 5
/// no auto-move, bit 6 no auto-complete, bit 7 the deal check.
#[must_use]
pub fn solitaire_config(d: &[u8]) -> solitaire::Config {
    let b = d.first().copied().unwrap_or(0);
    solitaire::Config {
        draw: if b & 1 != 0 { 3 } else { 1 },
        passes: [0, 1, 3, 0][usize::from(b >> 1 & 3)],
        scoring: solitaire::Scoring::from_u8((b >> 3 & 3) % 3).unwrap_or_default(),
        auto_move: b & 0x20 == 0,
        auto_complete: b & 0x40 == 0,
        winnable: b & 0x80 != 0,
    }
}

/// An independent model of the piles: cards with their faces, and the
/// rules written out plainly. Every move the game records is applied to it
/// and checked; the result must equal the game's board.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SolRef {
    pub piles: Vec<Vec<(u8, bool)>>,
    pub passes: u8,
}

fn sol_red(c: u8) -> bool {
    (c / 13) % 2 == 1
}

impl SolRef {
    #[must_use]
    pub fn of(b: &solitaire::Board) -> SolRef {
        let piles = (0..solitaire::PILES as u8)
            .map(|p| {
                let d = b.down(p);
                b.pile(p).iter().enumerate().map(|(i, &c)| (c, i >= d)).collect()
            })
            .collect();
        SolRef { piles, passes: b.passes() }
    }

    /// Apply a recorded move if the rules allow it.
    pub fn apply(&mut self, m: solitaire::Move, cfg: &solitaire::Config) -> Result<(), String> {
        let (f, t, n) = (usize::from(m.from), usize::from(m.to), usize::from(m.n));
        if f >= 13 || t >= 13 || f == t || n == 0 {
            return Err(format!("bad move {m:?}"));
        }
        if f == 0 && t == 1 {
            let want = self.piles[0].len().min(usize::from(cfg.draw));
            if want == 0 || n != want || m.flipped {
                return Err(format!("draw of {n} from a stock of {}", self.piles[0].len()));
            }
            for _ in 0..n {
                let (c, _) = self.piles[0].pop().unwrap();
                self.piles[1].push((c, true));
            }
            return Ok(());
        }
        if f == 1 && t == 0 {
            let allowed = cfg.passes == 0 || self.passes + 1 < cfg.passes;
            if !self.piles[0].is_empty() || n != self.piles[1].len() || n == 0 || !allowed {
                return Err("waste back to the stock not allowed".into());
            }
            while let Some((c, _)) = self.piles[1].pop() {
                self.piles[0].push((c, false));
            }
            self.passes += 1;
            return Ok(());
        }
        if f == 0 || t <= 1 {
            return Err(format!("{m:?} touches the stock or waste"));
        }
        let from = &self.piles[f];
        if from.len() < n {
            return Err("not that many cards".into());
        }
        let moving: Vec<(u8, bool)> = from[from.len() - n..].to_vec();
        if moving.iter().any(|&(_, up)| !up) {
            return Err("moves a face-down card".into());
        }
        if (f < 6 || t < 6) && n != 1 {
            return Err("only columns give or take several cards".into());
        }
        let first = moving[0].0;
        if (2..6).contains(&t) {
            if (2..6).contains(&f) {
                return Err("foundation to foundation".into());
            }
            let ok = match self.piles[t].last() {
                None => first % 13 == 0,
                Some(&(top, _)) => top / 13 == first / 13 && first % 13 == top % 13 + 1,
            };
            if !ok {
                return Err(format!("card {first} does not go onto foundation {t}"));
            }
        } else {
            for w in moving.windows(2) {
                if sol_red(w[0].0) == sol_red(w[1].0) || w[0].0 % 13 != w[1].0 % 13 + 1 {
                    return Err("not a run".into());
                }
            }
            let ok = match self.piles[t].last() {
                None => first % 13 == 12,
                Some(&(top, up)) => up && sol_red(top) != sol_red(first) && top % 13 == first % 13 + 1,
            };
            if !ok {
                return Err(format!("card {first} does not go onto column {t}"));
            }
        }
        let keep = self.piles[f].len() - n;
        let moved: Vec<(u8, bool)> = self.piles[f].drain(keep..).collect();
        self.piles[t].extend(moved);
        let turn = f >= 6 && self.piles[f].last().is_some_and(|&(_, up)| !up);
        if turn != m.flipped {
            return Err(format!("flip recorded {} but the rules say {turn}", m.flipped));
        }
        if turn && let Some(last) = self.piles[f].last_mut() {
            last.1 = true;
        }
        Ok(())
    }
}

/// Play Solitaire with the input's keys and steps (a byte with bit 7 set
/// is `(b & 0x7F) + 1` logic steps, any other an input); every invariant
/// asserted after each: the 52 cards each once (`Board::valid`), every
/// recorded move legal by the reference ([`SolRef`]) and the board equal
/// to the reference's, undo back to exactly the position before the move
/// it takes back, the score's rules, a save read back equal. Returns the
/// final game.
pub fn solitaire_run(data: &[u8]) -> solitaire::Solitaire {
    use solitaire::{Scoring, SolOut, Solitaire};
    let cfg = solitaire_config(data);
    let mut g = Solitaire::new(cfg, seed(data.get(1..9).unwrap_or(&[])));
    assert!(g.board().valid());
    // Positions before each player move still in the history: (history
    // length before it, position).
    let mut restore: Vec<(usize, (solitaire::Board, u16, u16, u32))> = Vec::new();
    let mut buf = [0u8; SAVE_MAX];
    for (k, &b) in data.iter().skip(9).enumerate() {
        let before = g.clone();
        let hist_before: Vec<solitaire::Move> = before.history().collect();
        let out = if b & 0x80 != 0 {
            let mut last = SolOut::None;
            for _ in 0..=(b & 0x7F) {
                let o = g.step();
                if o != SolOut::None {
                    last = o;
                }
            }
            last
        } else {
            match Input::from_byte(b) {
                Some(i) => g.input(i),
                None => continue,
            }
        };
        let b_after = g.board();
        assert!(b_after.valid(), "board invalid after byte {k}: {b_after:?}");
        let hist: Vec<solitaire::Move> = g.history().collect();
        if out == SolOut::Undone {
            // Back to the position before the last player move.
            assert!(hist.len() < hist_before.len());
            assert_eq!(&hist[..], &hist_before[..hist.len()], "undo pops the history");
            assert!(!hist_before[hist.len()].auto, "back to a player move");
            if let Some(i) = restore.iter().rposition(|(n, _)| *n == hist.len()) {
                assert_eq!(g.position(), restore[i].1, "byte {k}: undo restores exactly");
                restore.truncate(i);
            } else {
                restore.clear();
            }
        } else if hist != hist_before {
            // New moves: find what was kept (the oldest drop out of a full
            // history) and check the new ones against the reference.
            let kept = (0..=hist_before.len())
                .find(|&d| hist.starts_with(&hist_before[d..]))
                .expect("the history only grows at its end");
            let new = &hist[hist_before.len() - kept..];
            assert!(!new.is_empty());
            let mut r = SolRef::of(before.board());
            for m in new {
                r.apply(*m, &g.config()).unwrap_or_else(|e| panic!("byte {k}: illegal move {m:?}: {e}"));
            }
            assert_eq!(r, SolRef::of(b_after), "byte {k}: the board is the reference's");
            if kept > 0 {
                restore.clear();
            } else if !new[0].auto {
                restore.push((hist_before.len(), before.position()));
            }
        } else {
            assert_eq!(g.board(), before.board(), "byte {k}: the board changed without a move");
        }
        // Score rules.
        let up = g.board().on_foundations() as i32;
        match cfg.scoring {
            Scoring::Standard => assert!(g.score() >= 0),
            Scoring::Casino => assert_eq!(g.score(), 5 * up - 52),
            Scoring::None => assert_eq!(g.score(), 0),
        }
        assert_eq!(g.won(), up == 52);
        if let Some((p, n)) = g.held() {
            assert!(usize::from(n) <= g.board().face_up(p), "held cards are face up");
        }
        if k % 61 == 0 {
            let s = Session::Solitaire(g.clone());
            let n = s.encode(&mut buf).expect("fits SAVE_MAX");
            let back = Session::decode(&buf[..n], Some(orpheus_games::id::SOLITAIRE)).expect("reads back");
            let Session::Solitaire(x) = back else { panic!("another game") };
            // Held cards are not saved.
            assert_eq!(x.position(), g.position());
            assert_eq!(x.history().collect::<Vec<_>>(), hist);
            assert_eq!((x.steps(), x.cursor().0, x.seed()), (g.steps(), g.cursor().0, g.seed()));
        }
    }
    g
}

/// The fuzz target: the same input twice gives the same game.
pub fn solitaire_keys(data: &[u8]) {
    let a = solitaire_run(data);
    assert_eq!(a, solitaire_run(data), "deterministic");
}
