//! The simulator's Games back end (M14, 05 §7.13): the hub the screens
//! play in, and its files on the simulated card under `/.orpheus/games/`
//! (`<game>.sav`, `scores.bin`), written through `orpheus-fs` as the
//! firmware's card task does.
//!
//! Two ways to answer the screens' requests: at once (the default: the
//! goldens and most tests), or deferred (`set:gamesio=on`): through the
//! same `orpheus_apps::games::GamesIo` queue the firmware uses, one card
//! operation per simulator step, so the late answer (`Open`'s "Loading…",
//! saves that go out while the game goes on) is tested on the host.
//!
//! Script hooks (`set:`): `gameseed=N` fixes the next new game's seed (new
//! games are otherwise seeded from a counter, so runs stay deterministic);
//! `tiles=e.e.…` / `tiles6=…` put a 4096 game with that board (exponents,
//! row-major; 4 × 4 or 6 × 6) in RAM; `blocksrows=N` gives the Blocks game
//! in RAM a stack of `N` nearly full rows (every row lacks one cell);
//! `solnear=N` puts a Solitaire game in RAM with only `N` (1–4) kings left
//! in the columns (the queen of spades on the second: an almost won
//! game); `solplay=N` plays the first `N` moves of the deal check's line in
//! the Solitaire game in RAM; `sollong=1` a column of six face-down cards
//! and a king-to-ace run; `solstuck=1` a game with no move left (one
//! pass); `gamesio=on|off` the deferred mode.

use anyhow::{Result, bail};
use orpheus_apps::games::{self as app, CardOp, CardRun, Game, GamesIo, GamesRequest};
use orpheus_fs::OpenMode;
use orpheus_fs::host::block_on;
use orpheus_games::blocks::{self, Blocks, W};
use orpheus_games::scores::{self, Scores};
use orpheus_games::solitaire::{self, Board, CHECK_MOVES, Move, PILES, STOCK, Solitaire};
use orpheus_games::tiles::{self, Tiles};
use orpheus_games::{Hub, SAVE_MAX, Session};

use crate::media::Media;

pub struct SimGames {
    pub hub: Hub,
    /// The screen is on (`after_step` copies the key gate's state).
    pub display_on: bool,
    /// `set:gameseed=N`: the next new game's seed.
    pub next_seed: Option<u64>,
    seeds: u64,
    /// Card writes of save files and the score table (tests).
    pub writes: u32,
    /// `set:gamesio=on`: requests go through the firmware's queue.
    pub io: Option<GamesIo>,
    buf: [u8; SAVE_MAX],
    /// Card operations carried out in the deferred mode (tests).
    pub ops: Vec<CardOp>,
    /// The deferred mode's operation in steps.
    run: Option<CardRun>,
}

impl Default for SimGames {
    fn default() -> Self {
        SimGames {
            hub: Hub::default(),
            display_on: true,
            next_seed: None,
            seeds: 0,
            writes: 0,
            io: None,
            buf: [0; SAVE_MAX],
            ops: Vec::new(),
            run: None,
        }
    }
}

fn path(name: &str) -> String {
    format!("{}/{name}", app::DIR)
}

/// A file of the simulated card, if it exists.
pub fn read(media: &mut Media, path: &str) -> Option<Vec<u8>> {
    let vol = media.volume();
    let mut f = block_on(vol.open(path, OpenMode::Read)).ok()?;
    let mut buf = vec![0u8; usize::try_from(f.size()).ok()?.min(64 * 1024)];
    let n = block_on(vol.read_exact_or_eof(&mut f, &mut buf)).ok()?;
    let _ = block_on(vol.close(f));
    buf.truncate(n);
    Some(buf)
}

fn write(media: &mut Media, path: &str, data: &[u8]) -> bool {
    let vol = media.volume();
    let _ = block_on(vol.create_dir(app::DIR));
    let Ok(mut f) = block_on(vol.open(path, OpenMode::Truncate)) else { return false };
    let ok = block_on(vol.write(&mut f, data)).is_ok_and(|n| n == data.len());
    let _ = block_on(vol.close(f));
    let _ = block_on(vol.flush());
    ok
}

fn remove(media: &mut Media, path: &str) {
    let vol = media.volume();
    if block_on(vol.exists(path)).unwrap_or(false) {
        let _ = block_on(vol.remove(path));
        let _ = block_on(vol.flush());
    }
}

/// Cards by rank (1 = ace … 13 = king) and suit letter.
fn card(rank: u8, suit: char) -> u8 {
    let s = match suit {
        's' => 0,
        'h' => 1,
        'c' => 2,
        _ => 3,
    };
    s * 13 + rank - 1
}

/// The longest column there can be: six cards face down under a run from
/// the king to the ace (`set:sollong=1`); the other cards in the stock
/// and the columns.
fn long_column() -> Board {
    let run: Vec<u8> = (1..=13u8).rev().map(|r| card(r, if r % 2 == 0 { 'h' } else { 's' })).collect();
    let rest: Vec<u8> = (0..52u8).filter(|c| !run.contains(c)).collect();
    let (down, rest) = rest.split_at(6);
    let mut col6 = down.to_vec();
    col6.extend(&run);
    let (cols, stock) = rest.split_at(6);
    let piles: [&[u8]; PILES] = [
        stock,
        &[],
        &[],
        &[],
        &[],
        &[],
        &cols[0..1],
        &cols[1..2],
        &cols[2..3],
        &cols[3..4],
        &cols[4..5],
        &cols[5..6],
        &col6,
    ];
    Board::from_piles(piles, [0, 0, 0, 0, 0, 0, 6], 0).expect("a valid board")
}

/// No move left (`set:solstuck=1`, one pass): black tops that do not
/// build on each other, no ace in sight, the stock gone through.
fn stuck_board() -> Board {
    let tops =
        [card(2, 's'), card(4, 's'), card(6, 's'), card(8, 's'), card(10, 's'), card(12, 's'), card(4, 'c')];
    let waste_top = card(3, 'c');
    let rest: Vec<u8> = (0..52u8).filter(|c| !tops.contains(c) && *c != waste_top).collect();
    let mut cols: Vec<Vec<u8>> = Vec::new();
    let mut it = rest.iter().copied();
    for (i, &t) in tops.iter().enumerate() {
        let mut c: Vec<u8> = it.by_ref().take(i + 1).collect();
        c.push(t);
        cols.push(c);
    }
    let mut waste: Vec<u8> = it.collect();
    waste.push(waste_top);
    let piles: [&[u8]; PILES] = [
        &[],
        &waste,
        &[],
        &[],
        &[],
        &[],
        &cols[0],
        &cols[1],
        &cols[2],
        &cols[3],
        &cols[4],
        &cols[5],
        &cols[6],
    ];
    Board::from_piles(piles, [1, 2, 3, 4, 5, 6, 7], 0).expect("a valid board")
}

fn exists(media: &mut Media, path: &str) -> bool {
    block_on(media.volume().exists(path)).unwrap_or(false)
}

impl SimGames {
    /// A seed for a new game.
    pub fn seed(&mut self) -> u64 {
        if let Some(s) = self.next_seed.take() {
            return s;
        }
        self.seeds += 1;
        orpheus_games::Rng::seed_from(0x6A3E_5000 + self.seeds, self.seeds)
    }

    fn load(&mut self, media: &mut Media) {
        if self.hub.loaded {
            return;
        }
        self.hub.loaded = true;
        if let Some(s) = read(media, &path(app::SCORES_FILE)).and_then(|d| Scores::decode(&d)) {
            self.hub.scores = s;
        }
        for g in Game::ALL {
            let e = exists(media, &path(g.file()));
            self.hub.set_saved(g.id(), e);
        }
    }

    fn save(&mut self, media: &mut Media) {
        let Some(g) = self.hub.session.game().and_then(Game::from_id) else { return };
        if self.hub.session.in_progress() {
            let mut buf = [0u8; SAVE_MAX];
            if let Some(n) = self.hub.session.encode(&mut buf) {
                let ok = write(media, &path(g.file()), &buf[..n]);
                self.writes += 1;
                self.hub.set_saved(g.id(), ok);
            }
        } else {
            remove(media, &path(g.file()));
            self.hub.set_saved(g.id(), false);
        }
    }

    pub fn request(&mut self, r: GamesRequest, media: &mut Media) {
        if let Some(io) = &mut self.io {
            io.request(&mut self.hub, r);
            return;
        }
        match r {
            GamesRequest::Load => self.load(media),
            GamesRequest::Open(id) => {
                self.load(media);
                if self.hub.session.game() == Some(id) {
                    return;
                }
                // Another game in progress waits in its file.
                self.save(media);
                let Some(g) = Game::from_id(id) else { return };
                self.hub.session = read(media, &path(g.file()))
                    .and_then(|d| Session::decode(&d, Some(id)))
                    .unwrap_or(Session::None);
            }
            GamesRequest::Save => self.save(media),
            GamesRequest::Discard(id) => {
                if let Some(g) = Game::from_id(id) {
                    remove(media, &path(g.file()));
                    self.hub.set_saved(id, false);
                }
            }
            GamesRequest::SaveScores => {
                let mut buf = [0u8; scores::MAX_BYTES];
                if let Some(n) = self.hub.scores.encode(&mut buf) {
                    write(media, &path(app::SCORES_FILE), &buf[..n]);
                    self.writes += 1;
                }
            }
        }
    }

    /// The deferred mode: one step of the queued card operations, as the
    /// firmware's card task makes them in pieces while music plays
    /// (`orpheus_apps::games::CardRun`, `fw/firmware/src/games.rs`). True
    /// if the hub changed.
    pub fn pump(&mut self, media: &mut Media) -> bool {
        let Some(io) = &mut self.io else { return false };
        if self.run.is_none() {
            let Some((op, n)) = io.next(&self.hub, &mut self.buf) else { return false };
            self.ops.push(op);
            if matches!(op, CardOp::Write(_) | CardOp::WriteScores) {
                self.writes += 1;
            }
            self.run = Some(CardRun::new(op, n));
        }
        let Some(run) = &mut self.run else { return false };
        let Some(a) = block_on(run.step(media.volume(), &mut self.buf)) else { return false };
        self.run = None;
        let buf = self.buf;
        io.done(&mut self.hub, a, &buf)
    }

    /// The `set:` hooks of this module; `Ok(false)` if `k` is not one.
    pub fn set(
        &mut self,
        k: &str,
        v: &str,
        tiles_cfg: tiles::Config,
        blocks_cfg: blocks::Config,
        sol_cfg: solitaire::Config,
    ) -> Result<bool> {
        match k {
            "gameseed" => self.next_seed = Some(v.parse()?),
            "gamesio" => self.io = matches!(v, "on" | "1").then(GamesIo::new),
            "tiles" | "tiles6" => {
                let cells: Vec<u8> = v.split(['.', '/']).map(str::parse).collect::<Result<_, _>>()?;
                let size = if k == "tiles6" { 6 } else { 4 };
                let cfg = tiles::Config { size, ..tiles_cfg };
                self.hub.session = Session::Tiles(Tiles::from_cells(cfg, self.seed(), &cells));
            }
            "blocksrows" => {
                let n: usize = v.parse()?;
                if n > 18 {
                    bail!("blocksrows: at most 18");
                }
                let rows: Vec<[u8; W]> = (0..n)
                    .map(|i| {
                        let mut r = [0u8; W];
                        for (x, c) in r.iter_mut().enumerate() {
                            *c = if x == (i * 3 + 1) % W { 0 } else { 1 + ((x + i) % 7) as u8 };
                        }
                        r
                    })
                    .collect();
                self.hub.session = Session::Blocks(Blocks::with_stack(blocks_cfg, self.seed(), &rows));
            }
            "solnear" => {
                let n: usize = v.parse()?;
                if !(1..=4).contains(&n) {
                    bail!("solnear: 1 to 4 kings");
                }
                // K♠ | K♥ Q♠ | K♣ | K♦, the first `n` of them.
                let cols: [&[u8]; 4] = [&[12], &[25, 11], &[38], &[51]];
                let mut all: Vec<&[u8]> = cols[..n].to_vec();
                all.resize(7, &[]);
                self.hub.session = Session::Solitaire(Solitaire::nearly_done(sol_cfg, &all));
            }
            "solplay" => {
                // The first N moves of the deal check's winning line, made
                // as the player's (a game in the middle).
                let n: usize = v.parse()?;
                let Session::Solitaire(g) = &mut self.hub.session else {
                    bail!("solplay: no Solitaire game")
                };
                let mut line =
                    [Move { from: 0, to: 0, n: 0, flipped: false, auto: false }; CHECK_MOVES as usize];
                let cfg = solitaire::Config { auto_move: false, ..g.config() };
                let len = solitaire::solution(g.board(), &cfg, g.seed(), &mut line).unwrap_or(0);
                for m in &line[..len.min(n)] {
                    let ok = if m.from == STOCK || m.to == STOCK {
                        g.draw()
                    } else {
                        g.try_move(m.from, m.to, usize::from(m.n))
                    };
                    if !ok {
                        break;
                    }
                }
            }
            "sollong" | "solstuck" => {
                let b = if k == "sollong" { long_column() } else { stuck_board() };
                let cfg = if k == "solstuck" { solitaire::Config { passes: 1, ..sol_cfg } } else { sol_cfg };
                let mut g = Solitaire::with_board(cfg, 0, false, b);
                // The cursor on the last column.
                for _ in 0..6 {
                    g.input(orpheus_games::Input::Press(orpheus_games::Button::Right));
                }
                self.hub.session = Session::Solitaire(g);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
