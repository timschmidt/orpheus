//! The simulator's Games back end (M14, 05 §7.13): the hub the screens
//! play in, and its files on the simulated card under `/.orpheus/games/`
//! (`<game>.sav`, `scores.bin`), written through `orpheus-fs` as the
//! firmware's card task will.
//!
//! Script hooks (`set:`): `gameseed=N` fixes the next new game's seed (new
//! games are otherwise seeded from a counter, so runs stay deterministic);
//! `tiles=e.e.…` / `tiles6=…` put a 4096 game with that board (exponents,
//! row-major; 4 × 4 or 6 × 6) in RAM; `blocksrows=N` gives the Blocks game
//! in RAM a stack of `N` nearly full rows (every row lacks one cell).

use anyhow::{Result, bail};
use orpheus_apps::games::{self as app, Game, GamesRequest};
use orpheus_fs::OpenMode;
use orpheus_fs::host::block_on;
use orpheus_games::blocks::{self, Blocks, W};
use orpheus_games::scores::{self, Scores};
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
}

impl Default for SimGames {
    fn default() -> Self {
        SimGames { hub: Hub::default(), display_on: true, next_seed: None, seeds: 0, writes: 0 }
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
            let exists = block_on(media.volume().exists(&path(g.file()))).unwrap_or(false);
            self.hub.set_saved(g.id(), exists);
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

    /// The `set:` hooks of this module; `Ok(false)` if `k` is not one.
    pub fn set(
        &mut self,
        k: &str,
        v: &str,
        tiles_cfg: tiles::Config,
        blocks_cfg: blocks::Config,
    ) -> Result<bool> {
        match k {
            "gameseed" => self.next_seed = Some(v.parse()?),
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
            _ => return Ok(false),
        }
        Ok(true)
    }
}
