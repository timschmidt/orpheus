//! The codec fuzz targets' committed corpora (`tests/fuzz-corpus/<target>/`)
//! and regression inputs (`tests/regress/<target>/`), replayed on stable
//! through the same bodies the cargo-fuzz targets run, on every `cargo
//! test` (VER-4). The seeds are written by `cargo xtask test fuzz-seeds`.

#[path = "../../../../tests/replay.rs"]
mod replay;

use orpheus_codec_conformance::fuzz::TARGETS;

#[test]
fn codec_targets_corpus() {
    let mut n = 0;
    for (name, body) in TARGETS {
        n += replay::replay(name, body);
    }
    eprintln!("{n} codec fuzz inputs replayed");
}
