//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-seq --test
//! fuzz_corpus` rewrites `tests/fuzz-corpus/seq_load/seed-*`).

mod common;

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use orpheus_seq::{Project, demo};

#[test]
fn seq_load_corpus() {
    assert!(replay::replay("seq_load", bodies::seq_load) >= 8);
}

fn with_opt(opt: u8, f: Vec<u8>) -> Vec<u8> {
    let mut v = vec![opt];
    v.extend(f);
    v
}

/// Seeds: a new project, the demo, random full projects, and damaged
/// files (a cut, a flipped byte, a newer version, an unknown record).
#[test]
fn seq_seeds() {
    let mut v: Vec<(String, Vec<u8>)> = Vec::new();
    v.push(("new".into(), with_opt(3, common::saved(&Project::NEW))));
    let mut d = Box::new(Project::NEW);
    demo::beat(&mut d);
    let demo_file = common::saved(&d);
    v.push(("demo".into(), with_opt(3, demo_file.clone())));
    v.push(("demo-recrc".into(), with_opt(2, demo_file.clone())));
    for seed in [1u64, 7, 42] {
        v.push((format!("random{seed}"), with_opt(1, common::saved(&common::random(seed)))));
    }
    let mut cut = demo_file.clone();
    cut.truncate(cut.len() / 2);
    v.push(("cut".into(), with_opt(0, cut)));
    let mut flip = demo_file.clone();
    flip[40] ^= 0x10;
    v.push(("flip".into(), with_opt(1, flip)));
    let mut newer = demo_file.clone();
    newer[4] = 2;
    v.push(("newer".into(), with_opt(0, newer)));
    let mut unknown = demo_file[..demo_file.len() - 4].to_vec();
    unknown.extend([0x7E, 3, 0, 1, 2, 3, 0, 0, 0, 0]);
    v.push(("unknown-tag".into(), with_opt(0, unknown)));
    replay::write_seeds("seq_load", &v);
    for (name, s) in &v {
        eprintln!("seed {name}");
        bodies::seq_load(s);
    }
}
