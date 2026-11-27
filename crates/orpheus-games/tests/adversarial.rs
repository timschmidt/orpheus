//! Adversarial save files (VER-3, 07 §3.7): `/.orpheus/` game saves and
//! the score table come from the card, so anything may be in them — also
//! with a right CRC (a foreign or hand-made file). Every save of every
//! game cut at every byte, every payload byte replaced (complement, 0x00,
//! 0xFF) with the CRC made right, and any payload bytes (proptest): a file
//! is refused, or it loads, re-encodes to itself and survives play
//! (inputs and logic steps) without a panic.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

use adversarial::{config, damaged, prefixes};
use orpheus_games::save::{self, OVERHEAD, crc32};
use orpheus_games::scores::Scores;
use orpheus_games::{Input, SAVE_MAX, Session, blocks, solitaire, tiles};
use proptest::prelude::*;

/// A small deterministic generator for the play.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// `n` random inputs and logic steps on the session's game.
fn play(s: &mut Session, seed: u64, n: usize) {
    let mut r = Rng(seed | 1);
    for _ in 0..n {
        let x = r.next();
        let input = Input::from_byte(x as u8);
        let step = x >> 8 & 1 == 0;
        match s {
            Session::None => return,
            Session::Blocks(g) => {
                if let Some(i) = input {
                    let _ = g.input(i);
                }
                if step {
                    let _ = g.step();
                }
            }
            Session::Tiles(g) => {
                if let Some(i) = input {
                    let _ = g.input(i);
                }
            }
            Session::Solitaire(g) => {
                if let Some(i) = input {
                    let _ = g.input(i);
                }
                if step {
                    let _ = g.step();
                }
            }
            #[cfg(feature = "salvo")]
            Session::Salvo(g) => {
                if let Some(i) = input {
                    let _ = g.input(i);
                }
                if step {
                    let _ = g.step();
                }
            }
            #[cfg(feature = "bricks")]
            Session::Bricks(g) => {
                if let Some(i) = input {
                    let _ = g.input(i);
                }
                if step {
                    let _ = g.step();
                }
            }
        }
    }
    let _ = s.in_progress();
}

/// A game of each kind, played a little, from configuration bytes `c`.
fn sessions(c: &[u8], seed: u64) -> Vec<Session> {
    let mut v = vec![
        Session::Blocks(blocks::Blocks::new(bodies::blocks_config(c), seed)),
        Session::Tiles(tiles::Tiles::new(bodies::tiles_config(c), seed)),
        Session::Solitaire(solitaire::Solitaire::new(bodies::solitaire_config(c), seed)),
    ];
    #[cfg(feature = "salvo")]
    {
        let mut s = Session::None;
        s.start_salvo(bodies::salvo_config(c), seed);
        v.push(s);
    }
    #[cfg(feature = "bricks")]
    v.push(Session::Bricks(orpheus_games::bricks::Bricks::new(bodies::bricks_config(c), seed)));
    for (i, s) in v.iter_mut().enumerate() {
        play(s, seed ^ i as u64, 300);
    }
    v
}

fn encode(s: &Session) -> Vec<u8> {
    let mut buf = [0u8; SAVE_MAX];
    let n = s.encode(&mut buf).expect("a game in progress encodes");
    buf[..n].to_vec()
}

/// Make the CRC right again after a payload change.
fn reseal(b: &mut [u8]) {
    if b.len() >= OVERHEAD {
        let n = b.len() - 4;
        let c = crc32(&b[..n]);
        b[n..].copy_from_slice(&c.to_le_bytes());
    }
}

/// A save that loads survives play, and what the game then writes loads
/// again (a hostile-but-accepted state must not turn into a save the game
/// itself refuses: the player would lose the game). Decoding is not the
/// identity by design (Salvo saves a shot in flight as settled and plans
/// the computer's turn again on load; transient UI state is not saved).
fn load_and_play(b: &[u8], seed: u64) {
    let Some(mut s) = Session::decode(b, None) else { return };
    // The in-place variant agrees.
    let mut t = Session::None;
    assert!(t.decode_into(b, s.game()));
    assert_eq!(t, s);
    if s.in_progress() {
        let again = encode(&s);
        assert!(Session::decode(&again, s.game()).is_some(), "a loaded save, saved again, is refused");
    }
    play(&mut s, seed, 400);
    if s.game().is_some() && s.in_progress() {
        let again = encode(&s);
        assert!(Session::decode(&again, s.game()).is_some(), "a played game's save is refused");
    }
}

#[test]
fn saves_cut_at_every_byte() {
    for c in [[0u8; 10], [7, 3, 1, 2, 3, 4, 5, 6, 7, 8], [0xFF; 10]] {
        for s in sessions(&c, 0x5EED) {
            let b = encode(&s);
            // A real save loads (not necessarily equal: Solitaire does not
            // save the cards in hand, Salvo a shot in flight).
            assert!(Session::decode(&b, s.game()).is_some(), "a real save is refused");
            prefixes(&b, |p| assert!(Session::decode(p, None).is_none(), "a cut save loads"));
            // Cut and resealed (a consistent file of the wrong length).
            for n in OVERHEAD..b.len() {
                let mut p = b[..n].to_vec();
                let len = (n - OVERHEAD) as u16;
                p[6..8].copy_from_slice(&len.to_le_bytes());
                reseal(&mut p);
                load_and_play(&p, n as u64);
            }
        }
    }
}

#[test]
fn every_payload_byte_damaged_with_a_right_crc() {
    for c in [[0u8; 10], [9, 2, 5, 1, 1, 2, 3, 4, 5, 6]] {
        for s in sessions(&c, 77) {
            let b = encode(&s);
            damaged(&b, |i, d| {
                let mut d = d.to_vec();
                if (8..d.len() - 4).contains(&i) {
                    reseal(&mut d);
                }
                load_and_play(&d, i as u64);
            });
        }
    }
}

#[test]
fn kind_version_and_length_fields() {
    for s in sessions(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 3) {
        let b = encode(&s);
        for kind in 0..=255u8 {
            for version in [0u8, 1, 2, 3, 0xFF] {
                let mut d = b.clone();
                d[4] = kind;
                d[5] = version;
                reseal(&mut d);
                load_and_play(&d, u64::from(kind));
                assert!(Session::decode(&d, Some(kind.wrapping_add(1))).is_none(), "loaded as another game");
            }
        }
        for len in [0u16, 1, (b.len() - OVERHEAD) as u16 + 1, u16::MAX] {
            let mut d = b.clone();
            d[6..8].copy_from_slice(&len.to_le_bytes());
            reseal(&mut d);
            assert!(Session::decode(&d, None).is_none());
        }
    }
}

#[test]
fn score_table_hostile() {
    let mut buf = [0u8; 512];
    let n = save::seal(&mut buf, 0xFF, 1, |w| w.u8(0)).unwrap();
    let _ = Scores::decode(&buf[..n]);
    // Every count with too few, exactly enough and too many records.
    let s = Scores::default();
    let mut good = [0u8; 1024];
    if let Some(n) = s.encode(&mut good) {
        let g = good[..n].to_vec();
        let kind = g[4];
        let version = g[5];
        for count in 0..=255u8 {
            for recs in [0usize, 1, usize::from(count), usize::from(count) + 1] {
                let mut b = [0u8; 4096];
                let n = save::seal(&mut b, kind, version, |w| {
                    w.u8(count);
                    for k in 0..recs.min(200) {
                        w.u8(k as u8);
                        w.u16(0xFFFF);
                        w.u32(u32::MAX);
                        w.u32(u32::MAX);
                        w.u32(u32::MAX);
                        w.u16(u16::MAX);
                    }
                });
                if let Some(n) = n
                    && let Some(t) = Scores::decode(&b[..n])
                {
                    let mut again = [0u8; 4096];
                    let m = t.encode(&mut again).unwrap();
                    assert_eq!(Scores::decode(&again[..m]), Some(t));
                }
            }
        }
        damaged(&g, |_, d| {
            let mut d = d.to_vec();
            reseal(&mut d);
            let _ = Scores::decode(&d);
        });
    }
}

proptest! {
    #![proptest_config(config("games-hostile-saves", 256))]

    /// A real save with any payload bytes overwritten (CRC made right).
    #[test]
    fn any_payload_bytes(c in prop::collection::vec(any::<u8>(), 10), seed in any::<u64>(),
                         patches in prop::collection::vec((any::<u16>(), any::<u8>()), 1..16)) {
        for s in sessions(&c, seed) {
            let mut b = encode(&s);
            let body = b.len() - OVERHEAD;
            for (at, v) in &patches {
                let i = 8 + usize::from(*at) % body.max(1);
                b[i] = *v;
            }
            reseal(&mut b);
            load_and_play(&b, seed);
        }
    }
}

/// Every kept regression save (`tests/regress/games_save/`) is refused or
/// loads, without a panic, and survives play.
#[test]
fn regress_inputs_replay() {
    let dir = adversarial::root().join("tests").join("regress").join("games_save");
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
        let b = std::fs::read(e.path()).unwrap();
        load_and_play(&b, 1);
        n += 1;
    }
    assert!(n > 0);
}

/// A Bricks save with the CRC right but a hostile value: `broken[0]` of
/// 0x8000_0000 (`n * s * BRICK_POINTS` overflowed) or the paddle at
/// `i32::MAX` (`px + pw * ONE` overflowed).
#[cfg(feature = "bricks")]
fn bricks_overflow_save(field: &str) -> Vec<u8> {
    use orpheus_games::bricks::{CELLS, MAX_BALLS, MAX_CAPS};
    let s = Session::Bricks(orpheus_games::bricks::Bricks::new(bodies::bricks_config(&[0, 0, 0]), 1));
    let mut b = encode(&s);
    let end = b.len() - 4;
    let off = match field {
        "broken" => end - 4 - 2 - 12,
        _ => 8 + 7 + 16 + CELLS + MAX_BALLS * 12 + MAX_CAPS * 6,
    };
    let v: u32 = if field == "broken" { 0x8000_0000 } else { i32::MAX as u32 };
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    reseal(&mut b);
    b
}

/// VER-3, 2026-11-27: `Bricks::check`, which `Bricks::decode` runs on
/// every save file from the card, summed the broken-brick counters
/// (`n * s * BRICK_POINTS`) and the paddle and ball extents (`x + w *
/// ONE`) in `u32`/`i32` from the file's values: a save with a right CRC
/// and a large counter or position panicked with "attempt to multiply
/// with overflow" / "attempt to add with overflow" (simulator, browser
/// build) or wrapped (firmware, overflow checks off), where a wrapped
/// score sum could match the stored score and load a state the rules
/// cannot reach. Inputs: `tests/regress/games_save/bricks-*-overflow`.
#[cfg(feature = "bricks")]
#[test]
fn regress_bricks_save_check_overflow() {
    for field in ["broken", "paddle"] {
        let b = bricks_overflow_save(field);
        assert!(Session::decode(&b, None).is_none(), "{field}: a hostile save loads");
        let path = adversarial::root()
            .join("tests")
            .join("regress")
            .join("games_save")
            .join(format!("bricks-{field}-overflow"));
        if std::env::var_os("ORPHEUS_WRITE_SEEDS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &b).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), b, "{field}: the kept input");
    }
}
