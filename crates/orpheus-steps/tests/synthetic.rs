//! The detector on synthetic traces (05 §7.7 acceptance): walking and
//! running within ± 5 % of the steps taken, in the hand and in a pocket,
//! at the player's 25 Hz and at 50 Hz; zero steps on a table, while the
//! player is handled, on a bus, and (cadence gate) while it is shaken.

mod common;

use common::*;
use orpheus_steps::{Detector, Sensitivity};

const RATES: [u32; 2] = [25, 50];

fn det(rate: u32) -> Detector {
    Detector::new(u16::try_from(rate).unwrap(), Sensitivity::Medium)
}

fn check_walk(name: &str, g: Gait, secs: f64) {
    for rate in RATES {
        for seed in 1..=8 {
            let t = walk(rate, secs, g, seed);
            let got = count(&mut det(rate), &t);
            assert!(
                within(got, t.steps, 5),
                "{name} @ {rate} Hz seed {seed}: counted {got} of {} steps",
                t.steps
            );
        }
    }
}

#[test]
fn walking_in_the_hand() {
    check_walk("walk", WALK, 120.0);
}

#[test]
fn slow_walking() {
    check_walk("slow", SLOW, 120.0);
}

#[test]
fn brisk_walking() {
    check_walk("brisk", BRISK, 120.0);
}

#[test]
fn running() {
    check_walk("run", RUN, 90.0);
}

#[test]
fn walking_with_the_player_in_a_pocket() {
    check_walk("pocket", POCKET, 120.0);
}

#[test]
fn two_hundred_steps_twice() {
    // The user check: walk 200 counted steps, twice.
    for seed in [11, 12] {
        let t = walk(25, 200.0 / WALK.cadence + 0.3, WALK, seed);
        let got = count(&mut det(25), &t);
        assert!((190..=210).contains(&got), "seed {seed}: {got} of {}", t.steps);
    }
}

#[test]
fn zero_steps_on_a_table_for_ten_minutes() {
    for rate in RATES {
        for seed in 1..=5 {
            let t = rest(rate, 600.0, seed);
            let mut d = Detector::new(u16::try_from(rate).unwrap(), Sensitivity::High);
            assert_eq!(count(&mut d, &t), 0, "rest @ {rate} Hz seed {seed}");
        }
    }
}

#[test]
fn zero_steps_while_handled() {
    for rate in RATES {
        for seed in 1..=8 {
            let t = handling(rate, 120.0, seed);
            assert_eq!(count(&mut det(rate), &t), 0, "handling @ {rate} Hz seed {seed}");
        }
    }
}

#[test]
fn zero_steps_on_a_bus() {
    for rate in RATES {
        for seed in 1..=5 {
            let t = bus(rate, 600.0, seed);
            assert_eq!(count(&mut det(rate), &t), 0, "bus @ {rate} Hz seed {seed}");
        }
    }
}

#[test]
fn shaking_is_not_walking() {
    // 3.8–8 Hz (bursts vary ± 15 % around 4.5–7 Hz): faster than any walk
    // (the cadence window, and the too-fast rule on the unfiltered
    // signal). Slower shaking, at a
    // running cadence (below ≈ 3.7 Hz), cannot be told from running.
    for rate in RATES {
        for (hz, seed) in [(4.5, 1), (5.0, 5), (5.5, 2), (6.5, 3), (7.0, 4)] {
            let t = shake(rate, 120.0, hz, 1.2, seed);
            let got = count(&mut det(rate), &t);
            assert_eq!(got, 0, "shake {hz} Hz @ {rate} Hz");
        }
    }
}

#[test]
fn walks_with_stops() {
    for rate in RATES {
        let t = walk(rate, 40.0, WALK, 3)
            .then(rest(rate, 15.0, 4))
            .then(walk(rate, 30.0, BRISK, 5))
            .then(handling(rate, 20.0, 6))
            .then(walk(rate, 40.0, POCKET, 7));
        let got = count(&mut det(rate), &t);
        assert!(within(got, t.steps, 5), "@ {rate} Hz: {got} of {}", t.steps);
    }
}

#[test]
fn sensitivity_orders_the_counts() {
    // A very soft walk (0.08 g): High counts it, Low does not.
    let soft = Gait { amp: 0.08, ..SLOW };
    let t = walk(25, 120.0, soft, 9);
    let mut hi = Detector::new(25, Sensitivity::High);
    let mut lo = Detector::new(25, Sensitivity::Low);
    let (h, l) = (count(&mut hi, &t), count(&mut lo, &t));
    assert!(within(h, t.steps, 10), "high: {h} of {}", t.steps);
    assert!(l < t.steps / 4, "low: {l}");
}

#[test]
fn clipped_sprint_still_counts() {
    // ± 2 g clipping on a hard run.
    let sprint = Gait { amp: 1.8, cadence: 3.0, ..RUN };
    for rate in RATES {
        let t = walk(rate, 60.0, sprint, 21);
        let got = count(&mut det(rate), &t);
        assert!(within(got, t.steps, 5), "@ {rate} Hz: {got} of {}", t.steps);
    }
}

/// Regenerates the committed `traces/synthetic-*.csv` files:
/// `cargo test -p orpheus-steps --test synthetic -- --ignored write_traces`.
#[test]
#[ignore = "writes the committed synthetic traces"]
fn write_traces() {
    use orpheus_steps::trace::{Sample, format_header, format_sample};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("traces");
    let cases: [(&str, Trace, &str); 3] = [
        ("synthetic-walk-hand-25hz.csv", walk(25, 60.0, WALK, 31), "walking, player in the hand"),
        ("synthetic-walk-pocket-25hz.csv", walk(25, 60.0, POCKET, 32), "walking, player in a trouser pocket"),
        ("synthetic-table-25hz.csv", rest(25, 60.0, 33), "on a table"),
    ];
    for (name, t, what) in cases {
        let mut out = Vec::new();
        out.extend_from_slice(
            format!("# synthetic ({what}; crates/orpheus-steps/tests/common)\n").as_bytes(),
        );
        out.extend_from_slice(format!("# truth={}\n", t.steps).as_bytes());
        let mut b = [0u8; 128];
        let n = format_header(0, 25, 0x06, 0, &mut b);
        out.extend_from_slice(&b[..n]);
        for (i, s) in t.samples.iter().enumerate() {
            let ms = u32::try_from(i * 40).unwrap();
            let n = format_sample(&Sample { ms, x: s[0], y: s[1], z: s[2] }, &mut b);
            out.extend_from_slice(&b[..n]);
        }
        std::fs::write(dir.join(name), out).unwrap();
    }
}
