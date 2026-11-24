//! Shared by the `fuzz_corpus` integration tests of every crate with a
//! cargo-fuzz project (07 §3.4, VER-4): replay the committed corpus
//! (`tests/fuzz-corpus/<target>/`) and the regression inputs
//! (`tests/regress/<target>/`) through the target body on the stable
//! toolchain, on every `cargo test`; and (re)write the seed files.
//!
//! Included with `#[path = "../../../tests/replay.rs"] mod replay;`.

#![allow(dead_code)]

use std::path::PathBuf;

/// The repository root: the first ancestor of the including package that
/// holds `tests/replay.rs`.
pub fn root() -> PathBuf {
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while !d.join("tests").join("replay.rs").exists() {
        assert!(d.pop(), "repository root not found above {}", env!("CARGO_MANIFEST_DIR"));
    }
    d
}

/// Files of `tests/fuzz-corpus/<target>/` and `tests/regress/<target>/`,
/// sorted (regression inputs first).
pub fn inputs(target: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for d in ["regress", "fuzz-corpus"] {
        let dir = root().join("tests").join(d).join(target);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect())
            .unwrap_or_default();
        files.sort();
        v.extend(files);
    }
    v
}

/// Run `body` over every committed input of `target`; returns how many ran.
/// A failing input is named on stderr before it runs (the harness shows it
/// with the panic).
pub fn replay(target: &str, body: impl Fn(&[u8])) -> usize {
    let files = inputs(target);
    for f in &files {
        eprintln!("replay {target}: {}", f.display());
        let data = std::fs::read(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        body(&data);
    }
    files.len()
}

/// With `ORPHEUS_WRITE_SEEDS=1`: write `seeds` as `tests/fuzz-corpus/<target>/
/// seed-<name>` (files over 64 KiB are refused). Returns whether it wrote.
pub fn write_seeds(target: &str, seeds: &[(String, Vec<u8>)]) -> bool {
    if std::env::var_os("ORPHEUS_WRITE_SEEDS").is_none() {
        return false;
    }
    let dir = root().join("tests").join("fuzz-corpus").join(target);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, data) in seeds {
        assert!(data.len() <= 64 * 1024, "seed {target}/{name}: {} bytes (limit 64 KiB)", data.len());
        std::fs::write(dir.join(format!("seed-{name}")), data).unwrap();
    }
    eprintln!("wrote {} seeds to {}", seeds.len(), dir.display());
    true
}
