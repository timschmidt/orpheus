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
use orpheus_games::{Hub, Outcome, Rng, Session, blocks, id as gid, solitaire, tiles};

use crate::services::SettingsService;
use crate::settings::{Choice, Setting, SettingId, Unit, c, r, t};
use crate::strings::Str;

/// The games of the app, in list order. The artillery game (GAME-4/5)
/// is not listed until it is built.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Game {
    Blocks,
    Tiles,
    Solitaire,
}

impl Game {
    pub const ALL: [Game; 3] = [Game::Blocks, Game::Tiles, Game::Solitaire];

    /// The engine's id (save files, scores).
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Game::Blocks => gid::BLOCKS,
            Game::Tiles => gid::TILES,
            Game::Solitaire => gid::SOLITAIRE,
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
        }
    }

    /// The save file under [`DIR`].
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Game::Blocks => "blocks.sav",
            Game::Tiles => "4096.sav",
            Game::Solitaire => "solitaire.sav",
        }
    }

    /// The save file's path ([`DIR`] and [`Game::file`]).
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Game::Blocks => "/.orpheus/games/blocks.sav",
            Game::Tiles => "/.orpheus/games/4096.sav",
            Game::Solitaire => "/.orpheus/games/solitaire.sav",
        }
    }

    /// Its settings, in pause-menu order.
    #[must_use]
    pub fn settings(self) -> &'static [Setting] {
        match self {
            Game::Blocks => &SETTINGS[..BLOCKS_SETTINGS],
            Game::Tiles => &SETTINGS[BLOCKS_SETTINGS..SOLITAIRE_FROM],
            Game::Solitaire => &SETTINGS[SOLITAIRE_FROM..],
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

/// Every game setting (Blocks', 4096's, Solitaire's), each with the
/// default that suits a first game. Stored like the apps' settings; edited
/// in the pause menu's Settings.
pub const SETTINGS: [Setting; 22] = [
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
/// assists and timer) at once.
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
                    hub.loaded = true;
                    if let Some(s) = orpheus_games::Scores::decode(data) {
                        hub.scores = s;
                    }
                    for g in Game::ALL {
                        hub.set_saved(g.id(), a.saved & (1 << g.id()) != 0);
                    }
                }
                true
            }
            CardOp::Read(g) => {
                if hub.opening != Some(g) {
                    return false;
                }
                hub.opening = None;
                hub.session = if a.ok {
                    Session::decode(data, Some(g)).unwrap_or(Session::None)
                } else {
                    Session::None
                };
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

/// Start a new game of `game` with the settings as they are now.
///
/// Solitaire with casino scoring: the game it replaces pays out its
/// foundation cards, the new deal is charged to the bank (so that a deal
/// cannot be refused for free), and the table is written.
pub fn new_game<S: SettingsService + GamesService + ?Sized>(svc: &mut S, game: Game) {
    let seed = svc.game_seed();
    let banked = settle_casino(svc);
    let session = match game {
        Game::Blocks => Session::Blocks(blocks::Blocks::new(blocks_config(svc), seed)),
        Game::Tiles => Session::Tiles(tiles::Tiles::new(tiles_config(svc), seed)),
        Game::Solitaire => Session::Solitaire(solitaire::Solitaire::new(solitaire_config(svc), seed)),
    };
    let charge =
        matches!(&session, Session::Solitaire(s) if s.config().scoring == solitaire::Scoring::Casino);
    if charge {
        svc.games_mut().scores.add_to_bank(gid::SOLITAIRE, -solitaire::CASINO_DEAL, true);
    }
    svc.games_mut().session = session;
    if banked || charge {
        svc.games_request(GamesRequest::SaveScores);
    }
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
    };
    svc.games().scores.get(game.id(), variant).map(|r| r.best).filter(|&b| b > 0)
}

/// Whether the best result of `game` under the current settings is a time.
pub fn best_is_time<S: SettingsService + ?Sized>(svc: &S, game: Game) -> bool {
    match game {
        Game::Blocks => blocks_config(svc).mode == blocks::Mode::Lines40,
        Game::Solitaire => solitaire_config(svc).scoring == solitaire::Scoring::None,
        Game::Tiles => false,
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
        assert_eq!(
            Game::Blocks.settings().len() + Game::Tiles.settings().len() + Game::Solitaire.settings().len(),
            SETTINGS.len()
        );
        assert!(Game::Solitaire.settings().iter().all(|s| s.id.0 >= 0x0E41));
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
}
