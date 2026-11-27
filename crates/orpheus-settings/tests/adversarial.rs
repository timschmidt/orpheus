//! Adversarial inputs (VER-3, 07 §3.7) for the settings sectors on NOR: every
//! committed seed of the crate's fuzz targets swept (`sweep` in
//! `tests/adversarial.rs`: cut at every byte of its head and at spread
//! points beyond, every head byte damaged, 16/32-bit fields at
//! 0/1/max−1/max at every head offset in both byte orders) through the
//! fuzz bodies, whose oracles are in `fuzz/src/lib.rs` and
//! `docs/verification/baseline-2026-11.md` §8.1.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

#[allow(unused_imports)]
use adversarial::{config, seeds, sweep};

/// Head bytes swept for `settings_mount`.
const HEAD_SETTINGS_MOUNT: usize = 64;

#[test]
fn settings_mount_seeds_swept() {
    for (name, s) in seeds("settings_mount") {
        eprintln!("{name}");
        sweep(&s, 0, HEAD_SETTINGS_MOUNT, bodies::settings_mount);
    }
}
