//! Step timing: every hit on the floor of its exact time, at any tempo,
//! swing, micro-timing and ratchet, for an hour; tempo changes at the next
//! step; the audio equal to the same notes placed by hand.

mod common;

use common::{engine, hits, project};
use orpheus_seq::model::{MICRO_MAX, TEMPO_MAX, TEMPO_MIN};
use orpheus_seq::{Hit, Sequencer, Step};
use orpheus_synth::{Engine, Event};

/// Frame of a hit, computed independently of the player: in pairs of
/// steps, `start` = Σ of the swung step lengths before it (hundredths of a
/// pair), plus the delay (m/48 pair), plus j/r of the step's own length; a
/// pair lasts 30 / BPM s = 3000 / tempo(1/100 BPM) s.
fn exact(rate: u32, tempo: u16, before: &[u8], step_len: u8, micro: u8, j: u64, r: u64) -> u64 {
    let sum: i128 = before.iter().map(|&d| i128::from(d)).sum();
    // Pair fractions over 4800·r.
    let num = sum * 48 * i128::from(r)
        + i128::from(micro) * 100 * i128::from(r)
        + i128::from(j) * i128::from(step_len) * 48;
    let den = 4800 * i128::from(r);
    u64::try_from(num * 3000 * i128::from(rate) / (den * i128::from(tempo))).unwrap()
}

fn lens(swing: u8, n: usize, pattern_len: usize) -> Vec<u8> {
    (0..n).map(|k| if (k % pattern_len) % 2 == 0 { swing } else { 100 - swing }).collect()
}

#[test]
fn every_step_lands_on_its_exact_frame() {
    for rate in [8_000, 44_100, 48_000] {
        for tempo in [TEMPO_MIN, 9_337, 12_000, 13_333, 17_471, TEMPO_MAX] {
            for swing in [50, 57, 66, 75] {
                for len in [16usize, 15, 7] {
                    let mut p = project();
                    p.set_tempo(tempo);
                    p.set_swing(swing);
                    p.set_len(0, len);
                    for s in 0..len {
                        p.set_step(0, 0, s, Step::hit(100));
                    }
                    let steps = 3 * len + 1;
                    let d = lens(swing, steps, len);
                    let frames = exact(rate, tempo, &d[..steps - 1], 0, 0, 0, 1) as usize + 1;
                    let h = hits(&p, rate, frames);
                    assert_eq!(h.len(), steps, "rate {rate} tempo {tempo} swing {swing} len {len}");
                    for (k, hit) in h.iter().enumerate() {
                        assert_eq!(hit.frame, exact(rate, tempo, &d[..k], 0, 0, 0, 1), "step {k}");
                    }
                }
            }
        }
    }
}

/// Swing moves exactly the odd steps: at 66 % a straight pair of 1/16s
/// becomes a 2:1 shuffle, the even steps stay on the grid.
#[test]
fn swing_delays_the_odd_steps_only() {
    let mut p = project();
    for s in 0..16 {
        p.set_step(0, 0, s, Step::hit(100));
    }
    let straight = hits(&p, 48_000, 48_000);
    p.set_swing(66);
    let swung = hits(&p, 48_000, 48_000);
    assert_eq!(straight.len(), swung.len());
    // 120 BPM: a step is 6000 frames, a pair 12000.
    for (k, (a, b)) in straight.iter().zip(&swung).enumerate() {
        assert_eq!(a.frame, 6000 * k as u64);
        if k % 2 == 0 {
            assert_eq!(b.frame, a.frame, "even step {k} on the grid");
        } else {
            assert_eq!(b.frame, 12_000 * (k as u64 / 2) + 66 * 12_000 / 100, "odd step {k}");
        }
    }
}

#[test]
fn micro_timing_and_ratchets_are_exact() {
    for (rate, tempo, swing) in
        [(48_000, 12_000, 50), (44_100, 9_733, 61), (32_000, 23_999, 75), (8_000, 4_001, 53)]
    {
        for micro in 0..=MICRO_MAX {
            for r in 1..=4u8 {
                let mut p = project();
                p.set_tempo(tempo);
                p.set_swing(swing);
                // Steps 0 and 5 (an even and an odd one).
                for s in [0, 5] {
                    p.set_step(0, 2, s, Step::hit(90).with_micro(micro).with_ratchet(r));
                }
                let d = lens(swing, 16, 16);
                let end = exact(rate, tempo, &d, 0, 0, 0, 1) as usize;
                let h = hits(&p, rate, end);
                assert_eq!(h.len(), 2 * usize::from(r));
                let mut want = Vec::new();
                for s in [0usize, 5] {
                    for j in 0..u64::from(r) {
                        want.push(exact(rate, tempo, &d[..s], d[s], micro, j, u64::from(r)));
                    }
                }
                want.sort_unstable();
                let got: Vec<u64> = h.iter().map(|h| h.frame).collect();
                assert_eq!(got, want, "rate {rate} micro {micro} ratchet {r}");
            }
        }
    }
}

/// An hour at a tempo with no exact frame length: step 31,999 (and every
/// one before it) is still on its exact frame.
#[test]
fn no_drift_over_an_hour() {
    let rate = 44_100;
    let tempo = 13_337;
    let mut p = project();
    p.set_swing(58);
    p.set_tempo(tempo);
    p.set_step(0, 0, 0, Step::hit(100));
    p.set_step(0, 0, 3, Step::hit(100));
    let frames = 3600 * rate as usize;
    let h = hits(&p, rate, frames);
    let d = lens(58, 40_000, 16);
    let mut prefix = vec![0u64; d.len() + 1];
    for (i, x) in d.iter().enumerate() {
        prefix[i + 1] = prefix[i] + u64::from(*x);
    }
    let mut n = 0;
    for (k, hit) in h.iter().enumerate() {
        let step = 16 * (k / 2) + if k % 2 == 0 { 0 } else { 3 };
        let f = u64::try_from(i128::from(prefix[step]) * 30 * i128::from(rate) / i128::from(tempo)).unwrap();
        assert_eq!(hit.frame, f, "hit {k}");
        n += 1;
    }
    // 133.37 BPM: 8.89 steps a second, 32,011 in the hour, 2 hits in 16.
    assert_eq!(n, 4002, "{n} hits");
}

/// A tempo change takes effect at the next step: the step after the
/// change starts where the old tempo put it, and the steps after it are
/// spaced at the new tempo from there.
#[test]
fn tempo_change_at_the_next_step() {
    let rate = 48_000;
    let mut p = project();
    for s in 0..16 {
        p.set_step(0, 0, s, Step::hit(100));
    }
    let mut seq = Sequencer::new(rate);
    seq.play();
    let mut got: Vec<Hit> = Vec::new();
    // 120 BPM: steps every 6000 frames. Change in the middle of step 2.
    seq.run_dry(&p, 15_000, |h| got.push(h));
    p.set_tempo(6_000); // 60 BPM: 12000 frames a step
    seq.run_dry(&p, 60_000, |h| got.push(h));
    let frames: Vec<u64> = got.iter().map(|h| h.frame).collect();
    assert_eq!(&frames[..6], &[0, 6000, 12_000, 18_000, 30_000, 42_000]);
}

/// The audio of a played pattern equals the engine fed the same notes by
/// hand at the exact frames (one note-on per hit, part 0, the kit).
#[test]
fn audio_equals_notes_placed_by_hand() {
    let rate = 48_000;
    let mut p = project();
    p.set_tempo(10_300);
    p.set_swing(62);
    common::row(&mut p);
    let frames = 4 * rate as usize;
    let h = hits(&p, rate, frames);
    assert!(h.len() > 40);
    let auto = common::render(&p, rate, frames, &[517]);
    let mut e: Box<Engine> = engine(rate);
    e.set_kit(0, p.kit());
    e.set_patch(0, &orpheus_synth::Patch::kit(0));
    let mut by_hand = vec![0i16; 2 * frames];
    let ev: Vec<Event> = h.iter().map(|h| Event::note_on(h.frame as u32, 0, h.note, h.velocity)).collect();
    e.render_i16(&mut by_hand, &ev);
    assert!(auto == by_hand, "the sequencer's render equals the hand-placed notes");
    assert!(auto.iter().any(|s| s.unsigned_abs() > 3000), "audible");
}
