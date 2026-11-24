//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-input --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/input_samples/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

#[test]
fn input_samples_corpus() {
    replay::replay("input_samples", bodies::input_samples);
}

fn sample(l1: u16, l2: u16, flags: u8) -> [u8; 5] {
    let a = l1.to_le_bytes();
    let b = l2.to_le_bytes();
    [a[0], a[1], b[0], b[1], flags]
}

#[test]
fn seeds() {
    const IDLE: u16 = 1000;
    let mut v = Vec::new();
    // Each ladder step pressed and released (stable readings).
    let mut taps = Vec::new();
    for l1 in [30, 120, 250, 350] {
        taps.extend(sample(l1, IDLE, 2));
        taps.extend(sample(IDLE, IDLE, 2));
    }
    for l2 in [30, 140, 280, 420, 600] {
        taps.extend(sample(IDLE, l2, 2));
        taps.extend(sample(IDLE, IDLE, 2));
    }
    taps.extend(sample(IDLE, IDLE, 3));
    taps.extend(sample(IDLE, IDLE, 2));
    v.push(("taps".to_string(), taps));
    // Bounce: alternating single readings, then two keys and power held.
    let mut bounce = Vec::new();
    for i in 0..12u16 {
        bounce.extend(sample(if i % 2 == 0 { 185 } else { IDLE }, IDLE, 0));
    }
    bounce.extend(sample(60, 69, 3));
    bounce.extend(sample(60, 69, 1));
    bounce.extend(sample(IDLE, IDLE, 2));
    v.push(("bounce".to_string(), bounce));
    // Earphones in (ladder 1 in the sense band), threshold edges, high bits.
    let mut ear = Vec::new();
    for l1 in [385, 450, 591, 592, 61, 62, 184, 185, 0xFC3E, 0x0400] {
        ear.extend(sample(l1, 699, 2));
        ear.extend(sample(l1, 700, 2));
    }
    v.push(("earphones-thresholds".to_string(), ear));
    for (_, d) in &v {
        bodies::input_samples(d);
    }
    replay::write_seeds("input_samples", &v);
}
