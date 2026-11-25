//! 4096: property tests (random key sequences over many seeds and every
//! rule combination, through the fuzz body that asserts the invariants of
//! 05 §7.13) and deterministic replays.

#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

use orpheus_games::tiles::{Config, Dir, Phase, Tiles, TilesOut};
use orpheus_games::{Button, Input, Rng, SAVE_MAX, Session};

fn random_game(seed: u64, keys: usize) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let mut d: Vec<u8> = (0..12).map(|_| r.below(256) as u8).collect();
    let buttons = [Button::Up, Button::Down, Button::Left, Button::Right, Button::Ok, Button::Menu];
    for _ in 0..keys {
        // Mostly slides; now and then OK (undo / keep going) or junk.
        let n = if r.chance(100) { 6 } else { 4 };
        let b = buttons[r.below(n) as usize];
        d.push(Input::Press(b).to_byte());
        if r.chance(300) {
            d.push(Input::Release(b).to_byte());
        }
        if r.chance(10) {
            d.push(r.below(256) as u8);
        }
    }
    d
}

/// Sum grows only by spawns, one merge per tile, over exactly when stuck,
/// slides equal the reference, undo restores: 300 games.
#[test]
fn invariants_hold_over_random_games() {
    let mut overs = 0;
    for seed in 0..300 {
        let t = bodies::tiles_run(&random_game(seed, 700));
        overs += usize::from(t.over());
    }
    assert!(overs > 30, "some games must reach game over ({overs})");
}

/// Every rule combination plays.
#[test]
fn every_configuration() {
    for size in 0..3u8 {
        for goal in 0..4u8 {
            for four in 0..4u8 {
                for undo in 0..4u8 {
                    let mut d = random_game(u64::from(size * 64 + goal * 16 + four * 4 + undo), 200);
                    d[..4].copy_from_slice(&[size, goal, four, undo]);
                    let t = bodies::tiles_run(&d);
                    assert_eq!(t.n(), usize::from(4 + size));
                }
            }
        }
    }
}

/// A fixed game: the same seed and keys always end in the same place
/// (numbers recorded 2026-11-25; a change here changes saved games and
/// replays).
#[test]
fn deterministic_replay() {
    let mut t = Tiles::new(Config::DEFAULT, 0x4096);
    let order = [Dir::Left, Dir::Down, Dir::Right, Dir::Down];
    let mut k = 0;
    while !t.over() && k < 5000 {
        if t.slide(order[k % 4]).is_none() {
            // Stuck in the strategy's directions: try Up once.
            t.slide(Dir::Up);
        }
        k += 1;
    }
    let first = (t.score(), t.moves(), t.best_tile(), t.cells().to_vec());
    let mut again = Tiles::new(Config::DEFAULT, 0x4096);
    let mut k = 0;
    while !again.over() && k < 5000 {
        if again.slide(order[k % 4]).is_none() {
            again.slide(Dir::Up);
        }
        k += 1;
    }
    assert_eq!(first, (again.score(), again.moves(), again.best_tile(), again.cells().to_vec()));
    assert!(t.over());
    assert_eq!((t.score(), t.moves(), t.best_tile()), REPLAY_RESULT, "seed 0x4096 replay");
}

/// Score, moves and largest tile of [`deterministic_replay`].
const REPLAY_RESULT: (u32, u32, u8) = (2436, 211, 7);

/// A game saved and restored halfway goes on exactly as if it had not
/// been interrupted.
#[test]
fn save_and_continue_is_seamless() {
    let keys = random_game(77, 600);
    let whole = bodies::tiles_run(&keys);
    let half = bodies::tiles_run(&keys[..12 + 300]);
    let mut buf = [0u8; SAVE_MAX];
    let n = Session::Tiles(half).encode(&mut buf).unwrap();
    let Some(Session::Tiles(mut t)) = Session::decode(&buf[..n], None) else { panic!("decode") };
    for &b in &keys[12 + 300..] {
        if let Some(i) = Input::from_byte(b) {
            t.input(i);
        }
    }
    assert_eq!(t, whole);
}

#[test]
fn goal_then_keep_going_then_undo_brings_the_banner_back() {
    let cfg = Config { goal: 3, undo: 2, ..Config::DEFAULT };
    let mut t = Tiles::from_cells(cfg, 9, &[2, 2, 0, 0]);
    assert!(matches!(t.input(Input::Press(Button::Left)), TilesOut::Moved(_)));
    assert_eq!(t.phase(), Phase::Won);
    // Sliding on keeps going; undo returns to the goal moment.
    let moved = t.input(Input::Press(Button::Right));
    assert!(matches!(moved, TilesOut::Moved(_)));
    assert_eq!(t.phase(), Phase::Playing);
    assert_eq!(t.input(Input::Press(Button::Ok)), TilesOut::Undone);
    assert_eq!(t.phase(), Phase::Won);
}
