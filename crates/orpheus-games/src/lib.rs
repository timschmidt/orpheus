//! orpheus-games: the logic of the Games app (05 §7.13, M14).
//!
//! `no_std`, no allocator, no hardware, no floating point. Every game is a
//! pure state machine that the app drives with [`Input`]s (key edges from
//! the key mapper, already turned into game buttons) and, for real-time
//! games, fixed logic steps at [`STEP_HZ`] counted by a [`StepClock`] from
//! the UI's ticks. Drawing is the app's (`orpheus-apps`); nothing here
//! knows the screen.
//!
//! * [`rng`]: the seedable generator every game draws from; a game started
//!   with the same seed and fed the same inputs at the same steps is the
//!   same game on the host, in the simulator and on the player.
//! * [`save`]: the binary format of saved games and the score table
//!   (magic, kind, version, length, payload, CRC-32).
//! * [`scores`]: best results and statistics per game and per variant
//!   (the combination of rule settings).
//! * [`tiles`]: 4096 (GAME-3), sliding number tiles on 4 × 4 … 6 × 6.
//! * [`blocks`]: Blocks (GAME-1), the falling-tetromino game.
//! * [`Session`] and [`Hub`]: the one game in RAM, the scores, and which
//!   games have a save file; what the app's `GamesService` keeps.
//!
//! Original implementations of the genres: own names, rules texts, tables
//! (gravity, scoring, kicks) and look.

#![no_std]
#![forbid(unsafe_code)]

pub mod blocks;
pub mod rng;
pub mod save;
pub mod scores;
pub mod tiles;

pub use rng::Rng;
pub use scores::{Outcome, Record, Scores};

use blocks::Blocks;
use tiles::Tiles;

/// Logic steps per second of the real-time games.
pub const STEP_HZ: u32 = 60;

/// Game ids: the save files' kind byte and the score table's game field.
/// Never reused.
pub mod id {
    pub const BLOCKS: u8 = 1;
    pub const TILES: u8 = 2;
    /// Reserved: Solitaire (GAME-2) and the artillery game (GAME-4/5).
    pub const SOLITAIRE: u8 = 3;
    pub const SALVO: u8 = 4;
    /// Bit per game in [`super::Hub::saved`]: ids are below 8.
    pub const MAX: u8 = 7;
}

/// The game buttons. The app maps the player's keys onto them (Blocks:
/// Prev/Next = Left/Right; 4096: the same four direction keys).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Button {
    Up = 0,
    Down,
    Left,
    Right,
    Ok,
    Menu,
}

impl Button {
    pub const ALL: [Button; 6] =
        [Button::Up, Button::Down, Button::Left, Button::Right, Button::Ok, Button::Menu];

    #[must_use]
    pub const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// One key edge, as the key mapper reports it: pressed, released, or held
/// past the hold threshold (500 ms, `orpheus_ui::input`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    Press(Button),
    Release(Button),
    Long(Button),
}

impl Input {
    /// The input of a replay or fuzz byte: bits 0–2 the button (6 and 7
    /// are none), bits 3–4 the edge (press, release, long; 3 is none).
    #[must_use]
    pub fn from_byte(b: u8) -> Option<Input> {
        let button = *Button::ALL.get(usize::from(b & 7))?;
        match (b >> 3) & 3 {
            0 => Some(Input::Press(button)),
            1 => Some(Input::Release(button)),
            2 => Some(Input::Long(button)),
            _ => None,
        }
    }

    /// Inverse of [`Input::from_byte`].
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Input::Press(b) => b as u8,
            Input::Release(b) => b as u8 | 1 << 3,
            Input::Long(b) => b as u8 | 2 << 3,
        }
    }
}

/// What [`StepClock::advance`] found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Advance {
    /// Run this many logic steps now.
    Steps(u32),
    /// More than [`MAX_GAP_MS`] passed since the last tick: the screen was
    /// off, covered or the UI was stalled. The game pauses (05 §7.13: any
    /// interruption pauses); no steps are owed.
    Gap,
}

/// A tick gap longer than this pauses the game.
pub const MAX_GAP_MS: u64 = 250;

/// Turns the UI's ticks (about every 33 ms, not exactly) into a whole
/// number of [`STEP_HZ`] logic steps, carrying the remainder exactly.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StepClock {
    last: Option<u64>,
    /// Milliseconds × `STEP_HZ` not yet turned into a step (< 1000).
    acc: u32,
}

impl StepClock {
    #[must_use]
    pub const fn new() -> StepClock {
        StepClock { last: None, acc: 0 }
    }

    /// Forget the time base: the next [`StepClock::advance`] starts a new
    /// one (after a pause).
    pub fn reset(&mut self) {
        *self = StepClock::new();
    }

    /// The UI ticked at `now_ms`.
    pub fn advance(&mut self, now_ms: u64) -> Advance {
        let Some(last) = self.last.replace(now_ms) else { return Advance::Steps(0) };
        let dt = now_ms.saturating_sub(last);
        if dt > MAX_GAP_MS {
            self.acc = 0;
            return Advance::Gap;
        }
        // dt <= 250, so this is far from overflowing.
        self.acc += u32::try_from(dt).unwrap_or(0) * STEP_HZ;
        let n = self.acc / 1000;
        self.acc %= 1000;
        Advance::Steps(n)
    }
}

/// Logic steps in `ms` milliseconds, rounded.
#[must_use]
pub const fn ms_to_steps(ms: u32) -> u32 {
    (ms * STEP_HZ + 500) / 1000
}

/// Milliseconds of `steps` logic steps, rounded.
#[must_use]
pub const fn steps_to_ms(steps: u32) -> u32 {
    ((steps as u64 * 1000 + STEP_HZ as u64 / 2) / STEP_HZ as u64) as u32
}

/// The game in RAM (one at a time; the others wait in their save files).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[allow(clippy::large_enum_variant)]
pub enum Session {
    #[default]
    None,
    Blocks(Blocks),
    Tiles(Tiles),
}

/// Largest encoded save file ([`Session::encode`]).
pub const SAVE_MAX: usize = 512;

impl Session {
    /// The game's id, if any.
    #[must_use]
    pub fn game(&self) -> Option<u8> {
        match self {
            Session::None => None,
            Session::Blocks(_) => Some(id::BLOCKS),
            Session::Tiles(_) => Some(id::TILES),
        }
    }

    /// A game that can be continued (not over).
    #[must_use]
    pub fn in_progress(&self) -> bool {
        match self {
            Session::None => false,
            Session::Blocks(b) => !b.ended(),
            Session::Tiles(t) => !t.over(),
        }
    }

    /// The save file: [`save`] envelope with the game's kind and version.
    /// `None` if `buf` is too small (use [`SAVE_MAX`]) or there is no game.
    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        match self {
            Session::None => None,
            Session::Blocks(b) => save::seal(buf, id::BLOCKS, blocks::VERSION, |w| b.encode(w)),
            Session::Tiles(t) => save::seal(buf, id::TILES, tiles::VERSION, |w| t.encode(w)),
        }
    }

    /// A save file back; `None` if damaged, of an unknown version, or of
    /// another game than `expect` (when given).
    #[must_use]
    pub fn decode(data: &[u8], expect: Option<u8>) -> Option<Session> {
        let (kind, version, payload) = save::open(data)?;
        if expect.is_some_and(|e| e != kind) {
            return None;
        }
        let mut r = save::Reader::new(payload);
        let s = match (kind, version) {
            (id::BLOCKS, blocks::VERSION) => Session::Blocks(Blocks::decode(&mut r)?),
            (id::TILES, tiles::VERSION) => Session::Tiles(Tiles::decode(&mut r)?),
            _ => return None,
        };
        r.done().then_some(s)
    }
}

/// What the app's `GamesService` keeps for the screens: the game in RAM,
/// the score table, which games have a save file, and the hand-over flags
/// between the game screen and its pause menu.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Hub {
    pub session: Session,
    pub scores: Scores,
    /// Bit `id` set: `/.orpheus/games/<game>.sav` exists.
    pub saved: u8,
    /// The scores and `saved` were read from the card.
    pub loaded: bool,
    /// Set by the pause menu's Resume and New game: the game screen comes
    /// back playing. A game screen rebuilt without it was covered by
    /// something else (an interruption) and comes back paused.
    pub resume: bool,
}

impl Hub {
    /// A save file for `game` exists.
    #[must_use]
    pub fn has_save(&self, game: u8) -> bool {
        game <= id::MAX && self.saved & (1 << game) != 0
    }

    /// The game `game` can be continued: in RAM and not over, or saved.
    #[must_use]
    pub fn can_continue(&self, game: u8) -> bool {
        (self.session.game() == Some(game) && self.session.in_progress()) || self.has_save(game)
    }

    pub fn set_saved(&mut self, game: u8, on: bool) {
        if game <= id::MAX {
            if on {
                self.saved |= 1 << game;
            } else {
                self.saved &= !(1 << game);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_bytes_round_trip() {
        for b in 0..=255u8 {
            if let Some(i) = Input::from_byte(b) {
                assert_eq!(Input::from_byte(i.to_byte()), Some(i));
            }
        }
        assert_eq!(Input::from_byte(0), Some(Input::Press(Button::Up)));
        assert_eq!(Input::from_byte(6), None);
        assert_eq!(Input::from_byte(3 << 3), None);
        assert_eq!(Input::from_byte(Input::Long(Button::Down).to_byte()), Some(Input::Long(Button::Down)));
    }

    #[test]
    fn step_clock_is_exact_over_uneven_ticks() {
        let mut c = StepClock::new();
        assert_eq!(c.advance(1000), Advance::Steps(0));
        let mut steps = 0;
        let mut t = 1000;
        // 30 ticks of 33 ms and one of 10: exactly 1 s.
        for i in 0..31 {
            t += if i == 30 { 10 } else { 33 };
            if let Advance::Steps(n) = c.advance(t) {
                steps += n;
            }
        }
        assert_eq!(steps, STEP_HZ);
        assert_eq!(c.advance(t + 251), Advance::Gap);
        assert_eq!(c.advance(t + 251 + 50), Advance::Steps(3));
        c.reset();
        assert_eq!(c.advance(5), Advance::Steps(0));
        assert_eq!(ms_to_steps(167), 10);
        assert_eq!(ms_to_steps(500), 30);
        assert_eq!(steps_to_ms(7200), 120_000);
        assert_eq!(steps_to_ms(1), 17);
    }

    /// RAM of the games on the player (the firmware keeps one `Hub` in a
    /// static): recorded 2026-11-25, raise only with a reason.
    #[test]
    fn sizes() {
        use core::mem::size_of;
        assert!(size_of::<Blocks>() <= 344, "Blocks {}", size_of::<Blocks>());
        assert!(size_of::<Tiles>() <= 248, "Tiles {}", size_of::<Tiles>());
        assert!(size_of::<Session>() <= 352, "Session {}", size_of::<Session>());
        assert!(size_of::<Scores>() <= 488, "Scores {}", size_of::<Scores>());
        assert!(size_of::<Hub>() <= 848, "Hub {}", size_of::<Hub>());
    }

    #[test]
    fn hub_saved_bits() {
        let mut h = Hub::default();
        assert!(!h.can_continue(id::TILES));
        h.set_saved(id::TILES, true);
        assert!(h.has_save(id::TILES) && h.can_continue(id::TILES) && !h.has_save(id::BLOCKS));
        h.set_saved(id::TILES, false);
        assert!(!h.has_save(id::TILES));
        h.set_saved(200, true);
        assert_eq!(h.saved, 0);
        h.session = Session::Tiles(Tiles::new(tiles::Config::DEFAULT, 1));
        assert!(h.can_continue(id::TILES));
    }

    #[test]
    fn session_round_trip_and_rejects() {
        let mut buf = [0u8; SAVE_MAX];
        assert_eq!(Session::None.encode(&mut buf), None);
        let s = Session::Blocks(Blocks::new(blocks::Config::DEFAULT, 99));
        let n = s.encode(&mut buf).unwrap();
        assert_eq!(Session::decode(&buf[..n], Some(id::BLOCKS)), Some(s.clone()));
        assert_eq!(Session::decode(&buf[..n], Some(id::TILES)), None, "another game's file");
        let mut bad = buf;
        bad[20] ^= 1;
        assert_eq!(Session::decode(&bad[..n], None), None, "CRC");
        assert_eq!(Session::decode(&buf[..n - 1], None), None, "truncated");
        let t = Session::Tiles(Tiles::new(tiles::Config::DEFAULT, 5));
        let n = t.encode(&mut buf).unwrap();
        assert_eq!(Session::decode(&buf[..n], None), Some(t));
    }
}
