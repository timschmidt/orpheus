//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-power --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/power_samples/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

/// One record: cable, presence result (0 none, 1 absent, 2 present, 3
/// timeout), charger phase, battery raw, current step.
fn rec(cable: bool, presence: u8, phase: u8, raw: u16, step: u8) -> [u8; 4] {
    let r = raw.to_le_bytes();
    [u8::from(cable) | presence << 1 | phase << 3, r[0], r[1], step]
}

#[test]
fn power_samples_corpus() {
    replay::replay("power_samples", bodies::power_samples);
}

#[test]
fn seeds() {
    let mut v = Vec::new();
    // Cable in, battery present and low (trickle), rising through normal
    // charge to full (constant voltage, small current), idle full,
    // recharge, cable out.
    let mut full = Vec::new();
    full.extend(rec(false, 0, 0, 700, 0));
    full.extend(rec(true, 0, 1, 600, 7));
    full.extend(rec(true, 2, 1, 600, 7));
    for k in 0..250u16 {
        full.extend(rec(true, 0, 1, 600 + k / 2, 7));
    }
    for raw in (700..0x380).step_by(8) {
        full.extend(rec(true, 0, 2, raw, 7));
    }
    for _ in 0..4 {
        full.extend(rec(true, 0, 3, 0x37C, 1));
    }
    for _ in 0..5 {
        full.extend(rec(true, 0, 0, 0x376, 0));
    }
    full.extend(rec(true, 0, 0, 0x330, 0));
    full.extend(rec(false, 0, 0, 0x330, 0));
    v.push(("charge-to-full".to_string(), full));
    // No battery, detection timeout, full at insertion, cable bouncing.
    let mut odd = Vec::new();
    odd.extend(rec(true, 0, 0, 0, 0));
    odd.extend(rec(true, 1, 0, 0, 0));
    odd.extend(rec(false, 0, 0, 0, 0));
    odd.extend(rec(true, 0, 0, 900, 0));
    odd.extend(rec(true, 3, 0, 900, 0));
    odd.extend(rec(false, 0, 0, 900, 0));
    odd.extend(rec(true, 0, 0, 0x3FF, 0));
    odd.extend(rec(true, 2, 0, 0x3FF, 0));
    for k in 0..20u8 {
        odd.extend(rec(k % 3 != 0, 0, k % 4, 0x2AB + u16::from(k), k));
    }
    v.push(("odd".to_string(), odd));
    for (_, d) in &v {
        bodies::power_samples(d);
    }
    replay::write_seeds("power_samples", &v);
}
