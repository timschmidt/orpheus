//! Blocks: property tests (random key and step sequences over many seeds
//! and every rule combination, and games of a greedy player, through the
//! fuzz body that asserts the invariants after every input and step and
//! checks lines and score against an independent reference), and
//! deterministic replays.

#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

use orpheus_games::blocks::{Blocks, CLEAR_STEPS, Phase, ROWS, W};
use orpheus_games::{Button, Input, Rng, SAVE_MAX, Session};

fn random_game(seed: u64, ops: usize) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let mut d: Vec<u8> = (0..14).map(|_| r.below(256) as u8).collect();
    for _ in 0..ops {
        if r.chance(400) {
            d.push(0x80 | r.below(48) as u8);
        } else {
            let b = Button::ALL[r.below(6) as usize];
            let i = match r.below(10) {
                0..=5 => Input::Press(b),
                6..=8 => Input::Release(b),
                _ => Input::Long(b),
            };
            d.push(i.to_byte());
        }
    }
    d
}

/// No overlap, never outside the well, no full row left, level follows
/// lines, lines and score equal the reference: 150 random games.
#[test]
fn invariants_hold_over_random_games() {
    let mut lines = 0;
    let mut ended = 0;
    for seed in 0..150 {
        let b = bodies::blocks_run(&random_game(seed, 1500));
        lines += b.lines();
        ended += u32::from(b.ended());
    }
    assert!(lines > 10, "random play clears some rows ({lines})");
    assert!(ended > 10, "and tops out sometimes ({ended})");
}

#[test]
fn every_configuration() {
    for mode in 0..3u8 {
        for flags in 0..8u8 {
            for timing in 0..5u8 {
                let mut d = random_game(u64::from(mode * 40 + flags * 5 + timing), 400);
                d[..6].copy_from_slice(&[mode, timing * 3, flags, timing, timing, timing]);
                bodies::blocks_run(&d);
            }
        }
    }
}

/// Turn `turns` times, shift `shift` cells, hard drop.
fn plan(turns: u8, shift: i8) -> Vec<Input> {
    let mut v = Vec::new();
    for _ in 0..turns {
        v.extend([Input::Press(Button::Up), Input::Release(Button::Up)]);
    }
    let side = if shift < 0 { Button::Left } else { Button::Right };
    for _ in 0..shift.unsigned_abs() {
        v.extend([Input::Press(side), Input::Release(side)]);
    }
    v.extend([Input::Press(Button::Down), Input::Long(Button::Down), Input::Release(Button::Down)]);
    v
}

/// Lower is better: holes, height, bumpiness; cleared lines count for.
fn evaluate(b: &Blocks) -> i32 {
    let mut heights = [0i32; W];
    let mut holes = 0;
    for (x, h) in heights.iter_mut().enumerate() {
        if let Some(t) = (0..ROWS).find(|&y| b.cell(x, y) != 0) {
            *h = (ROWS - t) as i32;
            holes += (t..ROWS).filter(|&y| b.cell(x, y) == 0).count() as i32;
        }
    }
    let bump: i32 = heights.windows(2).map(|w| (w[0] - w[1]).abs()).sum();
    let total: i32 = heights.iter().sum();
    holes * 40 + total * 3 + bump * 2 - b.lines() as i32 * 30
}

/// A greedy player, by inputs only (as the screen gives them): for each
/// piece it tries every turn and column on a copy, hard-drops, and keeps
/// the placement with the fewest holes and the lowest, flattest stack.
/// Plays `b` and returns the ops in the fuzz body's format.
fn greedy_ops(b: &mut Blocks, pieces: u32) -> Vec<u8> {
    let mut ops = Vec::new();
    let mut k = 0;
    while k < pieces && !b.ended() {
        if b.active().is_none() {
            b.step();
            ops.push(0x80);
            continue;
        }
        let mut best: Option<(i32, u8, i8)> = None;
        for turns in 0..4u8 {
            for shift in -5..=5i8 {
                let mut c = b.clone();
                for i in plan(turns, shift) {
                    c.input(i);
                }
                for _ in 0..CLEAR_STEPS {
                    c.step();
                }
                let v = evaluate(&c) + if c.ended() { 100_000 } else { 0 };
                if best.is_none_or(|(bv, _, _)| v < bv) {
                    best = Some((v, turns, shift));
                }
            }
        }
        let (_, turns, shift) = best.unwrap_or((0, 0, 0));
        for i in plan(turns, shift) {
            b.input(i);
            ops.push(i.to_byte());
        }
        // Two steps (one step byte: `0x81`).
        b.step();
        b.step();
        ops.push(0x81);
        k += 1;
    }
    ops
}

fn header(cfg: [u8; 6], seed: u64) -> Vec<u8> {
    let mut d = cfg.to_vec();
    d.extend_from_slice(&seed.to_le_bytes());
    d
}

/// A fixed game of the greedy player: the same seed and inputs always give
/// the same lines, score and stack (recorded 2026-11-25; a change here
/// changes saved games and replays). The same ops through the fuzz body
/// check every invariant and the reference counts on the way.
#[test]
fn deterministic_replay() {
    let cfg = [0, 0, 0, 1, 2, 2];
    let mut a = Blocks::new(bodies::blocks_config(&cfg), 0xB10C);
    let ops = greedy_ops(&mut a, 300);
    let mut d = header(cfg, 0xB10C);
    d.extend(&ops);
    let replayed = bodies::blocks_run(&d);
    assert_eq!(replayed, a, "the ops replay the game");
    assert_eq!((a.pieces(), a.lines(), a.score(), a.phase()), REPLAY_RESULT, "seed 0xB10C replay");
}

const REPLAY_RESULT: (u32, u32, u32, Phase) = (300, 118, 104_018, Phase::Playing);

/// Greedy games in every mode and several start levels through the fuzz
/// body: many cleared rows, combos and level changes against the
/// reference.
#[test]
fn greedy_games_match_the_reference() {
    let mut lines = 0;
    for (mode, level) in [(0u8, 0u8), (1, 4), (2, 9), (0, 14)] {
        for seed in 0..3u64 {
            let cfg = [mode, level, (seed as u8) & 1, 1, 2, 2];
            let mut g = Blocks::new(bodies::blocks_config(&cfg), seed);
            let ops = greedy_ops(&mut g, 150);
            let mut d = header(cfg, seed);
            d.extend(&ops);
            let b = bodies::blocks_run(&d);
            assert_eq!(b, g);
            lines += b.lines();
        }
    }
    assert!(lines > 300, "{lines}");
}

/// Saved and restored halfway, the game goes on exactly as without the
/// interruption.
#[test]
fn save_and_continue_is_seamless() {
    let keys = random_game(5, 3000);
    let whole = bodies::blocks_run(&keys);
    let cut = 14 + 1400;
    let half = bodies::blocks_run(&keys[..cut]);
    let mut buf = [0u8; SAVE_MAX];
    let n = Session::Blocks(half).encode(&mut buf).unwrap();
    assert!(n < SAVE_MAX);
    let Some(Session::Blocks(mut b)) = Session::decode(&buf[..n], None) else { panic!("decode") };
    for &byte in &keys[cut..] {
        if byte & 0x80 != 0 {
            for _ in 0..=(byte & 0x7F) {
                b.step();
            }
        } else if let Some(i) = Input::from_byte(byte) {
            b.input(i);
        }
    }
    assert_eq!(b, whole);
}
