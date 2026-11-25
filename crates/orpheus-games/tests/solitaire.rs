//! Solitaire: property tests (random key sequences over many seeds and
//! every rule combination, through the fuzz body that checks every move
//! against an independent reference and every undo against the position
//! it restores), won games played with the keys, saves, deterministic
//! deals, and the deal check's effort.

#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

use orpheus_games::solitaire::{
    self, Board, CHECK_MOVES, Config, Move, PILES, STOCK, Scoring, SolOut, Solitaire, TABLEAU, is_tableau,
};
use orpheus_games::{Button, Input, Rng, SAVE_MAX, Session};

fn press(b: Button) -> u8 {
    Input::Press(b).to_byte()
}

fn release(b: Button) -> u8 {
    Input::Release(b).to_byte()
}

fn tap(d: &mut Vec<u8>, b: Button) {
    d.push(press(b));
    d.push(release(b));
}

fn header(cfg: u8, seed: u64) -> Vec<u8> {
    let mut d = vec![cfg];
    d.extend_from_slice(&seed.to_le_bytes());
    d
}

/// Random keys: mostly moves of the cursor and OK, now and then a held
/// OK (undo), Menu, logic steps or junk.
fn random_game(seed: u64, keys: usize) -> Vec<u8> {
    let mut r = Rng::new(seed);
    // No deal check here (its effort is tested on its own).
    let mut d = header((r.below(128)) as u8, r.next_u64());
    let moves = [Button::Left, Button::Right, Button::Up, Button::Down];
    for _ in 0..keys {
        match r.below(100) {
            0..=44 => tap(&mut d, moves[r.below(4) as usize]),
            45..=79 => tap(&mut d, Button::Ok),
            80..=84 => {
                d.push(press(Button::Ok));
                d.push(Input::Long(Button::Ok).to_byte());
                d.push(release(Button::Ok));
            }
            85..=89 => tap(&mut d, Button::Menu),
            90..=96 => d.push(0x80 | r.below(128) as u8),
            _ => d.push(r.below(256) as u8),
        }
    }
    d
}

/// 300 random games: every move legal by the reference, undo exact, the
/// 52 cards always there.
#[test]
fn invariants_hold_over_random_games() {
    let mut moved = 0;
    for seed in 0..300 {
        let g = bodies::solitaire_run(&random_game(seed, 600));
        moved += usize::from(g.moves() > 0);
    }
    assert!(moved > 150, "most random games make moves ({moved})");
}

/// Every rule combination plays (the deal check too, for a few).
#[test]
fn every_configuration() {
    for cfg in 0..=255u8 {
        if cfg & 0x80 != 0 && cfg % 16 != 0 {
            continue;
        }
        let mut d = random_game(u64::from(cfg) * 31 + 1, 150);
        d[0] = cfg;
        let g = bodies::solitaire_run(&d);
        let c = g.config();
        assert_eq!(c.draw, if cfg & 1 != 0 { 3 } else { 1 });
        assert_eq!(c.auto_move, cfg & 0x20 == 0);
    }
}

/// Ring position of a pile (the cursor's Prev/Next order).
fn ring(p: u8) -> i32 {
    i32::from(p)
}

/// Keys that take the cursor from `from` to `to` along the ring.
fn walk(d: &mut Vec<u8>, from: u8, to: u8) {
    let n = PILES as i32;
    let fwd = (ring(to) - ring(from)).rem_euclid(n);
    if fwd <= n / 2 {
        (0..fwd).for_each(|_| tap(d, Button::Right));
    } else {
        (0..n - fwd).for_each(|_| tap(d, Button::Left));
    }
}

/// The keys for one move of a winning line, from the cursor at `cur`.
fn keys_for(d: &mut Vec<u8>, cur: &mut u8, m: &Move) {
    if m.from == STOCK || m.to == STOCK {
        walk(d, *cur, STOCK);
        *cur = STOCK;
        tap(d, Button::Ok);
        return;
    }
    walk(d, *cur, m.from);
    if is_tableau(m.from) {
        (1..m.n).for_each(|_| tap(d, Button::Up));
    }
    tap(d, Button::Ok);
    walk(d, m.from, m.to);
    tap(d, Button::Ok);
    *cur = m.to;
}

/// The deal check's winning lines played with the keys (no assists, so
/// the line is the game's): the games are won, through the fuzz body.
#[test]
fn won_with_the_keys() {
    let mut won = 0;
    for (k, draw) in [(0u8, 1u8), (1, 3)] {
        let cfg = Config { draw, auto_move: false, auto_complete: false, winnable: false, ..Config::DEFAULT };
        for seed in 0..40u64 {
            let b = Board::deal(seed);
            let mut line = [Move { from: 0, to: 0, n: 0, flipped: false, auto: false }; CHECK_MOVES as usize];
            let Some(n) = solitaire::solution(&b, &cfg, seed, &mut line) else { continue };
            let mut d = header(k | 0x60, seed);
            let mut cur = TABLEAU;
            for m in &line[..n] {
                keys_for(&mut d, &mut cur, m);
            }
            let g = bodies::solitaire_run(&d);
            assert!(g.won(), "seed {seed} draw {draw}: the line wins with the keys");
            assert!(g.ended());
            won += 1;
        }
    }
    assert!(won >= 10, "{won} won games");
}

/// Auto-move and auto-complete finish a game the player nearly won.
#[test]
fn assists_finish_the_game() {
    let cfg = Config::DEFAULT;
    // Spades to the jack, hearts to the queen, clubs and diamonds done but
    // their kings: K♠ | K♥ Q♠ | K♣ | K♦.
    let mut g = Solitaire::nearly_done(cfg, &[&[12], &[25, 11], &[38], &[51], &[], &[], &[]]);
    assert!(g.completing());
    let mut steps = 0;
    while !g.won() && steps < 200 {
        g.step();
        steps += 1;
    }
    assert!(g.won(), "auto-complete");
    assert!(steps <= 5 * solitaire::AUTO_STEPS + 1);
    // With auto-move only: the queen goes up by itself after a move.
    let cfg = Config { auto_complete: false, ..Config::DEFAULT };
    let mut g = Solitaire::nearly_done(cfg, &[&[12], &[25, 11], &[38], &[51], &[], &[], &[]]);
    assert!(g.try_move(TABLEAU + 1, TABLEAU + 4, 2));
    assert!(g.won(), "every card was safe");
    assert!(g.undo());
    assert_eq!(g.board().len(TABLEAU + 1), 2, "one undo takes the move and the game's moves back");
    // Menu sends every card it can.
    let cfg = Config { auto_move: false, auto_complete: false, ..Config::DEFAULT };
    let mut g = Solitaire::nearly_done(cfg, &[&[12], &[25, 11], &[38], &[51], &[], &[], &[]]);
    assert_eq!(g.input(Input::Press(Button::Menu)), SolOut::Moved);
    assert!(g.won());
}

/// Hold OK takes back; its release picks nothing up; a refused place
/// keeps the cards in hand.
#[test]
fn keys() {
    let cfg = Config { winnable: false, auto_move: false, ..Config::DEFAULT };
    let mut g = Solitaire::new(cfg, 9);
    // Stock: OK draws.
    for _ in 0..6 {
        g.input(Input::Press(Button::Left));
    }
    assert_eq!(g.cursor().0, STOCK);
    g.input(Input::Press(Button::Ok));
    assert_eq!(g.input(Input::Release(Button::Ok)), SolOut::Moved);
    assert_eq!(g.board().len(solitaire::WASTE), 1);
    g.input(Input::Press(Button::Ok));
    assert_eq!(g.input(Input::Long(Button::Ok)), SolOut::Undone);
    assert_eq!(g.input(Input::Release(Button::Ok)), SolOut::None, "the release after a hold");
    assert_eq!(g.board().len(solitaire::WASTE), 0);
    // Pick up a column's card and put it on a column it does not fit.
    g.input(Input::Press(Button::Right));
    assert_eq!(g.cursor().0, solitaire::WASTE);
    while g.cursor().0 != TABLEAU {
        g.input(Input::Press(Button::Right));
    }
    g.input(Input::Release(Button::Ok));
    assert_eq!(g.held(), Some((TABLEAU, 1)));
    let refused = (1..7u8).find(|&c| !g.board().legal(TABLEAU, TABLEAU + c, 1)).unwrap();
    for _ in 0..refused {
        g.input(Input::Press(Button::Right));
    }
    assert_eq!(g.input(Input::Release(Button::Ok)), SolOut::Refused);
    assert_eq!(g.held(), Some((TABLEAU, 1)), "still in hand");
    assert!(g.drop_held());
    // Up and Down change rows at a column's ends.
    g.input(Input::Press(Button::Up));
    assert!(!is_tableau(g.cursor().0));
    g.input(Input::Press(Button::Down));
    assert!(is_tableau(g.cursor().0));
}

/// A game saved and read back halfway plays on exactly as one that was
/// not.
#[test]
fn save_and_continue_is_seamless() {
    let d = random_game(77, 800);
    let whole = bodies::solitaire_run(&d);
    let cut = 9 + 700;
    let first = bodies::solitaire_run(&d[..cut]);
    let mut buf = [0u8; SAVE_MAX];
    let n = Session::Solitaire(first.clone()).encode(&mut buf).unwrap();
    assert!(n <= SAVE_MAX);
    let Some(Session::Solitaire(mut g)) = Session::decode(&buf[..n], None) else { panic!("read back") };
    // Held cards are not saved: the same as putting them back.
    let mut cont = first;
    cont.drop_held();
    assert_eq!(g, cont);
    for &b in &d[cut..] {
        if b & 0x80 != 0 {
            for _ in 0..=(b & 0x7F) {
                g.step();
                cont.step();
            }
        } else if let Some(i) = Input::from_byte(b) {
            g.input(i);
            cont.input(i);
        }
    }
    assert_eq!(g, cont);
    let _ = whole;
}

/// The columns' top cards of the deal of seed 0x501.
const TOPS_0X501: [u8; 7] = [9, 23, 3, 15, 29, 32, 16];

/// The deals are fixed by the seed (recorded 2026-11-25: a change here
/// changes every saved and replayed game).
#[test]
fn deterministic_deals() {
    let b = Board::deal(0x501);
    let tops: Vec<u8> = (TABLEAU..PILES as u8).map(|p| b.top(p).unwrap()).collect();
    assert_eq!(tops, TOPS_0X501);
    let g = Solitaire::new(Config { auto_move: false, ..Config::DEFAULT }, 0x501);
    let again = Solitaire::new(Config { auto_move: false, ..Config::DEFAULT }, 0x501);
    assert_eq!(g, again);
    assert_eq!(g.board(), &Board::deal(g.seed()));
    std::println!("deal 0x501 tops {tops:?}, checked deal seed {:#x} ({})", g.seed(), g.checked());
}

/// The deal check: how often it wins, and its effort (host time; the
/// player is slower, see docs/STATUS.md). Fails if the check finds
/// nothing for the default rules.
#[test]
fn deal_check_effort() {
    for (name, cfg) in [
        ("draw one", Config::DEFAULT),
        ("draw three", Config { draw: 3, ..Config::DEFAULT }),
        ("draw three, one pass", Config { draw: 3, passes: 1, ..Config::DEFAULT }),
    ] {
        let n = 60u64;
        let mut wins = 0;
        let mut checked = 0;
        let mut worst = std::time::Duration::ZERO;
        let t0 = std::time::Instant::now();
        for seed in 0..n {
            if solitaire::winnable(&Board::deal(seed), &cfg, seed) {
                wins += 1;
            }
            let t = std::time::Instant::now();
            let g = Solitaire::new(cfg, seed * 7919);
            worst = worst.max(t.elapsed());
            checked += usize::from(g.checked());
        }
        let total = t0.elapsed();
        std::println!(
            "{name}: one deal won by the check {wins}/{n}; new games with a checked deal {checked}/{n}; \
             {:.2} ms per new game on average (host), worst {:.2} ms",
            total.as_secs_f64() * 1000.0 / n as f64,
            worst.as_secs_f64() * 1000.0
        );
        if cfg.draw == 1 {
            assert!(checked as u64 > n * 9 / 10, "{name}: {checked}/{n}");
        }
    }
    // Scores of the three ways.
    for scoring in [Scoring::Standard, Scoring::Casino, Scoring::None] {
        let g = Solitaire::new(Config { scoring, winnable: false, auto_move: false, ..Config::DEFAULT }, 1);
        let want = match scoring {
            Scoring::Standard => 0,
            Scoring::Casino => -52,
            Scoring::None => 0,
        };
        assert_eq!(g.score(), want);
    }
}
