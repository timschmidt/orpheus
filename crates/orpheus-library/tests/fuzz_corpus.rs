//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-library --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/library_open/seed-*`):
//! library files the builder writes for random cards.

mod common;

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use common::{MemStore, Rng, build, random_card};
use orpheus_library::Config;

#[test]
fn library_open_corpus() {
    replay::replay("library_open", bodies::library_open);
}

#[test]
fn seeds() {
    let mut v = Vec::new();
    for (i, (tracks, articles)) in
        [(0usize, false), (4, false), (25, true), (60, false)].into_iter().enumerate()
    {
        let mut r = Rng(0xC0FFEE + i as u64);
        let mut card = random_card(&mut r, tracks);
        let mut store = MemStore::new();
        build(
            &mut store,
            &mut card,
            Config { serial: 0x1234, ignore_articles: articles, ..Config::default() },
        );
        let (_, file) = store.files.iter().find(|(n, _)| n.starts_with("lib-")).expect("a library file");
        for sel in [0u8, 3, 4, 8] {
            let mut d = vec![sel];
            d.extend_from_slice(file);
            if d.len() <= 64 * 1024 {
                v.push((format!("{tracks}-{sel}"), d));
            }
        }
    }
    assert!(v.len() >= 8, "library seeds over 64 KiB");
    for (_, d) in &v {
        bodies::library_open(d);
    }
    replay::write_seeds("library_open", &v);
}
