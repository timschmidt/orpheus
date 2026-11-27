//! Shared by the adversarial-input suites (VER-3, 07 §3.7): every crate
//! that reads data from outside has a `tests/adversarial.rs` built from
//! these pieces — truncation at every byte, every single-byte corruption,
//! length fields at their edge values, hostile names, and the `proptest`
//! configuration (a fixed seed per property, so every run is the same and
//! a failure reproduces; shrunk failures recorded under
//! `tests/regress/proptest/<name>.txt` and replayed first on later runs).
//!
//! Included with `#[path = "../../../tests/adversarial.rs"] mod adversarial;`
//! (the crate needs `proptest` as a dev-dependency for [`config`]).

#![allow(dead_code)]

use std::path::PathBuf;

/// The repository root (the first ancestor holding `tests/replay.rs` and
/// `tests/regress/`; crates have their own `tests/adversarial.rs`).
pub fn root() -> PathBuf {
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while !(d.join("tests").join("replay.rs").exists() && d.join("tests").join("regress").is_dir()) {
        assert!(d.pop(), "repository root not found above {}", env!("CARGO_MANIFEST_DIR"));
    }
    d
}

/// `f` on every proper prefix of `data` (lengths 0 to `len - 1`): the
/// input cut at every byte.
pub fn prefixes(data: &[u8], mut f: impl FnMut(&[u8])) {
    for n in 0..data.len() {
        f(&data[..n]);
    }
}

/// `f(i, damaged)` for every byte `i` of `data` replaced in turn by its
/// complement, by 0x00 and by 0xFF (the values that hit length fields,
/// flags and erased NOR hardest); unchanged replacements are skipped.
pub fn damaged(data: &[u8], mut f: impl FnMut(usize, &[u8])) {
    let mut b = data.to_vec();
    for i in 0..data.len() {
        let orig = data[i];
        for v in [!orig, 0x00, 0xFF] {
            if v == orig {
                continue;
            }
            b[i] = v;
            f(i, &b);
        }
        b[i] = orig;
    }
}

/// The table every file format gets when its fields are not spelled out:
/// `data` (after its first `skip` bytes, a fuzz body's option bytes, which
/// are kept) cut at every byte of its first `head` bytes and at 64 spread
/// points beyond; every byte of the first `head` damaged; and a 16- and a
/// 32-bit field at every offset of the first `head / 2` bytes, both byte
/// orders, set to 0, 1, max − 1 and max (where length fields sit).
pub fn sweep(data: &[u8], skip: usize, head: usize, mut f: impl FnMut(&[u8])) {
    let (pre, body) = data.split_at(skip.min(data.len()));
    let with = |b: &[u8]| -> Vec<u8> { [pre, b].concat() };
    let n = body.len();
    let mut cuts: Vec<usize> = (0..n.min(head)).collect();
    if n > head {
        cuts.extend((head..n).step_by(((n - head) / 64).max(1)));
    }
    for c in cuts {
        f(&with(&body[..c]));
    }
    damaged(&body[..n.min(head)], |_, d| {
        let mut b = d.to_vec();
        b.extend_from_slice(&body[d.len()..]);
        f(&with(&b));
    });
    for w in [2usize, 4] {
        for off in 0..n.min(head / 2).saturating_sub(w - 1) {
            let max = if w == 2 { 0xFFFF } else { 0xFFFF_FFFF };
            for v in [0u64, 1, max - 1, max] {
                for be in [false, true] {
                    let mut b = body.to_vec();
                    if be {
                        put_be(&mut b, off, w, v);
                    } else {
                        put_le(&mut b, off, w, v);
                    }
                    f(&with(&b));
                }
            }
        }
    }
}

/// The committed seed files of fuzz target `target`
/// (`tests/fuzz-corpus/<target>/seed-*`), sorted by name.
pub fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = root().join("tests").join("fuzz-corpus").join(target);
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("seed-"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
        .collect();
    v.sort();
    assert!(!v.is_empty(), "no seeds for {target}");
    v
}

/// Edge values of an unsigned field `bits` wide (1–64), plus the real
/// value `real` and its neighbours: 0, 1, real − 1, real, real + 1, half,
/// max − 1, max, each once and within the field.
pub fn edges(bits: u32, real: u64) -> Vec<u64> {
    let max = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
    let mut v =
        vec![0, 1, real.wrapping_sub(1), real, real.wrapping_add(1), max / 2, max / 2 + 1, max - 1, max];
    v.retain(|&x| x <= max);
    v.sort_unstable();
    v.dedup();
    v
}

/// Write the low `n` bytes of `v` little-endian at `off`.
pub fn put_le(buf: &mut [u8], off: usize, n: usize, v: u64) {
    buf[off..off + n].copy_from_slice(&v.to_le_bytes()[..n]);
}

/// Write the low `n` bytes of `v` big-endian at `off`.
pub fn put_be(buf: &mut [u8], off: usize, n: usize, v: u64) {
    buf[off..off + n].copy_from_slice(&v.to_be_bytes()[8 - n..]);
}

/// File and display names a card can carry that a reader must survive:
/// empty, dots, control characters, separators, invalid FAT characters,
/// right-to-left and combining marks, zero-width characters, characters
/// outside the BMP (UTF-16 surrogate pairs), a BOM, replacement
/// characters, very long names.
pub fn hostile_names() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        ".",
        "..",
        "...",
        " ",
        "   leading",
        "trailing   ",
        "trailing.",
        "a\u{0}b",
        "tab\there",
        "new\nline",
        "\u{7}bell",
        "\u{1b}[31mansi",
        "slash/inside",
        "back\\slash",
        "colon:star*q?\"<>|",
        "\u{202E}gpj.exe",
        "e\u{301}\u{301}\u{301}\u{301}\u{301}",
        "zero\u{200B}width\u{FEFF}",
        "\u{1F3B5}\u{1F3B6}\u{1F3B8}",
        "\u{10FFFF}",
        "\u{FFFD}\u{FFFD}",
        "日本語のファイル名.mp3",
        "Ελληνικά.flac",
        "%s%n%x",
        "CON",
        "nul.txt",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    v.push("a".repeat(255));
    v.push("a".repeat(256));
    v.push("é".repeat(200));
    v.push("\u{1F3B5}".repeat(128));
    v.push("x".repeat(4096));
    v
}

/// The `proptest` configuration of property `name`: `cases` cases from a
/// seed derived from the name (FNV-1a), ChaCha, no forking; a failure's
/// shrunk seed is written to `tests/regress/proptest/<name>.txt`, which
/// is replayed before the generated cases on every later run.
pub fn config(name: &str, cases: u32) -> proptest::test_runner::Config {
    use proptest::test_runner::{Config, FileFailurePersistence, RngAlgorithm, RngSeed};
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let path = root().join("tests").join("regress").join("proptest").join(format!("{name}.txt"));
    let path: &'static str = Box::leak(path.to_string_lossy().into_owned().into_boxed_str());
    Config {
        cases,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(h),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        max_shrink_iters: 4096,
        ..Config::default()
    }
}
