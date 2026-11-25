//! The fuzz targets' committed corpus and regression inputs, replayed on
//! stable through the bodies the cargo-fuzz targets run (VER-4); and the
//! seed journeys (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-games --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/{tiles,blocks,solitaire,salvo,bricks}_keys/seed-*`;
//! Salvo and Bricks need `--features salvo,bricks`).

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
fn solitaire_keys_corpus() {
    assert!(replay::replay("solitaire_keys", bodies::solitaire_keys) > 0, "no corpus");
}

#[cfg(feature = "salvo")]
#[test]
fn salvo_keys_corpus() {
    assert!(replay::replay("salvo_keys", bodies::salvo_keys) > 0, "no corpus");
}

#[cfg(feature = "bricks")]
#[test]
fn bricks_keys_corpus() {
    assert!(replay::replay("bricks_keys", bodies::bricks_keys) > 0, "no corpus");
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

fn tap(d: &mut Vec<u8>, b: Button) {
    d.push(press(b));
    d.push(release(b));
}

/// Solitaire: a game won with the keys (the deal check's line, no
/// assists), and longer mixed sequences under other rules.
fn solitaire_seeds() -> Vec<(String, Vec<u8>)> {
    use orpheus_games::solitaire::{self, Board, Config, Move, PILES, STOCK, TABLEAU, is_tableau};
    let mut out = Vec::new();
    let walk = |d: &mut Vec<u8>, from: u8, to: u8| {
        let n = PILES as i32;
        let fwd = (i32::from(to) - i32::from(from)).rem_euclid(n);
        let (b, k) = if fwd <= n / 2 { (Button::Right, fwd) } else { (Button::Left, n - fwd) };
        (0..k).for_each(|_| tap(d, b));
    };
    let cfg = Config { auto_move: false, auto_complete: false, winnable: false, ..Config::DEFAULT };
    let seed =
        (0..100u64).find(|&s| solitaire::winnable(&Board::deal(s), &cfg, s)).expect("a deal the check wins");
    let mut line =
        [Move { from: 0, to: 0, n: 0, flipped: false, auto: false }; solitaire::CHECK_MOVES as usize];
    let n = solitaire::solution(&Board::deal(seed), &cfg, seed, &mut line).unwrap();
    let mut d = vec![0x60];
    d.extend_from_slice(&seed.to_le_bytes());
    let mut cur = TABLEAU;
    for m in &line[..n] {
        if m.from == STOCK || m.to == STOCK {
            walk(&mut d, cur, STOCK);
            cur = STOCK;
            tap(&mut d, Button::Ok);
            continue;
        }
        walk(&mut d, cur, m.from);
        if is_tableau(m.from) {
            (1..m.n).for_each(|_| tap(&mut d, Button::Up));
        }
        tap(&mut d, Button::Ok);
        walk(&mut d, m.from, m.to);
        tap(&mut d, Button::Ok);
        cur = m.to;
    }
    out.push(("won-draw1".to_string(), d));
    for (name, cfg) in
        [("draw3-assists", 0x01u8), ("casino-onepass", 0x0A), ("draw3-threepasses-checked", 0x85)]
    {
        let mut d = vec![cfg];
        d.extend_from_slice(&(u64::from(cfg) * 0x9E37 + 5).to_le_bytes());
        for k in 0..500u32 {
            let b = [Button::Right, Button::Ok, Button::Up, Button::Ok, Button::Left, Button::Down]
                [(k % 6) as usize];
            tap(&mut d, b);
            if k % 11 == 0 {
                tap(&mut d, Button::Menu);
            }
            if k % 23 == 0 {
                d.push(press(Button::Ok));
                d.push(long(Button::Ok));
                d.push(release(Button::Ok));
            }
            if k % 7 == 0 {
                d.extend(steps(20));
            }
        }
        out.push((name.to_string(), d));
    }
    out
}

/// Salvo journeys: a human aims and fires against computers of each
/// difficulty, in each terrain and gravity, with the weapon list, the
/// overview, fast flights, a pass screen and the shop between rounds.
#[cfg(feature = "salvo")]
fn salvo_seeds() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    // players | kinds 1-2 | kinds 3-4 | rounds | gravity | wind | drag |
    // style/rough/relief | seed/width/health | flags
    let cfgs: [(&str, [u8; 10]); 6] = [
        ("duel-easy-hills", [0, 2 << 3, 2 | 2 << 3, 0, 2, 1, 1, 1 << 3 | 1 << 5, 2 << 2 | 1 << 4, 0]),
        ("three-mountains-gusts", [1, 3 << 3, 1 | 4 << 3, 1, 2, 2, 1, 1 | 2 << 3 | 2 << 5, 3 << 2, 1]),
        ("four-islands-moon", [2, 4 << 3, 2 | 3 << 3, 1, 0, 1, 2, 3 | 1 << 3, 1 << 2 | 2 << 4, 4]),
        ("two-humans-canyon-shop", [0, 0, 0, 1, 3, 3, 0, 2 | 2 << 5, 0, 16]),
        ("computers-plateaus", [2, 1 | 4 << 3, 2 | 3 << 3, 2, 1, 2, 1, 4 | 1 << 3, 3 << 2, 2]),
        ("mixed-fixed-seed-jupiter", [0, 4 << 3, 0, 0, 3, 3, 2, 5 | 2 << 3, 1 | 1 << 2, 1 | 8]),
    ];
    let keys = [
        press(Button::Up),
        release(Button::Up),
        press(Button::Right),
        release(Button::Right),
        press(Button::Menu),
        press(Button::Down),
        press(Button::Ok),
        press(Button::Ok),
        release(Button::Ok),
    ];
    for (k, (name, cfg)) in cfgs.iter().enumerate() {
        let mut d = cfg.to_vec();
        d.extend((0x5A1_7000u64 + k as u64).to_le_bytes());
        for r in 0..40 {
            d.extend(&keys[..(r % keys.len()) + 1]);
            d.extend(steps(if r % 3 == 0 { 400 } else { 120 }));
            if r % 7 == 0 {
                d.push(press(Button::Ok));
                d.push(long(Button::Ok));
                d.push(release(Button::Ok));
            }
        }
        out.push((name.to_string(), d));
    }
    out
}

/// Bricks journeys: the autopilot under the ball in each level set, speed
/// and paddle size, with power-ups on and off; and two played with keys
/// only (the paddle swept, OK pressed).
#[cfg(feature = "bricks")]
fn bricks_seeds() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    // start level | speed, paddle, paddle speed | lives, power-ups off,
    // level set, autopilot (bit 7)
    let cfgs: [(&str, [u8; 3]); 8] = [
        ("auto-mixed-normal", [0, 1 | 1 << 2 | 1 << 4, 1 | 2 << 3 | 0x80]),
        ("auto-handmade-fast-small", [3, 2 | 2 << 4, 2 | 0x80]),
        ("auto-generated-slow-large", [8, 2 << 2, 1 | 1 << 3 | 0x80]),
        ("auto-no-powerups-level9", [8, 1 | 1 << 2, 1 | 4 | 2 << 3 | 0x80]),
        ("auto-one-life-fast", [5, 2 | 1 << 2 | 2 << 4, 0x80]),
        ("auto-generated-level10", [9, 1, 2 | 1 << 3 | 0x80]),
        ("keys-sweep", [0, 1 | 1 << 2, 1 | 2 << 3]),
        ("keys-down-up", [2, 2 | 2 << 4, 2 | 1 << 3]),
    ];
    for (k, (name, cfg)) in cfgs.iter().enumerate() {
        let mut d = cfg.to_vec();
        d.extend((0x0B21_C000u64 + k as u64).to_le_bytes());
        if cfg[2] & 0x80 != 0 {
            for r in 0..400u32 {
                d.extend(steps(20 + r % 50));
                if r % 13 == 0 {
                    tap(&mut d, Button::Ok);
                }
            }
        } else {
            let (l, r) = if name.contains("down") {
                (Button::Down, Button::Up)
            } else {
                (Button::Left, Button::Right)
            };
            for i in 0..300u32 {
                let b = if i % 2 == 0 { l } else { r };
                d.push(press(b));
                d.extend(steps(10 + (i * 7) % 40));
                d.push(release(b));
                if i % 5 == 0 {
                    tap(&mut d, Button::Ok);
                }
                d.extend(steps(i % 30 + 1));
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
    #[cfg(feature = "bricks")]
    {
        let v = bricks_seeds();
        for (_, d) in &v {
            bodies::bricks_keys(d);
        }
        replay::write_seeds("bricks_keys", &v);
    }
    #[cfg(feature = "salvo")]
    {
        let v = salvo_seeds();
        for (_, d) in &v {
            bodies::salvo_keys(d);
        }
        replay::write_seeds("salvo_keys", &v);
    }
    let t = tiles_seeds();
    let b = blocks_seeds();
    let s = solitaire_seeds();
    for (_, d) in &s {
        bodies::solitaire_keys(d);
    }
    for (_, d) in &t {
        bodies::tiles_keys(d);
    }
    for (_, d) in &b {
        bodies::blocks_keys(d);
    }
    replay::write_seeds("tiles_keys", &t);
    replay::write_seeds("blocks_keys", &b);
    replay::write_seeds("solitaire_keys", &s);
}
