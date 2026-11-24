//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-index --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/index_open/seed-*`):
//! index files the builder writes for random directories.

mod common;

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use common::{MemStore, MockDir, Rng, build, random_dir};
use orpheus_index::{SortBy, Spec};

#[test]
fn index_open_corpus() {
    replay::replay("index_open", bodies::index_open);
}

#[test]
fn seeds() {
    let mut v = Vec::new();
    let mut r = Rng(0x1D3C);
    let specs = [
        Spec::default(),
        Spec { sort: SortBy::Date, ..Spec::default() },
        Spec { sort: SortBy::Size, show_hidden: true, ..Spec::default() },
        Spec { audio_only: true, ..Spec::default() },
        Spec { images_only: true, sort: SortBy::Date, ..Spec::default() },
    ];
    for (i, (&spec, n)) in specs.iter().zip([0usize, 1, 40, 150, 300]).enumerate() {
        let entries = random_dir(&mut r, n);
        let mut store = MemStore::new();
        let ix = build(&mut store, &mut MockDir::new(entries), spec, 4096);
        let file = store.get(ix.file()).unwrap().clone();
        for sel in [0u8, 3] {
            let mut d = vec![sel | (i as u8) << 2];
            d.extend_from_slice(&file);
            v.push((format!("{i}-{n}-{sel}"), d));
        }
    }
    for (_, d) in &v {
        bodies::index_open(d);
    }
    replay::write_seeds("index_open", &v);
}
