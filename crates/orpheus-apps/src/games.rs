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
#[cfg(feature = "salvo")]
use orpheus_games::salvo;
use orpheus_games::{Hub, Outcome, Rng, Session, blocks, id as gid, solitaire, tiles};

use crate::services::SettingsService;
use crate::settings::{Choice, Setting, SettingId, Unit, c, r, t};
use crate::strings::Str;

/// The games of the app, in list order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Game {
    Blocks,
    Tiles,
    Solitaire,
    #[cfg(feature = "salvo")]
    Salvo,
}

impl Game {
    #[cfg(feature = "salvo")]
    pub const ALL: [Game; 4] = [Game::Blocks, Game::Tiles, Game::Solitaire, Game::Salvo];
    #[cfg(not(feature = "salvo"))]
    pub const ALL: [Game; 3] = [Game::Blocks, Game::Tiles, Game::Solitaire];

    /// The engine's id (save files, scores).
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Game::Blocks => gid::BLOCKS,
            Game::Tiles => gid::TILES,
            Game::Solitaire => gid::SOLITAIRE,
            #[cfg(feature = "salvo")]
            Game::Salvo => gid::SALVO,
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
            Game::Solitaire => Str::GameSolitaire,
            #[cfg(feature = "salvo")]
            Game::Salvo => Str::GameSalvo,
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
            Game::Solitaire => &[
                Str::HelpSolitaire,
                Str::HelpSolitaire2,
                Str::HelpSolitaire3,
                Str::HelpSolitaire4,
                Str::HelpSolitaire5,
            ],
            #[cfg(feature = "salvo")]
            Game::Salvo => {
                &[Str::HelpSalvo, Str::HelpSalvo2, Str::HelpSalvo3, Str::HelpSalvo4, Str::HelpSalvo5]
            }
        }
    }

    /// The save file under [`DIR`].
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Game::Blocks => "blocks.sav",
            Game::Tiles => "4096.sav",
            Game::Solitaire => "solitaire.sav",
            #[cfg(feature = "salvo")]
            Game::Salvo => "salvo.sav",
        }
    }

    /// The save file's path ([`DIR`] and [`Game::file`]).
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Game::Blocks => "/.orpheus/games/blocks.sav",
            Game::Tiles => "/.orpheus/games/4096.sav",
            Game::Solitaire => "/.orpheus/games/solitaire.sav",
            #[cfg(feature = "salvo")]
            Game::Salvo => "/.orpheus/games/salvo.sav",
        }
    }

    /// Its settings, in pause-menu order (Salvo's: [`Game::groups`]).
    #[must_use]
    pub fn settings(self) -> &'static [Setting] {
        match self {
            Game::Blocks => &SETTINGS[..BLOCKS_SETTINGS],
            Game::Tiles => &SETTINGS[BLOCKS_SETTINGS..SOLITAIRE_FROM],
            Game::Solitaire => &SETTINGS[SOLITAIRE_FROM..SALVO_FROM],
            #[cfg(feature = "salvo")]
            Game::Salvo => &SETTINGS[SALVO_FROM..SALVO_WORDS],
        }
    }

    /// Its settings in groups, each a row of the pause menu's Settings
    /// that opens a list of its own (a game with many settings; one group
    /// for the others).
    #[must_use]
    pub fn groups(self) -> &'static [(Str, &'static [Setting])] {
        #[cfg(feature = "salvo")]
        if self == Game::Salvo {
            const SALVO_GROUPS: [(Str, &[Setting]); 3] = [
                (Str::GamePlayers, &SETTINGS_SALVO_PLAYERS),
                (Str::World, &SETTINGS_SALVO_WORLD),
                (Str::GameRules, &SETTINGS_SALVO_RULES),
            ];
            return &SALVO_GROUPS;
        }
        &[]
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
/// [`SCORES_FILE`] in [`DIR`].
pub const SCORES_PATH: &str = "/.orpheus/games/scores.bin";

/// Store keys of the games' settings: `0x0E__` (the games' own range, like
/// the apps' `0x0D__`), Blocks from `0x0E01`, 4096 from `0x0E21`,
/// Solitaire from `0x0E41`.
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
    pub const SOL_DRAW: SettingId = SettingId(0x0E41);
    pub const SOL_PASSES: SettingId = SettingId(0x0E42);
    pub const SOL_SCORING: SettingId = SettingId(0x0E43);
    pub const SOL_AUTO_MOVE: SettingId = SettingId(0x0E44);
    pub const SOL_AUTO_COMPLETE: SettingId = SettingId(0x0E45);
    pub const SOL_TIMER: SettingId = SettingId(0x0E46);
    pub const SOL_WINNABLE: SettingId = SettingId(0x0E47);
    pub const SALVO_PLAYERS: SettingId = SettingId(0x0E61);
    /// Player 1 … 4: human or a computer's difficulty (0x0E62 …).
    pub const SALVO_PLAYER1: SettingId = SettingId(0x0E62);
    pub const SALVO_ROUNDS: SettingId = SettingId(0x0E66);
    pub const SALVO_TIMER: SettingId = SettingId(0x0E67);
    pub const SALVO_TERRAIN: SettingId = SettingId(0x0E68);
    pub const SALVO_ROUGHNESS: SettingId = SettingId(0x0E69);
    pub const SALVO_RELIEF: SettingId = SettingId(0x0E6A);
    pub const SALVO_SEED: SettingId = SettingId(0x0E6B);
    pub const SALVO_WIDTH: SettingId = SettingId(0x0E6C);
    pub const SALVO_GRAVITY: SettingId = SettingId(0x0E6D);
    pub const SALVO_WIND: SettingId = SettingId(0x0E6E);
    pub const SALVO_CUSTOM_WIND: SettingId = SettingId(0x0E6F);
    pub const SALVO_GUSTS: SettingId = SettingId(0x0E70);
    pub const SALVO_DRAG: SettingId = SettingId(0x0E71);
    pub const SALVO_HEALTH: SettingId = SettingId(0x0E72);
    pub const SALVO_FALL: SettingId = SettingId(0x0E73);
    pub const SALVO_WEAPONS: SettingId = SettingId(0x0E74);
    pub const SALVO_SHOP: SettingId = SettingId(0x0E75);
    pub const SALVO_GUIDE: SettingId = SettingId(0x0E76);
    /// The four stored words Salvo's settings are packed into.
    pub const SALVO_WORD0: SettingId = SettingId(0x0E80);
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
const DRAWS: &[Choice] = &[Choice::S(Str::GameDrawOne), Choice::S(Str::GameDrawThree)];
const PASSES: &[Choice] = &[Choice::S(Str::GameUnlimited), Choice::Lit("3"), Choice::Lit("1")];
const SCORINGS: &[Choice] =
    &[Choice::S(Str::GameStandard), Choice::S(Str::GameCasino), Choice::S(Str::GameNoScore)];
/// Passes through the stock per choice of `PASSES` (0 = unlimited).
pub const PASS_LIMITS: [u8; 3] = [0, 3, 1];

const BLOCKS_SETTINGS: usize = 10;
const SOLITAIRE_FROM: usize = 15;
const SALVO_FROM: usize = 22;
#[cfg(feature = "salvo")]
const SALVO_WORDS: usize = SALVO_FROM + 22;
/// Game settings with a value of their own in the settings cache: all
/// but Salvo's packed ones (its four words are).
#[cfg(feature = "salvo")]
pub const STORED_SETTINGS: usize = SALVO_FROM + 4;
#[cfg(not(feature = "salvo"))]
pub const STORED_SETTINGS: usize = SALVO_FROM;

/// Salvo's settings (feature `salvo`): its choices, groups, and the four
/// words they are packed into.
#[cfg(feature = "salvo")]
mod salvo_defs {
    use super::*;

    pub const PLAYER_COUNTS: &[Choice] = &[Choice::Lit("2"), Choice::Lit("3"), Choice::Lit("4")];
    pub const KINDS: &[Choice] = &[
        Choice::S(Str::GameHuman),
        Choice::S(Str::GameNovice),
        Choice::S(Str::GameEasy),
        Choice::S(Str::GameHard),
        Choice::S(Str::GameExpert),
    ];
    pub const ROUNDS: &[Choice] = &[Choice::Lit("1"), Choice::Lit("3"), Choice::Lit("5"), Choice::Lit("10")];
    /// Rounds of each choice of `ROUNDS`.
    pub const ROUND_COUNTS: [u8; 4] = [1, 3, 5, 10];
    pub const TIMERS: &[Choice] =
        &[Choice::S(Str::Off), Choice::Secs(15), Choice::Secs(30), Choice::Secs(60)];
    /// Seconds per turn of each choice of `TIMERS` (0 = none).
    pub const TIMER_SECS: [u8; 4] = [0, 15, 30, 60];
    pub const TERRAINS: &[Choice] = &[
        Choice::S(Str::GameHills),
        Choice::S(Str::GameMountains),
        Choice::S(Str::GameCanyon),
        Choice::S(Str::GameIslands),
        Choice::S(Str::GamePlateaus),
        Choice::S(Str::GameMixed),
    ];
    pub const LEVELS: &[Choice] = &[Choice::S(Str::Low), Choice::S(Str::Medium), Choice::S(Str::High)];
    pub const WIDTHS: &[Choice] =
        &[Choice::Lit("128 px"), Choice::Lit("192 px"), Choice::Lit("256 px"), Choice::Lit("384 px")];
    pub const GRAVITIES: &[Choice] = &[
        Choice::S(Str::GameMoon),
        Choice::S(Str::GameMars),
        Choice::S(Str::GameEarth),
        Choice::S(Str::GameJupiter),
    ];
    pub const WINDS: &[Choice] = &[
        Choice::S(Str::GameNoScore),
        Choice::S(Str::GameLight),
        Choice::S(Str::GameStrong),
        Choice::S(Str::GameCustom),
    ];
    /// Largest wind (m/s) of the first three choices of `WINDS`.
    pub const WIND_MAX: [u8; 3] = [0, 4, 12];
    pub const DRAGS: &[Choice] =
        &[Choice::S(Str::Off), Choice::S(Str::GameRealistic), Choice::S(Str::GameHeavy)];
    pub const HEALTHS: &[Choice] =
        &[Choice::Lit("50"), Choice::Lit("100"), Choice::Lit("150"), Choice::Lit("200")];
    /// Tank health of each choice of `HEALTHS`.
    pub const HEALTH_POINTS: [u8; 4] = [50, 100, 150, 200];
    pub const WEAPON_SETS: &[Choice] = &[Choice::S(Str::GameShellsOnly), Choice::S(Str::GameAllWeapons)];
    pub const GUIDES: &[Choice] =
        &[Choice::S(Str::GameNoScore), Choice::S(Str::GameLastShot), Choice::S(Str::GamePreview)];

    pub const W0: u16 = 0x0E80;
    pub const W1: u16 = 0x0E81;
    pub const W2: u16 = 0x0E82;
    pub const W3: u16 = 0x0E83;

    /// Salvo's settings by group (players, world, rules), packed into the four
    /// stored words `0x0E80 …` (bit fields; see `Setting::pack`).
    pub const SETTINGS_SALVO_PLAYERS: [Setting; 7] = [
        c(0x0E61, Str::GamePlayers, PLAYER_COUNTS, 0).packed(W0, 0, 2),
        c(0x0E62, Str::GamePlayer1, KINDS, 0).packed(W0, 2, 3),
        c(0x0E63, Str::GamePlayer2, KINDS, 2).packed(W0, 5, 3),
        c(0x0E64, Str::GamePlayer3, KINDS, 2).packed(W0, 8, 3),
        c(0x0E65, Str::GamePlayer4, KINDS, 2).packed(W0, 11, 3),
        c(0x0E66, Str::GameRounds, ROUNDS, 1).packed(W0, 14, 2),
        c(0x0E67, Str::GameTurnTimer, TIMERS, 0).packed(W1, 0, 2),
    ];
    pub const SETTINGS_SALVO_WORLD: [Setting; 10] = [
        c(0x0E68, Str::GameTerrain, TERRAINS, 0).packed(W1, 2, 3),
        c(0x0E69, Str::GameRoughness, LEVELS, 1).packed(W1, 5, 2),
        c(0x0E6A, Str::GameRelief, LEVELS, 1).packed(W1, 7, 2),
        r(0x0E6B, Str::GameTerrainSeed, 0, 99, 1, Unit::None, 0).packed(W1, 9, 7),
        c(0x0E6C, Str::GameWorldWidth, WIDTHS, 2).packed(W2, 0, 2),
        c(0x0E6D, Str::GameGravity, GRAVITIES, 2).packed(W2, 2, 2),
        c(0x0E6E, Str::GameWind, WINDS, 1).packed(W2, 4, 2),
        r(0x0E6F, Str::GameCustomWind, 1, 30, 1, Unit::MetresPerSecond, 8).packed(W2, 6, 5),
        t(0x0E70, Str::GameGusts, 0).packed(W2, 11, 1),
        c(0x0E71, Str::GameDrag, DRAGS, 1).packed(W2, 12, 2),
    ];
    pub const SETTINGS_SALVO_RULES: [Setting; 5] = [
        c(0x0E72, Str::GameTankHealth, HEALTHS, 1).packed(W3, 0, 2),
        t(0x0E73, Str::GameFallDamage, 1).packed(W3, 2, 1),
        c(0x0E74, Str::GameWeapons, WEAPON_SETS, 1).packed(W3, 3, 1),
        t(0x0E75, Str::GameShop, 1).packed(W3, 4, 1),
        c(0x0E76, Str::GameAimGuide, GUIDES, 1).packed(W3, 5, 2),
    ];

    /// The default of a word: its settings' defaults in their bits.
    pub const fn word_default(word: u16) -> i16 {
        let groups: [&[Setting]; 3] = [&SETTINGS_SALVO_PLAYERS, &SETTINGS_SALVO_WORLD, &SETTINGS_SALVO_RULES];
        let mut v: u16 = 0;
        let mut g = 0;
        while g < groups.len() {
            let mut i = 0;
            while i < groups[g].len() {
                let s = &groups[g][i];
                if let Some((w, shift, _)) = s.pack
                    && w.0 == word
                {
                    v |= (s.default as u16) << shift;
                }
                i += 1;
            }
            g += 1;
        }
        v as i16
    }

    /// A stored word of packed settings (never shown).
    pub const fn word(id: u16) -> Setting {
        r(id, Str::GameSalvo, i16::MIN, i16::MAX, 1, Unit::None, word_default(id))
    }

    pub const fn salvo_settings() -> [Setting; 26] {
        let mut out = [word(W0); 26];
        let mut k = 0;
        let groups: [&[Setting]; 3] = [&SETTINGS_SALVO_PLAYERS, &SETTINGS_SALVO_WORLD, &SETTINGS_SALVO_RULES];
        let mut g = 0;
        while g < groups.len() {
            let mut i = 0;
            while i < groups[g].len() {
                out[k] = groups[g][i];
                k += 1;
                i += 1;
            }
            g += 1;
        }
        out[k] = word(W0);
        out[k + 1] = word(W1);
        out[k + 2] = word(W2);
        out[k + 3] = word(W3);
        out
    }

    pub const SALVO: [Setting; 26] = salvo_settings();
}

#[cfg(feature = "salvo")]
pub use salvo_defs::{HEALTH_POINTS, ROUND_COUNTS, TIMER_SECS, WIND_MAX};
#[cfg(feature = "salvo")]
use salvo_defs::{SALVO, SETTINGS_SALVO_PLAYERS, SETTINGS_SALVO_RULES, SETTINGS_SALVO_WORLD, W0, word};

/// Every game setting (Blocks', 4096's, Solitaire's, Salvo's and its
/// words), each with the default that suits a first game. Stored like the
/// apps' settings; edited in the pause menu's Settings.
#[cfg(not(feature = "salvo"))]
pub const SETTINGS: [Setting; 22] = FIRST;
#[cfg(feature = "salvo")]
pub const SETTINGS: [Setting; 48] = {
    let mut out = [word(W0); 48];
    let mut i = 0;
    while i < SALVO_FROM {
        out[i] = FIRST[i];
        i += 1;
    }
    while i < 48 {
        out[i] = SALVO[i - SALVO_FROM];
        i += 1;
    }
    out
};

const FIRST: [Setting; 22] = [
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
    c(0x0E41, Str::GameDraw, DRAWS, 0),
    c(0x0E42, Str::GamePasses, PASSES, 0),
    c(0x0E43, Str::GameScoring, SCORINGS, 0),
    t(0x0E44, Str::GameAutoMove, 1),
    t(0x0E45, Str::GameAutoComplete, 1),
    t(0x0E46, Str::GameTimer, 1),
    t(0x0E47, Str::GameWinnable, 1),
];

/// Settings that change the rules apply from the next game; the others
/// (ghost, previews, animations, auto-shift and repeat, Solitaire's
/// assists and timer, Salvo's aim guide) at once.
#[must_use]
pub fn applies_next_game(sid: SettingId) -> bool {
    ![
        id::BLOCKS_GHOST,
        id::BLOCKS_PREVIEWS,
        id::BLOCKS_AUTO_SHIFT,
        id::BLOCKS_REPEAT,
        id::TILES_ANIMATIONS,
        id::SOL_AUTO_MOVE,
        id::SOL_AUTO_COMPLETE,
        id::SOL_TIMER,
        id::SALVO_GUIDE,
    ]
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

/// The Solitaire rules from the settings.
pub fn solitaire_config<S: SettingsService + ?Sized>(s: &S) -> solitaire::Config {
    solitaire::Config {
        draw: if s.setting(id::SOL_DRAW) == 1 { 3 } else { 1 },
        passes: pick(&PASS_LIMITS, s.setting(id::SOL_PASSES), 0),
        scoring: solitaire::Scoring::from_u8(u8::try_from(s.setting(id::SOL_SCORING)).unwrap_or(0))
            .unwrap_or_default(),
        auto_move: s.setting(id::SOL_AUTO_MOVE) != 0,
        auto_complete: s.setting(id::SOL_AUTO_COMPLETE) != 0,
        winnable: s.setting(id::SOL_WINNABLE) != 0,
    }
    .sanitised()
}

/// Solitaire's timer is shown.
pub fn solitaire_timer<S: SettingsService + ?Sized>(s: &S) -> bool {
    s.setting(id::SOL_TIMER) != 0
}

/// The Salvo rules from the settings.
#[cfg(feature = "salvo")]
pub fn salvo_config<S: SettingsService + ?Sized>(s: &S) -> salvo::Config {
    let v = |sid: SettingId| u8::try_from(s.setting(sid)).unwrap_or(0);
    let mut kinds = [salvo::Kind::Human; 4];
    for (i, k) in kinds.iter_mut().enumerate() {
        *k = salvo::Kind::from_u8(v(SettingId(id::SALVO_PLAYER1.0 + i as u16))).unwrap_or_default();
    }
    let wind = match v(id::SALVO_WIND) {
        w @ 0..=2 => WIND_MAX[usize::from(w)],
        _ => v(id::SALVO_CUSTOM_WIND).clamp(1, 30),
    };
    salvo::Config {
        players: 2 + v(id::SALVO_PLAYERS).min(2),
        kinds,
        rounds: pick(&ROUND_COUNTS, s.setting(id::SALVO_ROUNDS), 3),
        gravity: v(id::SALVO_GRAVITY).min(3),
        wind,
        gusts: v(id::SALVO_GUSTS) != 0,
        drag: v(id::SALVO_DRAG).min(2),
        style: salvo::Style::from_u8(v(id::SALVO_TERRAIN)).unwrap_or_default(),
        rough: v(id::SALVO_ROUGHNESS).min(2),
        relief: v(id::SALVO_RELIEF).min(2),
        terrain_seed: u16::from(v(id::SALVO_SEED)),
        width: v(id::SALVO_WIDTH).min(3),
        health: pick(&HEALTH_POINTS, s.setting(id::SALVO_HEALTH), 100),
        fall_damage: v(id::SALVO_FALL) != 0,
        full_set: v(id::SALVO_WEAPONS) != 0,
        shop: v(id::SALVO_SHOP) != 0,
        timer: pick(&TIMER_SECS, s.setting(id::SALVO_TIMER), 0),
    }
    .sanitised()
}

/// Salvo's aim guide: 0 none, 1 the last shot's trace, 2 a short preview.
#[cfg(feature = "salvo")]
pub fn salvo_guide<S: SettingsService + ?Sized>(s: &S) -> u8 {
    u8::try_from(s.setting(id::SALVO_GUIDE)).unwrap_or(0).min(2)
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

/// What the card side is asked to do, one at a time ([`GamesIo`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardOp {
    /// Read [`SCORES_PATH`] into the buffer and see which games have a
    /// save file ([`Game::path`]).
    Load,
    /// Read the game's save file into the buffer.
    Read(u8),
    /// Write the buffer's bytes as the game's save file (creating
    /// [`DIR`]).
    Write(u8),
    /// Remove the game's save file (none is fine).
    Remove(u8),
    /// Write the buffer's bytes as [`SCORES_PATH`].
    WriteScores,
}

/// The card side's answer to a [`CardOp`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CardAnswer {
    pub ok: bool,
    /// Bytes read into the buffer (`Load`, `Read`).
    pub len: u16,
    /// `Load`: bit `id` set for every game with a save file.
    pub saved: u8,
}

impl CardAnswer {
    /// The operation could not be carried out (no card, a card error, no
    /// answer in time).
    pub const FAILED: CardAnswer = CardAnswer { ok: false, len: 0, saved: 0 };
}

/// Operations queued at most.
const IO_QUEUE: usize = 8;

/// The storage side of [`GamesService`] for a firmware whose card belongs
/// to another task: requests become card operations in a short queue, one
/// out at a time, each with the bytes it writes encoded when it goes out
/// (so a save holds the game as it is then) and its answer applied to the
/// hub when it comes back. `Open` is answered late: until the save file is
/// read the hub's `opening` names the game and the game screen waits.
/// The simulator runs the same logic in its deferred mode (`set:gamesio=on`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GamesIo {
    queue: [Option<CardOp>; IO_QUEUE],
    out: Option<CardOp>,
}

impl Default for GamesIo {
    fn default() -> Self {
        GamesIo::new()
    }
}

impl GamesIo {
    #[must_use]
    pub const fn new() -> GamesIo {
        GamesIo { queue: [None; IO_QUEUE], out: None }
    }

    /// The operation out (sent to the card side, not answered yet).
    #[must_use]
    pub fn out(&self) -> Option<CardOp> {
        self.out
    }

    /// Something is queued or out.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.out.is_some() || self.queue[0].is_some()
    }

    fn push(&mut self, op: CardOp) {
        if self.queue.contains(&Some(op)) {
            return;
        }
        if let Some(slot) = self.queue.iter_mut().find(|s| s.is_none()) {
            *slot = Some(op);
        }
    }

    /// The operation out could not be sent (the card side's queue was
    /// full): it goes back to the front, to be sent again.
    pub fn put_back(&mut self) {
        if let Some(op) = self.out.take() {
            self.queue.copy_within(..IO_QUEUE - 1, 1);
            self.queue[0] = Some(op);
        }
    }

    fn pop(&mut self) -> Option<CardOp> {
        let op = self.queue[0].take()?;
        self.queue.copy_within(1.., 0);
        self.queue[IO_QUEUE - 1] = None;
        Some(op)
    }

    /// A screen's request, as card operations. The score table is read
    /// before anything else.
    pub fn request(&mut self, hub: &mut Hub, r: GamesRequest) {
        if !hub.loaded && self.out != Some(CardOp::Load) {
            self.push(CardOp::Load);
        }
        match r {
            GamesRequest::Load => {}
            GamesRequest::Open(id) => {
                if hub.session.game() == Some(id) {
                    hub.opening = None;
                    return;
                }
                if hub.opening == Some(id) {
                    return;
                }
                // Another game in progress waits in its file.
                if let Some(g) = hub.session.game().filter(|_| hub.session.in_progress()) {
                    self.push(CardOp::Write(g));
                }
                hub.opening = Some(id);
                self.push(CardOp::Read(id));
            }
            GamesRequest::Save => {
                if let Some(g) = hub.session.game() {
                    self.push(CardOp::Write(g));
                }
            }
            GamesRequest::Discard(id) => {
                hub.set_saved(id, false);
                self.push(CardOp::Remove(id));
            }
            GamesRequest::SaveScores => self.push(CardOp::WriteScores),
        }
    }

    /// The next operation for the card side and the length of the bytes
    /// it writes (encoded into `buf`, at least [`orpheus_games::SAVE_MAX`]
    /// bytes); `None` while one is out or when nothing waits. A save of a
    /// game no longer in RAM is dropped; of a finished one, it removes the
    /// file; a read nobody waits for any more is dropped.
    pub fn next(&mut self, hub: &Hub, buf: &mut [u8]) -> Option<(CardOp, usize)> {
        if self.out.is_some() {
            return None;
        }
        while let Some(op) = self.pop() {
            let job = match op {
                CardOp::Load | CardOp::Remove(_) => Some((op, 0)),
                CardOp::Read(g) => (hub.opening == Some(g)).then_some((op, 0)),
                CardOp::Write(g) if hub.session.game() == Some(g) => {
                    if hub.session.in_progress() {
                        hub.session.encode(buf).map(|n| (op, n))
                    } else {
                        Some((CardOp::Remove(g), 0))
                    }
                }
                CardOp::Write(_) => None,
                CardOp::WriteScores => hub.scores.encode(buf).map(|n| (op, n)),
            };
            if let Some(j) = job {
                self.out = Some(j.0);
                return Some(j);
            }
        }
        None
    }

    /// The card side answered the operation out; `buf` holds the bytes it
    /// read. True if the hub changed (the screens look again).
    pub fn done(&mut self, hub: &mut Hub, a: CardAnswer, buf: &[u8]) -> bool {
        let Some(op) = self.out.take() else { return false };
        let data = buf.get(..usize::from(a.len)).unwrap_or(&[]);
        match op {
            CardOp::Load => {
                // A failed read is tried again with the next request (the
                // card may be back by then); the table is never written
                // before it was read.
                if a.ok {
                    loaded(hub, a.saved, data);
                }
                true
            }
            CardOp::Read(g) => {
                if hub.opening != Some(g) {
                    return false;
                }
                hub.opening = None;
                if !(a.ok && hub.session.decode_into(data, Some(g))) {
                    hub.session = Session::None;
                }
                true
            }
            CardOp::Write(g) => {
                hub.set_saved(g, a.ok);
                true
            }
            CardOp::Remove(g) => {
                hub.set_saved(g, false);
                true
            }
            // A failed write is made again with the next score change.
            CardOp::WriteScores => false,
        }
    }
}

/// The score table and the save files' bits read. A frame of its own:
/// the decoded table (≈ 490 B) stayed in [`GamesIo::done`]'s frame under
/// a save file's decoding, the UI task's deepest path (STACK, 2026-11-25).
#[inline(never)]
fn loaded(hub: &mut Hub, saved: u8, data: &[u8]) {
    hub.loaded = true;
    if let Some(s) = orpheus_games::Scores::decode(data) {
        hub.scores = s;
    }
    for g in Game::ALL {
        hub.set_saved(g.id(), saved & (1 << g.id()) != 0);
    }
}

/// Allocation sectors (exFAT bitmap or FAT) one [`Stage::Space`] step
/// reads at most: well under a millisecond of card reads on the player.
const SPACE_SECTORS: u32 = 8;

/// [`Stage::Space`] steps before the allocation searches the rest itself
/// (only a nearly full card: 2,040 sectors are 1 TB of exFAT bitmap at
/// 128 KiB clusters).
const SPACE_STEPS: u8 = 255;

/// Where a [`CardRun`] is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    /// Open the file and read it (`Load`: the score table), or (`Write`)
    /// rewrite it in place when it exists and its cluster holds the bytes
    /// (`Volume::rewrite`: nothing freed or allocated), its entry into
    /// the sector cache.
    Open,
    /// `Write`: the file is missing (or its clusters do not fit): the
    /// volume's next-free hint moved to a free cluster, [`SPACE_SECTORS`]
    /// allocation sectors per step (`Volume::find_free_step`; `k` steps so
    /// far, after [`SPACE_STEPS`] the allocation searches the rest).
    Space(u8),
    /// `Write`: open truncated (creating the file), write the bytes and
    /// the entry into the sector cache; `true` once the folders were made.
    Alloc(bool),
    /// `Write`: the file could not be created; make `/.orpheus` (0) and
    /// [`DIR`] (1), then [`Stage::Alloc`] again.
    MakeDir(u8),
    /// `Load`: does the save file of `Game::ALL[k]` exist? `Remove`: does
    /// the file exist (`k` 0)?
    Exists(u8),
    /// `Remove`: remove the file.
    Remove,
    /// The volume's dirty sectors back to the card, one per step, then
    /// the sync and the clean flag (`Volume::flush_step`).
    Flush,
}

/// A [`CardOp`] carried out in steps (a path walk with a read, or with a
/// write into the sector cache; one sector written back; the sync), so that
/// the firmware's card task runs it in pieces between DAC ring refills
/// while music plays: whole, a save written before another game is read
/// was one 100 ms piece, beyond every chore budget (`serve:games`, 8
/// underruns on the player, 2026-11-25). The bytes are those of
/// [`GamesIo::next`]'s buffer; the answer is [`GamesIo::done`]'s. Only
/// files under [`DIR`] (CLAUDE.md rule 5). No file handle is kept between
/// steps (a few bytes of state in the card task).
///
/// A save that exists is rewritten in place (its one cluster holds any
/// save): a truncating open freed its cluster and the write allocated one,
/// on the owner's card (exFAT keeps no next-free hint) a search of 452
/// bitmap sectors in the first step after every start, one 41 ms stretch
/// (2026-11-25). A new file's allocation is prepared in bounded steps
/// ([`Stage::Space`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CardRun {
    op: CardOp,
    stage: Stage,
    /// Bytes to write.
    len: u16,
    answer: CardAnswer,
}

/// The save file of engine id `g`.
fn save_path(g: u8) -> Option<&'static str> {
    Game::from_id(g).map(Game::path)
}

impl CardRun {
    /// `op` with `len` bytes to write (`GamesIo::next`'s answer).
    #[must_use]
    pub fn new(op: CardOp, len: usize) -> CardRun {
        let stage = if matches!(op, CardOp::Remove(_)) { Stage::Exists(0) } else { Stage::Open };
        CardRun {
            op,
            stage,
            len: u16::try_from(len).unwrap_or(0),
            answer: CardAnswer { ok: true, len: 0, saved: 0 },
        }
    }

    #[must_use]
    pub fn op(&self) -> CardOp {
        self.op
    }

    /// The file the operation is on.
    fn path(&self) -> Option<&'static str> {
        match self.op {
            CardOp::Load | CardOp::WriteScores => Some(SCORES_PATH),
            CardOp::Read(g) | CardOp::Write(g) | CardOp::Remove(g) => save_path(g),
        }
    }

    /// One piece: steps while `more` says another may start (the firmware's
    /// `audio::piece_left`: always for the first, and always outside a
    /// chore, so the idle card task finishes in one piece). `Some` with the
    /// answer when the operation is done.
    pub async fn piece<D: orpheus_fs::BlockDevice>(
        &mut self,
        vol: &mut orpheus_fs::Volume<'_, D>,
        buf: &mut [u8],
        mut more: impl FnMut() -> bool,
    ) -> Option<CardAnswer> {
        while more() {
            if let Some(a) = self.step(vol, buf).await {
                return Some(a);
            }
        }
        None
    }

    /// One step. `Some` with the answer when done.
    pub async fn step<D: orpheus_fs::BlockDevice>(
        &mut self,
        vol: &mut orpheus_fs::Volume<'_, D>,
        buf: &mut [u8],
    ) -> Option<CardAnswer> {
        let Some(path) = self.path() else { return self.fail() };
        match self.stage {
            Stage::Open => {
                if matches!(self.op, CardOp::Write(_) | CardOp::WriteScores) {
                    self.rewrite(vol, path, buf).await;
                    return None;
                }
                let read = Self::read(vol, path, buf).await;
                match (read, self.op) {
                    (Some(n), CardOp::Load) => {
                        self.answer.len = n;
                        self.stage = Stage::Exists(0);
                    }
                    // No table yet (or a damaged one) is fine: an empty one.
                    (None, CardOp::Load) => self.stage = Stage::Exists(0),
                    (Some(n), _) => {
                        self.answer.len = n;
                        return Some(self.answer);
                    }
                    (None, _) => return self.fail(),
                }
            }
            Stage::Space(k) => {
                self.stage = match vol.find_free_step(SPACE_SECTORS).await {
                    Ok(false) if k < SPACE_STEPS => Stage::Space(k + 1),
                    _ => Stage::Alloc(false),
                };
            }
            Stage::Alloc(made) => return self.open_write(vol, path, buf, made).await,
            Stage::MakeDir(k) => {
                let _ = vol.create_dir(if k == 0 { "/.orpheus" } else { DIR }).await;
                self.stage = if k == 0 { Stage::MakeDir(1) } else { Stage::Alloc(true) };
            }
            Stage::Exists(k) => {
                if let CardOp::Remove(_) = self.op {
                    if !vol.exists(path).await.unwrap_or(false) {
                        return Some(self.answer);
                    }
                    self.stage = Stage::Remove;
                } else {
                    let Some(g) = Game::ALL.get(usize::from(k)) else { return Some(self.answer) };
                    if vol.exists(g.path()).await.unwrap_or(false) {
                        self.answer.saved |= 1 << g.id();
                    }
                    if usize::from(k) + 1 >= Game::ALL.len() {
                        return Some(self.answer);
                    }
                    self.stage = Stage::Exists(k + 1);
                }
            }
            Stage::Remove => {
                if vol.remove(path).await.is_err() {
                    self.answer.ok = false;
                }
                self.stage = Stage::Flush;
            }
            Stage::Flush => match vol.flush_step().await {
                Ok(true) => return Some(self.answer),
                Ok(false) => {}
                Err(_) => return self.fail(),
            },
        }
        None
    }

    /// A whole file of at most `buf.len()` bytes into `buf`.
    async fn read<D: orpheus_fs::BlockDevice>(
        vol: &mut orpheus_fs::Volume<'_, D>,
        path: &str,
        buf: &mut [u8],
    ) -> Option<u16> {
        let mut f = vol.open(path, orpheus_fs::OpenMode::Read).await.ok()?;
        if f.size() > buf.len() as u64 {
            return None;
        }
        // A file opened for reading needs no close.
        let n = vol.read_exact_or_eof(&mut f, buf).await.ok()?;
        u16::try_from(n).ok()
    }

    /// The bytes over the file's own cluster and its directory entry (into
    /// the sector cache; [`Stage::Flush`] writes them back), or, if the
    /// file is missing or its clusters do not fit, on to
    /// [`Stage::Space`].
    ///
    /// Power cut: the data sector reaches the card before the entry (the
    /// cache's order), so a cut leaves the old file, the new bytes with
    /// the old length, or the new file; the volume never needs repair (no
    /// cluster changes hands). A save is framed with its length and a CRC
    /// (`orpheus_games::save::open`): new bytes under an old length of
    /// another size read as no save, as the truncated empty file a cut
    /// left before.
    async fn rewrite<D: orpheus_fs::BlockDevice>(
        &mut self,
        vol: &mut orpheus_fs::Volume<'_, D>,
        path: &str,
        buf: &[u8],
    ) {
        let data = buf.get(..usize::from(self.len)).unwrap_or(&[]);
        self.stage = Stage::Space(0);
        if let Ok(mut f) = vol.open(path, orpheus_fs::OpenMode::ReadWrite).await {
            match vol.rewrite(&mut f, data).await {
                Ok(false) => {}
                Ok(true) => {
                    self.answer.ok = vol.write_entry(&mut f).await.is_ok();
                    self.stage = Stage::Flush;
                }
                // What was done is still written back.
                Err(_) => {
                    self.answer.ok = false;
                    self.stage = Stage::Flush;
                }
            }
        }
    }

    /// Open truncated, write the bytes and the directory entry (into the
    /// sector cache; [`Stage::Flush`] writes them back). `made`: the
    /// folders were made already.
    async fn open_write<D: orpheus_fs::BlockDevice>(
        &mut self,
        vol: &mut orpheus_fs::Volume<'_, D>,
        path: &str,
        buf: &[u8],
        made: bool,
    ) -> Option<CardAnswer> {
        let mut f = match vol.open(path, orpheus_fs::OpenMode::Truncate).await {
            Ok(f) => f,
            Err(_) if !made => {
                self.stage = Stage::MakeDir(0);
                return None;
            }
            Err(_) => return self.fail(),
        };
        let data = buf.get(..usize::from(self.len)).unwrap_or(&[]);
        let wrote = vol.write(&mut f, data).await.is_ok_and(|n| n == data.len());
        // What was done is still written back.
        self.answer.ok = wrote && vol.write_entry(&mut f).await.is_ok();
        self.stage = Stage::Flush;
        None
    }

    fn fail(&mut self) -> Option<CardAnswer> {
        self.answer = CardAnswer::FAILED;
        Some(self.answer)
    }
}

/// Start a new game of `game` with the settings as they are now.
///
/// Solitaire with casino scoring: the game it replaces pays out its
/// foundation cards, the new deal is charged to the bank (so that a deal
/// cannot be refused for free), and the table is written.
pub fn new_game<S: SettingsService + GamesService + ?Sized>(svc: &mut S, game: Game) {
    let seed = svc.game_seed();
    let banked = settle_casino(svc);
    let charge = start_session(svc, game, seed);
    if charge {
        svc.games_mut().scores.add_to_bank(gid::SOLITAIRE, -solitaire::CASINO_DEAL, true);
    }
    // A request may apply an answer of the card side at once (the
    // firmware): the new game is built by then, in a frame of its own.
    if banked || charge {
        svc.games_request(GamesRequest::SaveScores);
    }
}

/// The new game into the session; true for a casino deal (charged).
/// Salvo's is built in place (680 bytes: no copy on the stack).
#[inline(never)]
fn start_session<S: SettingsService + GamesService + ?Sized>(svc: &mut S, game: Game, seed: u64) -> bool {
    let session = match game {
        Game::Blocks => Session::Blocks(blocks::Blocks::new(blocks_config(svc), seed)),
        Game::Tiles => Session::Tiles(tiles::Tiles::new(tiles_config(svc), seed)),
        Game::Solitaire => Session::Solitaire(solitaire::Solitaire::new(solitaire_config(svc), seed)),
        #[cfg(feature = "salvo")]
        Game::Salvo => {
            let cfg = salvo_config(svc);
            svc.games_mut().session.start_salvo(cfg, seed);
            return false;
        }
    };
    let charge =
        matches!(&session, Session::Solitaire(s) if s.config().scoring == solitaire::Scoring::Casino);
    svc.games_mut().session = session;
    charge
}

/// What a casino game earned so far: 5 per card on the foundations.
fn casino_earned(s: &Session) -> Option<i32> {
    match s {
        Session::Solitaire(g) if g.config().scoring == solitaire::Scoring::Casino => {
            Some(solitaire::CASINO_CARD * i32::try_from(g.board().on_foundations()).unwrap_or(0))
        }
        _ => None,
    }
}

/// A casino game in progress that is left for a new one pays out what it
/// earned (its deal was charged when it was dealt). True if it did.
fn settle_casino<S: GamesService + ?Sized>(svc: &mut S) -> bool {
    let hub = svc.games_mut();
    match casino_earned(&hub.session) {
        Some(earned) if hub.session.in_progress() => {
            hub.scores.add_to_bank(gid::SOLITAIRE, earned, false);
            true
        }
        _ => false,
    }
}

/// The casino bank as shown during a game: the table's bank (the deal
/// already paid) plus what the game on screen has earned so far.
#[must_use]
pub fn casino_bank<S: GamesService + ?Sized>(svc: &S) -> i32 {
    let hub = svc.games();
    let earned = if hub.session.in_progress() { casino_earned(&hub.session).unwrap_or(0) } else { 0 };
    hub.scores.bank(gid::SOLITAIRE) + earned
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
        Session::Solitaire(g) if g.ended() => {
            let ms = orpheus_games::steps_to_ms(g.steps());
            let won = g.won();
            let up = g.board().on_foundations();
            let o = match g.config().scoring {
                solitaire::Scoring::Standard => Outcome {
                    result: u32::try_from(g.score()).unwrap_or(0),
                    detail: ms,
                    lower_is_better: false,
                    ranked: true,
                    won,
                },
                // The game's payout (5 per card); the bank is the table's.
                solitaire::Scoring::Casino => Outcome {
                    result: up * solitaire::CASINO_CARD.unsigned_abs(),
                    detail: ms,
                    lower_is_better: false,
                    ranked: true,
                    won,
                },
                // No score: the time of a won game.
                solitaire::Scoring::None => {
                    Outcome { result: ms.max(1), detail: up, lower_is_better: true, ranked: won, won }
                }
            };
            Some((gid::SOLITAIRE, g.config().variant(), o))
        }
        #[cfg(feature = "salvo")]
        Session::Salvo(g) if g.ended() => {
            // The best human's points; won when a human leads. A game of
            // computers only is not ranked.
            let (best, lead) = g.human_result().unwrap_or((0, false));
            let ranked = g.config().humans() > 0;
            Some((
                gid::SALVO,
                g.config().variant(),
                Outcome {
                    result: u32::try_from(best).unwrap_or(0),
                    detail: u32::from(g.config().rounds),
                    lower_is_better: false,
                    ranked,
                    won: lead,
                },
            ))
        }
        _ => None,
    }
}

/// A game ended: count it in the scores, write them, and remove the save
/// file (a finished game is not offered again). Returns true for a new
/// best.
pub fn finish<S: GamesService + ?Sized>(svc: &mut S) -> bool {
    let Some((game, variant, o)) = outcome(&svc.games().session) else { return false };
    if let Some(earned) = casino_earned(&svc.games().session) {
        svc.games_mut().scores.add_to_bank(gid::SOLITAIRE, earned, false);
    }
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
        Game::Solitaire => solitaire_config(svc).variant(),
        #[cfg(feature = "salvo")]
        Game::Salvo => salvo_config(svc).variant(),
    };
    svc.games().scores.get(game.id(), variant).map(|r| r.best).filter(|&b| b > 0)
}

/// Whether the best result of `game` under the current settings is a time.
pub fn best_is_time<S: SettingsService + ?Sized>(svc: &S, game: Game) -> bool {
    match game {
        Game::Blocks => blocks_config(svc).mode == blocks::Mode::Lines40,
        Game::Solitaire => solitaire_config(svc).scoring == solitaire::Scoring::None,
        Game::Tiles => false,
        #[cfg(feature = "salvo")]
        Game::Salvo => false,
    }
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
        // Every setting in its game's list, Salvo's four words besides.
        let words = if cfg!(feature = "salvo") { 4 } else { 0 };
        assert_eq!(Game::ALL.iter().map(|g| g.settings().len()).sum::<usize>() + words, SETTINGS.len());
        assert!(Game::Solitaire.settings().iter().all(|s| (0x0E41..0x0E61).contains(&s.id.0)));
        #[cfg(feature = "salvo")]
        {
            assert!(
                Game::Salvo.settings().iter().all(|s| (0x0E61..0x0E80).contains(&s.id.0) && s.pack.is_some())
            );
            let grouped: usize = Game::Salvo.groups().iter().map(|(_, l)| l.len()).sum();
            assert_eq!(grouped, Game::Salvo.settings().len(), "every Salvo setting in a group");
        }
        assert_eq!(SETTINGS.iter().filter(|s| s.stored()).count(), STORED_SETTINGS);
        assert!(Game::Tiles.settings().iter().all(|s| (0x0E21..0x0E41).contains(&s.id.0)));
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
        for g in Game::ALL {
            assert_eq!(g.path(), std::format!("{DIR}/{}", g.file()));
        }
        assert_eq!(SCORES_PATH, std::format!("{DIR}/{SCORES_FILE}"));
    }

    /// The firmware's card queue: the table first, a game in progress
    /// written before another is read, `Open` answered late, reads nobody
    /// waits for dropped, a finished game's save removed.
    #[test]
    fn games_io_queue() {
        use orpheus_games::{SAVE_MAX, blocks::Blocks, tiles::Tiles};
        let mut hub = Hub::new();
        let mut io = GamesIo::new();
        let mut buf = [0u8; SAVE_MAX];
        io.request(&mut hub, GamesRequest::Open(gid::TILES));
        assert_eq!(hub.opening, Some(gid::TILES));
        assert_eq!(io.next(&hub, &mut buf), Some((CardOp::Load, 0)));
        assert_eq!(io.next(&hub, &mut buf), None, "one out at a time");
        let ok = CardAnswer { ok: true, len: 0, saved: 1 << gid::TILES };
        assert!(io.done(&mut hub, ok, &buf));
        assert!(hub.loaded && hub.has_save(gid::TILES));
        // The save file arrives.
        let saved = Session::Tiles(Tiles::new(tiles::Config::DEFAULT, 4));
        let mut file = [0u8; SAVE_MAX];
        let n = saved.encode(&mut file).unwrap();
        assert_eq!(io.next(&hub, &mut buf), Some((CardOp::Read(gid::TILES), 0)));
        buf[..n].copy_from_slice(&file[..n]);
        assert!(io.done(&mut hub, CardAnswer { ok: true, len: n as u16, saved: 0 }, &buf));
        assert_eq!((hub.opening, &hub.session), (None, &saved));
        // Another game: this one is written first, with its bytes.
        io.request(&mut hub, GamesRequest::Open(gid::BLOCKS));
        let (op, len) = io.next(&hub, &mut buf).unwrap();
        assert_eq!(op, CardOp::Write(gid::TILES));
        assert_eq!(Session::decode(&buf[..len], Some(gid::TILES)), Some(saved));
        assert!(io.done(&mut hub, CardAnswer { ok: true, ..CardAnswer::default() }, &buf));
        // The player went back before the read: the read is dropped.
        hub.opening = None;
        assert_eq!(io.next(&hub, &mut buf), None);
        assert!(!io.busy());
        // A finished game's save goes; a failed read starts no game.
        hub.session = Session::Blocks(Blocks::new(blocks::Config::DEFAULT, 1));
        io.request(&mut hub, GamesRequest::Open(gid::SOLITAIRE));
        assert_eq!(io.next(&hub, &mut buf).map(|j| j.0), Some(CardOp::Write(gid::BLOCKS)));
        io.done(&mut hub, CardAnswer::FAILED, &buf);
        assert!(!hub.has_save(gid::BLOCKS), "a failed write is no save");
        assert_eq!(io.next(&hub, &mut buf).map(|j| j.0), Some(CardOp::Read(gid::SOLITAIRE)));
        io.done(&mut hub, CardAnswer::FAILED, &buf);
        assert_eq!((hub.opening, hub.session.game()), (None, None));
        hub.session = Session::Tiles(Tiles::from_cells(
            tiles::Config::DEFAULT,
            1,
            &[1, 2, 1, 2, 2, 1, 2, 1, 1, 2, 1, 2, 2, 1, 2, 1],
        ));
        assert!(!hub.session.in_progress());
        io.request(&mut hub, GamesRequest::Save);
        assert_eq!(io.next(&hub, &mut buf).map(|j| j.0), Some(CardOp::Remove(gid::TILES)));
        io.done(&mut hub, CardAnswer { ok: true, ..CardAnswer::default() }, &buf);
        // Scores: encoded when they go out; the queue holds each op once.
        io.request(&mut hub, GamesRequest::SaveScores);
        io.request(&mut hub, GamesRequest::SaveScores);
        let (op, len) = io.next(&hub, &mut buf).unwrap();
        assert_eq!((op, orpheus_games::Scores::decode(&buf[..len]).is_some()), (CardOp::WriteScores, true));
        io.done(&mut hub, CardAnswer { ok: true, ..CardAnswer::default() }, &buf);
        assert_eq!(io.next(&hub, &mut buf), None);
        // A failed table read is tried again; the table is not written
        // before it was read.
        let mut hub = Hub::new();
        let mut io = GamesIo::new();
        io.request(&mut hub, GamesRequest::Load);
        io.next(&hub, &mut buf);
        io.done(&mut hub, CardAnswer::FAILED, &buf);
        assert!(!hub.loaded);
        io.request(&mut hub, GamesRequest::SaveScores);
        assert_eq!(io.next(&hub, &mut buf).map(|j| j.0), Some(CardOp::Load));
    }

    /// A formatted card image (`mkfs`; `None` if the tool is missing):
    /// exFAT with 128 KiB clusters (the owner's card) or FAT32 with
    /// 512-byte clusters (two FAT copies, the most metadata per byte).
    fn card_image(exfat: bool) -> Option<orpheus_fs::host::RamDisk> {
        use std::process::Command;
        let tool = if exfat { "mkfs.exfat" } else { "mkfs.fat" };
        let dir = std::env::temp_dir().join("orpheus-apps-games-card");
        std::fs::create_dir_all(&dir).ok()?;
        let p = dir.join(std::format!("{tool}-{}.img", std::process::id()));
        let f = std::fs::File::create(&p).ok()?;
        f.set_len(if exfat { 192 << 20 } else { 64 << 20 }).ok()?;
        drop(f);
        let mut c = Command::new(tool);
        if exfat {
            c.args(["-q", "-c", "131072"]);
        } else {
            c.args(["-F", "32", "-s", "1"]);
        }
        let ok = c.arg(&p).output().is_ok_and(|o| o.status.success());
        let disk = ok.then(|| orpheus_fs::host::RamDisk::load(&p).ok()).flatten();
        let _ = std::fs::remove_file(&p);
        if disk.is_none() {
            std::eprintln!("{tool} missing: skipped");
        }
        disk
    }

    /// Player, 2026-11-25: with music playing, switching games (a save
    /// written, then another read) was one `serve:games` piece of 100 ms,
    /// above every chore budget (≈ 42 ms), forced after the starvation
    /// bound: 8 underruns. Now every card operation runs in steps of at
    /// most one path walk, file read or write, one sector written back or
    /// the sync, and a piece whose budget allows one step does no more. The
    /// result is the same as in one piece (the idle card task's case:
    /// power-off, USB hand-over).
    #[test]
    fn regress_games_card_ops_in_pieces() {
        use orpheus_fs::host::{RamDisk, block_on};
        use orpheus_fs::{CacheSlot, MountOptions, Volume, WalkCache};
        use orpheus_games::{SAVE_MAX, tiles::Tiles};
        for exfat in [true, false] {
            let Some(disk) = card_image(exfat) else { continue };
            let mut results = std::vec::Vec::new();
            for one_step in [true, false] {
                let mut cache = std::vec![CacheSlot::EMPTY; 8];
                let mut walks = WalkCache::EMPTY;
                let opts = MountOptions { read_only: false, write_prefix: None, partition: None };
                let mut vol: Volume<'_, RamDisk> =
                    block_on(Volume::mount(disk.clone(), &mut cache, opts)).expect("mount");
                vol.set_walk_cache(&mut walks);
                block_on(vol.create_dir("/Music")).unwrap();
                block_on(vol.flush()).unwrap();
                let save = Session::Tiles(Tiles::new(tiles::Config::DEFAULT, 4));
                let mut file = [0u8; SAVE_MAX];
                let n = save.encode(&mut file).unwrap();
                let scores = orpheus_games::Scores::new();
                let mut table = [0u8; SAVE_MAX];
                let tn = scores.encode(&mut table).unwrap();
                let ops = [
                    (CardOp::Load, 0),
                    (CardOp::Write(gid::TILES), n),
                    (CardOp::Write(gid::TILES), n),
                    (CardOp::WriteScores, tn),
                    (CardOp::Read(gid::TILES), 0),
                    (CardOp::Load, 0),
                    (CardOp::Remove(gid::TILES), 0),
                    (CardOp::Read(gid::TILES), 0),
                    (CardOp::Remove(gid::BLOCKS), 0),
                    (CardOp::Load, 0),
                ];
                let mut answers = std::vec::Vec::new();
                for (op, len) in ops {
                    let mut buf = [0u8; SAVE_MAX];
                    match op {
                        CardOp::Write(_) => buf[..n].copy_from_slice(&file[..n]),
                        CardOp::WriteScores => buf[..tn].copy_from_slice(&table[..tn]),
                        _ => {}
                    }
                    let mut run = CardRun::new(op, len);
                    let mut pieces = 0;
                    let mut most = (0u64, 0u64);
                    let a = loop {
                        let (r0, w0) = (vol.device().reads, vol.device().writes);
                        let mut first = true;
                        let mut more = || core::mem::replace(&mut first, !one_step);
                        let a = block_on(run.piece(&mut vol, &mut buf, &mut more));
                        pieces += 1;
                        let (r, w) = (vol.device().reads - r0, vol.device().writes - w0);
                        most = (most.0.max(r), most.1.max(w));
                        // Making the games folder zeroes its cluster
                        // (once in a card's life): not bounded here.
                        let made_dir = matches!(run.stage, Stage::MakeDir(1) | Stage::Alloc(true));
                        if one_step && !made_dir {
                            // FAT32's truncating open: the dirty flag in
                            // both FATs and the freed chain's FAT sector.
                            assert!(w <= 3, "{op:?} (exFAT {exfat}): a piece wrote {w} sectors");
                            assert!(r + w <= 6, "{op:?} (exFAT {exfat}): a piece did {r} reads, {w} writes");
                        }
                        if let Some(a) = a {
                            break a;
                        }
                        assert!(pieces < 40, "{op:?} never ends");
                    };
                    if !one_step {
                        assert_eq!(pieces, 1, "{op:?}: one piece outside a chore");
                    } else if matches!(op, CardOp::Write(_) | CardOp::WriteScores) {
                        // Open and write, at least one sector back, the sync.
                        assert!(pieces >= 3, "{op:?}: {pieces} pieces");
                    }
                    if one_step {
                        std::eprintln!(
                            "exFAT {exfat}: {op:?}: {pieces} pieces, ≤ {} reads, ≤ {} writes",
                            most.0,
                            most.1
                        );
                    }
                    if op == CardOp::Read(gid::TILES) && a.ok {
                        assert_eq!(
                            Session::decode(&buf[..usize::from(a.len)], Some(gid::TILES)),
                            Some(save.clone())
                        );
                    }
                    answers.push(a);
                }
                assert_eq!(vol.dirty_sectors(), 0);
                let (_, r) = block_on(vol.unmount());
                r.unwrap();
                results.push(answers);
            }
            let a = &results[0];
            assert_eq!(a, &results[1], "pieces or not: the same answers");
            assert!(a[0].ok && a[0].saved == 0 && a[0].len == 0, "no table, no saves");
            assert!(a[1].ok && a[2].ok && a[3].ok, "writes");
            assert!(a[4].ok && usize::from(a[4].len) > 0, "read back");
            assert!(a[5].ok && a[5].saved == 1 << gid::TILES && a[5].len > 0, "table and a save");
            assert!(a[6].ok && !a[7].ok && a[8].ok, "removed; read fails; nothing to remove");
            assert_eq!(a[9].saved, 0);
        }
    }

    /// Casino: the deal is charged, a game left pays its cards out.
    #[test]
    fn casino_deal_charged_and_paid() {
        struct G(SettingsCache, Hub, u32);
        impl SettingsService for G {
            fn setting(&self, id: SettingId) -> i16 {
                self.0.get(id)
            }
            fn set_setting(&mut self, id: SettingId, v: i16) {
                self.0.set(id, v);
            }
            fn factory_reset(&mut self) {}
        }
        impl GamesService for G {
            fn games(&self) -> &Hub {
                &self.1
            }
            fn games_mut(&mut self) -> &mut Hub {
                &mut self.1
            }
            fn games_request(&mut self, r: GamesRequest) {
                if r == GamesRequest::SaveScores {
                    self.2 += 1;
                }
            }
            fn game_seed(&mut self) -> u64 {
                9
            }
        }
        let mut g = G(SettingsCache::new(), Hub::new(), 0);
        g.set_setting(id::SOL_SCORING, 1);
        new_game(&mut g, Game::Solitaire);
        assert_eq!((g.1.scores.bank(gid::SOLITAIRE), g.2), (-52, 1));
        let up = match &g.1.session {
            Session::Solitaire(s) => s.board().on_foundations() as i32,
            _ => unreachable!(),
        };
        assert_eq!(casino_bank(&g), -52 + 5 * up);
        new_game(&mut g, Game::Solitaire);
        assert_eq!(g.1.scores.bank(gid::SOLITAIRE), -104 + 5 * up);
        assert!(!best_is_time(&g, Game::Solitaire));
        g.set_setting(id::SOL_SCORING, 2);
        assert!(best_is_time(&g, Game::Solitaire));
    }

    /// A block device that counts sector I/O. While `build`, writes of
    /// all-zero sectors are dropped: the images are sparse and read as
    /// zeros wherever nothing was written (filling a card image with music
    /// would otherwise write gigabytes of zeros).
    struct Counted<D> {
        inner: D,
        reads: u64,
        writes: u64,
        build: bool,
    }

    impl<D: orpheus_fs::BlockDevice> orpheus_fs::BlockDevice for Counted<D> {
        type Error = D::Error;
        async fn read(&mut self, lba: u32, buf: &mut [orpheus_fs::Sector]) -> Result<(), D::Error> {
            self.reads += buf.len() as u64;
            self.inner.read(lba, buf).await
        }
        async fn write(&mut self, lba: u32, buf: &[orpheus_fs::Sector]) -> Result<(), D::Error> {
            if self.build && buf.iter().all(|s| *s == [0u8; orpheus_fs::SECTOR_SIZE]) {
                return Ok(());
            }
            self.writes += buf.len() as u64;
            self.inner.write(lba, buf).await
        }
        async fn sync(&mut self) -> Result<(), D::Error> {
            self.inner.sync().await
        }
        fn sector_count(&self) -> u32 {
            self.inner.sector_count()
        }
    }

    type OwnerVol<'c> = orpheus_fs::Volume<'c, Counted<orpheus_fs::host::FileDisk>>;

    /// Mount an owner-like image as the firmware does: 8 cache slots, a
    /// walk cache with `/.orpheus` warmed.
    fn owner_mount<'c>(
        path: &std::path::Path,
        cache: &'c mut [orpheus_fs::CacheSlot],
        walks: &'c mut orpheus_fs::WalkCache,
        build: bool,
    ) -> OwnerVol<'c> {
        use orpheus_fs::host::{FileDisk, block_on};
        let disk = Counted { inner: FileDisk::open(path).unwrap(), reads: 0, writes: 0, build };
        let opts = orpheus_fs::MountOptions { read_only: false, write_prefix: None, partition: None };
        let mut vol = block_on(orpheus_fs::Volume::mount(disk, cache, opts)).expect("mount");
        vol.set_walk_cache(walks);
        if !build {
            block_on(vol.warm_walk("/.orpheus")).unwrap();
        }
        vol
    }

    /// The owner's card where it matters (2026-11-25: a games save's first
    /// piece was one 41 ms stretch while music played): a sparse image of
    /// exFAT with 128 KiB clusters over 238 GiB (an allocation bitmap of
    /// 476 sectors) or FAT32 with 32 KiB clusters over 8 GiB (a FAT of
    /// 2,048 sectors), 300 folders in the root, music filling 95 % of the
    /// clusters from the start, then `/.orpheus/games` with saves of two
    /// games and the score table. `None` if `mkfs` is missing.
    fn owner_card(exfat: bool) -> Option<std::path::PathBuf> {
        use orpheus_fs::host::block_on;
        use orpheus_fs::{CacheSlot, OpenMode, WalkCache};
        use orpheus_games::{SAVE_MAX, tiles::Tiles};
        use std::process::Command;
        let tool = if exfat { "mkfs.exfat" } else { "mkfs.fat" };
        let dir = std::env::temp_dir().join("orpheus-apps-games-owner");
        std::fs::create_dir_all(&dir).ok()?;
        let p = dir.join(std::format!("{tool}-{}-{exfat}.img", std::process::id()));
        let f = std::fs::File::create(&p).ok()?;
        f.set_len(if exfat { 238 << 30 } else { 8 << 30 }).ok()?;
        drop(f);
        let mut c = Command::new(tool);
        if exfat {
            c.args(["-q", "-c", "131072"]);
        } else {
            c.args(["-F", "32", "-s", "64"]);
        }
        if !c.arg(&p).output().is_ok_and(|o| o.status.success()) {
            std::eprintln!("{tool} missing: skipped");
            let _ = std::fs::remove_file(&p);
            return None;
        }
        let mut cache = std::vec![CacheSlot::EMPTY; 8];
        let mut walks = WalkCache::EMPTY;
        let mut vol = owner_mount(&p, &mut cache, &mut walks, true);
        block_on(async {
            for k in 0..300 {
                vol.create_dir(&std::format!("/Album {k:03}")).await.unwrap();
            }
            // In files of at most 2 GiB (FAT's limit is 4 GiB).
            let mut music = u64::from(vol.cluster_count()) * 95 / 100 * u64::from(vol.cluster_size());
            let mut k = 0;
            while music > 0 {
                let len = music.min(2 << 30);
                let mut f = vol.open(&std::format!("/music{k}.bin"), OpenMode::Create).await.unwrap();
                vol.set_len(&mut f, len).await.unwrap();
                vol.close(f).await.unwrap();
                music -= len;
                k += 1;
            }
        });
        let mut buf = [0u8; SAVE_MAX];
        let n = Session::Tiles(Tiles::new(tiles::Config::DEFAULT, 4)).encode(&mut buf).unwrap();
        for op in [CardOp::Write(gid::TILES), CardOp::Write(gid::BLOCKS), CardOp::WriteScores] {
            let mut run = CardRun::new(op, n);
            let a = block_on(run.piece(&mut vol, &mut buf, || true)).unwrap();
            assert!(a.ok);
        }
        let (_, r) = block_on(vol.unmount());
        r.unwrap();
        Some(p)
    }

    /// FAT32: the FSINFO next-free hint set to "unknown" (0xFFFFFFFF), as
    /// a card last written by a system that does not keep it is.
    fn forget_fsinfo_hint(p: &std::path::Path) {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().read(true).write(true).open(p).unwrap();
        let mut bs = [0u8; 512];
        f.read_exact(&mut bs).unwrap();
        let fsi = u64::from(u16::from_le_bytes([bs[48], bs[49]]));
        f.seek(SeekFrom::Start(fsi * 512 + 492)).unwrap();
        f.write_all(&[0xFF; 4]).unwrap();
    }

    fn io_of(vol: &mut OwnerVol<'_>) -> (u64, u64) {
        let d = vol.device();
        (d.reads, d.writes)
    }

    /// Player, 2026-11-25, music playing, games switched: the first piece
    /// of every save was one 41 ms stretch (`chore: starved serve:games
    /// 43/26 ms`, 12 underruns a minute): the truncating open freed the
    /// save's cluster and the write allocated one, and exFAT keeps no
    /// next-free hint, so the allocation searched the bitmap from its start
    /// (on this image 452 sector reads in one step; FAT32 with an unknown
    /// FSINFO hint, 1,946). Now a save that exists is rewritten in its own
    /// cluster and a new file's allocation is searched for in steps of
    /// [`SPACE_SECTORS`]: every piece of a budget of one step is a few
    /// sector I/Os, and the images stay fsck-clean.
    #[test]
    fn regress_games_save_first_piece_owner_card() {
        use orpheus_fs::host::block_on;
        use orpheus_fs::{CacheSlot, WalkCache};
        use orpheus_games::{SAVE_MAX, tiles::Tiles};
        for exfat in [true, false] {
            let Some(p) = owner_card(exfat) else { continue };
            if !exfat {
                forget_fsinfo_hint(&p);
            }
            let save = Session::Tiles(Tiles::new(tiles::Config { size: 5, ..tiles::Config::DEFAULT }, 7));
            let mut file = [0u8; SAVE_MAX];
            let n = save.encode(&mut file).unwrap();
            let mut cache = std::vec![CacheSlot::EMPTY; 8];
            let mut walks = WalkCache::EMPTY;
            // As after a start: a cold sector cache, `/.orpheus` warmed.
            let mut vol = owner_mount(&p, &mut cache, &mut walks, false);
            let ops = [
                // Saves that exist: in place.
                CardOp::Write(gid::TILES),
                CardOp::WriteScores,
                // A new file: the allocation searched for in steps.
                CardOp::Write(gid::SOLITAIRE),
                // Removed and written again: the freed cluster at once.
                CardOp::Remove(gid::BLOCKS),
                CardOp::Write(gid::BLOCKS),
                CardOp::Load,
                CardOp::Read(gid::TILES),
            ];
            for op in ops {
                let mut buf = file;
                let mut run = CardRun::new(op, n);
                let mut steps = std::vec::Vec::new();
                let a = loop {
                    let (r0, w0) = io_of(&mut vol);
                    let mut first = true;
                    let a = block_on(run.piece(&mut vol, &mut buf, || core::mem::take(&mut first)));
                    let (r, w) = io_of(&mut vol);
                    steps.push((r - r0, w - w0));
                    if let Some(a) = a {
                        break a;
                    }
                    assert!(steps.len() < 300, "{op:?} never ends");
                };
                std::eprintln!("exFAT {exfat}: {op:?}: {} pieces, (reads, writes) {steps:?}", steps.len());
                assert!(a.ok, "exFAT {exfat}: {op:?} failed");
                for (k, &(r, w)) in steps.iter().enumerate() {
                    // The search: SPACE_SECTORS reads. Otherwise a walk, the
                    // dirty flag (FAT: both FATs), one sector back.
                    let most = if matches!(op, CardOp::Write(_)) { u64::from(SPACE_SECTORS) + 2 } else { 4 };
                    assert!(r + w <= most, "exFAT {exfat}: {op:?} piece {k}: {r} reads, {w} writes");
                }
                if op == CardOp::Write(gid::TILES) {
                    // The save's first piece: the old code's 452 (1,946) reads.
                    let (r, w) = steps[0];
                    assert!(r + w <= 4, "exFAT {exfat}: first piece of a save: {r} reads, {w} writes");
                    assert!(steps.iter().map(|s| s.0).sum::<u64>() <= 4, "a save read {steps:?}");
                }
                if op == CardOp::Read(gid::TILES) {
                    assert_eq!(
                        Session::decode(&buf[..usize::from(a.len)], Some(gid::TILES)),
                        Some(save.clone())
                    );
                }
                if op == CardOp::Load {
                    let all = 1 << gid::TILES | 1 << gid::BLOCKS | 1 << gid::SOLITAIRE;
                    assert_eq!(a.saved, all);
                }
            }
            let (_, r) = block_on(vol.unmount());
            r.unwrap();
            let fsck = if exfat { "fsck.exfat" } else { "fsck.fat" };
            match std::process::Command::new(fsck).arg("-n").arg(&p).output() {
                Ok(o) => assert!(
                    o.status.success(),
                    "{fsck}: {}{}",
                    std::string::String::from_utf8_lossy(&o.stdout),
                    std::string::String::from_utf8_lossy(&o.stderr)
                ),
                Err(_) => std::eprintln!("{fsck} missing: not checked"),
            }
            let _ = std::fs::remove_file(&p);
        }
    }
}
