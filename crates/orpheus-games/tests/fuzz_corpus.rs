//! The fuzz targets' committed corpus and regression inputs, replayed on
//! stable through the bodies the cargo-fuzz targets run (VER-4); and the
//! seed journeys (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-games --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/{tiles,blocks}_keys/seed-*`).

#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use orpheus_games::{Button, Input};

#[test]
fn tiles_keys_corpus() {
    assert!(replay::replay("tiles_keys", bodies::tiles_keys) > 0, "no corpus");
}

#[test]
fn blocks_keys_corpus() {
    assert!(replay::replay("blocks_keys", bodies::blocks_keys) > 0, "no corpus");
}

fn press(b: Button) -> u8 {
    Input::Press(b).to_byte()
}

fn release(b: Button) -> u8 {
    Input::Release(b).to_byte()
}

fn long(b: Button) -> u8 {
    Input::Long(b).to_byte()
}

/// `n` logic steps as step bytes.
fn steps(n: u32) -> Vec<u8> {
    let mut v = Vec::new();
    let mut left = n;
    while left > 0 {
        let k = left.min(128);
        v.push(0x80 | (k - 1) as u8);
        left -= k;
    }
    v
}

fn tiles_seeds() -> Vec<(String, Vec<u8>)> {
    let dirs = [Button::Up, Button::Down, Button::Left, Button::Right];
    let mut out = Vec::new();
    for (name, cfg) in [
        ("4x4", [0u8, 1, 1, 1]),
        ("5x5-undo3", [1, 0, 2, 3]),
        ("6x6-endless", [2, 3, 3, 0]),
        ("4x4-goal2048-no4", [0, 0, 0, 2]),
    ] {
        let mut d = cfg.to_vec();
        d.extend_from_slice(&0x1234_5678_9ABC_DEF0u64.wrapping_mul(u64::from(cfg[0]) + 1).to_le_bytes());
        // A corner strategy with some undos: long enough to fill the board.
        for k in 0..600usize {
            let b = match k % 7 {
                0 | 2 => Button::Left,
                1 | 4 => Button::Down,
                3 => Button::Right,
                5 => dirs[k % 4],
                _ => Button::Up,
            };
            d.push(press(b));
            d.push(release(b));
            if k % 37 == 0 {
                d.push(press(Button::Ok));
            }
        }
        out.push((name.to_string(), d));
    }
    out
}

fn blocks_seeds() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (name, cfg) in [
        ("endless", [0u8, 0, 0, 1, 2, 2]),
        ("lines40-random", [1, 0, 1, 0, 0, 0]),
        ("attack-nohold-nokicks", [2, 4, 6, 3, 4, 4]),
        ("level15", [0, 14, 0, 1, 2, 1]),
    ] {
        let mut d = cfg.to_vec();
        d.extend_from_slice(&(u64::from(cfg[0]) * 7 + 3).to_le_bytes());
        // Per piece: turn, shift with auto-repeat, sometimes hold, soft or
        // hard drop.
        for k in 0..120u32 {
            for _ in 0..k % 4 {
                d.push(press(Button::Up));
                d.push(release(Button::Up));
            }
            let side = if k % 2 == 0 { Button::Left } else { Button::Right };
            d.push(press(side));
            d.extend(steps(4 + (k * 5) % 30));
            d.push(release(side));
            if k % 9 == 4 {
                d.push(press(Button::Menu));
                d.push(release(Button::Menu));
            }
            d.push(press(Button::Down));
            if k % 3 == 0 {
                d.extend(steps(40));
                d.push(release(Button::Down));
                d.extend(steps(40));
            } else {
                d.push(long(Button::Down));
                d.push(release(Button::Down));
                d.extend(steps(14));
            }
        }
        out.push((name.to_string(), d));
    }
    out
}

/// The seeds run through the bodies (and are written with
/// `ORPHEUS_WRITE_SEEDS=1`).
#[test]
fn seeds() {
    let t = tiles_seeds();
    let b = blocks_seeds();
    for (_, d) in &t {
        bodies::tiles_keys(d);
    }
    for (_, d) in &b {
        bodies::blocks_keys(d);
    }
    replay::write_seeds("tiles_keys", &t);
    replay::write_seeds("blocks_keys", &b);
}
