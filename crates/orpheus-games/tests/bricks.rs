//! Bricks (GAME-6): the invariants over many random games, no tunnelling
//! at the highest speed, the paddle's angles, power-ups, lives and levels,
//! saves and a pinned replay.

#![cfg(feature = "bricks")]

use orpheus_games::bricks::{
    self, ANGLES, BALL, BASE_SPEEDS, BRICK_H, BRICKS_X, BRICKS_Y, Bricks, CATCH_STEPS, CELLS, CLEAR_POINTS,
    CLEAR_STEPS, COLS, COS, Config, EMPTY, FIELD_H, FIELD_W, HOLD_STEPS, LevelSet, MAX_LIVES, MAX_SPEED, ONE,
    PADDLE_H, PADDLE_WIDTHS, PADDLE_Y, Phase, Power, ROWS, SIN, SOLID, SPEEDUP_STEPS, WIDE_STEP, brick,
};
use orpheus_games::{Button, Input, Rng, SAVE_MAX, Session, id};

#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

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

/// The sine table against `f64`.
#[test]
fn angle_table() {
    for i in 0..ANGLES.len() {
        let a = f64::from(ANGLES[i]).to_radians();
        assert!((f64::from(SIN[i]) - a.sin() * 4096.0).abs() <= 0.5, "sin {}", ANGLES[i]);
        assert!((f64::from(COS[i]) - a.cos() * 4096.0).abs() <= 0.5, "cos {}", ANGLES[i]);
    }
}

/// Many random games (random settings, keys and steps, half of them with
/// the autopilot under the ball): after every input and step the ball is
/// in no wall, brick or paddle, bricks only lose hits within a level, the
/// score is the bricks broken and levels cleared, saves read back, and the
/// game is deterministic (`bodies::bricks_run`).
#[test]
fn random_games_keep_the_invariants() {
    let mut r = Rng::new(0x000B_21C4);
    let (mut levels, mut broken, mut over, mut caught) = (0u32, 0u32, 0u32, 0u32);
    for game in 0..120 {
        let mut d: Vec<u8> = (0..11).map(|_| r.next_u32() as u8).collect();
        if game % 2 == 0 {
            d[2] |= 0x80;
        }
        for _ in 0..400 {
            let b = match r.below(12) {
                0..=4 => 0x80 | r.below(64) as u8,
                5 => Input::Press(Button::Ok).to_byte(),
                _ => r.below(48) as u8,
            };
            d.push(b);
        }
        let g = bodies::bricks_run(&d);
        levels += u32::from(g.cleared());
        broken += g.broken().iter().sum::<u32>();
        over += u32::from(g.ended());
        caught += u32::from(g.lives() > bodies::bricks_config(&d).lives);
    }
    eprintln!(
        "120 games: {levels} levels cleared, {broken} bricks broken, {over} over, {caught} with lives gained"
    );
    assert!(levels > 10 && broken > 2000 && over > 10, "the games went somewhere");
}

/// A row of one-hit bricks right across the grid; a ball under it at the
/// highest speed in every direction, from many places (sub-pixel ones
/// too, none that reach a side wall first): it never gets above the row's
/// bottom without breaking a brick.
#[test]
fn no_tunnelling_through_bricks_at_max_speed() {
    let row = ROWS - 1;
    let mut cells = [EMPTY; CELLS];
    for c in 0..COLS {
        cells[row * COLS + c] = brick(1);
    }
    let bottom = (BRICKS_Y + (row as i32 + 1) * BRICK_H) * ONE;
    let mut tries = 0;
    for ang in 0..7u8 {
        for left in [false, true] {
            for x in 36..=89 {
                for sub in [0, 37, 128, 255] {
                    let mut g = Bricks::with_cells(Config::DEFAULT, 1, &cells);
                    g.set_paddle(0);
                    assert!(g.place_ball(0, x * ONE + sub, 80 * ONE + sub, ang, left, true));
                    g.set_speed(MAX_SPEED);
                    let mut broke = false;
                    for _ in 0..20 {
                        let out = g.step();
                        g.check().unwrap();
                        let b = g.balls()[0];
                        if out.broke > 0 {
                            broke = true;
                            break;
                        }
                        assert_eq!(b.left(), left, "ang {ang} x {x}.{sub}: no side wall on the way {b:?}");
                        assert!(
                            b.y >= bottom,
                            "ang {ang} left {left} x {x}.{sub}: through the row at {}",
                            b.y
                        );
                    }
                    assert!(broke);
                    tries += 1;
                }
            }
        }
    }
    assert_eq!(tries, 7 * 2 * 54 * 4);
}

/// Random balls at the highest speed in a dense field: a step that turned
/// no ball moved it along a straight line, and no brick that stood before
/// and after the step is on that line (checked every 1/8 px, the brick
/// less the pixel the sub-steps' staircase may stray from the line).
#[test]
fn swept_path_never_crosses_a_brick() {
    let mut r = Rng::new(77);
    let mut checked = 0u32;
    for game in 0..400u64 {
        let mut g =
            Bricks::new(Config { set: LevelSet::Generated, start_level: 10, ..Config::DEFAULT }, game);
        g.set_paddle(r.below(100) as i32 * ONE);
        let x = r.below(((FIELD_W - BALL) * ONE) as u32) as i32;
        let y = r.below((100 * ONE) as u32) as i32;
        if !g.place_ball(0, x, y, r.below(7) as u8, r.below(2) == 0, r.below(2) == 0) {
            continue;
        }
        g.set_speed(MAX_SPEED);
        for _ in 0..300 {
            let before = g.clone();
            let b0 = before.balls()[0];
            let out = g.step();
            g.check().unwrap();
            let b1 = g.balls()[0];
            if !b1.live() || g.phase() != Phase::Play {
                break;
            }
            let turned = (b0.left(), b0.up(), b0.ang) != (b1.left(), b1.up(), b1.ang);
            if turned || out.broke + out.hits > 0 {
                continue;
            }
            let (dx, dy) = (b1.x - b0.x, b1.y - b0.y);
            let n = (dx.abs().max(dy.abs()) / 32).max(1);
            for k in 0..=n {
                let (x, y) = (b0.x + dx * k / n, b0.y + dy * k / n);
                for (i, (&c0, &c1)) in before.cells().iter().zip(g.cells()).enumerate() {
                    if c0 == EMPTY || c0 != c1 {
                        continue;
                    }
                    let (row, col) = ((i / COLS) as i32, (i % COLS) as i32);
                    let (bx, by) = ((BRICKS_X + col * 10) * ONE, (BRICKS_Y + row * BRICK_H) * ONE);
                    // The ball moves x then y in sub-steps of at most a
                    // pixel, so its real path is a staircase within a pixel
                    // of the line: the brick is shrunk by that much (a ball
                    // through it would still cross its 3 × 8 px core).
                    let overlap = x < bx + 9 * ONE
                        && x + BALL * ONE > bx + ONE
                        && y < by + (BRICK_H - 1) * ONE
                        && y + BALL * ONE > by + ONE;
                    assert!(
                        !overlap,
                        "game {game}: the path ({},{})->({},{}) crosses cell {i}",
                        b0.x, b0.y, b1.x, b1.y
                    );
                }
            }
            checked += 1;
        }
    }
    assert!(checked > 10_000, "{checked} straight steps");
}

/// A ball falling onto the paddle at the highest speed always comes back
/// up (never through it), wherever it meets it.
#[test]
fn no_tunnelling_through_the_paddle_at_max_speed() {
    for ang in 0..3u8 {
        for left in [false, true] {
            for off in 6..=13 {
                for sub in [0, 100, 255] {
                    let mut g = Bricks::new(Config::DEFAULT, 3);
                    let (px, w) = g.paddle();
                    assert_eq!(w, 22);
                    assert!(g.place_ball(0, px + off * ONE + sub, 95 * ONE + sub, ang, left, false));
                    g.set_speed(MAX_SPEED);
                    let mut up = false;
                    for _ in 0..30 {
                        g.step();
                        g.check().unwrap();
                        let b = g.balls()[0];
                        assert!(b.live(), "ang {ang} off {off}.{sub}: lost");
                        assert!(b.y + BALL * ONE <= (PADDLE_Y + PADDLE_H) * ONE, "below the paddle's top");
                        if b.up() {
                            up = true;
                            break;
                        }
                    }
                    assert!(up, "ang {ang} off {off}.{sub}: never bounced");
                }
            }
        }
    }
}

/// Walls at the highest speed: the ball never leaves the field sideways
/// or upwards, from any start, and bounces off unbreakable bricks.
#[test]
fn walls_and_solid_bricks_at_max_speed() {
    let mut cells = [EMPTY; CELLS];
    for c in (0..COLS).step_by(2) {
        cells[3 * COLS + c] = SOLID;
        cells[5 * COLS + c + 1] = SOLID;
    }
    cells[0] = brick(3);
    let mut r = Rng::new(9);
    for _ in 0..400 {
        let mut g = Bricks::with_cells(Config::DEFAULT, 2, &cells);
        let x = r.below(((FIELD_W - BALL) * ONE) as u32) as i32;
        let y = r.below((90 * ONE) as u32) as i32;
        if !g.place_ball(0, x, y, r.below(7) as u8, r.below(2) == 0, r.below(2) == 0) {
            continue;
        }
        g.set_speed(MAX_SPEED);
        for _ in 0..200 {
            g.step();
            g.check().unwrap();
        }
    }
}

/// The paddle turns the ball by where it meets it: centre nearly straight
/// up, the ends at 60°, each side to its own side.
#[test]
fn paddle_angle_by_contact() {
    let land = |off: i32| {
        let mut g = Bricks::new(Config::DEFAULT, 1);
        let (px, w) = g.paddle();
        // Straight down onto the paddle: ball centre `off` px from its centre.
        let x = px + i32::from(w) * ONE / 2 + off * ONE - BALL * ONE / 2;
        assert!(g.place_ball(0, x, (PADDLE_Y - BALL - 1) * ONE, 0, false, false));
        for _ in 0..3 {
            g.step();
        }
        let b = g.balls()[0];
        assert!(b.up());
        (b.ang, b.left())
    };
    assert_eq!(land(0).0, 0);
    let (a, l) = land(-12);
    assert_eq!((a, l), (6, true), "the left end");
    let (a, l) = land(12);
    assert_eq!((a, l), (6, false), "the right end");
    let (a, _) = land(5);
    assert!((2..=4).contains(&a), "half way: {a}");
}

/// The paddle stops at a ball beside it (it never pushes into one).
#[test]
fn paddle_stops_at_a_ball_beside_it() {
    let mut g = Bricks::new(Config::DEFAULT, 1);
    let (px, w) = g.paddle();
    let right = px + i32::from(w) * ONE;
    // A ball falling past the paddle's right end, 2 px away.
    assert!(g.place_ball(0, right + 2 * ONE, PADDLE_Y * ONE, 0, false, false));
    g.set_speed(ONE);
    g.input(Input::Press(Button::Right));
    g.step();
    g.check().unwrap();
    let (nx, _) = g.paddle();
    assert!(nx + i32::from(w) * ONE <= g.balls()[0].x, "the paddle went into the ball");
}

/// Serve: the ball rides on the paddle until OK; Prev/Next and Down/Up
/// move the paddle, faster while held; the last key pressed wins.
#[test]
fn serve_and_paddle_keys() {
    let mut g = Bricks::new(Config::DEFAULT, 1);
    assert_eq!(g.phase(), Phase::Serve);
    let (x0, _) = g.paddle();
    let b0 = g.balls()[0].x;
    g.input(Input::Press(Button::Right));
    g.step();
    let (x1, _) = g.paddle();
    for _ in 0..20 {
        g.step();
    }
    let (x2, _) = g.paddle();
    assert!(x1 > x0 && x2 - x1 > 20 * (x1 - x0), "accelerates: {x0} {x1} {x2}");
    assert_eq!(g.balls()[0].x - b0, x2 - x0, "the ball rides along");
    g.input(Input::Press(Button::Down));
    g.step();
    assert!(g.paddle().0 < x2, "Down moves left and wins over Next");
    g.input(Input::Release(Button::Down));
    g.step();
    assert!(g.paddle().0 >= x2 - 2 * ONE, "Next held again");
    g.release_all();
    g.input(Input::Press(Button::Ok));
    assert_eq!(g.phase(), Phase::Play);
    assert!(!g.balls()[0].stuck() && g.balls()[0].up());
    // The paddle speed setting.
    let run = |sens: u8| {
        let mut g = Bricks::new(Config { sens, ..Config::DEFAULT }, 1);
        g.input(Input::Press(Button::Left));
        for _ in 0..8 {
            g.step();
        }
        g.paddle().0
    };
    assert!(run(0) > run(1) && run(1) > run(2));
}

/// Losing every ball costs a life and serves again; the last one ends
/// the game.
#[test]
fn lives_and_game_over() {
    let mut g = Bricks::new(Config { lives: 3, ..Config::DEFAULT }, 1);
    for life in (0..3).rev() {
        assert!(g.place_ball(0, 2 * ONE, 100 * ONE, 0, false, false));
        let mut out = Default::default();
        for _ in 0..40 {
            out = g.step();
            if out.lost {
                break;
            }
        }
        assert!(out.lost);
        assert_eq!(g.lives(), life);
        assert_eq!(out.ended, life == 0);
        assert_eq!(g.phase(), if life == 0 { Phase::Over } else { Phase::Serve });
        g.check().unwrap();
    }
    assert!(g.ended());
    assert_eq!(g.step(), Default::default(), "nothing moves after the end");
}

/// A cleared level scores 100 and the next one comes after the banner.
#[test]
fn levels_clear_and_follow() {
    let mut g = Bricks::new(Config { set: LevelSet::Mixed, ..Config::DEFAULT }, 5);
    assert_eq!(g.cells(), &{
        let mut c = bricks::layout(LevelSet::Mixed, 1, 5);
        for (x, y) in c.iter_mut().zip(g.cells()) {
            *x |= y & bricks::CARRY;
        }
        c
    });
    g.input(Input::Press(Button::Ok));
    g.clear_now();
    assert_eq!(g.phase(), Phase::Cleared);
    assert_eq!(g.score(), CLEAR_POINTS);
    for _ in 0..CLEAR_STEPS {
        g.check().unwrap();
        g.step();
    }
    assert_eq!((g.level(), g.phase()), (2, Phase::Serve));
    // Mixed: generated after the eighth.
    g.goto_level(9);
    let cells: Vec<u8> = g.cells().iter().map(|c| c & !bricks::CARRY).collect();
    assert_eq!(cells, bricks::layout(LevelSet::Generated, 9, 5).to_vec());
    assert_ne!(bricks::layout(LevelSet::Generated, 9, 5), bricks::layout(LevelSet::Generated, 9, 6));
    // Without power-ups no brick carries one.
    let g = Bricks::new(Config { powerups: false, ..Config::DEFAULT }, 5);
    assert!(g.cells().iter().all(|c| c & bricks::CARRY == 0));
}

/// Breaking bricks: one point step per hit the brick took when new, multi-
/// hit bricks lose a hit at a time, unbreakable ones stay.
#[test]
fn brick_hits_and_points() {
    let mut cells = [EMPTY; CELLS];
    cells[9 * COLS + 5] = brick(3);
    cells[9 * COLS + 6] = SOLID;
    cells[COLS] = brick(1);
    let mut g = Bricks::with_cells(Config { powerups: false, ..Config::DEFAULT }, 1, &cells);
    // Straight up under the three-hit brick, three times.
    let x = (BRICKS_X + 5 * 10 + 3) * ONE;
    for hits in [2, 1, 0] {
        assert!(g.place_ball(0, x, 80 * ONE, 0, false, true));
        for _ in 0..40 {
            if g.step().hits + g.step().broke > 0 {
                break;
            }
        }
        assert_eq!(bricks::hits_left(g.cell(9, 5)), hits);
    }
    assert_eq!(g.score(), 30);
    assert_eq!(g.broken(), [0, 0, 1]);
    assert_eq!(g.cell(9, 6), SOLID);
}

/// The ball speeds up gently and stops at the cap; Slow takes it down.
#[test]
fn speed_up_and_slow() {
    let mut g = Bricks::new(Config::DEFAULT, 1);
    let base = BASE_SPEEDS[1];
    assert_eq!(g.speed(), base);
    g.input(Input::Press(Button::Ok));
    let mut seen = vec![g.speed()];
    for _ in 0..(u32::from(SPEEDUP_STEPS) * 20) {
        // Keep a ball in play high up.
        if g.phase() != Phase::Play || g.balls()[0].y > 90 * ONE {
            let _ = g.place_ball(0, 60 * ONE, 70 * ONE, 3, false, true);
        }
        g.step();
        if *seen.last().unwrap() != g.speed() {
            seen.push(g.speed());
        }
        if g.phase() == Phase::Cleared {
            break;
        }
    }
    let ups: Vec<i32> = seen.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(ups[..ups.len() - 1].iter().all(|&u| u == base / 24), "{seen:?}");
    assert_eq!(*seen.last().unwrap(), base * 3 / 2, "capped at 1.5×: {seen:?}");
    g.drop_capsule(Power::Slow, (g.paddle().0 / ONE + 4) as u8, PADDLE_Y - 6);
    let before = g.speed();
    for _ in 0..20 {
        g.step();
    }
    assert_eq!(g.speed(), before * 3 / 4);
}

/// Every power-up, caught on the paddle.
#[test]
fn power_ups() {
    let catch = |p: Power| {
        let mut g = Bricks::new(Config::DEFAULT, 1);
        g.input(Input::Press(Button::Ok));
        assert!(g.place_ball(0, 60 * ONE, 60 * ONE, 3, false, true));
        let x = (g.paddle().0 / ONE + 4) as u8;
        g.drop_capsule(p, x, PADDLE_Y - 8);
        let mut got = None;
        for _ in 0..20 {
            let out = g.step();
            g.check().unwrap();
            got = got.or(out.caught);
        }
        assert_eq!(got, Some(p));
        g
    };
    let g = catch(Power::Wide);
    assert_eq!(g.paddle().1, PADDLE_WIDTHS[1] + WIDE_STEP, "a pixel a step");
    let g = catch(Power::Split);
    assert_eq!(g.balls().iter().filter(|b| b.live()).count(), 3);
    let g = catch(Power::Life);
    assert_eq!(g.lives(), 4);
    let mut g = catch(Power::Catch);
    assert!(g.catching() > CATCH_STEPS - 30);
    // The next ball the paddle meets stays on it until OK or the wait.
    let (px, w) = g.paddle();
    assert!(g.place_ball(0, px + i32::from(w) / 2 * ONE, 95 * ONE, 0, false, false));
    let mut held = false;
    for _ in 0..30 {
        g.step();
        held |= g.balls()[0].stuck();
    }
    assert!(held && g.balls()[0].stuck());
    for _ in 0..HOLD_STEPS {
        g.step();
    }
    assert!(!g.balls()[0].stuck() && g.balls()[0].up(), "let go after the wait");
    // Lives stop at nine.
    let mut g = Bricks::new(Config { lives: MAX_LIVES, ..Config::DEFAULT }, 1);
    g.input(Input::Press(Button::Ok));
    g.drop_capsule(Power::Life, (g.paddle().0 / ONE + 4) as u8, PADDLE_Y - 8);
    for _ in 0..20 {
        g.step();
    }
    assert_eq!(g.lives(), MAX_LIVES);
}

/// A game in play (several balls, capsules, the catch running) saved and
/// read back is the same game; damaged files are refused.
#[test]
fn save_round_trip() {
    let mut d = vec![3u8, 0x15, 0x80 | 8];
    d.extend(0x000B_21C5_u64.to_le_bytes());
    d.extend(steps(3000));
    let mut g = bodies::bricks_run(&d);
    if g.ended() {
        g = Bricks::new(Config::DEFAULT, 4);
        g.input(Input::Press(Button::Ok));
    }
    g.drop_capsule(Power::Split, 10, 60);
    g.drop_capsule(Power::Catch, 50, 70);
    let s = Session::Bricks(g.clone());
    let mut buf = [0u8; SAVE_MAX];
    let n = s.encode(&mut buf).unwrap();
    eprintln!("a Bricks save: {n} bytes");
    assert!(n <= 300, "{n} bytes");
    assert_eq!(Session::decode(&buf[..n], Some(id::BRICKS)), Some(s));
    assert_eq!(Session::decode(&buf[..n], Some(id::BLOCKS)), None);
    let mut bad = buf;
    bad[40] ^= 4;
    assert_eq!(Session::decode(&bad[..n], None), None, "CRC");
    assert_eq!(Session::decode(&buf[..n - 1], None), None, "truncated");
    assert!(Session::Bricks(g).in_progress());
}

/// The same seed and keys give the same game, pinned by a checksum of
/// its saves along the way (the autopilot plays several levels). The
/// checksum leaves out each save's own CRC: a CRC-32 over data and its
/// CRC is the same constant (0x2144df1c) for every file.
#[test]
fn deterministic_and_pinned() {
    const PINNED: u32 = 0x8a1f_d172;
    let cfg = Config { speed: 2, ..Config::DEFAULT };
    let run = || {
        let mut g = Bricks::new(cfg, 0x000B_21C6);
        let mut dir = 0i8;
        let mut crc = 0u32;
        let mut buf = [0u8; SAVE_MAX];
        for k in 0..12_000usize {
            for i in bodies::bricks_autopilot(&g, &mut dir, (k / 500 % 9) as i32 - 4, k) {
                g.input(i);
            }
            g.step();
            if k % 100 == 0 {
                let n = Session::Bricks(g.clone()).encode(&mut buf).unwrap();
                crc = orpheus_games::save::crc32(&buf[..n - 4]) ^ crc.rotate_left(5);
            }
        }
        (g, crc)
    };
    let (a, ca) = run();
    let (b, cb) = run();
    assert_eq!((a.clone(), ca), (b, cb));
    eprintln!(
        "pinned checksum {ca:#010x}: level {}, score {}, cleared {}, lives {}",
        a.level(),
        a.score(),
        a.cleared(),
        a.lives()
    );
    assert!(a.cleared() >= 1, "the autopilot cleared a level");
    assert_eq!(ca, PINNED, "the game changed: if on purpose, pin {ca:#010x}");
}

/// The engine's speed on the host (release): a step of play with three
/// balls, printed for docs/STATUS.md.
#[test]
fn step_cost() {
    let mut g = Bricks::new(Config { speed: 2, ..Config::DEFAULT }, 8);
    g.input(Input::Press(Button::Ok));
    let mut dir = 0i8;
    let t0 = std::time::Instant::now();
    let mut n = 0u32;
    for k in 0..200_000usize {
        for i in bodies::bricks_autopilot(&g, &mut dir, 3, k) {
            g.input(i);
        }
        g.step();
        n += 1;
        if g.ended() {
            g = Bricks::new(Config { speed: 2, ..Config::DEFAULT }, k as u64);
        }
    }
    let ns = t0.elapsed().as_nanos() as f64 / f64::from(n);
    eprintln!("{ns:.0} ns per logic step on the host");
    eprintln!(
        "RAM: Bricks {} B, Session {} B, Hub {} B",
        size_of::<Bricks>(),
        size_of::<Session>(),
        size_of::<orpheus_games::Hub>()
    );
    let _ = (FIELD_H, ROWS);
}
