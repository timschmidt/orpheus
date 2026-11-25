//! The Games app's model (05 §7.13, M14; feature `games`): the games,
//! their settings, the service the screens use, and starting, ending and
//! recording games. The rules are `orpheus-games`; the screens are
//! `screens::games`.
//!
//! The game in progress lives in the service ([`GamesService::games`], an
//! `orpheus_games::Hub`), not in the screen, so a screen covered by
//! anything (the pause menu, the lock screen, an alarm, a notice) is
//! rebuilt from it without losing a step; the service writes it to
//! `/.orpheus/games/<game>.sav` when asked ([`GamesRequest::Save`]: on
//! pause and quit) and the scores to `/.orpheus/games/scores.bin`.

pub use orpheus_games as engine;
use orpheus_games::{Hub, Outcome, Rng, Session, blocks, id as gid, tiles};

use crate::services::SettingsService;
use crate::settings::{Choice, Setting, SettingId, Unit, c, r, t};
use crate::strings::Str;

/// The games of the app, in list order. Solitaire (GAME-2) and the
/// artillery game (GAME-4/5) are not listed until they are built.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Game {
    Blocks,
    Tiles,
}

impl Game {
    pub const ALL: [Game; 2] = [Game::Blocks, Game::Tiles];

    /// The engine's id (save files, scores).
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Game::Blocks => gid::BLOCKS,
            Game::Tiles => gid::TILES,
        }
    }

    #[must_use]
    pub fn from_id(id: u8) -> Option<Game> {
        Game::ALL.into_iter().find(|g| g.id() == id)
    }

    #[must_use]
    pub const fn label(self) -> Str {
        match self {
            Game::Blocks => Str::GameBlocks,
            Game::Tiles => Str::Game4096,
        }
    }

    /// How to play: the paragraphs of the rules text.
    #[must_use]
    pub const fn help(self) -> &'static [Str] {
        match self {
            Game::Blocks => {
                &[Str::HelpBlocks, Str::HelpBlocks2, Str::HelpBlocks3, Str::HelpBlocks4, Str::HelpBlocks5]
            }
            Game::Tiles => &[Str::Help4096, Str::Help40962, Str::Help40963],
        }
    }

    /// The save file under [`DIR`].
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Game::Blocks => "blocks.sav",
            Game::Tiles => "4096.sav",
        }
    }

    /// Its settings, in pause-menu order.
    #[must_use]
    pub fn settings(self) -> &'static [Setting] {
        match self {
            Game::Blocks => &SETTINGS[..BLOCKS_SETTINGS],
            Game::Tiles => &SETTINGS[BLOCKS_SETTINGS..],
        }
    }

    /// The session holds a game of this kind.
    #[must_use]
    pub fn is(self, s: &Session) -> bool {
        s.game() == Some(self.id())
    }
}

/// Where the games keep their files (05 §7.13).
pub const DIR: &str = "/.orpheus/games";
pub const SCORES_FILE: &str = "scores.bin";

/// Store keys of the games' settings: `0x0E__` (the games' own range, like
/// the apps' `0x0D__`), Blocks from `0x0E01`, 4096 from `0x0E21`.
pub mod id {
    use crate::settings::SettingId;
    pub const BLOCKS_MODE: SettingId = SettingId(0x0E01);
    pub const BLOCKS_START_LEVEL: SettingId = SettingId(0x0E02);
    pub const BLOCKS_PIECES: SettingId = SettingId(0x0E03);
    pub const BLOCKS_GHOST: SettingId = SettingId(0x0E04);
    pub const BLOCKS_PREVIEWS: SettingId = SettingId(0x0E05);
    pub const BLOCKS_HOLD: SettingId = SettingId(0x0E06);
    pub const BLOCKS_LOCK_DELAY: SettingId = SettingId(0x0E07);
    pub const BLOCKS_AUTO_SHIFT: SettingId = SettingId(0x0E08);
    pub const BLOCKS_REPEAT: SettingId = SettingId(0x0E09);
    pub const BLOCKS_KICKS: SettingId = SettingId(0x0E0A);
    pub const TILES_BOARD: SettingId = SettingId(0x0E21);
    pub const TILES_GOAL: SettingId = SettingId(0x0E22);
    pub const TILES_FOUR: SettingId = SettingId(0x0E23);
    pub const TILES_UNDO: SettingId = SettingId(0x0E24);
    pub const TILES_ANIMATIONS: SettingId = SettingId(0x0E25);
}

/// Lock delay, auto-shift delay and repeat interval choices (ms).
pub const LOCK_MS: [u16; 4] = [250, 500, 750, 1000];
pub const AUTO_SHIFT_MS: [u16; 5] = [100, 133, 167, 200, 267];
pub const REPEAT_MS: [u16; 5] = [0, 17, 33, 50, 83];
/// 4096 goal tiles as exponents (0 = endless) and the chance of a 4.
pub const GOALS: [u8; 4] = [11, 12, 13, 0];
pub const FOUR_PER_MILLE: [u16; 4] = [0, 100, 250, 500];

const MODES: &[Choice] =
    &[Choice::S(Str::GameEndless), Choice::S(Str::GameFortyLines), Choice::S(Str::GameTwoMinutes)];
const PIECES: &[Choice] = &[Choice::S(Str::GameBag), Choice::S(Str::GameRandom)];
const ZERO_TO_THREE: &[Choice] = &[Choice::Lit("0"), Choice::Lit("1"), Choice::Lit("2"), Choice::Lit("3")];
const LOCKS: &[Choice] =
    &[Choice::Lit("0.25 s"), Choice::Lit("0.5 s"), Choice::Lit("0.75 s"), Choice::Lit("1 s")];
const SHIFTS: &[Choice] = &[
    Choice::Lit("100 ms"),
    Choice::Lit("133 ms"),
    Choice::Lit("167 ms"),
    Choice::Lit("200 ms"),
    Choice::Lit("267 ms"),
];
const REPEATS: &[Choice] = &[
    Choice::Lit("0 ms"),
    Choice::Lit("17 ms"),
    Choice::Lit("33 ms"),
    Choice::Lit("50 ms"),
    Choice::Lit("83 ms"),
];
const BOARDS: &[Choice] = &[Choice::Lit("4 × 4"), Choice::Lit("5 × 5"), Choice::Lit("6 × 6")];
const GOAL_TILES: &[Choice] =
    &[Choice::Lit("2048"), Choice::Lit("4096"), Choice::Lit("8192"), Choice::S(Str::GameEndless)];
const FOURS: &[Choice] = &[Choice::Lit("0 %"), Choice::Lit("10 %"), Choice::Lit("25 %"), Choice::Lit("50 %")];

const BLOCKS_SETTINGS: usize = 10;

/// Every game setting (Blocks', then 4096's), each with the default that
/// suits a first game. Stored like the apps' settings; edited in the
/// pause menu's Settings.
pub const SETTINGS: [Setting; 15] = [
    c(0x0E01, Str::GameMode, MODES, 0),
    r(0x0E02, Str::GameStartLevel, 1, 15, 1, Unit::None, 1),
    c(0x0E03, Str::GamePieces, PIECES, 0),
    t(0x0E04, Str::GameGhost, 1),
    c(0x0E05, Str::GamePreviews, ZERO_TO_THREE, 3),
    t(0x0E06, Str::GameHoldPiece, 1),
    c(0x0E07, Str::GameLockDelay, LOCKS, 1),
    c(0x0E08, Str::GameAutoShift, SHIFTS, 2),
    c(0x0E09, Str::GameRepeat, REPEATS, 2),
    t(0x0E0A, Str::GameKicks, 1),
    c(0x0E21, Str::GameBoard, BOARDS, 0),
    c(0x0E22, Str::GameGoalTile, GOAL_TILES, 1),
    c(0x0E23, Str::GameChanceOfFour, FOURS, 1),
    c(0x0E24, Str::GameUndoSteps, ZERO_TO_THREE, 1),
    t(0x0E25, Str::Animations, 1),
];

/// Settings that change the rules apply from the next game; the others
/// (ghost, previews, animations, auto-shift and repeat) at once.
#[must_use]
pub fn applies_next_game(sid: SettingId) -> bool {
    ![id::BLOCKS_GHOST, id::BLOCKS_PREVIEWS, id::BLOCKS_AUTO_SHIFT, id::BLOCKS_REPEAT, id::TILES_ANIMATIONS]
        .contains(&sid)
}

fn pick<T: Copy>(table: &[T], v: i16, default: T) -> T {
    usize::try_from(v).ok().and_then(|i| table.get(i)).copied().unwrap_or(default)
}

/// The Blocks rules from the settings.
pub fn blocks_config<S: SettingsService + ?Sized>(s: &S) -> blocks::Config {
    blocks::Config {
        mode: blocks::Mode::from_u8(u8::try_from(s.setting(id::BLOCKS_MODE)).unwrap_or(0))
            .unwrap_or_default(),
        start_level: u8::try_from(s.setting(id::BLOCKS_START_LEVEL)).unwrap_or(1),
        bag: s.setting(id::BLOCKS_PIECES) == 0,
        hold: s.setting(id::BLOCKS_HOLD) != 0,
        lock_ms: pick(&LOCK_MS, s.setting(id::BLOCKS_LOCK_DELAY), 500),
        das_ms: pick(&AUTO_SHIFT_MS, s.setting(id::BLOCKS_AUTO_SHIFT), 167),
        arr_ms: pick(&REPEAT_MS, s.setting(id::BLOCKS_REPEAT), 33),
        kicks: s.setting(id::BLOCKS_KICKS) != 0,
    }
    .sanitised()
}

/// The auto-shift delay and repeat interval from the settings (ms).
pub fn blocks_handling<S: SettingsService + ?Sized>(s: &S) -> (u16, u16) {
    let c = blocks_config(s);
    (c.das_ms, c.arr_ms)
}

/// Blocks view settings: ghost piece shown, previews shown (0–3).
pub fn blocks_view<S: SettingsService + ?Sized>(s: &S) -> (bool, u8) {
    (s.setting(id::BLOCKS_GHOST) != 0, u8::try_from(s.setting(id::BLOCKS_PREVIEWS)).unwrap_or(3).min(3))
}

/// The 4096 rules from the settings.
pub fn tiles_config<S: SettingsService + ?Sized>(s: &S) -> tiles::Config {
    tiles::Config {
        size: 4 + u8::try_from(s.setting(id::TILES_BOARD)).unwrap_or(0).min(2),
        goal: pick(&GOALS, s.setting(id::TILES_GOAL), 12),
        four_per_mille: pick(&FOUR_PER_MILLE, s.setting(id::TILES_FOUR), 100),
        undo: u8::try_from(s.setting(id::TILES_UNDO)).unwrap_or(1).min(3),
    }
    .sanitised()
}

pub fn tiles_animations<S: SettingsService + ?Sized>(s: &S) -> bool {
    s.setting(id::TILES_ANIMATIONS) != 0
}

/// What the screens ask of the service's storage side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GamesRequest {
    /// Read the score table and see which games have a save file (once;
    /// the Games list asks when it opens).
    Load,
    /// Make `game` (an engine id) the session: write the session that is
    /// in RAM to its file if it is another game in progress, then read
    /// `game`'s save file (the session is `None` if there is none or it
    /// is damaged). The first request also reads the score table.
    Open(u8),
    /// Write the session to its save file (a game in progress), or remove
    /// the file (a finished game).
    Save,
    /// Remove `game`'s save file.
    Discard(u8),
    /// Write the score table.
    SaveScores,
}

/// The Games app's service (part of `Services` with the feature).
pub trait GamesService {
    fn games(&self) -> &Hub;
    fn games_mut(&mut self) -> &mut Hub;
    fn games_request(&mut self, r: GamesRequest);
    /// A seed for a new game: different every time on the player (the
    /// uptime and the clock); the simulator's is scripted.
    fn game_seed(&mut self) -> u64;
    /// Whether the screen is on (a game pauses when it goes off).
    fn display_on(&self) -> bool {
        true
    }
}

/// Start a new game of `game` with the settings as they are now.
pub fn new_game<S: SettingsService + GamesService + ?Sized>(svc: &mut S, game: Game) {
    let seed = svc.game_seed();
    let session = match game {
        Game::Blocks => Session::Blocks(blocks::Blocks::new(blocks_config(svc), seed)),
        Game::Tiles => Session::Tiles(tiles::Tiles::new(tiles_config(svc), seed)),
    };
    svc.games_mut().session = session;
}

/// The score-table entry of a finished game: (game id, variant, outcome).
#[must_use]
pub fn outcome(s: &Session) -> Option<(u8, u16, Outcome)> {
    match s {
        Session::None => None,
        Session::Blocks(b) if b.ended() => {
            let done = b.phase() == blocks::Phase::Done;
            let o = match b.config().mode {
                blocks::Mode::Lines40 => Outcome {
                    result: orpheus_games::steps_to_ms(b.steps()),
                    detail: b.lines(),
                    lower_is_better: true,
                    ranked: done,
                    won: done,
                },
                mode => Outcome {
                    result: b.score(),
                    detail: b.lines(),
                    lower_is_better: false,
                    ranked: true,
                    won: done && mode == blocks::Mode::Attack,
                },
            };
            Some((gid::BLOCKS, b.config().variant(), o))
        }
        Session::Tiles(t) if t.over() => Some((
            gid::TILES,
            t.config().variant(),
            Outcome {
                result: t.score(),
                detail: u32::from(t.best_tile()),
                lower_is_better: false,
                ranked: true,
                won: t.goal_reached(),
            },
        )),
        _ => None,
    }
}

/// A game ended: count it in the scores, write them, and remove the save
/// file (a finished game is not offered again). Returns true for a new
/// best.
pub fn finish<S: GamesService + ?Sized>(svc: &mut S) -> bool {
    let Some((game, variant, o)) = outcome(&svc.games().session) else { return false };
    let best = svc.games_mut().scores.record(game, variant, o);
    svc.games_request(GamesRequest::SaveScores);
    svc.games_request(GamesRequest::Discard(game));
    best
}

/// The best result for `game` under the current settings, if any.
pub fn best<S: SettingsService + GamesService + ?Sized>(svc: &S, game: Game) -> Option<u32> {
    let variant = match game {
        Game::Blocks => blocks_config(svc).variant(),
        Game::Tiles => tiles_config(svc).variant(),
    };
    svc.games().scores.get(game.id(), variant).map(|r| r.best).filter(|&b| b > 0)
}

/// Whether the best result of `game` under the current settings is a time.
pub fn best_is_time<S: SettingsService + ?Sized>(svc: &S, game: Game) -> bool {
    game == Game::Blocks && blocks_config(svc).mode == blocks::Mode::Lines40
}

/// A seed from two numbers that differ between games (uptime, clock).
#[must_use]
pub fn seed_from(a: u64, b: u64) -> u64 {
    Rng::seed_from(a, b)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::settings::{Kind, SettingsCache, all, def};

    struct S(SettingsCache);
    impl SettingsService for S {
        fn setting(&self, id: SettingId) -> i16 {
            self.0.get(id)
        }
        fn set_setting(&mut self, id: SettingId, v: i16) {
            self.0.set(id, v);
        }
        fn factory_reset(&mut self) {}
    }

    /// Every game setting has a unique key in the games' range, a valid
    /// default and an entry in the VER-13 manifest.
    #[test]
    fn settings_table() {
        let mut ids = std::collections::BTreeSet::new();
        for s in all() {
            assert!(ids.insert(s.id), "duplicate setting id {:#06x}", s.id.0);
        }
        let manifest = include_str!("../../../tests/settings/effects.toml");
        for s in &SETTINGS {
            assert_eq!(s.id.0 >> 8, 0x0E);
            assert_eq!(s.validate(s.default), s.default);
            assert!(def(s.id).is_some());
            let key = std::format!("id = \"{:#06X}\"", s.id.0).replace("0X", "0x");
            assert!(manifest.contains(&key), "{key} missing from tests/settings/effects.toml");
        }
        assert_eq!(Game::Blocks.settings().len() + Game::Tiles.settings().len(), SETTINGS.len());
        assert!(Game::Tiles.settings().iter().all(|s| s.id.0 >= 0x0E21));
        assert!(matches!(def(id::BLOCKS_MODE).map(|d| d.kind), Some(Kind::Choice(_))));
    }

    #[test]
    fn defaults_make_the_engines_defaults() {
        let s = S(SettingsCache::new());
        assert_eq!(blocks_config(&s), blocks::Config::DEFAULT);
        assert_eq!(tiles_config(&s), tiles::Config::DEFAULT);
        assert_eq!(blocks_view(&s), (true, 3));
        let mut s = s;
        s.set_setting(id::TILES_BOARD, 2);
        s.set_setting(id::TILES_GOAL, 3);
        s.set_setting(id::TILES_FOUR, 0);
        s.set_setting(id::TILES_UNDO, 3);
        assert_eq!(tiles_config(&s), tiles::Config { size: 6, goal: 0, four_per_mille: 0, undo: 3 });
        s.set_setting(id::BLOCKS_MODE, 1);
        s.set_setting(id::BLOCKS_REPEAT, 0);
        s.set_setting(id::BLOCKS_LOCK_DELAY, 3);
        let c = blocks_config(&s);
        assert_eq!((c.mode, c.arr_ms, c.lock_ms), (blocks::Mode::Lines40, 0, 1000));
        assert!(best_is_time(&s, Game::Blocks));
        assert!(applies_next_game(id::BLOCKS_MODE) && !applies_next_game(id::BLOCKS_GHOST));
    }

    #[test]
    fn games_and_files() {
        for g in Game::ALL {
            assert_eq!(Game::from_id(g.id()), Some(g));
        }
        assert_eq!(Game::from_id(0), None);
        assert_ne!(Game::Blocks.file(), Game::Tiles.file());
    }
}
